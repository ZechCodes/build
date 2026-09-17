use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use super::claude_checklist::ClaudeChecklist;
use super::*;
use crate::harness::shell_tail::ShellTail;

/// Provider-neutral cached execution and checklist state.
#[derive(Debug, Default)]
pub struct SurfaceLedger {
    pub(super) workflows: Vec<SurfaceWorkflow>,
    pub(super) subagents: Vec<SurfaceAgent>,
    pub(super) shells: Vec<SurfaceShell>,
    pub(super) checklist: ChecklistCollection,
    pub(super) task_checklist: ChecklistCollection,
    pub(super) claude_checklist: Option<ClaudeChecklist>,
    pub(super) provider_session_generation: u64,
    pub(super) goal_observation: Option<SurfaceObservation>,
    pub(super) workflow_omissions: usize,
    pub(super) subagent_omissions: usize,
    pub(super) shell_omissions: usize,
    pub(super) shell_outputs: HashMap<String, PathBuf>,
    pub(super) pending_checklist_creates: HashMap<String, PendingChecklistCreate>,
    pub(super) pending_subagent_choices: HashMap<String, SubagentChoice>,
}

use super::execution::*;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingChecklistCreate {
    subject: String,
    description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SubagentChoice {
    pub(super) model: Option<String>,
    pub(super) reasoning_effort: Option<String>,
}

impl SurfaceLedger {
    pub fn for_claude(provider_session_generation: u64) -> Self {
        Self {
            claude_checklist: Some(ClaudeChecklist::default()),
            provider_session_generation,
            goal_observation: Some(SurfaceObservation::unsupported()),
            ..Self::default()
        }
    }

    pub fn read_task_event(&mut self, subtype: &str, event: &Value) -> bool {
        let task_id = match event["task_id"].as_str() {
            Some(named) => named.to_string(),
            None => return false,
        };
        match subtype {
            "task_started" => match event["task_type"].as_str() {
                Some("local_workflow") => self.apply_workflow(subtype, &task_id, event),
                Some("local_agent") => self.apply_subagent(subtype, &task_id, event),
                Some("local_bash") => self.apply_shell(
                    &task_id,
                    ShellReport::Started {
                        description: bounded_text(event, "description"),
                    },
                ),
                _ => false,
            },
            "task_progress" | "task_updated" | "task_notification" => match (
                held_named(&mut self.workflows, &task_id).is_some(),
                held_named(&mut self.subagents, &task_id).is_some(),
                held_named(&mut self.shells, &task_id).is_some(),
            ) {
                (true, _, _) => self.apply_workflow(subtype, &task_id, event),
                (_, true, _) => self.apply_subagent(subtype, &task_id, event),
                (_, _, true) => match shell_close_reported_by(subtype, event) {
                    Some(closed) => self.apply_shell(&task_id, closed),
                    None => false,
                },
                _ => false,
            },
            _ => false,
        }
    }

    pub fn read_tool_call(&mut self, name: &str, tool_use_block: &Value) -> bool {
        match name {
            "Agent" | "Task" => {
                let Some(call_id) = tool_use_block["id"].as_str() else {
                    return false;
                };
                let input = &tool_use_block["input"];
                self.pending_subagent_choices.insert(
                    call_id.to_string(),
                    SubagentChoice {
                        model: bounded_text(input, "model"),
                        reasoning_effort: bounded_text(input, "reasoning_effort")
                            .or_else(|| bounded_text(input, "effort")),
                    },
                );
                false
            }
            _ => self.apply_checklist_call(name, tool_use_block),
        }
    }

    pub fn read_subagent_message(&mut self, call_id: &str, message: &Value) -> bool {
        let Some(held) = self
            .subagents
            .iter_mut()
            .find(|agent| agent.spawning_call_id.as_deref() == Some(call_id))
        else {
            return false;
        };
        let reported = SurfaceAgent {
            model: bounded_text(message, "model").or_else(|| held.model.clone()),
            reasoning_effort: bounded_text(message, "reasoningEffort")
                .or_else(|| bounded_text(message, "reasoning_effort"))
                .or_else(|| bounded_text(message, "effort"))
                .or_else(|| held.reasoning_effort.clone()),
            ..held.clone()
        };
        replace_when_changed(held, reported)
    }

    pub fn read_tool_answer(
        &mut self,
        tool: &str,
        call_id: &str,
        whole_event: &Value,
        answered_text: &str,
        now_ms: u64,
    ) -> bool {
        match tool {
            "Bash" => self.apply_shell_launch(whole_event, answered_text, now_ms),
            "Agent" | "Task" => self.apply_subagent_launch_answer(call_id, whole_event),
            _ => self.apply_checklist_answer(tool, call_id, whole_event),
        }
    }

    pub fn read_shell_tail(&mut self, shell_id: &str, tail: ShellTail) -> bool {
        self.apply_shell(shell_id, ShellReport::Tailed(tail))
    }

    pub fn running_shell_outputs(&self) -> Vec<(String, PathBuf)> {
        self.shells
            .iter()
            .filter(|shell| shell.state.as_deref() == Some(RUNNING))
            .filter_map(|shell| {
                self.shell_outputs
                    .get(&shell.id)
                    .map(|output_path| (shell.id.clone(), output_path.clone()))
            })
            .collect()
    }

    pub fn close_pending_creates(&mut self) {
        self.pending_checklist_creates.clear();
        self.pending_subagent_choices.clear();
        if let Some(checklist) = self.claude_checklist.as_mut() {
            checklist.clear_pending();
        }
    }

    #[cfg(test)]
    pub fn pending_create_count(&self) -> usize {
        self.pending_checklist_creates.len()
    }

    pub fn snapshot(&self) -> Option<AgentSurfaces> {
        let (workflows, workflow_omissions) =
            bounded_terminal_entries(&self.workflows, |workflow| {
                matches!(workflow.state.as_deref(), Some(DONE | FAILED))
            });
        let (subagents, subagent_omissions) = bounded_terminal_entries(&self.subagents, |agent| {
            matches!(agent.state.as_deref(), Some(DONE | FAILED))
        });
        let (shells, shell_omissions) = bounded_terminal_entries(&self.shells, |shell| {
            matches!(shell.state.as_deref(), Some(DONE | FAILED))
        });
        let held = AgentSurfaces {
            workflows,
            subagents,
            shells,
            checklist: self.checklist.items().to_vec(),
            checklist_provenance: self.checklist.provenance().cloned(),
            observations: SurfaceObservations {
                goal: self.goal_observation.clone(),
                checklist: self.checklist.observation().cloned(),
                workflows: execution_observation(
                    &self.workflows,
                    self.workflow_omissions.saturating_add(workflow_omissions),
                ),
                subagents: execution_observation(
                    &self.subagents,
                    self.subagent_omissions.saturating_add(subagent_omissions),
                ),
                shells: execution_observation(
                    &self.shells,
                    self.shell_omissions.saturating_add(shell_omissions),
                ),
            },
            goal: None,
        };
        match held.is_empty() {
            true => None,
            false => Some(held),
        }
    }

    pub fn replace_checklist(
        &mut self,
        items: Vec<SurfaceChecklistItem>,
        provenance: ChecklistProvenance,
        observed_at: impl Into<String>,
        original_count: Option<usize>,
    ) -> bool {
        self.checklist
            .replace(items, provenance, observed_at, original_count)
    }

    pub fn replace_checklist_with_coverage(
        &mut self,
        items: Vec<SurfaceChecklistItem>,
        provenance: ChecklistProvenance,
        observed_at: impl Into<String>,
        original_count: Option<usize>,
        coverage: SurfaceCoverage,
    ) -> bool {
        self.checklist.replace_with_coverage(
            items,
            provenance,
            observed_at,
            original_count,
            coverage,
        )
    }

    pub fn upsert_checklist_item(
        &mut self,
        item: SurfaceChecklistItem,
        provenance: ChecklistProvenance,
        observed_at: impl Into<String>,
    ) -> bool {
        self.checklist.upsert(item, provenance, observed_at)
    }

    pub fn mark_checklist_stale(&mut self) -> bool {
        self.checklist.mark_stale()
    }

    pub fn mark_retained_checklist_stale(&mut self) -> bool {
        let displayed_changed =
            self.checklist.observation().is_some() && self.checklist.mark_stale();
        let task_changed =
            self.task_checklist.observation().is_some() && self.task_checklist.mark_stale();
        displayed_changed || task_changed
    }

    pub fn set_checklist_carried_from_prior_turn(&mut self, carried: bool) -> bool {
        self.checklist.set_carried_from_prior_turn(carried)
    }

    fn apply_workflow(&mut self, subtype: &str, task_id: &str, event: &Value) -> bool {
        match subtype {
            "task_started" => {
                let started = SurfaceWorkflow {
                    id: task_id.to_string(),
                    name: bounded_text(event, "workflow_name").unwrap_or_default(),
                    description: bounded_text(event, "description"),
                    state: Some(RUNNING.to_string()),
                    phases: Vec::new(),
                };
                upsert_by_id(&mut self.workflows, started)
            }
            "task_progress" => {
                let reported = match event["workflow_progress"].as_array() {
                    Some(entries) => read_workflow_phases(task_id, entries),
                    None => return false,
                };
                match held_named(&mut self.workflows, task_id) {
                    Some(held) => replace_when_changed(&mut held.phases, reported),
                    None => false,
                }
            }
            "task_updated" | "task_notification" => {
                self.close_workflow(task_id, status_reported_by(subtype, event))
            }
            _ => false,
        }
    }

    fn close_workflow(&mut self, task_id: &str, status: Option<&str>) -> bool {
        let closed = match status.and_then(wire_task_state) {
            Some(state) => Some(state.to_string()),
            None => return false,
        };
        let changed = match held_named(&mut self.workflows, task_id) {
            Some(held) => replace_when_changed(&mut held.state, closed),
            None => false,
        };
        if changed {
            self.workflow_omissions += trim_terminal_entries(&mut self.workflows, |entry| {
                matches!(entry.state.as_deref(), Some(DONE | FAILED))
            });
        }
        changed
    }

    fn apply_subagent(&mut self, subtype: &str, task_id: &str, event: &Value) -> bool {
        match subtype {
            "task_started" => {
                let accumulated = held_named(&mut self.subagents, task_id)
                    .map(|held| held.clone())
                    .unwrap_or_default();
                let spawn_choice = event["tool_use_id"]
                    .as_str()
                    .and_then(|call_id| self.pending_subagent_choices.get(call_id));
                let started = started_subagent(&accumulated, task_id, event, spawn_choice);
                upsert_by_id(&mut self.subagents, started)
            }
            "task_progress" => match held_named(&mut self.subagents, task_id) {
                Some(held) => {
                    let progressed = progressed_subagent(held, event);
                    replace_when_changed(held, progressed)
                }
                None => false,
            },
            "task_updated" => {
                self.close_subagent(task_id, status_reported_by(subtype, event), None)
            }
            "task_notification" => self.close_subagent(
                task_id,
                status_reported_by(subtype, event),
                bounded_text(event, "summary"),
            ),
            _ => false,
        }
    }

    fn apply_subagent_launch_answer(&mut self, call_id: &str, event: &Value) -> bool {
        let choice = self
            .pending_subagent_choices
            .remove(call_id)
            .unwrap_or_default();
        let result = &event["tool_use_result"];
        let resolved_model = bounded_text(result, "resolvedModel");
        let resolved_effort = bounded_text(result, "reasoningEffort")
            .or_else(|| bounded_text(result, "reasoning_effort"))
            .or_else(|| bounded_text(result, "effort"));
        let Some(held) = self
            .subagents
            .iter_mut()
            .find(|agent| agent.spawning_call_id.as_deref() == Some(call_id))
        else {
            self.pending_subagent_choices.insert(
                call_id.to_string(),
                SubagentChoice {
                    model: resolved_model.or(choice.model),
                    reasoning_effort: resolved_effort.or(choice.reasoning_effort),
                },
            );
            return false;
        };
        let reported = SurfaceAgent {
            model: resolved_model
                .or_else(|| held.model.clone())
                .or(choice.model),
            reasoning_effort: resolved_effort
                .or_else(|| held.reasoning_effort.clone())
                .or(choice.reasoning_effort),
            ..held.clone()
        };
        replace_when_changed(held, reported)
    }

    fn close_subagent(
        &mut self,
        task_id: &str,
        status: Option<&str>,
        summary: Option<String>,
    ) -> bool {
        let claimed = match status.and_then(wire_task_state) {
            Some(state) => state,
            None => return false,
        };
        let changed = match held_named(&mut self.subagents, task_id) {
            Some(held) => {
                let closed = match claimed {
                    FAILED => SurfaceAgent {
                        state: Some(claimed.to_string()),
                        error: summary.or_else(|| held.error.clone()),
                        ..held.clone()
                    },
                    _ => SurfaceAgent {
                        state: Some(claimed.to_string()),
                        result: summary.or_else(|| held.result.clone()),
                        ..held.clone()
                    },
                };
                replace_when_changed(held, closed)
            }
            None => false,
        };
        if changed {
            self.subagent_omissions += trim_terminal_entries(&mut self.subagents, |entry| {
                matches!(entry.state.as_deref(), Some(DONE | FAILED))
            });
        }
        changed
    }

    fn apply_shell_launch(&mut self, event: &Value, answered_text: &str, now_ms: u64) -> bool {
        let shell_id = match event["tool_use_result"]["backgroundTaskId"].as_str() {
            Some(named) => named.to_string(),
            None => return false,
        };
        let output_path = match output_path_named_in(answered_text) {
            Some(named) => named,
            None => return false,
        };
        self.apply_shell(
            &shell_id,
            ShellReport::Launched {
                output_path,
                started_at: stamped_at(event).unwrap_or(now_ms),
            },
        )
    }

    fn apply_shell(&mut self, shell_id: &str, reported: ShellReport) -> bool {
        let changed = match reported {
            ShellReport::Started { description } => {
                let started = SurfaceShell {
                    id: shell_id.to_string(),
                    description,
                    state: Some(RUNNING.to_string()),
                    started_at: None,
                    exit_code: None,
                    tail: Vec::new(),
                    closed_by_notification: false,
                };
                upsert_by_id(&mut self.shells, started)
            }
            ShellReport::Launched {
                output_path,
                started_at,
            } => {
                let stamped = match held_named(&mut self.shells, shell_id) {
                    Some(held) => {
                        let launched = SurfaceShell {
                            started_at: held.started_at.or(Some(started_at)),
                            ..held.clone()
                        };
                        replace_when_changed(held, launched)
                    }
                    None => return false,
                };
                let named_before = self
                    .shell_outputs
                    .insert(shell_id.to_string(), output_path.clone());
                stamped || named_before.as_ref() != Some(&output_path)
            }
            ShellReport::Closed { state, exit_code } => {
                match held_named(&mut self.shells, shell_id) {
                    Some(held) => {
                        let closed = SurfaceShell {
                            state: Some(shell_state_for(exit_code, state).to_string()),
                            exit_code,
                            closed_by_notification: true,
                            ..held.clone()
                        };
                        replace_when_changed(held, closed)
                    }
                    None => false,
                }
            }
            ShellReport::StatusChanged { state } => match held_named(&mut self.shells, shell_id) {
                Some(held) => {
                    let settled = shell_state_for(held.exit_code, state).to_string();
                    replace_when_changed(&mut held.state, Some(settled))
                }
                None => false,
            },
            ShellReport::Tailed(tail) => match held_named(&mut self.shells, shell_id) {
                Some(held) => {
                    let marked = match held.closed_by_notification {
                        true => None,
                        false => tail.exit_code,
                    };
                    let tailed = SurfaceShell {
                        tail: tail.lines,
                        exit_code: marked.or(held.exit_code),
                        state: match marked {
                            Some(_) => Some(shell_state_for(marked, DONE).to_string()),
                            None => held.state.clone(),
                        },
                        ..held.clone()
                    };
                    replace_when_changed(held, tailed)
                }
                None => false,
            },
        };
        if changed {
            self.shell_omissions += trim_terminal_entries(&mut self.shells, |entry| {
                matches!(entry.state.as_deref(), Some(DONE | FAILED))
            });
            let terminal = self
                .shells
                .iter()
                .find(|entry| entry.id == shell_id)
                .map(|entry| matches!(entry.state.as_deref(), Some(DONE | FAILED)))
                .unwrap_or(true);
            if terminal {
                self.shell_outputs.remove(shell_id);
            }
        }
        changed
    }

    fn apply_checklist_call(&mut self, tool: &str, block: &Value) -> bool {
        match tool {
            "TaskCreate" => {
                let call_id = match block["id"].as_str() {
                    Some(named) => named.to_string(),
                    None => return false,
                };
                let input = &block["input"];
                self.pending_checklist_creates.insert(
                    call_id,
                    PendingChecklistCreate {
                        subject: bounded_text(input, "subject").unwrap_or_default(),
                        description: bounded_text(input, "description"),
                    },
                );
                false
            }
            "TodoWrite" => {
                if let Some(checklist) = self.claude_checklist.as_mut() {
                    checklist.stage_todo_write(block);
                }
                false
            }
            _ => false,
        }
    }

    fn apply_checklist_answer(&mut self, tool: &str, call_id: &str, event: &Value) -> bool {
        match tool {
            "TaskCreate" => {
                self.drain_pending_create_and_append(call_id, &event["tool_use_result"]["task"])
            }
            "TaskUpdate" => self.apply_status_change(&event["tool_use_result"]),
            "TodoWrite" => {
                let succeeded = tool_result_succeeded(event, call_id);
                let confirmed = self
                    .claude_checklist
                    .as_mut()
                    .and_then(|checklist| checklist.finish_todo_write(call_id, succeeded));
                match confirmed {
                    Some(confirmed) => {
                        let original_count = confirmed.original_count;
                        let coverage = match confirmed.truncated {
                            true => SurfaceCoverage::Partial,
                            false => SurfaceCoverage::Complete,
                        };
                        let items = confirmed
                            .todos
                            .into_iter()
                            .enumerate()
                            .map(|(position, todo)| todo_item(position, todo))
                            .collect();
                        let collection_epoch = self
                            .checklist
                            .provenance()
                            .filter(|provenance| {
                                provenance.source == ChecklistSource::TodoWrite
                                    && provenance.provider_session_generation
                                        == self.provider_session_generation
                                    && self.checklist.observation().is_some_and(|observation| {
                                        observation.freshness() == Some(SurfaceFreshness::Current)
                                    })
                            })
                            .map_or(confirmed.collection_epoch, |provenance| {
                                provenance.collection_epoch
                            });
                        self.replace_checklist_with_coverage(
                            items,
                            ChecklistProvenance::new(
                                ChecklistSource::TodoWrite,
                                self.provider_session_generation,
                                None,
                                collection_epoch,
                            ),
                            receipt_time(),
                            Some(original_count),
                            coverage,
                        )
                    }
                    None => false,
                }
            }
            _ => false,
        }
    }

    fn drain_pending_create_and_append(&mut self, call_id: &str, task: &Value) -> bool {
        let described = self.pending_checklist_creates.remove(call_id);
        let id = match task["id"].as_str() {
            Some(named) => named.to_string(),
            None => return false,
        };
        let created = SurfaceChecklistItem {
            id,
            subject: bounded_text(task, "subject")
                .or_else(|| described.as_ref().map(|pending| pending.subject.clone()))
                .unwrap_or_default(),
            description: described.and_then(|pending| pending.description),
            state: Some(ChecklistState::Pending),
        };
        let changed = self.task_checklist.upsert(
            created,
            ChecklistProvenance::new(
                ChecklistSource::TaskCreate,
                self.provider_session_generation,
                None,
                0,
            ),
            receipt_time(),
        );
        self.project_task_checklist(changed)
    }

    fn apply_status_change(&mut self, answered: &Value) -> bool {
        let restated = match answered["statusChange"]["to"].as_str() {
            Some(claimed) => Some(ChecklistState::from_provider(claimed)),
            None => return false,
        };
        let task_id = match answered["taskId"].as_str() {
            Some(named) => named.to_string(),
            None => return false,
        };
        let Some(held) = self
            .task_checklist
            .items()
            .iter()
            .find(|item| item.id == task_id)
        else {
            return false;
        };
        let changed = SurfaceChecklistItem {
            state: restated,
            ..held.clone()
        };
        let changed = self.task_checklist.upsert(
            changed,
            ChecklistProvenance::new(
                ChecklistSource::TaskCreate,
                self.provider_session_generation,
                None,
                0,
            ),
            receipt_time(),
        );
        self.project_task_checklist(changed)
    }

    fn project_task_checklist(&mut self, task_collection_changed: bool) -> bool {
        let already_current = self.checklist.provenance().is_some_and(|provenance| {
            provenance.source == ChecklistSource::TaskCreate
                && provenance.provider_session_generation == self.provider_session_generation
                && provenance.collection_epoch == 0
        }) && self
            .checklist
            .observation()
            .is_some_and(|observation| observation.freshness() == Some(SurfaceFreshness::Current));
        if !task_collection_changed && already_current {
            return false;
        }
        self.checklist = self.task_checklist.clone();
        true
    }
}

#[cfg(test)]
mod tests;
