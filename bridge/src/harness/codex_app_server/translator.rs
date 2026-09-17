use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::Value;

use super::limits::TranslatorLimits;
use super::protocol::{tag_for, ItemLifecycle, ItemNotification, ServerNotification};
use crate::harness::adk::bounded_activity_text;
use crate::harness::{ActivityReport, AgentActivity, ToolOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSummaryCategory {
    Command,
    FileChange,
    Mcp,
    WebSearch,
    ImageView,
    Sleep,
    ImageGeneration,
    Collaboration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemReportKind {
    Reasoning,
    Narration,
    SubAgentActivity,
    ContextCompaction,
    EnteredReviewMode,
    ExitedReviewMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressionReason {
    UserMessageEcho,
    HookPrompt,
    FunctionCallOutput,
    ExperimentalPlan,
    BuildMcp,
    DeferredDynamicTool,
    UnknownItem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemClassification {
    TrackedTool { summary: ToolSummaryCategory },
    Emitting { report: ItemReportKind },
    Suppressed { reason: SuppressionReason },
}

const ITEM_CLASSIFICATIONS: &[(&str, ItemClassification)] = &[
    (
        "agentMessage",
        ItemClassification::Emitting {
            report: ItemReportKind::Narration,
        },
    ),
    (
        "reasoning",
        ItemClassification::Emitting {
            report: ItemReportKind::Reasoning,
        },
    ),
    (
        "commandExecution",
        ItemClassification::TrackedTool {
            summary: ToolSummaryCategory::Command,
        },
    ),
    (
        "fileChange",
        ItemClassification::TrackedTool {
            summary: ToolSummaryCategory::FileChange,
        },
    ),
    (
        "collabAgentToolCall",
        ItemClassification::TrackedTool {
            summary: ToolSummaryCategory::Collaboration,
        },
    ),
    (
        "subAgentActivity",
        ItemClassification::Emitting {
            report: ItemReportKind::SubAgentActivity,
        },
    ),
    (
        "webSearch",
        ItemClassification::TrackedTool {
            summary: ToolSummaryCategory::WebSearch,
        },
    ),
    (
        "imageView",
        ItemClassification::TrackedTool {
            summary: ToolSummaryCategory::ImageView,
        },
    ),
    (
        "sleep",
        ItemClassification::TrackedTool {
            summary: ToolSummaryCategory::Sleep,
        },
    ),
    (
        "imageGeneration",
        ItemClassification::TrackedTool {
            summary: ToolSummaryCategory::ImageGeneration,
        },
    ),
    (
        "contextCompaction",
        ItemClassification::Emitting {
            report: ItemReportKind::ContextCompaction,
        },
    ),
    (
        "userMessage",
        ItemClassification::Suppressed {
            reason: SuppressionReason::UserMessageEcho,
        },
    ),
    (
        "hookPrompt",
        ItemClassification::Suppressed {
            reason: SuppressionReason::HookPrompt,
        },
    ),
    (
        "functionCallOutput",
        ItemClassification::Suppressed {
            reason: SuppressionReason::FunctionCallOutput,
        },
    ),
    (
        "plan",
        ItemClassification::Suppressed {
            reason: SuppressionReason::ExperimentalPlan,
        },
    ),
    (
        "enteredReviewMode",
        ItemClassification::Emitting {
            report: ItemReportKind::EnteredReviewMode,
        },
    ),
    (
        "exitedReviewMode",
        ItemClassification::Emitting {
            report: ItemReportKind::ExitedReviewMode,
        },
    ),
    (
        "dynamicToolCall",
        ItemClassification::Suppressed {
            reason: SuppressionReason::DeferredDynamicTool,
        },
    ),
];

pub fn classify_item(item: &Value) -> ItemClassification {
    let item_type = item["type"].as_str().unwrap_or_default();
    if item_type == "mcpToolCall" {
        return if item["server"].as_str() == Some("build") {
            ItemClassification::Suppressed {
                reason: SuppressionReason::BuildMcp,
            }
        } else {
            ItemClassification::TrackedTool {
                summary: ToolSummaryCategory::Mcp,
            }
        };
    }
    tag_for(ITEM_CLASSIFICATIONS, item_type).unwrap_or(ItemClassification::Suppressed {
        reason: SuppressionReason::UnknownItem,
    })
}

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
    charge: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CompletedItemKey {
    turn_id: String,
    item_id: String,
}

struct CompletedItemLedger {
    keys: BTreeSet<CompletedItemKey>,
    order: VecDeque<CompletedItemKey>,
    retained_bytes: usize,
    key_limit: usize,
    byte_limit: usize,
}

impl CompletedItemLedger {
    fn new(key_limit: usize, byte_limit: usize) -> CompletedItemLedger {
        CompletedItemLedger {
            keys: BTreeSet::new(),
            order: VecDeque::new(),
            retained_bytes: 0,
            key_limit,
            byte_limit,
        }
    }

    fn contains_and_refresh(&mut self, turn_id: &str, item_id: &str) -> bool {
        let key = completed_key(turn_id, item_id);
        if !self.keys.contains(&key) {
            return false;
        }
        if let Some(index) = self.order.iter().position(|existing| existing == &key) {
            self.order.remove(index);
        }
        self.order.push_back(key);
        true
    }

    fn insert(&mut self, turn_id: &str, item_id: &str) {
        let key = completed_key(turn_id, item_id);
        let charge = key_charge(&key);
        if self.key_limit == 0 || charge > self.byte_limit {
            return;
        }
        while self.keys.len() >= self.key_limit
            || self.retained_bytes.saturating_add(charge) > self.byte_limit
        {
            let Some(expired) = self.order.pop_front() else {
                return;
            };
            self.retained_bytes -= key_charge(&expired);
            self.keys.remove(&expired);
        }
        self.retained_bytes += charge;
        self.keys.insert(key.clone());
        self.order.push_back(key);
    }

    fn clear_turn(&mut self, turn_id: &str) {
        self.order.retain(|key| {
            if key.turn_id == turn_id {
                self.retained_bytes -= key_charge(key);
                self.keys.remove(key);
                false
            } else {
                true
            }
        });
    }

    fn clear(&mut self) {
        self.keys.clear();
        self.order.clear();
        self.retained_bytes = 0;
    }
}

fn completed_key(turn_id: &str, item_id: &str) -> CompletedItemKey {
    CompletedItemKey {
        turn_id: turn_id.to_string(),
        item_id: item_id.to_string(),
    }
}

fn key_charge(key: &CompletedItemKey) -> usize {
    key.turn_id.len() + key.item_id.len()
}

pub struct CodexActivityTranslator {
    limits: TranslatorLimits,
    open_tools: BTreeMap<String, OpenTool>,
    open_bytes: usize,
    completed: CompletedItemLedger,
    unknown_events: u64,
}

impl CodexActivityTranslator {
    pub fn new(limits: TranslatorLimits) -> CodexActivityTranslator {
        CodexActivityTranslator {
            completed: CompletedItemLedger::new(
                limits.completed_items,
                limits.completed_item_bytes,
            ),
            limits,
            open_tools: BTreeMap::new(),
            open_bytes: 0,
            unknown_events: 0,
        }
    }

    pub fn translate_notification(
        &mut self,
        notification: &ServerNotification,
    ) -> Result<Vec<ActivityReport>, TranslationError> {
        match notification {
            ServerNotification::Item(item) => self.translate_item(item),
            ServerNotification::Unknown => {
                self.unknown_events = self.unknown_events.saturating_add(1);
                Ok(Vec::new())
            }
            ServerNotification::Delta
            | ServerNotification::Error(_)
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
        let classification = classify_item(&notification.item);
        if matches!(classification, ItemClassification::Suppressed { .. }) {
            return Ok(Vec::new());
        }
        let id = required(&notification.item, "id")?;
        if self
            .completed
            .contains_and_refresh(&notification.turn_id, id)
        {
            return Ok(Vec::new());
        }
        let reports = match (notification.lifecycle, classification) {
            (ItemLifecycle::Started, ItemClassification::TrackedTool { summary }) => {
                self.open_tool(notification, summary)?
            }
            (ItemLifecycle::Completed, ItemClassification::TrackedTool { .. }) => {
                self.complete_tool(&notification.item)
            }
            (lifecycle, ItemClassification::Emitting { report }) => {
                emit_item(notification, lifecycle, report)
            }
            (_, ItemClassification::Suppressed { .. }) => unreachable!(),
        };
        if notification.lifecycle == ItemLifecycle::Completed {
            self.completed.insert(&notification.turn_id, id);
        }
        Ok(reports)
    }

    fn open_tool(
        &mut self,
        notification: &ItemNotification,
        category: ToolSummaryCategory,
    ) -> Result<Vec<ActivityReport>, TranslationError> {
        let item = &notification.item;
        let id = required(item, "id")?;
        if self.open_tools.contains_key(id) {
            return Ok(Vec::new());
        }
        if self.open_tools.len() >= self.limits.open_items {
            return Err(TranslationError::ItemCountLimit(self.limits.open_items));
        }
        let summary = tool_summary(category, item);
        let charge = id.len() + notification.turn_id.len();
        if self.open_bytes.saturating_add(charge) > self.limits.open_item_bytes {
            return Err(TranslationError::ItemBytesLimit(
                self.limits.open_item_bytes,
            ));
        }
        self.open_tools.insert(
            id.to_string(),
            OpenTool {
                turn_id: notification.turn_id.clone(),
                charge,
            },
        );
        self.open_bytes += charge;
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
        let outcome = tool_outcome(item);
        vec![ActivityReport::own_work(AgentActivity::ToolResult {
            call_id: id.to_string(),
            outcome,
            summary: bounded_activity_text(&tool_result_detail(item, outcome)),
        })]
    }

    pub fn close_turn(&mut self, turn_id: &str) -> Result<Vec<ActivityReport>, TranslationError> {
        Ok(self.drain_turn(turn_id))
    }

    fn drain_turn(&mut self, turn_id: &str) -> Vec<ActivityReport> {
        let closing = self
            .open_tools
            .iter()
            .filter(|(_, open)| open.turn_id == turn_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let reports = closing
            .into_iter()
            .filter_map(|id| {
                let open = self.open_tools.remove(&id)?;
                self.open_bytes -= open.charge;
                Some(ActivityReport::own_work(AgentActivity::ToolResult {
                    call_id: id,
                    outcome: ToolOutcome::Unanswered,
                    summary: String::new(),
                }))
            })
            .collect();
        self.completed.clear_turn(turn_id);
        reports
    }

    pub fn close_all(&mut self) -> Vec<ActivityReport> {
        let turns = self
            .open_tools
            .values()
            .map(|open| open.turn_id.clone())
            .collect::<BTreeSet<_>>();
        let reports = turns
            .into_iter()
            .flat_map(|turn_id| self.drain_turn(&turn_id))
            .collect();
        self.completed.clear();
        reports
    }

    #[cfg(test)]
    pub fn open_item_count(&self) -> usize {
        self.open_tools.len()
    }

    #[cfg(test)]
    pub fn completed_item_count(&self) -> usize {
        self.completed.keys.len()
    }
}

fn required<'a>(value: &'a Value, field: &str) -> Result<&'a str, TranslationError> {
    value[field]
        .as_str()
        .ok_or_else(|| TranslationError::Malformed(format!("{field} is missing")))
}

fn emit_item(
    notification: &ItemNotification,
    lifecycle: ItemLifecycle,
    report: ItemReportKind,
) -> Vec<ActivityReport> {
    match report {
        ItemReportKind::Reasoning if lifecycle == ItemLifecycle::Completed => {
            reasoning_report(&notification.item).into_iter().collect()
        }
        ItemReportKind::Narration if lifecycle == ItemLifecycle::Completed => {
            narration_report(&notification.item).into_iter().collect()
        }
        ItemReportKind::SubAgentActivity if lifecycle == ItemLifecycle::Completed => {
            subagent_report(&notification.item, lifecycle)
                .into_iter()
                .collect()
        }
        ItemReportKind::ContextCompaction => {
            vec![ActivityReport::own_work(AgentActivity::Compaction {
                completed: lifecycle == ItemLifecycle::Completed,
            })]
        }
        ItemReportKind::EnteredReviewMode if lifecycle == ItemLifecycle::Completed => {
            vec![ActivityReport::bounded_task_update("Entered review mode")]
        }
        ItemReportKind::ExitedReviewMode if lifecycle == ItemLifecycle::Completed => {
            vec![ActivityReport::bounded_task_update("Exited review mode")]
        }
        _ => Vec::new(),
    }
}

fn reasoning_report(item: &Value) -> Option<ActivityReport> {
    let summary = item["summary"]
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
        .unwrap_or_default();
    (!summary.is_empty()).then(|| {
        ActivityReport::own_work(AgentActivity::Reasoning {
            summary: bounded_activity_text(&summary),
        })
    })
}

fn narration_report(item: &Value) -> Option<ActivityReport> {
    let summary = item["text"].as_str().unwrap_or_default().trim();
    (!summary.is_empty()).then(|| {
        ActivityReport::own_work(AgentActivity::Narration {
            summary: bounded_activity_text(summary),
        })
    })
}

fn subagent_report(item: &Value, lifecycle: ItemLifecycle) -> Option<ActivityReport> {
    let summary = format!(
        "{} - {}",
        item["agentPath"].as_str().unwrap_or("Sub-agent"),
        item["kind"]
            .as_str()
            .unwrap_or_else(|| lifecycle_word(lifecycle))
    );
    Some(ActivityReport::bounded_task_update(&summary))
}

fn lifecycle_word(lifecycle: ItemLifecycle) -> &'static str {
    match lifecycle {
        ItemLifecycle::Started => "started",
        ItemLifecycle::Completed => "completed",
    }
}

fn outcome_word(outcome: ToolOutcome) -> &'static str {
    match outcome {
        ToolOutcome::Ok => "completed",
        ToolOutcome::Error => "failed",
        ToolOutcome::Unanswered => "unanswered",
    }
}

fn tool_summary(category: ToolSummaryCategory, item: &Value) -> String {
    let summary = match category {
        ToolSummaryCategory::Command => command_text(item)
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .unwrap_or("Command")
            .to_string(),
        ToolSummaryCategory::FileChange => file_change_summary(item),
        ToolSummaryCategory::Mcp => format!(
            "MCP {}.{}{}",
            item["server"].as_str().unwrap_or("server"),
            item["tool"].as_str().unwrap_or("tool"),
            argument_hint(&item["arguments"])
        ),
        ToolSummaryCategory::WebSearch => labeled_value("Web search", item["query"].as_str()),
        ToolSummaryCategory::ImageView => labeled_value("Image view", item["path"].as_str()),
        ToolSummaryCategory::Sleep => labeled_value(
            "Sleep",
            item["durationMs"]
                .as_u64()
                .map(|value| format!("{value} ms"))
                .as_deref(),
        ),
        ToolSummaryCategory::ImageGeneration => labeled_value(
            "Image generation",
            item["prompt"]
                .as_str()
                .or_else(|| item["description"].as_str()),
        ),
        ToolSummaryCategory::Collaboration => collaboration_summary(item),
    };
    bounded_activity_text(&summary)
}

fn optional_hint(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!(" {value}"))
        .unwrap_or_default()
}

fn collaboration_summary(item: &Value) -> String {
    let arguments = argument_hint(&item["arguments"]);
    let hint = match arguments.is_empty() {
        true => optional_hint(item["prompt"].as_str()),
        false => arguments,
    };
    format!(
        "Collaboration {}{hint}",
        item["tool"].as_str().unwrap_or("activity")
    )
}

fn command_text(item: &Value) -> Option<&str> {
    item["commandActions"]
        .as_array()
        .filter(|actions| actions.len() == 1)
        .and_then(|actions| actions[0]["command"].as_str())
        .or_else(|| item["command"].as_str())
}

fn labeled_value(label: &str, value: Option<&str>) -> String {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => format!("{label} {value}"),
        None => label.to_string(),
    }
}

