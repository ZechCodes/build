use std::collections::BTreeMap;

use serde_json::Value;

use super::limits::AppServerLimits;
use crate::harness::adk::{one_line, TOOL_SUMMARY_LIMIT};
use crate::harness::{ActivityReport, AgentActivity, ToolOutcome};

#[derive(Debug, thiserror::Error)]
pub enum TranslationError {
    #[error("malformed Codex item lifecycle: {0}")]
    Malformed(String),
    #[error("Codex open item limit exceeded ({0})")]
    ItemCountLimit(usize),
    #[error("Codex open item byte limit exceeded ({0})")]
    ItemBytesLimit(usize),
}

#[derive(Debug, Clone)]
struct OpenTool {
    turn_id: String,
    summary: String,
    suppressed: bool,
    charge: usize,
}

pub struct CodexActivityTranslator {
    limits: AppServerLimits,
    open_tools: BTreeMap<String, OpenTool>,
    open_bytes: usize,
    unknown_events: u64,
}

impl CodexActivityTranslator {
    pub fn new(limits: AppServerLimits) -> CodexActivityTranslator {
        CodexActivityTranslator {
            limits,
            open_tools: BTreeMap::new(),
            open_bytes: 0,
            unknown_events: 0,
        }
    }

    pub fn translate(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<Vec<ActivityReport>, TranslationError> {
        match method {
            "item/started" => self.item_started(params),
            "item/completed" => self.item_completed(params),
            "error" => Ok(error_report(params).into_iter().collect()),
            known if known.contains("delta") => Ok(Vec::new()),
            _ => {
                self.unknown_events = self.unknown_events.saturating_add(1);
                Ok(Vec::new())
            }
        }
    }

    fn item_started(&mut self, params: &Value) -> Result<Vec<ActivityReport>, TranslationError> {
        let item = item(params)?;
        let kind = item["type"].as_str().unwrap_or_default();
        match classified_item(item, kind) {
            ItemKind::Tool => self.open_tool(params, item, false),
            ItemKind::BuildMcp => self.open_tool(params, item, true),
            ItemKind::Task => Ok(task_report(item, "started").into_iter().collect()),
            ItemKind::Deferred | ItemKind::Speech | ItemKind::Unknown => Ok(Vec::new()),
        }
    }

    fn item_completed(&mut self, params: &Value) -> Result<Vec<ActivityReport>, TranslationError> {
        let item = item(params)?;
        let kind = item["type"].as_str().unwrap_or_default();
        match classified_item(item, kind) {
            ItemKind::Speech => Ok(speech_report(kind, item).into_iter().collect()),
            ItemKind::Tool | ItemKind::BuildMcp => Ok(self.complete_tool(item)),
            ItemKind::Task => Ok(task_report(item, "completed").into_iter().collect()),
            ItemKind::Deferred | ItemKind::Unknown => Ok(Vec::new()),
        }
    }

    fn open_tool(
        &mut self,
        params: &Value,
        item: &Value,
        suppressed: bool,
    ) -> Result<Vec<ActivityReport>, TranslationError> {
        let id = required(item, "id")?;
        if self.open_tools.contains_key(id) {
            return Ok(Vec::new());
        }
        if self.open_tools.len() >= self.limits.open_items {
            return Err(TranslationError::ItemCountLimit(self.limits.open_items));
        }
        let turn_id = required(params, "turnId")?;
        let summary = tool_summary(item);
        let charge = id.len() + turn_id.len() + summary.len();
        if self.open_bytes.saturating_add(charge) > self.limits.open_item_bytes {
            return Err(TranslationError::ItemBytesLimit(
                self.limits.open_item_bytes,
            ));
        }
        self.open_tools.insert(
            id.to_string(),
            OpenTool {
                turn_id: turn_id.to_string(),
                summary: summary.clone(),
                suppressed,
                charge,
            },
        );
        self.open_bytes += charge;
        if suppressed {
            return Ok(Vec::new());
        }
        Ok(vec![ActivityReport::own_work(AgentActivity::ToolUse {
            call_id: id.to_string(),
            summary,
        })])
    }

    fn complete_tool(&mut self, item: &Value) -> Vec<ActivityReport> {
        let Some(id) = item["id"].as_str() else {
            return Vec::new();
        };
        let Some(open) = self.open_tools.remove(id) else {
            return Vec::new();
        };
        self.open_bytes -= open.charge;
        if open.suppressed {
            return Vec::new();
        }
        vec![ActivityReport::own_work(AgentActivity::ToolResult {
            call_id: id.to_string(),
            outcome: tool_outcome(item),
            summary: one_line(
                &format!(
                    "{} {}",
                    open.summary,
                    match tool_outcome(item) {
                        ToolOutcome::Ok => "completed",
                        ToolOutcome::Error => "failed",
                        ToolOutcome::Unanswered => "unanswered",
                    }
                ),
                TOOL_SUMMARY_LIMIT,
            ),
        })]
    }

    pub fn close_turn(&mut self, turn_id: &str) -> Vec<ActivityReport> {
        let closing = self
            .open_tools
            .iter()
            .filter(|(_, open)| open.turn_id == turn_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        closing
            .into_iter()
            .filter_map(|id| {
                let open = self.open_tools.remove(&id)?;
                self.open_bytes -= open.charge;
                (!open.suppressed).then(|| {
                    ActivityReport::own_work(AgentActivity::ToolResult {
                        call_id: id,
                        outcome: ToolOutcome::Unanswered,
                        summary: String::new(),
                    })
                })
            })
            .collect()
    }

    pub fn close_all(&mut self) -> Vec<ActivityReport> {
        let turns = self
            .open_tools
            .values()
            .map(|open| open.turn_id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        turns
            .into_iter()
            .flat_map(|turn_id| self.close_turn(&turn_id))
            .collect()
    }

    #[cfg(test)]
    pub fn open_item_count(&self) -> usize {
        self.open_tools.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemKind {
    Speech,
    Tool,
    BuildMcp,
    Task,
    Deferred,
    Unknown,
}

fn item_kind(kind: &str) -> ItemKind {
    match kind {
        "reasoning" | "agentMessage" => ItemKind::Speech,
        "commandExecution"
        | "fileChange"
        | "webSearch"
        | "imageView"
        | "sleep"
        | "imageGeneration"
        | "collabAgentToolCall" => ItemKind::Tool,
        "subAgentActivity" | "contextCompaction" => ItemKind::Task,
        "dynamicToolCall" => ItemKind::Deferred,
        _ => ItemKind::Unknown,
    }
}

fn classified_item(item: &Value, kind: &str) -> ItemKind {
    match kind {
        "mcpToolCall" => mcp_kind(item),
        _ => item_kind(kind),
    }
}

fn item(params: &Value) -> Result<&Value, TranslationError> {
    let item = params
        .get("item")
        .filter(|item| item.is_object())
        .ok_or_else(|| TranslationError::Malformed("item is missing".to_string()))?;
    if item["type"].as_str() == Some("mcpToolCall") {
        return Ok(item);
    }
    Ok(item)
}

fn required<'a>(value: &'a Value, field: &str) -> Result<&'a str, TranslationError> {
    value[field]
        .as_str()
        .ok_or_else(|| TranslationError::Malformed(format!("{field} is missing")))
}

fn speech_report(kind: &str, item: &Value) -> Option<ActivityReport> {
    let summary = match kind {
        "reasoning" => item["summary"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|part| {
                        part.as_str()
                            .or_else(|| part["text"].as_str())
                            .map(str::trim)
                            .filter(|text| !text.is_empty())
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        "agentMessage" => item["text"].as_str().unwrap_or_default().trim().to_string(),
        _ => String::new(),
    };
    if summary.is_empty() {
        return None;
    }
    let activity = match kind {
        "reasoning" => AgentActivity::Reasoning { summary },
        _ => AgentActivity::Narration { summary },
    };
    Some(ActivityReport::own_work(activity))
}

fn tool_summary(item: &Value) -> String {
    let kind = item["type"].as_str().unwrap_or("Tool");
    let summary = match kind {
        "commandExecution" => "Command".to_string(),
        "fileChange" => "File change".to_string(),
        "mcpToolCall" => format!(
            "MCP {}.{}",
            item["server"].as_str().unwrap_or("server"),
            item["tool"].as_str().unwrap_or("tool")
        ),
        "webSearch" => "Web search".to_string(),
        "imageView" => "Image view".to_string(),
        "sleep" => "Sleep".to_string(),
        "imageGeneration" => "Image generation".to_string(),
        "collabAgentToolCall" => format!(
            "Collaboration {}",
            item["tool"].as_str().unwrap_or("activity")
        ),
        _ => kind.to_string(),
    };
    one_line(&summary, TOOL_SUMMARY_LIMIT)
}

fn tool_outcome(item: &Value) -> ToolOutcome {
    let failed_status = matches!(
        item["status"].as_str(),
        Some("failed" | "declined" | "interrupted" | "error")
    );
    let failed_exit = item["exitCode"].as_i64().is_some_and(|code| code != 0);
    if failed_status || failed_exit || item["error"].is_object() {
        ToolOutcome::Error
    } else {
        ToolOutcome::Ok
    }
}

fn task_report(item: &Value, lifecycle: &str) -> Option<ActivityReport> {
    let kind = item["type"].as_str()?;
    let summary = match kind {
        "subAgentActivity" => format!(
            "{} - {}",
            item["agentPath"].as_str().unwrap_or("Sub-agent"),
            item["kind"].as_str().unwrap_or(lifecycle)
        ),
        "contextCompaction" => format!("Context compaction {lifecycle}"),
        _ => return None,
    };
    Some(ActivityReport::own_work(AgentActivity::TaskUpdate {
        summary: one_line(&summary, TOOL_SUMMARY_LIMIT),
    }))
}

fn error_report(params: &Value) -> Option<ActivityReport> {
    let message = params["error"]["message"]
        .as_str()
        .or_else(|| params["message"].as_str())?
        .trim();
    (!message.is_empty()).then(|| {
        ActivityReport::own_work(AgentActivity::TaskUpdate {
            summary: one_line(message, TOOL_SUMMARY_LIMIT),
        })
    })
}

fn mcp_kind(item: &Value) -> ItemKind {
    match item["server"].as_str() {
        Some("build") => ItemKind::BuildMcp,
        _ => ItemKind::Tool,
    }
}
