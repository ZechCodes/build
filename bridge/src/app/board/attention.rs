use crate::app::{named_agent_id, require_str, thread_cursor, thread_detail};
use crate::run::RunState;
use crate::store::now_rfc3339;
use crate::thread::ThreadDetail;

use serde_json::{json, Value};

use super::super::{AppState, EntitylessRow, ReadReport};

/// What a mutation tail found on the conversation it just wrote: the thread it
/// looked at, how far that thread has got, and the newest attention-class item
/// to land since a tail last looked (`None` when nothing did, or when this is
/// the first look at that conversation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct ConversationNews {
    pub(in crate::app) thread_id: String,
    pub(in crate::app) sequence: u64,
    pub(in crate::app) attention_reason: Option<&'static str>,
}

impl AppState {
    #[cfg(test)]
    pub(in crate::app) fn has_attention(&self, entity_id: &str) -> bool {
        self.board.attention().attention(entity_id).is_some()
    }

    /// Move `entity_state_changed_at` only when the entity's wire state
    /// actually differs from the last one a mutation tail observed. A fresh
    /// entity's first mutation stamps it — creation is a state change.
    pub(in crate::app) fn stamp_state_change(
        &mut self,
        entity_id: &str,
        state: String,
        now: String,
    ) {
        self.board
            .attention_mut()
            .observe_state(entity_id, state, now);
    }