fn argument_hint(arguments: &Value) -> String {
    const USEFUL_KEYS: &[&str] = &[
        "query",
        "path",
        "url",
        "command",
        "description",
        "prompt",
        "input",
        "task",
    ];
    let value = arguments.as_str().or_else(|| {
        let object = arguments.as_object()?;
        USEFUL_KEYS
            .iter()
            .find_map(|key| object.get(*key).and_then(Value::as_str))
    });
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!(" {value}"))
        .unwrap_or_default()
}

fn file_change_summary(item: &Value) -> String {
    let Some(changes) = item["changes"].as_array() else {
        return "File change".to_string();
    };
    let described = changes
        .iter()
        .filter_map(|change| {
            let path = change["path"].as_str()?.trim();
            if path.is_empty() {
                return None;
            }
            let kind = change["kind"]
                .as_str()
                .or_else(|| change["kind"]["type"].as_str())
                .unwrap_or_default();
            if let Some(destination) = change["kind"]["move_path"].as_str() {
                return Some(format!("move {path} → {destination}"));
            }
            let kind = match kind {
                "add" | "added" | "create" => "add",
                "delete" | "deleted" | "remove" => "delete",
                "rename" | "renamed" => "rename",
                _ => "edit",
            };
            Some(format!("{kind} {path}"))
        })
        .collect::<Vec<_>>();
    match described.is_empty() {
        true => "File change".to_string(),
        false => format!("File change {}", described.join(", ")),
    }
}

