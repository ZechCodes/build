use crate::app::{chosen_option_id, require_str, AppState};
use crate::store::now_rfc3339;
use serde_json::{json, Value};

mod routing;

pub use routing::RoutedCapture;
pub(in crate::app) use routing::{capture_after_routing, RouteRecorded};

/// A capture as it ships: the stored record itself, so the wire and the store
/// can never disagree about what the user said.
pub(in crate::app) fn capture_json(capture: &crate::capture::Capture) -> Value {
    serde_json::to_value(capture).expect("a capture always serializes")
}

impl AppState {
    /// Re-attach every stored capture on boot, putting a route that was in
    /// flight back where the router picks work up. The reset is written, not
    /// merely remembered: the router re-fires from the record, so the record is
    /// what has to say the route never finished.
    ///
    /// The re-fire happens here too: the router process died with the daemon,
    /// and nobody else is going to ask again. A capture holding an unanswered
    /// question is left waiting — the answer is what lets a router decide — and
    /// a failed one keeps its one-tap retry with the user. A re-fire that
    /// cannot start (no projects yet, say) is logged, not fatal: the capture is
    /// safe on disk either way, which is the whole point of capture-first.
    pub(in crate::app) fn recover_captures(
        &mut self,
        captures: Vec<crate::capture::Capture>,
    ) -> Result<(), String> {
        let mut refire = Vec::new();
        for stored in captures {
            let recovered = stored.recovered_at_boot();
            if recovered != stored {
                self.require_store()?
                    .save_capture(&recovered)
                    .map_err(|e| e.to_string())?;
            }
            if recovered.state == crate::capture::CaptureState::Unrouted
                && !recovered.awaiting_answer()
            {
                refire.push(recovered.id.clone());
            }
            self.captures.insert(recovered.id.clone(), recovered);
        }
        for capture_id in refire {
            if let Err(error) = self.begin_routing(&capture_id) {
                eprintln!("boot: could not re-fire routing for {capture_id}: {error}");
            }
        }
        Ok(())
    }

    /// `capture.create` — keep what the user said, then decide about it.
    ///
    /// The record is on disk before this answers, and the answer IS the record.
    /// Routing is triggered off the stored capture afterwards, so a router that
    /// never starts, never answers, or dies mid-decision costs a routing
    /// decision and never the text: the only part of a capture the user cannot
    /// produce again.
    pub(crate) fn capture_create(&mut self, params: &Value) -> Result<Value, String> {
        let said = require_str(params, "text")?;
        let text = said.trim();
        if text.is_empty() {
            return Err("capture.create: text is empty — there is nothing to keep".to_string());
        }
        let capture =
            crate::capture::Capture::new(crate::capture::new_capture_id(), text, now_rfc3339());
        self.require_store()?
            .save_capture(&capture)
            .map_err(|e| e.to_string())?;
        let capture_id = capture.id.clone();
        self.captures.insert(capture_id.clone(), capture);
        // Only now: the text is safe, so a router that never starts costs a
        // routing decision and nothing else. A failure to start IS the routing
        // decision failing, and the capture says so rather than sitting in a
        // state nothing will ever move it out of.
        if let Err(error) = self.begin_routing(&capture_id) {
            eprintln!("capture {capture_id}: could not start the router: {error}");
            self.mark_routing_failed(&capture_id);
        }
        self.capture_get(&json!({ "capture_id": capture_id }))
    }

    /// `capture.answer` — the user answers the router's question, and the
    /// router looks at the capture again with the answer in hand.
    ///
    /// The answer is either words they typed (`text`) or one of the options the
    /// router offered (`option_id`, or `option_index` counting from the first
    /// one offered). A tapped option reaches the router as words too: the label
    /// the user saw and the destination it stood for.
    pub(crate) fn capture_answer(&mut self, params: &Value) -> Result<Value, String> {
        let capture_id = require_str(params, "capture_id")?;
        let capture = self
            .captures
            .get(&capture_id)
            .ok_or_else(|| format!("unknown capture_id: {capture_id}"))?;
        let answered = match chosen_option_id(capture, params)? {
            Some(option_id) => capture.answered_with_option(&option_id)?,
            None => {
                let text = require_str(params, "text")?;
                let text = text.trim();
                if text.is_empty() {
                    return Err("capture.answer: text is empty — that answers nothing".to_string());
                }
                capture.answered(text)?
            }
        };
        self.save_capture(answered)?;
        if let Err(error) = self.begin_routing(&capture_id) {
            eprintln!("capture {capture_id}: could not re-fire the router: {error}");
            self.mark_routing_failed(&capture_id);
        }
        self.capture_get(&json!({ "capture_id": capture_id }))
    }

