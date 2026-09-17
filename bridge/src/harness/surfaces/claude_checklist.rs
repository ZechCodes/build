use std::collections::HashMap;

use serde_json::Value;

use super::bounds::{bounded, CHECKLIST_ITEM_LIMIT, CHECKLIST_TEXT_LIMIT, PROVIDER_TOKEN_LIMIT};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClaudeTodo {
    pub(super) content: String,
    pub(super) active_form: Option<String>,
    pub(super) state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConfirmedTodoWrite {
    pub(super) collection_epoch: u64,
    pub(super) original_count: usize,
    pub(super) truncated: bool,
    pub(super) todos: Vec<ClaudeTodo>,
}

#[derive(Debug)]
struct PendingTodoWrite {
    issuance: u64,
    original_count: usize,
    truncated: bool,
    todos: Vec<ClaudeTodo>,
}

/// Correlates Claude's authoritative TodoWrite calls with their results.
///
/// Calls remain invisible until Claude confirms them. Publication order is
/// based on successful writes only, so a failed newer call cannot suppress an
/// older call whose successful result arrives later.
#[derive(Debug, Default)]
pub(super) struct ClaudeChecklist {
    next_issuance: u64,
    latest_successful_issuance: Option<u64>,
    pending: HashMap<String, PendingTodoWrite>,
}

impl ClaudeChecklist {
    pub(super) fn clear_pending(&mut self) {
        self.pending.clear();
    }

    pub(super) fn stage_todo_write(&mut self, block: &Value) {
        let Some(call_id) = block["id"].as_str() else {
            return;
        };
        let Some((todos, original_count, truncated)) = decode_todos(&block["input"]["todos"])
        else {
            return;
        };
        let issuance = self.next_issuance;
        self.next_issuance = self.next_issuance.saturating_add(1);
        if self.pending.len() >= CHECKLIST_ITEM_LIMIT {
            if let Some(oldest) = self
                .pending
                .iter()
                .min_by_key(|(_, pending)| pending.issuance)
                .map(|(call_id, _)| call_id.clone())
            {
                self.pending.remove(&oldest);
            }
        }
        self.pending.insert(
            bounded(call_id, PROVIDER_TOKEN_LIMIT).0,
            PendingTodoWrite {
                issuance,
                original_count,
                truncated,
                todos,
            },
        );
    }

    pub(super) fn finish_todo_write(
        &mut self,
        call_id: &str,
        succeeded: bool,
    ) -> Option<ConfirmedTodoWrite> {
        let pending = self
            .pending
            .remove(&bounded(call_id, PROVIDER_TOKEN_LIMIT).0)?;
        if !succeeded
            || self
                .latest_successful_issuance
                .is_some_and(|published| pending.issuance <= published)
        {
            return None;
        }
        self.latest_successful_issuance = Some(pending.issuance);
        Some(ConfirmedTodoWrite {
            collection_epoch: pending.issuance,
            original_count: pending.original_count,
            truncated: pending.truncated,
            todos: pending.todos,
        })
    }

    #[cfg(test)]
    fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

fn decode_todos(value: &Value) -> Option<(Vec<ClaudeTodo>, usize, bool)> {
    let raw = value.as_array()?;
    let mut todos = Vec::with_capacity(raw.len().min(CHECKLIST_ITEM_LIMIT));
    let mut truncated = raw.len() > CHECKLIST_ITEM_LIMIT;
    for value in raw {
        let (todo, item_truncated) = decode_todo(value)?;
        truncated |= item_truncated;
        if todos.len() < CHECKLIST_ITEM_LIMIT {
            todos.push(todo);
        }
    }
    Some((todos, raw.len(), truncated))
}

fn decode_todo(value: &Value) -> Option<(ClaudeTodo, bool)> {
    let (content, content_cut) = bounded(value["content"].as_str()?, CHECKLIST_TEXT_LIMIT);
    let (state, state_cut) = bounded(value["status"].as_str()?, PROVIDER_TOKEN_LIMIT);
    let active_form = match value.get("activeForm") {
        None | Some(Value::Null) => None,
        Some(Value::String(active_form)) => Some(bounded(active_form, CHECKLIST_TEXT_LIMIT)),
        Some(_) => return None,
    };
    let active_form_cut = active_form.as_ref().is_some_and(|(_, cut)| *cut);
    Some((
        ClaudeTodo {
            content,
            active_form: active_form.map(|(value, _)| value),
            state,
        },
        content_cut || state_cut || active_form_cut,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn todo_write(id: &str, todos: Value) -> Value {
        json!({ "id": id, "input": { "todos": todos } })
    }

    fn one_todo(subject: &str, state: &str) -> Value {
        json!([{ "content": subject, "status": state, "activeForm": format!("doing {subject}") }])
    }

    #[test]
    fn a_write_is_invisible_until_its_successful_result() {
        let mut checklist = ClaudeChecklist::default();
        checklist.stage_todo_write(&todo_write("one", one_todo("ship", "pending")));

        assert_eq!(checklist.pending_count(), 1);
        assert_eq!(
            checklist.finish_todo_write("one", true),
            Some(ConfirmedTodoWrite {
                collection_epoch: 0,
                original_count: 1,
                truncated: false,
                todos: vec![ClaudeTodo {
                    content: "ship".to_string(),
                    active_form: Some("doing ship".to_string()),
                    state: "pending".to_string(),
                }],
            })
        );
        assert_eq!(checklist.pending_count(), 0);
    }

    #[test]
    fn a_failed_result_drains_the_staged_write_without_publishing_it() {
        let mut checklist = ClaudeChecklist::default();
        checklist.stage_todo_write(&todo_write("one", one_todo("ship", "pending")));

        assert_eq!(checklist.finish_todo_write("one", false), None);
        assert_eq!(checklist.pending_count(), 0);
    }

    #[test]
    fn latest_successful_issuance_wins_when_results_arrive_out_of_order() {
        let mut checklist = ClaudeChecklist::default();
        checklist.stage_todo_write(&todo_write("older", one_todo("old", "pending")));
        checklist.stage_todo_write(&todo_write("newer", one_todo("new", "completed")));

        assert_eq!(
            checklist.finish_todo_write("newer", true).unwrap().todos[0].content,
            "new"
        );
        assert_eq!(checklist.finish_todo_write("older", true), None);
    }

    #[test]
    fn every_eligible_success_reaches_the_shared_collection_for_deduplication() {
        let mut checklist = ClaudeChecklist::default();
        checklist.stage_todo_write(&todo_write("one", one_todo("ship", "pending")));
        assert!(checklist.finish_todo_write("one", true).is_some());
        checklist.stage_todo_write(&todo_write("two", one_todo("ship", "pending")));

        assert!(checklist.finish_todo_write("two", true).is_some());
    }

    #[test]
    fn a_newer_failure_does_not_suppress_an_older_success() {
        let mut checklist = ClaudeChecklist::default();
        checklist.stage_todo_write(&todo_write("older", one_todo("old", "pending")));
        checklist.stage_todo_write(&todo_write("newer", one_todo("new", "completed")));

        assert_eq!(checklist.finish_todo_write("newer", false), None);
        assert_eq!(
            checklist.finish_todo_write("older", true).unwrap().todos[0].content,
            "old"
        );
    }

    #[test]
    fn the_entire_array_must_decode_before_a_write_is_staged() {
        let mut checklist = ClaudeChecklist::default();
        checklist.stage_todo_write(&todo_write(
            "broken",
            json!([
                { "content": "valid", "status": "pending" },
                { "content": "missing a state" }
            ]),
        ));

        assert_eq!(checklist.pending_count(), 0);
        assert_eq!(checklist.finish_todo_write("broken", true), None);
    }

    #[test]
    fn an_empty_array_is_a_valid_clear() {
        let mut checklist = ClaudeChecklist::default();
        checklist.stage_todo_write(&todo_write("clear", json!([])));

        assert_eq!(
            checklist.finish_todo_write("clear", true),
            Some(ConfirmedTodoWrite {
                collection_epoch: 0,
                original_count: 0,
                truncated: false,
                todos: vec![],
            })
        );
    }

    #[test]
    fn unknown_states_are_retained_for_the_shared_collection_to_bound() {
        let mut checklist = ClaudeChecklist::default();
        checklist.stage_todo_write(&todo_write("one", one_todo("ship", "waiting_on_alice")));

        assert_eq!(
            checklist.finish_todo_write("one", true).unwrap().todos[0].state,
            "waiting_on_alice"
        );
    }

    #[test]
    fn staged_content_is_bounded_and_confirmation_reports_the_truncation() {
        let mut checklist = ClaudeChecklist::default();
        let oversized = "é".repeat(CHECKLIST_TEXT_LIMIT);
        checklist.stage_todo_write(&todo_write("one", one_todo(&oversized, "waiting_on_alice")));

        let confirmed = checklist.finish_todo_write("one", true).unwrap();
        assert!(confirmed.truncated);
        assert_eq!(confirmed.original_count, 1);
        assert!(confirmed.todos[0].content.len() <= CHECKLIST_TEXT_LIMIT);
        assert!(confirmed.todos[0]
            .content
            .is_char_boundary(confirmed.todos[0].content.len()));
    }
}