fn tool_result_detail(item: &Value, outcome: ToolOutcome) -> String {
    if let Some(error) = error_text(&item["error"]) {
        return error;
    }
    match item["type"].as_str().unwrap_or_default() {
        "commandExecution" => command_result_detail(item, outcome),
        "fileChange" if outcome == ToolOutcome::Ok => item["changes"]
            .as_array()
            .map(|changes| {
                format!(
                    "{} file{} changed",
                    changes.len(),
                    if changes.len() == 1 { "" } else { "s" }
                )
            })
            .unwrap_or_else(|| outcome_word(outcome).to_string()),
        "mcpToolCall" | "collabAgentToolCall" => {
            result_text(&item["result"]).unwrap_or_else(|| outcome_word(outcome).to_string())
        }
        _ => result_text(&item["result"]).unwrap_or_else(|| outcome_word(outcome).to_string()),
    }
}

fn command_result_detail(item: &Value, outcome: ToolOutcome) -> String {
    let output = item["aggregatedOutput"]
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let duration = item["durationMs"].as_u64();
    let exit = item["exitCode"].as_i64().map(|code| match duration {
        Some(duration) => format!("exit {code} in {duration} ms"),
        None => format!("exit {code}"),
    });
    match (exit, output) {
        (Some(exit), Some(output)) => format!("{exit}: {output}"),
        (Some(exit), None) => exit,
        (None, Some(output)) => output.to_string(),
        (None, None) => outcome_word(outcome).to_string(),
    }
}

fn error_text(error: &Value) -> Option<String> {
    error
        .as_str()
        .or_else(|| error["message"].as_str())
        .or_else(|| error["error"].as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn result_text(result: &Value) -> Option<String> {
    result
        .as_str()
        .or_else(|| result["text"].as_str())
        .or_else(|| result["content"].as_str())
        .or_else(|| {
            result["content"]
                .as_array()?
                .iter()
                .find_map(|block| block.as_str().or_else(|| block["text"].as_str()))
        })
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn tool_outcome(item: &Value) -> ToolOutcome {
    let failed_status = matches!(
        item["status"].as_str(),
        Some("failed" | "declined" | "interrupted" | "error")
    );
    let failed_exit = item["exitCode"].as_i64().is_some_and(|code| code != 0);
    let has_error = item["error"].is_object()
        || item["error"]
            .as_str()
            .is_some_and(|error| !error.trim().is_empty());
    if failed_status || failed_exit || has_error {
        ToolOutcome::Error
    } else {
        ToolOutcome::Ok
    }
}
