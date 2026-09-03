use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::limits::AppServerLimits;
use super::protocol::{
    ErrorNotification, ItemLifecycle, ItemNotification, ItemType, ServerNotification,
};
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
    #[error("Codex completed item limit exceeded ({0})")]
    CompletedItemCountLimit(usize),
    #[error("Codex completed item byte limit exceeded ({0})")]
    CompletedItemBytesLimit(usize),
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
    completed_turn_id: Option<String>,
    completed_items: BTreeSet<String>,
    completed_bytes: usize,
    unknown_events: u64,
}

impl CodexActivityTranslator {
    pub fn new(limits: AppServerLimits) -> CodexActivityTranslator {
        CodexActivityTranslator {
            limits,
            open_tools: BTreeMap::new(),
            open_bytes: 0,
            completed_turn_id: None,
            completed_items: BTreeSet::new(),
            completed_bytes: 0,
            unknown_events: 0,
        }
    }

    pub fn translate_notification(
        &mut self,
        notification: &ServerNotification,
    ) -> Result<Vec<ActivityReport>, TranslationError> {
        match notification {
            ServerNotification::Item(item) => self.translate_item(item),
            ServerNotification::Error(params) => Ok(error_report(params).into_iter().collect()),
            ServerNotification::Unknown => {
                self.unknown_events = self.unknown_events.saturating_add(1);
                Ok(Vec::new())
            }
            ServerNotification::Delta
            | ServerNotification::ThreadStarted { .. }
            | ServerNotification::TurnStarted { .. }
            | ServerNotification::TurnCompleted { .. } => Ok(Vec::new()),
        }
    }