    /// `capture.cancel` — the user abandons a capture rather than answering it.
    ///
    /// The counterpart of every question: a decision surface with no way out
    /// leaves the user answering a question they have stopped caring about. The
    /// router working on it is stopped, and the record goes — a capture nobody
    /// wants routed is not a row anybody should have to look at again.
    ///
    /// Only while the capture is still its own presence. Once it became an
    /// issue or a branch, that work is what there is to cancel, and it is
    /// cancelled where it lives.
    pub(crate) fn capture_cancel(&mut self, params: &Value) -> Result<Value, String> {
        let capture_id = require_str(params, "capture_id")?;
        let capture = self
            .captures
            .get(&capture_id)
            .ok_or_else(|| format!("unknown capture_id: {capture_id}"))?;
        if !capture.is_on_the_feed() {
            return Err(format!(
                "capture.cancel: this capture already became {}; cancel that instead",
                capture
                    .routing
                    .as_ref()
                    .map(|routing| routing.kind.as_str())
                    .unwrap_or("work")
            ));
        }
        self.abandon_router_session(&capture_id);
        self.require_store()?
            .delete_capture(&capture_id)
            .map_err(|e| e.to_string())?;
        self.captures.remove(&capture_id);
        Ok(json!({ "capture_id": capture_id, "cancelled": true }))
    }