    /// Record where every live conversation has got to, without pushing. Called
    /// once at boot: the store holds history that was announced when it
    /// happened, and a restart must not announce it again.
    pub(in crate::app) fn seed_conversation_attention_sequences(&mut self) {
        let addresses: Vec<(String, String)> = self
            .plans
            .iter()
            .map(|(id, plan)| (id, &plan.agents))
            .chain(self.runs.iter().map(|(id, run)| (id, &run.agents)))
            .flat_map(|(id, roster)| {
                roster
                    .iter()
                    .map(|agent| (id.clone(), agent.id.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        let sequences: Vec<_> = addresses
            .into_iter()
            .filter_map(|(entity_id, agent_id)| {
                self.agent_conversation(&entity_id, Some(&agent_id))
                    .ok()
                    .map(|thread| (thread.id.clone(), thread.last_sequence()))
            })
            .collect();
        for (conversation_id, sequence) in sequences {
            self.board
                .attention_mut()
                .seed_conversation(conversation_id, sequence);
        }
    }

    /// Boot migration: give every stored entity the inbox anchor it would have
    /// had, and leave every anchored one exactly where it is.
    ///
    /// Seeding at creation alone would file a two-week-old issue you picked
    /// back up yesterday under two weeks ago, so the seed is walked forward
    /// through the user messages its conversation already holds — the same rule
    /// a live message goes through, replayed over the history that predates it.
    /// The first boot after this ships does the work; every boot after finds
    /// the anchors it wrote and does nothing.
    pub(in crate::app) fn seed_anchors_for_records_without_one(&mut self) {
        let histories: Vec<(String, String, Vec<String>)> = self
            .plans
            .iter()
            .map(|(id, plan)| (id, &plan.agents))
            .chain(self.runs.iter().map(|(id, run)| (id, &run.agents)))
            .filter(|(id, _)| {
                self.board
                    .attention()
                    .attention(id.as_str())
                    .is_none_or(|attention| attention.anchor_at.is_none())
            })
            .map(|(id, roster)| {
                // Every user message the entity ever had, not the tail the load
                // left resident: an anchor is replayed over the whole history,
                // and one replayed over part of it puts the entry in a place it
                // never had. A history the store cannot hand back leaves the
                // anchor to what is held rather than leaving the entry unseeded.
                let mut said_at: Vec<String> = roster
                    .iter()
                    .flat_map(|agent| {
                        let thread = self
                            .agent_conversation(id, Some(&agent.id))
                            .unwrap_or(&agent.thread);
                        match self.whole_conversation(thread) {
                            Ok(items) => crate::thread::Thread::user_message_times_in(&items)
                                .map(str::to_string)
                                .collect::<Vec<String>>(),
                            Err(error) => {
                                eprintln!("anchor seeding: {error}");
                                thread.user_message_times().map(str::to_string).collect()
                            }
                        }
                    })
                    .collect();
                said_at.sort();
                let created_at = self
                    .board
                    .attention()
                    .clock(id)
                    .created_at
                    .unwrap_or_else(now_rfc3339);
                (id.clone(), created_at, said_at)
            })
            .collect();
        if histories.is_empty() {
            return;
        }
        for (entity_id, created_at, said_at) in histories {
            self.board
                .attention_mut()
                .seed_anchor_from_history(&entity_id, &created_at, &said_at);
        }
        self.persist_attention();
    }

    /// What the mutation tail found on the conversation it just wrote: how far
    /// it has got, and the newest attention-class item to land since a tail
    /// last looked.
    pub(in crate::app) fn conversation_news(
        &self,
        thread: &crate::thread::Thread,
    ) -> ConversationNews {
        let observed = self
            .board
            .attention()
            .conversation_watermark(&thread.id)
            .previous;
        ConversationNews {
            thread_id: thread.id.clone(),
            sequence: thread.last_sequence(),
            attention_reason: observed.and_then(|seen| thread.unread_since(seen).reason),
        }
    }

    /// Fire one content-free web-push notify when an attention-class item lands
    /// on an entity's conversation.
    ///
    /// Event-driven, not state-driven: the state only chooses the kind's label
    /// (a plan at its review gate is `plan_ready`, not a generic attention).
    /// The watermark moves whether or not the push goes out, so one piece of
    /// news notifies once even when two entities share the conversation.
    pub(in crate::app) fn push_attention_notify(
        &mut self,
        entity_id: &str,
        news: ConversationNews,
        state_kind: Option<&'static str>,
    ) {
        let first_look = self
            .board
            .attention_mut()
            .advance_conversation(news.thread_id, news.sequence);
        if first_look || self.notifier.is_none() {
            return;
        }
        if let Some(kind) = self.attention_push_kind(entity_id, news.attention_reason, state_kind) {
            self.spawn_notify(entity_id.to_string(), kind);
        }
    }

    /// What one piece of news pushes as, or `None` when the phone stays dark: a
    /// muted entry, news that needs nobody, a reason with no push label, or a
    /// second push inside the entity's debounce window.
    ///
    /// Mute is checked before the window is spent, so a silence costs nothing:
    /// the first news after unmuting pushes instead of sitting out a window it
    /// never entered.
    pub(in crate::app) fn attention_push_kind(
        &mut self,
        entity_id: &str,
        reason: Option<&str>,
        state_kind: Option<&'static str>,
    ) -> Option<&'static str> {
        if self.is_muted(entity_id) {
            return None;
        }
        let kind = crate::notify::kind_for_attention(reason?, state_kind)?;
        self.notify_throttle
            .should_notify(entity_id, crate::notify::unix_seconds())
            .then_some(kind)
    }

    /// Spawn the actual notify POST off the app lock. A delivery failure only
    /// logs — it never blocks the mutation.
    pub(in crate::app) fn spawn_notify(&self, entity_id: String, kind: &'static str) {
        let Some(notifier) = &self.notifier else {
            return;
        };
        let notifier = notifier.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    if let Err(e) = notifier.notify(&entity_id, kind).await {
                        eprintln!("push notify: {e}");
                    }
                });
            }
            Err(_) => eprintln!("push notify: no async runtime; skipped"),
        }
    }

    pub(in crate::app) fn observe_conversation_working(&mut self, entity_id: &str, now: &str) {
        let working = self.entity_effectively_working(entity_id);
        self.observe_working_state(entity_id, working, now);
    }

    pub(in crate::app) fn entity_effectively_working(&self, entity_id: &str) -> bool {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return false;
        };
        roster.iter().any(|agent| agent.working_since.is_some())
    }

    pub(in crate::app) fn observe_working_state(
        &mut self,
        entity_id: &str,
        working: bool,
        now: &str,
    ) {
        if self
            .board
            .attention_mut()
            .observe_working(entity_id, working, now)
        {
            self.persist_attention();
        }
    }

    /// Record that the human acted on `id`, now.
    pub(in crate::app) fn touch_attention(&mut self, id: &str) {
        let now = now_rfc3339();
        self.board.attention_mut().interact(id, &now);
        self.persist_attention();
    }

    /// Record that the human has seen `id` as of its current state clock, and
    /// has read its conversations through to the end — every agent's, or one
    /// named agent's. The read cursors are what unread is derived against, so
    /// this is the one place a badge clears.
    ///
    /// `report` is what the panel claims to have read — the window it holds and
    /// the message its viewport reached. See
    /// [`conversation_last_sequences`](Self::conversation_last_sequences).
    pub(in crate::app) fn see_attention(
        &mut self,
        id: &str,
        agent_id: Option<&str>,
        report: ReadReport,
    ) {
        let Some(state_changed_at) = self.entity_state_clock(id) else {
            return;
        };
        let read_through = self.conversation_last_sequences(id, agent_id, report);
        self.board
            .attention_mut()
            .mark_seen(id, Some(&state_changed_at), &read_through);
        self.persist_attention();
    }

    /// How far each of an entity's conversations has got, as
    /// `(agent_id, sequence)` — for one agent when the caller named one.
    ///
    /// The entity-level conversation of a planned run is its Issue's, so its
    /// first agent is read through to the end of THAT thread; every other agent
    /// speaks in its own.
    ///
    /// A conversation reaches a client as a window on its newest items, and
    /// `read_from_sequence` is where that window starts. The end of it is not
    /// the end of the conversation, so a report from such a reader carries no
    /// claim about the items below the floor: an unread message calling the
    /// human down there leaves that conversation out of the answer entirely,
    /// keeping its badge until the reader scrolls back far enough to be sent
    /// it. A caller that names no floor is one holding the whole conversation.
    pub(in crate::app) fn conversation_last_sequences(
        &self,
        entity_id: &str,
        agent_id: Option<&str>,
        report: ReadReport,
    ) -> Vec<(String, u64)> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return Vec::new();
        };
        roster
            .iter()
            .filter(|agent| agent_id.is_none_or(|named| named == agent.id))
            .filter_map(|agent| {
                let thread = self
                    .agent_conversation(entity_id, Some(&agent.id))
                    .unwrap_or(&agent.thread);
                let hidden_below = report.window_floor.is_some_and(|floor| {
                    thread.unread_attention_below(floor, self.read_cursor(entity_id, &agent.id))
                });
                (!hidden_below).then(|| (agent.id.clone(), report.reached(thread.last_sequence())))
            })
            .collect()
    }

    /// Where each of an entity's conversations stands, as
    /// `(agent_id, last_attention_sequence)` in roster order — the lines a
    /// dismissal draws, and the lines it is judged against afterwards.
    ///
    /// The entity-level conversation of a planned run is its Issue's, so the
    /// first agent's line is drawn in THAT thread; every other agent's in its
    /// own. The first pair is the roster's first agent — the only one the
    /// pre-agent dismissal folds onto.
    ///
    /// A hand-off does not draw a line and does not cross one: one agent's
    /// words in another agent's conversation are the work happening, not the
    /// row calling the human back.
    pub(in crate::app) fn dismissal_lines(&self, entity_id: &str) -> Vec<(String, u64)> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return Vec::new();
        };
        roster
            .iter()
            .map(|agent| {
                let thread = self
                    .agent_conversation(entity_id, Some(&agent.id))
                    .unwrap_or(&agent.thread);
                (agent.id.clone(), thread.last_own_message_sequence())
            })
            .collect()
    }

    pub(in crate::app) fn migrate_legacy_dismissals(&mut self) {
        let entity_ids: Vec<String> = self.plans.keys().chain(self.runs.keys()).cloned().collect();
        let mut changed = false;
        for id in entity_ids {
            let Some(attention) = self.board.attention().attention(&id) else {
                continue;
            };
            if attention.dismissal_tracks_messages {
                continue;
            }
            let message_lines = self.dismissal_lines(&id);
            let old_lines = self.legacy_dismissal_lines(&id);
            let was_still_dismissed = !old_lines.is_empty()
                && old_lines.iter().enumerate().all(
                    |(position, (agent_id, latest_attention_sequence))| {
                        attention.is_dismissed_for(
                            agent_id,
                            position == 0,
                            *latest_attention_sequence,
                        )
                    },
                );
            changed |= self.board.attention_mut().migrate_dismissal_to_messages(
                &id,
                &message_lines,
                was_still_dismissed,
            );
        }
        if changed {
            self.persist_attention();
        }
    }

    pub(in crate::app) fn legacy_dismissal_lines(&self, entity_id: &str) -> Vec<(String, u64)> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return Vec::new();
        };
        roster
            .iter()
            .map(|agent| {
                let thread = self
                    .agent_conversation(entity_id, Some(&agent.id))
                    .unwrap_or(&agent.thread);
                (agent.id.clone(), thread.last_attention_sequence())
            })
            .collect()
    }

    /// The conversation an entity's own surfaces render: its currently implicit
    /// addressed agent's stable binding.
    pub(in crate::app) fn entity_conversation(
        &self,
        entity_id: &str,
    ) -> Option<&crate::thread::Thread> {
        self.agent_conversation(entity_id, None).ok()
    }

    /// The conversation a caller means: the agent it named, or the entity's
    /// implicit primary for legacy callers. The resolved agent's persisted
    /// binding, never its current roster position, selects the history.
    pub(in crate::app) fn agent_conversation(
        &self,
        entity_id: &str,
        agent_id: Option<&str>,
    ) -> Result<&crate::thread::Thread, String> {
        let address = self.resolve_conversation_address(entity_id, agent_id)?;
        self.conversation_at(&address)
    }

    /// The `thread` a detail poll ships when it asked for one in particular:
    /// the named agent's conversation, cut to what the poll can hold — only
    /// what has happened past the cursor it already holds, or as much of the
    /// newest conversation as its `thread_limit` allows. `None` when the poll
    /// named no agent and carried no cursor — the view's own thread already is
    /// exactly that.
    pub(in crate::app) fn detail_thread_value(
        &self,
        entity_id: &str,
        params: &Value,
    ) -> Result<Option<Value>, String> {
        let addressed = named_agent_id(params)?;
        let cursor = thread_cursor(params);
        if addressed.is_none() && cursor.is_none() && params.get("conversation_id").is_none() {
            return Ok(None);
        }
        let address = self.resolve_conversation_params(entity_id, params)?;
        let thread = self.conversation_at(&address)?;
        Ok(Some(match cursor {
            // A conversation is loaded as its tail, so a cursor from before a
            // restart can be owed news memory does not hold: an item under the
            // tail that the process before this one mutated in place. Where it
            // is, the delta is completed out of the store.
            Some(after_sequence) if thread.cursor_reaches_stored_history(after_sequence) => {
                self.stored_thread_delta(thread, after_sequence)?
            }
            Some(after_sequence) => thread.wire_value_after(after_sequence),
            None => match thread_detail(params) {
                ThreadDetail::Page(limit) => self.thread_page_at(thread, None, limit)?,
                _ => thread.wire_value(),
            },
        }))
    }

    /// What an entry says about itself in the inbox: whether an attention-class
    /// item landed past the human's read cursor, how many, and why the newest
    /// one needs them.
    ///
    /// This is the whole of `needs_attention` now. A state that needs the human
    /// is only an input to it, by way of the event that state transition emits.
    pub(in crate::app) fn unread_for(
        &self,
        entity_id: &str,
        thread: Option<&crate::thread::Thread>,
    ) -> crate::thread::UnreadSummary {
        // Muted is told here rather than at the cursor: the entry says nothing
        // is waiting while the cursor keeps the truth, so unmuting shows what
        // arrived instead of a conversation silently marked read.
        if self.is_muted(entity_id) {
            return crate::thread::UnreadSummary::default();
        }
        // The entry's badge is the union of its agents': the first agent's
        // count comes off the conversation the entity's own surfaces render
        // (an Issue's, for a planned implementation), every other agent's off
        // its own.
        let Ok(roster) = self.entity_agents(entity_id) else {
            return match thread {
                Some(thread) => self.unread_including_history(
                    entity_id,
                    thread,
                    self.read_cursor(entity_id, ""),
                ),
                None => crate::thread::UnreadSummary::default(),
            };
        };
        let mut summary = crate::thread::UnreadSummary::default();
        for agent in roster.iter() {
            let agent_thread = self
                .agent_conversation(entity_id, Some(&agent.id))
                .unwrap_or_else(|_| thread.unwrap_or(&agent.thread));
            let unread = self.unread_including_history(
                &agent_thread.agent.id,
                agent_thread,
                self.read_cursor(entity_id, &agent.id),
            );
            summary.count += unread.count;
            summary.reason = unread.reason.or(summary.reason);
        }
        summary
    }

    /// What one agent's bubble says: how much of its conversation has needed
    /// the human since they last read it. A muted entry silences every bubble
    /// under it — the entry's badge is the union of theirs, so one that still
    /// counted would contradict the entry above it.
    pub(in crate::app) fn agent_unread(
        &self,
        entity_id: &str,
        agent: &crate::agent::Agent,
        thread: &crate::thread::Thread,
    ) -> crate::thread::UnreadSummary {
        if self.is_muted(entity_id) {
            return crate::thread::UnreadSummary::default();
        }
        self.unread_including_history(
            &thread.agent.id,
            thread,
            self.read_cursor(entity_id, &agent.id),
        )
    }

    /// One conversation's unread, counting the part of it this process did not
    /// load.
    ///
    /// A conversation is held as its newest items, so counting the badge off
    /// what is resident under-reports exactly when it matters most — the human
    /// has not read in a while and the unread has fallen under the tail. The
    /// badge is the one number they use to decide whether to look, so it is
    /// counted in the database rather than guessed from the tail. A
    /// conversation that was loaded whole has no history under it and asks
    /// nothing.
    pub(in crate::app) fn unread_including_history(
        &self,
        agent_id: &str,
        thread: &crate::thread::Thread,
        cursor: u64,
    ) -> crate::thread::UnreadSummary {
        let mut summary = thread.unread_since(cursor);
        let floor = thread.resident_from_sequence();
        if floor == 0 || floor <= cursor {
            return summary;
        }
        let Some(store) = &self.store else {
            return summary;
        };
        match store.unread_attention_between(agent_id, cursor, floor) {
            Ok(under) => summary.count += under,
            // A badge is not worth failing a poll over: report what is
            // resident, which is an undercount rather than a wrong kind of
            // answer.
            Err(error) => eprintln!("unread under the tail for {agent_id}: {error}"),
        }
        summary
    }

    /// How far the human has read one agent's conversation, folding in the
    /// pre-agent cursor the entity's first agent inherited.
    pub(in crate::app) fn read_cursor(&self, entity_id: &str, agent_id: &str) -> u64 {
        let is_primary = self
            .entity_agents(entity_id)
            .is_ok_and(|roster| roster.is_primary(agent_id));
        self.board
            .attention()
            .read_cursor(entity_id, agent_id, is_primary)
    }

    /// The entity's state clock — what a `seen` stamp is versioned against. A
    /// bare worktree has no lifecycle of its own, so seeing it is simply now.
    pub(in crate::app) fn entity_state_clock(&self, id: &str) -> Option<String> {
        Some(
            self.board
                .attention()
                .clock(id)
                .state_changed_at
                .unwrap_or_else(now_rfc3339),
        )
    }

    /// Write the attention map, pruned to the entities that still exist. Cheap
    /// (one small file) and done on every stamp, so a crash costs at most the
    /// action in flight rather than the day's ordering.
    pub(in crate::app) fn persist_attention(&mut self) {
        // The inbox is ordered and coloured by this map, so a stamp on it IS a
        // feed change — an interaction, a seen, a mute, a dismissal. Noted
        // ahead of the write, because the map moved whether or not there is a
        // store under this bridge to write it to.
        self.note_board_changed();
        let Ok(store) = self.require_store() else {
            return;
        };
        let live: std::collections::HashSet<String> = self
            .runs
            .keys()
            .chain(self.plans.keys())
            .cloned()
            .chain(self.attention_worktree_ids())
            .chain(self.live_row_keys())
            .collect();
        let attention = self
            .board
            .attention()
            .persistence()
            .into_entries()
            .into_iter()
            .collect();
        if let Err(e) = store.save_attention(&attention, &live) {
            eprintln!("attention: {e}");
        }
    }

    /// Keys worth keeping for the rows no entity stands behind: the ones whose
    /// project is still registered.
    ///
    /// That is the whole liveness test. The branch such a row names may be
    /// checked out anywhere, or nowhere yet, so pruning its dismissal against a
    /// checkout would lose it every time the user moved one — and a branch that
    /// becomes a run has its record dropped at adoption
    /// ([`forget_row_dismissals`](Self::forget_row_dismissals)) rather than
    /// waiting to be pruned.
    pub(in crate::app) fn live_row_keys(&self) -> Vec<String> {
        self.board
            .attention()
            .attention_ids()
            .filter(|key| {
                crate::attention::RowKey::parse(key)
                    .is_some_and(|row| self.projects.iter().any(|p| p.id == row.project_id()))
            })
            .map(str::to_string)
            .collect()
    }

    /// Worktree ids worth keeping attention for: every one the scan can still
    /// see. Their records live nowhere else, so the scan IS the liveness test —
    /// and a project whose scan has never landed is no evidence that its
    /// checkouts are gone. Until every project has a list, every key that names
    /// a checkout is kept and the write after the first scan prunes; a key of
    /// any other shape answers to the map that owns it either way.
    pub(in crate::app) fn attention_worktree_ids(&self) -> Vec<String> {
        let project_ids: Vec<String> = self
            .projects
            .iter()
            .filter(|project| project.is_git)
            .map(|project| project.id.clone())
            .collect();
        if project_ids.iter().any(|project_id| {
            !self
                .board
                .diff()
                .external_scan(project_id)
                .has_readable_scan
        }) {
            return self
                .board
                .attention()
                .attention_ids()
                .filter(|key| crate::worktree::is_checkout_id(key))
                .map(str::to_string)
                .collect();
        }
        project_ids
            .iter()
            .flat_map(|project_id| {
                self.board
                    .diff()
                    .external_scan(project_id)
                    .worktrees
                    .iter()
                    .map(|worktree| worktree.id.clone())
            })
            .collect()
    }

    /// `entity.seen` — the human has looked at this run/plan/worktree as it
    /// stands. Versioned against the entity's state clock, so a later change
    /// makes it unseen again rather than staying read forever.
    pub(crate) fn entity_seen(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        // A named agent reads one bubble through; no agent reads the whole
        // entry, which is what opening the entry means.
        let named = named_agent_id(params)?;
        let agent_id = if named.is_some() || params.get("conversation_id").is_some() {
            Some(
                self.resolve_conversation_params(&entity_id, params)?
                    .agent_id,
            )
        } else {
            None
        };
        let report = ReadReport {
            window_floor: params.get("read_from_sequence").and_then(Value::as_u64),
            through: params.get("read_through_sequence").and_then(Value::as_u64),
        };
        self.see_attention(&entity_id, agent_id.as_deref(), report);
        Ok(json!({ "ok": true }))
    }

    /// `entity.mute` — the human telling one entry to stop asking, or to start
    /// again.
    ///
    /// A muted entry keeps its place in the inbox with live status: it pushes
    /// nothing and badges nothing, and that is all mute does. The read cursors
    /// are untouched, so unmuting shows exactly what was waiting.
    pub(crate) fn entity_mute(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        let muted = params
            .get("muted")
            .and_then(Value::as_bool)
            .ok_or("entity.mute: muted must be true or false")?;
        if !self.entity_takes_attention(&entity_id) {
            return Err(format!("entity.mute: unknown entity {entity_id}"));
        }
        self.board.attention_mut().set_muted(&entity_id, muted);
        self.persist_attention();
        Ok(json!({ "entity_id": entity_id, "muted": muted }))
    }

    /// `entity.dismiss` — the human clearing one row out of the inbox until the
    /// work speaks again. Every row can be cleared, including the ones nothing
    /// stands behind.
    ///
    /// A row with an entity behind it is named by that entity's id, and the
    /// line is drawn at the end of every owned conversation: the row stays out
    /// of the list until a user or agent sends another message.
    ///
    /// A row with no entity — a branch checked out somewhere Build never cut —
    /// is named by what it IS: `{ project_id, branch }`. With no conversation,
    /// unrelated git changes cannot revive it; adoption gives it a conversation
    /// and identity.
    ///
    /// There is no un-dismiss verb because there is nothing to undo: the next
    /// thing the work says brings the row back by itself, which is the whole
    /// feature.
    ///
    /// Clearing something out of the way is not picking it up, so this is not
    /// an interaction: it moves no anchor and no resume point. It touches no
    /// read cursor (what was waiting is still waiting), no mute (silencing is
    /// mute's job), and no push.
    pub(crate) fn entity_dismiss(&mut self, params: &Value) -> Result<Value, String> {
        if params.get("entity_id").is_none() {
            let row = self.dismissable_row(params)?;
            self.clear_row(&row);
            return Ok(json!({
                "project_id": row.project_id,
                "branch": row.branch,
                "dismissed": true,
            }));
        }
        let entity_id = require_str(params, "entity_id")?;
        if !self.entity_takes_attention(&entity_id) {
            return Err(format!("entity.dismiss: unknown entity {entity_id}"));
        }
        // A checkout Build never cut carries an id but no conversation: it is
        // one of the entity-less rows above wearing the id the feed ships, and
        // it is cleared as that row so both ways of naming it land in one
        // place.
        if let Some(row) = self.checkout_row(&entity_id) {
            self.clear_row(&row);
            return Ok(json!({ "entity_id": entity_id, "dismissed": true }));
        }
        // Every agent on the row gets its own line, drawn where its own
        // conversation stands right now — clearing the row IS reading it, and
        // the client relies on that. One line could never speak for the rest:
        // each agent numbers its conversation from 1, so a sequence taken off
        // the first agent says nothing about where the second one has got to.
        let lines = self.dismissal_lines(&entity_id);
        self.board
            .attention_mut()
            .set_entity_dismissal(&entity_id, &lines);
        self.persist_attention();
        Ok(json!({ "entity_id": entity_id, "dismissed": true }))
    }

    /// Write one entity-less row's dismissal: cleared at the commit it is
    /// sitting on, which is what brings it back.
    pub(in crate::app) fn clear_row(&mut self, row: &EntitylessRow) {
        self.board
            .attention_mut()
            .dismiss_row_at_head(&row.key, row.head.as_deref());
        self.persist_attention();
    }

    /// The entity-less row `{ project_id, branch }` names.
    ///
    /// A branch is resolved against the feed's own sources, so a name that is
    /// not a row anyone can clear is refused rather than written as a
    /// dismissal nothing will ever read.
    pub(in crate::app) fn dismissable_row(
        &mut self,
        params: &Value,
    ) -> Result<EntitylessRow, String> {
        let project_id = params
            .get("project_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                "entity.dismiss: name a row — an entity_id, or a project_id with a branch"
                    .to_string()
            })?;
        if !self.projects.iter().any(|p| p.id == project_id) {
            return Err(format!("entity.dismiss: unknown project {project_id}"));
        }
        let branch = params
            .get("branch")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                format!("entity.dismiss: {project_id} is a project, not a row — name a branch")
            })?;
        if let Some(run_id) = self.unarchived_run_on_branch(&project_id, &branch) {
            return Err(format!(
                "entity.dismiss: {branch} is run {run_id} — clear it by entity_id, so the line \
                 is drawn in its conversation"
            ));
        }
        let checkout = self.find_checkout(
            &project_id,
            &format!("entity.dismiss: {project_id} has no row for {branch}"),
            |checkout| checkout.branch.as_deref() == Some(branch.as_str()),
        )?;
        Ok(EntitylessRow {
            key: crate::attention::branch_row_key(&project_id, &branch),
            head: Some(checkout.head_sha),
            project_id,
            branch: Some(branch),
        })
    }

    /// The entity-less row an external worktree's id names, or `None` when the
    /// id is a run's or an issue's. A checkout on a branch IS that branch's
    /// row; one with no branch is only ever itself.
    pub(in crate::app) fn checkout_row(&mut self, worktree_id: &str) -> Option<EntitylessRow> {
        let project_ids: Vec<String> = self
            .projects
            .iter()
            .filter(|project| project.is_git)
            .map(|project| project.id.clone())
            .collect();
        for project_id in project_ids {
            let Some(checkout) = self
                .external_worktrees(&project_id)
                .worktrees
                .into_iter()
                .find(|worktree| worktree.id == worktree_id)
            else {
                continue;
            };
            return Some(EntitylessRow {
                key: match &checkout.branch {
                    Some(branch) => crate::attention::branch_row_key(&project_id, branch),
                    None => worktree_id.to_string(),
                },
                head: Some(checkout.head_sha),
                project_id,
                branch: checkout.branch,
            });
        }
        None
    }

    /// The run whose ROW holds a branch in this project, if one does — where
    /// that row's dismissal belongs, because a run's line is drawn in its
    /// conversation.
    ///
    /// Wider than [`run_on_branch`](Self::run_on_branch): a merged or abandoned
    /// run keeps its row until it is archived, and a row on the feed is a row
    /// the human can clear.
    pub(in crate::app) fn unarchived_run_on_branch(
        &self,
        project_id: &str,
        branch: &str,
    ) -> Option<String> {
        self.runs
            .iter()
            .find(|(run_id, active)| {
                active.run.state != RunState::Archived
                    && active.worktree.branch() == branch
                    && self.projects.project_id_of(run_id) == Some(project_id)
            })
            .map(|(run_id, _)| run_id.clone())
    }

    /// Forget what was cleared against the entity-less rows a checkout has just
    /// stopped being. These records hold a dismissal and nothing else, so
    /// dropping the record IS forgetting the dismissal.
    pub(in crate::app) fn take_row_dismissal(
        &mut self,
        project_id: &str,
        branch: &str,
    ) -> (bool, Option<String>) {
        let keys = vec![crate::attention::branch_row_key(project_id, branch)];
        let removed = self.board.attention_mut().take_row_attentions(&keys);
        let dismissed = removed
            .iter()
            .any(|attention| attention.is_dismissed_at_head(None));
        let first_observed_at = removed
            .iter()
            .filter_map(|attention| attention.first_observed_at.clone())
            .min();
        if !removed.is_empty() {
            self.persist_attention();
        }
        (dismissed, first_observed_at)
    }

    /// Whether an entity-less row has been cleared out of the inbox: the human
    /// dismissed it, and it is still sitting on the commit they left it on.
    pub(in crate::app) fn row_is_dismissed(&self, key: &str, head: Option<&str>) -> bool {
        self.board.attention().row_is_dismissed(key, head)
    }

    /// Whether `entity_id` names something the attention map keeps a record
    /// for: a run, an issue, or a worktree the scan can still see. Anything
    /// else would be written and pruned in the same breath.
    pub(in crate::app) fn entity_takes_attention(&self, entity_id: &str) -> bool {
        self.runs.contains_key(entity_id)
            || self.plans.contains_key(entity_id)
            || self
                .attention_worktree_ids()
                .iter()
                .any(|id| id == entity_id)
    }

    /// The wire view of a plan (spec §board.list): identity, state, project,
    /// model, timestamps, its stage docs (with open-comment counts), and the
    /// id of its active run if any (single-active-writer → at most one).
    /// `thread_detail` picks a bounded digest (list surfaces) or the full
    /// conversation (detail surfaces + mutation responses).
    /// The rail's two facts about an entity: when this stretch of work on it
    /// began (its sort key) and whether the human has seen where it got to. The
    /// seen comparison happens HERE, against the state clock, so every surface
    /// agrees on it rather than each re-deriving it.
    pub(in crate::app) fn attention_json(&self, id: &str) -> Value {
        let attention = self
            .board
            .attention()
            .attention(id)
            .cloned()
            .unwrap_or_default();
        let clock = self.board.attention().clock(id);
        let created_at = clock.created_at.unwrap_or_else(now_rfc3339);
        let state_changed_at = clock.state_changed_at.unwrap_or_else(|| created_at.clone());
        json!({
            "resume_at": attention.sort_key(&created_at),
            "interacted": attention.last_interaction_at.is_some(),
            "seen": attention.has_seen(&state_changed_at),
            // The inbox's own key, on every surface that renders an entity, so
            // a detail view and the list it was opened from agree about where
            // this piece of work sits.
            "anchor": attention.anchor(&created_at),
        })
    }

    /// Where this entity sits in the inbox.
    ///
    /// The anchor, or — for a record written before anchors that boot has not
    /// reached — the day it was created, which is what the seed would have
    /// made it.
    pub(in crate::app) fn anchor_of(&self, entity_id: &str) -> String {
        let created_at = self
            .board
            .attention()
            .clock(entity_id)
            .created_at
            .unwrap_or_else(now_rfc3339);
        match self.board.attention().attention(entity_id) {
            Some(attention) => attention.anchor(&created_at),
            None => created_at,
        }
    }

    /// Give a newly created entity its place in the inbox. Idempotent, so every
    /// mutation can call it and only the first one does anything.
    pub(in crate::app) fn seed_anchor(&mut self, entity_id: &str) {
        let created_at = self
            .board
            .attention()
            .clock(entity_id)
            .created_at
            .unwrap_or_else(now_rfc3339);
        if !self
            .board
            .attention_mut()
            .seed_anchor(entity_id, &created_at)
        {
            return;
        }
        self.persist_attention();
    }

    /// The user said something to this entity: move its anchor if they had gone
    /// quiet for [`crate::attention::ANCHOR_GAP`], and leave it exactly where it
    /// is otherwise.
    ///
    /// Only the user's own words reach here. An agent filling a conversation
    /// all night is the work happening, and the work happening must never
    /// reorder the inbox under the person reading it.
    ///
    /// Called with the entity still in its map: the attention file is pruned to
    /// what exists when it is written, so a stamp taken while a record is
    /// checked out would be dropped on the way to disk.
    pub(in crate::app) fn note_user_message(&mut self, entity_id: &str) {
        let created_at = self
            .board
            .attention()
            .clock(entity_id)
            .created_at
            .unwrap_or_else(now_rfc3339);
        let now = now_rfc3339();
        let attention = self.board.attention_mut();
        attention.seed_anchor(entity_id, &created_at);
        attention.note_user_message(entity_id, &now);
        self.persist_attention();
    }

    /// Take a capture's anchor onto the work it just became, so the inbox holds
    /// ONE entry for a thing the user said and not two.
    pub(in crate::app) fn inherit_capture_anchor(&mut self, entity_id: &str, capture_id: &str) {
        let Some(capture) = self.captures.get(capture_id) else {
            return;
        };
        let anchor = capture.anchor().to_string();
        self.board
            .attention_mut()
            .inherit_anchor(entity_id, &anchor);
        self.persist_attention();
    }
}