    #[cfg(test)]
    pub fn translate(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<Vec<ActivityReport>, TranslationError> {
        let notification = ServerNotification::decode(method, params.clone())
            .map_err(TranslationError::Malformed)?;
        self.translate_notification(&notification)
    }

    fn translate_item(
        &mut self,
        notification: &ItemNotification,
    ) -> Result<Vec<ActivityReport>, TranslationError> {
        self.select_completed_turn(&notification.turn_id);
        let id = required(&notification.item, "id")?;
        if self.completed_items.contains(id) {
            return Ok(Vec::new());
        }
        match notification.lifecycle {
            ItemLifecycle::Started => self.item_started(notification),
            ItemLifecycle::Completed => {
                self.ensure_completed_capacity(id)?;
                let reports = self.item_completed(notification);
                self.remember_completed(id);
                Ok(reports)
            }
        }
    }

    fn select_completed_turn(&mut self, turn_id: &str) {
        if self.completed_turn_id.as_deref() == Some(turn_id) {
            return;
        }
        self.completed_turn_id = Some(turn_id.to_string());
        self.completed_items.clear();
        self.completed_bytes = 0;
    }

    fn ensure_completed_capacity(&self, id: &str) -> Result<(), TranslationError> {
        if self.completed_items.len() >= self.limits.completed_items {
            return Err(TranslationError::CompletedItemCountLimit(
                self.limits.completed_items,
            ));
        }
        if self.completed_bytes.saturating_add(id.len()) > self.limits.completed_item_bytes {
            return Err(TranslationError::CompletedItemBytesLimit(
                self.limits.completed_item_bytes,
            ));
        }
        Ok(())
    }

    fn remember_completed(&mut self, id: &str) {
        self.completed_items.insert(id.to_string());
        self.completed_bytes += id.len();
    }

    fn item_started(
        &mut self,
        notification: &ItemNotification,
    ) -> Result<Vec<ActivityReport>, TranslationError> {
        match notification.item_type {
            ItemType::CommandExecution
            | ItemType::FileChange
            | ItemType::McpToolCall
            | ItemType::WebSearch
            | ItemType::ImageView
            | ItemType::Sleep
            | ItemType::ImageGeneration
            | ItemType::CollabAgentToolCall => self.open_tool(notification, false),
            ItemType::BuildMcpToolCall => self.open_tool(notification, true),
            ItemType::SubAgentActivity | ItemType::ContextCompaction => {
                Ok(task_report(notification, "started").into_iter().collect())
            }
            ItemType::Reasoning
            | ItemType::AgentMessage
            | ItemType::DynamicToolCall
            | ItemType::Unknown => Ok(Vec::new()),
        }
    }

    fn item_completed(&mut self, notification: &ItemNotification) -> Vec<ActivityReport> {
        match notification.item_type {
            ItemType::Reasoning | ItemType::AgentMessage => {
                speech_report(notification).into_iter().collect()
            }
            ItemType::CommandExecution
            | ItemType::FileChange
            | ItemType::BuildMcpToolCall
            | ItemType::McpToolCall
            | ItemType::WebSearch
            | ItemType::ImageView
            | ItemType::Sleep
            | ItemType::ImageGeneration
            | ItemType::CollabAgentToolCall => self.complete_tool(&notification.item),
            ItemType::SubAgentActivity | ItemType::ContextCompaction => {
                task_report(notification, "completed").into_iter().collect()
            }
            ItemType::DynamicToolCall | ItemType::Unknown => Vec::new(),
        }
    }

    fn open_tool(
        &mut self,
        notification: &ItemNotification,
        suppressed: bool,
    ) -> Result<Vec<ActivityReport>, TranslationError> {
        let item = &notification.item;
        let id = required(item, "id")?;
        if self.open_tools.contains_key(id) {
            return Ok(Vec::new());
        }
        if self.open_tools.len() >= self.limits.open_items {
            return Err(TranslationError::ItemCountLimit(self.limits.open_items));
        }
        let summary = tool_summary(notification.item_type, item);
        let charge = id.len() + notification.turn_id.len() + summary.len();
        if self.open_bytes.saturating_add(charge) > self.limits.open_item_bytes {
            return Err(TranslationError::ItemBytesLimit(
                self.limits.open_item_bytes,
            ));
        }
        self.open_tools.insert(
            id.to_string(),
            OpenTool {
                turn_id: notification.turn_id.clone(),
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
        let outcome = tool_outcome(item);
        vec![ActivityReport::own_work(AgentActivity::ToolResult {
            call_id: id.to_string(),
            outcome,
            summary: one_line(
                &format!(
                    "{} {}",
                    open.summary,
                    match outcome {
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

    #[cfg(test)]
    pub fn completed_item_count(&self) -> usize {
        self.completed_items.len()
    }
}

fn required<'a>(value: &'a Value, field: &str) -> Result<&'a str, TranslationError> {
    value[field]
        .as_str()
        .ok_or_else(|| TranslationError::Malformed(format!("{field} is missing")))
}

fn speech_report(notification: &ItemNotification) -> Option<ActivityReport> {
    let summary = match notification.item_type {
        ItemType::Reasoning => notification.item["summary"]
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
        ItemType::AgentMessage => notification.item["text"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string(),
        _ => return None,
    };
    if summary.is_empty() {
        return None;
    }
    let summary = one_line(&summary, TOOL_SUMMARY_LIMIT);
    let activity = match notification.item_type {
        ItemType::Reasoning => AgentActivity::Reasoning { summary },
        ItemType::AgentMessage => AgentActivity::Narration { summary },
        _ => unreachable!(),
    };
    Some(ActivityReport::own_work(activity))
}

fn tool_summary(item_type: ItemType, item: &Value) -> String {
    let summary = match item_type {
        ItemType::CommandExecution => "Command".to_string(),
        ItemType::FileChange => "File change".to_string(),
        ItemType::BuildMcpToolCall | ItemType::McpToolCall => format!(
            "MCP {}.{}",
            item["server"].as_str().unwrap_or("server"),
            item["tool"].as_str().unwrap_or("tool")
        ),
        ItemType::WebSearch => "Web search".to_string(),
        ItemType::ImageView => "Image view".to_string(),
        ItemType::Sleep => "Sleep".to_string(),
        ItemType::ImageGeneration => "Image generation".to_string(),
        ItemType::CollabAgentToolCall => format!(
            "Collaboration {}",
            item["tool"].as_str().unwrap_or("activity")
        ),
        _ => "Tool".to_string(),
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

fn task_report(notification: &ItemNotification, lifecycle: &str) -> Option<ActivityReport> {
    let summary = match notification.item_type {
        ItemType::SubAgentActivity => format!(
            "{} - {}",
            notification.item["agentPath"]
                .as_str()
                .unwrap_or("Sub-agent"),
            notification.item["kind"].as_str().unwrap_or(lifecycle)
        ),
        ItemType::ContextCompaction => format!("Context compaction {lifecycle}"),
        _ => return None,
    };
    Some(ActivityReport::own_work(AgentActivity::TaskUpdate {
        summary: one_line(&summary, TOOL_SUMMARY_LIMIT),
    }))
}

fn error_report(notification: &ErrorNotification) -> Option<ActivityReport> {
    let message = notification.error.message.trim();
    (!message.is_empty()).then(|| {
        ActivityReport::own_work(AgentActivity::TaskUpdate {
            summary: one_line(message, TOOL_SUMMARY_LIMIT),
        })
    })
}