    /// `capture.reroute` — the user moves a capture the router got wrong.
    ///
    /// With a `project_id` this routes by hand, through the very internals the
    /// router's own tools use: one path to a destination, so a manual route and
    /// a routed one are the same kind of thing afterwards. With none it re-fires
    /// the router, which is what the one-tap retry on a failed route is.
    pub(crate) fn capture_reroute(&mut self, params: &Value) -> Result<Value, String> {
        let capture_id = require_str(params, "capture_id")?;
        if !self.captures.contains_key(&capture_id) {
            return Err(format!("unknown capture_id: {capture_id}"));
        }
        let Some(project_id) = params
            .get("project_id")
            .and_then(Value::as_str)
            .filter(|project_id| !project_id.is_empty())
            .map(str::to_string)
        else {
            self.begin_routing(&capture_id)?;
            return self.capture_get(&json!({ "capture_id": capture_id }));
        };
        let kind = params
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or(crate::capture::CaptureTarget::Issue.as_str())
            .to_string();
        if kind == crate::capture::CaptureTarget::Issue.as_str() {
            return Err(crate::app::issues::ISSUES_RETIRED_ERROR.to_string());
        }
        // A router still deciding this capture would route it a second time on
        // top of the user's own choice.
        self.abandon_router_session(&capture_id);
        // The branch the user named, if they named one. Without it the branch
        // is named after what was said — the same rule the router dispatches by.
        let branch = params
            .get("branch")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|branch| !branch.is_empty());
        let text = self.captures[&capture_id].text.clone();
        let rationale = Some("rerouted by the user".to_string());
        match kind.as_str() {
            // The dispatch's git runs through the drain, so the row this answers
            // with is read once the branch is real — in the apply phase, which
            // is where the route is written down.
            "branch" => self.route_to_branch(
                &capture_id,
                &project_id,
                branch,
                &text,
                rationale,
                capture_after_routing,
            ),
            other => Err(format!(
                "capture.reroute: {other:?} is not a destination — branch is the supported work"
            )),
        }
    }

    pub(crate) fn capture_list(&self) -> Value {
        let captures: Vec<Value> = self
            .captures_oldest_first()
            .into_iter()
            .map(capture_json)
            .collect();
        json!({ "captures": captures })
    }

    pub(crate) fn capture_get(&self, params: &Value) -> Result<Value, String> {
        let capture_id = require_str(params, "capture_id")?;
        let capture = self
            .captures
            .get(&capture_id)
            .ok_or_else(|| format!("unknown capture_id: {capture_id}"))?;
        Ok(capture_json(capture))
    }

    /// Every capture in the order it was said. The map is keyed by id, and the
    /// order captures arrived in is the order they read in.
    fn captures_oldest_first(&self) -> Vec<&crate::capture::Capture> {
        let mut captures: Vec<&crate::capture::Capture> = self.captures.values().collect();
        captures.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        captures
    }

    /// The feed rows for captures that are still their own presence: unrouted,
    /// being routed, failed, or holding a question nobody has answered. A
    /// routed, quiet capture is spoken for by the issue or branch it became.
    pub(in crate::app) fn capture_candidates(&self) -> Vec<crate::branch::WorkItemCandidate> {
        self.captures_oldest_first()
            .into_iter()
            .filter(|capture| capture.is_on_the_feed())
            .map(|capture| self.capture_candidate(capture))
            .collect()
    }

    fn capture_candidate(
        &self,
        capture: &crate::capture::Capture,
    ) -> crate::branch::WorkItemCandidate {
        let routing = capture.routing.as_ref();
        let project_id = routing.map(|routing| routing.project_id.clone());
        let issue_id = routing
            .filter(|routing| routing.kind == crate::capture::CaptureTarget::Issue)
            .map(|routing| routing.target_id.clone());
        let branch = routing
            .filter(|routing| routing.kind == crate::capture::CaptureTarget::Branch)
            .map(|routing| routing.target_id.clone());
        let reason = capture.unread_reason();
        let row = json!({
            "kind": crate::branch::WorkItemKind::Capture.as_str(),
            "capture_id": capture.id,
            "project_id": project_id.clone().unwrap_or_default(),
            "project": project_id
                .as_deref()
                .map(|project_id| self.project_name_by_id(project_id))
                .unwrap_or_default(),
            "branch": branch,
            "title": capture.title(),
            "text": capture.text,
            "state": capture.state.as_str(),
            "created_at": capture.created_at,
            "unread": reason.is_some(),
            "unread_count": u32::from(reason.is_some()),
            "unread_reason": reason,
            // A capture is working exactly while the router has it: there are
            // no agents under it to time, so there is no working clock either.
            "working": capture.state == crate::capture::CaptureState::Routing,
            "working_time": Value::Null,
            "agents": Vec::<Value>::new(),
            "stat": Value::Null,
            // What the user said is what they last touched: a capture sorts by
            // when it was taken until it becomes work with a life of its own.
            "resume_at": capture.created_at,
            // The oldest anchor there is, and the one the issue or branch this
            // becomes will inherit.
            "anchor": capture.anchor(),
            "last_activity": capture
                .question
                .as_ref()
                .map(|question| question.asked_at.clone())
                .unwrap_or_else(|| capture.created_at.clone()),
            // Nothing to archive and nothing to silence: a capture leaves the
            // feed by being routed, not by being dismissed. Every row still
            // carries the preflight, so a client can read it without asking
            // what kind of row it is first.
            "can_finish": false,
            "finish": { "warnings": [] },
            "muted": false,
            "dismissed": false,
            "worktree_path": Value::Null,
            "worktree_id": Value::Null,
            "run_id": Value::Null,
            "issue_id": issue_id,
            "routing": routing,
            "question": capture.question,
            "progress": capture.progress,
        });
        crate::branch::WorkItemCandidate {
            kind: crate::branch::WorkItemKind::Capture,
            key: crate::branch::WorkItemKey::Capture {
                capture_id: capture.id.clone(),
            },
            source: None,
            issue_id: None,
            implementation_active: false,
            row,
        }
    }

    /// Write a capture through: the store first, then the map, so the record on
    /// disk is never behind the one this process is answering from.
    pub(in crate::app) fn save_capture(
        &mut self,
        capture: crate::capture::Capture,
    ) -> Result<(), String> {
        self.require_store()?
            .save_capture(&capture)
            .map_err(|e| e.to_string())?;
        self.captures.insert(capture.id.clone(), capture);
        // A capture is a feed row from the moment it is taken, and it moves
        // again at every step of the route it is on.
        self.note_board_changed();
        Ok(())
    }

    /// The router gave up, or never got started. Quiet, because every caller is
    /// already reporting the failure that led here.
    pub(in crate::app) fn mark_routing_failed(&mut self, capture_id: &str) {
        let Some(failed) = self
            .captures
            .get(capture_id)
            .map(crate::capture::Capture::routing_failed)
        else {
            return;
        };
        if let Err(error) = self.save_capture(failed) {
            eprintln!("capture {capture_id}: could not record the failed route: {error}");
        }
    }
}
