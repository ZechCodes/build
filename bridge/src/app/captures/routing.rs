use super::capture_json;
#[cfg(test)]
use crate::app::MCP_CONTROL_METHOD;
use crate::app::{
    issue_session, AppState, DeferredJob, PendingAgentTurn, PlanSessionOpening, TabKey, TurnText,
};
use crate::mcp::{BridgeAction, DoneReport, DoneStatus};
use crate::plan::PlanState;
use crate::store::now_rfc3339;
use crate::thread::ThreadDetail;
use serde_json::{json, Value};

/// The capture a dispatch is the destination of. Recorded once the branch it
/// went to is real, which is why it rides the dispatch rather than its caller:
/// a router reaching a branch and a user rerouting one both answer from the
/// apply phase, with no git under the app mutex on the way.
pub struct RoutedCapture {
    pub capture_id: String,
    pub rationale: Option<String>,
    /// What the caller answers with, given the capture as its route was
    /// written — the capture's own record for the user's reroute, which
    /// redraws the row; where the work went for the router's tool, which is
    /// told what it did. Nothing here can refuse: every refusal a dispatch can
    /// make came before the write that made its run real.
    pub answer: fn(&crate::capture::Capture, Value) -> Value,
}

/// A capture's route, written down ahead of the run it names being durable,
/// and what its caller hears once it is.
pub(in crate::app) struct RouteRecorded {
    pub(in crate::app) capture: crate::capture::Capture,
    pub(in crate::app) answered_with: fn(&crate::capture::Capture, Value) -> Value,
}

impl RouteRecorded {
    /// The caller's answer over the dispatch — or the dispatch itself, when no
    /// capture routed it.
    pub(in crate::app) fn answer(route: Option<Self>, dispatched: Value) -> Value {
        match route {
            Some(route) => (route.answered_with)(&route.capture, dispatched),
            None => dispatched,
        }
    }
}

/// A router or a reroute asked, on its way to a destination it has already
/// reached: the capture is routed and the Issue holds the text whatever happens
/// here, so a session that could not start says so and never fails the route.
#[derive(Clone)]
pub(in crate::app) struct RoutedIssueDrafting {
    pub(in crate::app) issue_id: String,
    pub(in crate::app) project_id: String,
    pub(in crate::app) capture_id: String,
    pub(in crate::app) answer: fn(&crate::capture::Capture, Value) -> Value,
}

impl RoutedIssueDrafting {
    /// What the route answers with: the capture as it now stands, or the
    /// destination itself, and whether an agent is reading it.
    fn reply(&self, state: &AppState, planning: bool) -> Result<Value, String> {
        let capture = state
            .captures
            .get(&self.capture_id)
            .ok_or("the capture went while its issue was being opened")?;
        Ok((self.answer)(
            capture,
            json!({
                "issue_id": self.issue_id,
                "project_id": self.project_id,
                // No branch was cut: that is what a router means by dispatched.
                "dispatched": false,
                // An agent IS reading the capture, on the primary checkout.
                "planning": planning,
            }),
        ))
    }
}

impl PlanSessionOpening for RoutedIssueDrafting {
    fn open(
        self: Box<Self>,
        state: &mut AppState,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String> {
        let started =
            state.open_inert_plan_drafting(&self.issue_id, workspace, ThreadDetail::Digest);
        if let Err(error) = &started {
            eprintln!("route: {} could not start planning: {error}", self.issue_id);
        }
        self.reply(state, started.is_ok())
    }

    fn refused(self: Box<Self>, state: &mut AppState, error: String) -> Result<Value, String> {
        eprintln!("route: {} could not start planning: {error}", self.issue_id);
        self.reply(state, false)
    }
}

/// The reroute's answer: the capture as its row now reads.
pub(in crate::app) fn capture_after_routing(
    capture: &crate::capture::Capture,
    _dispatched: Value,
) -> Value {
    capture_json(capture)
}

/// The router tool's answer: where the work went.
fn the_dispatch_itself(_capture: &crate::capture::Capture, dispatched: Value) -> Value {
    dispatched
}

impl AppState {
    /// One router tool, drained — the synchronous twin of
    /// [`AppState::dispatch`], for the tests that speak to the daemon directly
    /// and have no mutex to release. Running it is what the MCP control socket
    /// does with the guard released.
    #[cfg(test)]
    pub(in crate::app) fn router_action(
        &mut self,
        capture_id: &str,
        action: BridgeAction,
    ) -> Result<Value, String> {
        let (answered, deferred) = self.router_deferring(capture_id, action);
        match deferred {
            Some(deferred) => {
                let done = deferred.run();
                self.apply_deferred(MCP_CONTROL_METHOD, &Value::Null, done)
            }
            None => answered,
        }
    }

    /// One router tool, without draining: the router's twin of
    /// [`AppState::dispatch_deferring`]. A tool whose git must not run under
    /// the app mutex hands that work back the same way a verb does, and a tool
    /// that refused leaves the same nothing behind it.
    pub(in crate::app) fn router_deferring(
        &mut self,
        capture_id: &str,
        action: BridgeAction,
    ) -> (Result<Value, String>, Option<DeferredJob>) {
        let queued_before = self.delivery_queue.checkpoint();
        let answered = self.on_router_mcp_action(capture_id, action);
        if answered.is_err() {
            self.drop_turns_queued_since(queued_before);
        }
        (answered, self.take_deferred())
    }

    /// Put the router on a capture: a session in a scratch directory of its
    /// own, with the capture, the decision rule and its tools.
    ///
    /// Single-flight per capture. Everything that can ask for a route — the
    /// capture arriving, an answered question, a manual retry — asks through
    /// here, and a capture already being decided is left to the router deciding
    /// it rather than given a second one that would race it to a destination.
    pub(in crate::app) fn begin_routing(&mut self, capture_id: &str) -> Result<(), String> {
        if self.router_sessions.contains_key(capture_id) {
            return Ok(());
        }
        let capture = self
            .captures
            .get(capture_id)
            .ok_or_else(|| format!("unknown capture_id: {capture_id}"))?
            .clone();
        // The router routes TO projects; with none there is no destination to
        // reach and no point spawning one to discover that.
        let project_id = self.default_project()?;
        let choice = crate::router::router_model_choice(self.router_choice.as_ref());
        let session = crate::router::RouterSession::new(capture_id, &self.state_root, choice)
            .map_err(|error| error.to_string())?;
        std::fs::create_dir_all(session.scratch_dir()).map_err(|error| {
            format!(
                "could not cut router scratch at {}: {error}",
                session.scratch_dir().display()
            )
        })?;
        // Written before the turn is queued: the router reads the record, and
        // the record has to say it is being routed before anything can read it.
        self.save_capture(capture.routing_started())?;
        let prompt = crate::templates::render(
            &crate::templates::Templates::default().router,
            &crate::templates::Vars {
                capture_text: &capture.text,
                user_answer: capture
                    .question
                    .as_ref()
                    .and_then(|question| question.answer.as_deref())
                    .unwrap_or(""),
                ..crate::templates::Vars::default()
            },
        );
        self.projects
            .bind_entity(capture_id.to_string(), project_id);
        self.delivery_queue.enqueue(PendingAgentTurn {
            operation_id: None,
            root: Self::canonical_root(session.scratch_dir()),
            owner: capture_id.to_string(),
            agent_id: session.agent_id().to_string(),
            conversation_id: session.agent_id().to_string(),
            model_choice: session.choice().clone(),
            choice_revision: 0,
            interrupt: false,
            // A router is one decision long, so there is no warm half: every
            // turn it ever hears is the whole job — and no conversation, so no
            // catch-up packet either.
            say: Some(TurnText {
                cold: prompt.clone(),
                warm: prompt,
            }),
            phase: "route",
            wants_catch_up: false,
            survives_refusal: false,
        });
        self.router_sessions.insert(capture_id.to_string(), session);
        Ok(())
    }

    /// The capture a router session speaks for, from the agent id its harness
    /// authenticated with.
    pub(in crate::app) fn capture_of_router_agent(&self, agent_id: &str) -> Option<String> {
        self.router_sessions
            .values()
            .find(|session| session.agent_id() == agent_id)
            .map(|session| session.capture_id().to_string())
    }

    /// Execute one router tool against Build.
    ///
    /// The scope gate is here rather than only in the tool inventory the router
    /// is shown: the surface a session is on is a property of the session, and
    /// a harness that writes its own frames must not reach past it.
    fn on_router_mcp_action(
        &mut self,
        capture_id: &str,
        action: BridgeAction,
    ) -> Result<Value, String> {
        if action.surface() != crate::mcp::McpSurface::Router {
            return Err(format!(
                "{} is a coding agent's tool; this session routes captures",
                action.tool_name()
            ));
        }
        // A router reaches one destination. A second route from the same
        // session would leave two artifacts and one record naming one of them.
        // The user is not bound by this: rerouting is exactly the act of
        // choosing a second destination, and it settles the first one.
        if matches!(
            action,
            BridgeAction::CreateIssue { .. } | BridgeAction::DispatchBranch { .. }
        ) {
            self.require_undecided(capture_id)?;
        }
        match action {
            BridgeAction::ListProjects => Ok(self.defer_project_list()),
            BridgeAction::ListWork => Ok(self.router_work_digest()),
            BridgeAction::ReadConversation {
                entity_id,
                agent_id,
                limit,
            } => self.router_read_conversation(&entity_id, agent_id.as_deref(), limit),
            BridgeAction::CreateIssue {
                project_id,
                goal,
                rationale,
            } => self.route_to_issue(
                capture_id,
                &project_id,
                &goal,
                rationale,
                the_dispatch_itself,
            ),
            BridgeAction::DispatchBranch {
                project_id,
                branch,
                instruction,
                rationale,
            } => self.route_to_branch(
                capture_id,
                &project_id,
                branch.as_deref(),
                &instruction,
                rationale,
                the_dispatch_itself,
            ),
            BridgeAction::AskUser { question, options } => {
                self.router_ask_user(capture_id, &question, &options)
            }
            coding_tool => Err(format!(
                "{} is a coding agent's tool; this session routes captures",
                coding_tool.tool_name()
            )),
        }
    }

    /// The work in flight, small enough for a router to read in one go: what
    /// each item is, where it lives, and whether anyone is working it.
    fn router_work_digest(&mut self) -> Value {
        let items = self.board_list();
        let work: Vec<Value> = items["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|row| row["kind"] != "capture")
            .map(|row| {
                json!({
                    "kind": row["kind"],
                    "entity_id": row["run_id"].as_str().or_else(|| row["issue_id"].as_str()).or_else(|| row["worktree_id"].as_str()),
                    "project_id": row["project_id"],
                    "project": row["project"],
                    "branch": row["branch"],
                    "issue_id": row["issue_id"],
                    "title": row["title"],
                    "state": row["state"],
                    "working": row["working"],
                })
            })
            .collect();
        json!({ "work": work })
    }

    /// One work item's conversation, as the catch-up the agents themselves are
    /// given. Read-only: the router has no way to post here, by design — it
    /// hands work over, it does not join it.
    ///
    /// Read through the same door the packet is: a router deciding where work
    /// belongs must not be handed an empty transcript because the agent it is
    /// reading spent the afternoon calling tools.
    fn router_read_conversation(
        &self,
        entity_id: &str,
        agent_id: Option<&str>,
        limit: usize,
    ) -> Result<Value, String> {
        let address = self.resolve_conversation_address(entity_id, agent_id)?;
        let thread = self.conversation_at(&address)?;
        Ok(json!({
            "entity_id": entity_id,
            "agent_id": address.agent_id,
            "conversation_id": address.conversation_id,
            "transcript": self.catch_up_packet(thread, limit),
        }))
    }

    /// The default destination: an issue on the best-guess project, with its
    /// planning agent started — no branch, no worktree, no code touched.
    ///
    /// The issue is filed inert and the route recorded first, so a capture the
    /// record could not be written for never gets an agent. Then the session
    /// starts, because what the user said IS a sent message: `create_plan`
    /// seeds it onto the issue's conversation as one, and a sent message with
    /// nobody listening is the whole bug this closes.
    pub(in crate::app) fn route_to_issue(
        &mut self,
        capture_id: &str,
        project_id: &str,
        goal: &str,
        rationale: Option<String>,
        answer: fn(&crate::capture::Capture, Value) -> Value,
    ) -> Result<Value, String> {
        let issue = self.plan_create(&json!({
            "project_id": project_id,
            "goal": goal,
            "dispatch": false,
        }))?;
        let issue_id = issue["issue_id"]
            .as_str()
            .ok_or("the issue was filed under no id")?
            .to_string();
        self.record_routing(
            capture_id,
            crate::capture::CaptureRouting {
                project_id: project_id.to_string(),
                kind: crate::capture::CaptureTarget::Issue,
                target_id: issue_id.clone(),
                routed_at: now_rfc3339(),
                rationale,
            },
            &issue_id,
        )?;
        // The planning agent works in the primary checkout — issues plan on
        // main, they do not own a worktree — and the workspace it needs there
        // is disk, so it goes to the drain like every other verb's.
        //
        // Never fatal to the route: the capture is recorded and the issue holds
        // the text, so a session that could not start leaves an inert,
        // re-startable issue rather than losing the destination.
        let routed = RoutedIssueDrafting {
            issue_id: issue_id.clone(),
            project_id: project_id.to_string(),
            capture_id: capture_id.to_string(),
            answer,
        };
        match self.reserve_plan_drafting(&issue_id, Box::new(routed.clone())) {
            Ok(Some(job)) => Ok(self.defer_job(job)),
            // Nothing to start: a session is already open for this issue, or
            // one is already on its way to the same checkout.
            Ok(None) => routed.reply(self, true),
            Err(error) => {
                eprintln!("route: {issue_id} could not start planning: {error}");
                routed.reply(self, false)
            }
        }
    }

    /// The confident destination: an agent on a branch, working. One call, and
    /// `branch.dispatch` owns the unwinding if any part of it fails — a router
    /// is the worst possible owner of a half-built branch.
    pub(in crate::app) fn route_to_branch(
        &mut self,
        capture_id: &str,
        project_id: &str,
        branch: Option<&str>,
        instruction: &str,
        rationale: Option<String>,
        answer: fn(&crate::capture::Capture, Value) -> Value,
    ) -> Result<Value, String> {
        self.dispatch_branch(
            &json!({
                "project_id": project_id,
                "branch": branch,
                "instruction": instruction,
            }),
            Some(RoutedCapture {
                capture_id: capture_id.to_string(),
                rationale,
                answer,
            }),
        )
    }

    /// The router asks the one question that would let it decide. The capture
    /// goes back to unrouted — a question is not a route — and the question is
    /// what the inbox entry says it needs.
    ///
    /// The router may offer up to three concrete choices beside the question.
    /// They are a shortcut through the answer, not a narrowing of it: typing an
    /// answer, and abandoning the capture, are there whatever it offered.
    fn router_ask_user(
        &mut self,
        capture_id: &str,
        question: &str,
        options: &[crate::capture::CaptureOptionDraft],
    ) -> Result<Value, String> {
        let question = question.trim();
        if question.is_empty() {
            return Err("ask_user: the question is empty".to_string());
        }
        let capture = self
            .captures
            .get(capture_id)
            .ok_or_else(|| format!("unknown capture_id: {capture_id}"))?;
        if capture.awaiting_answer() {
            return Err("you have already asked about this capture".to_string());
        }
        let options = crate::capture::numbered_options(options)?;
        let asked = capture.asked(crate::capture::CaptureQuestion {
            options: options.clone(),
            ..crate::capture::CaptureQuestion::new(question, now_rfc3339())
        });
        self.save_capture(asked)?;
        Ok(json!({ "capture_id": capture_id, "asked": question, "options": options }))
    }

    /// Whether a capture is still the router's to decide.
    fn require_undecided(&self, capture_id: &str) -> Result<(), String> {
        let capture = self
            .captures
            .get(capture_id)
            .ok_or_else(|| format!("unknown capture_id: {capture_id}"))?;
        if capture.state == crate::capture::CaptureState::Routed {
            return Err(
                "this capture already has a destination; the user reroutes it from here"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Record where a capture went, and settle what it was routed to before.
    /// `entity_id` is the work it became — the issue filed, or the run the
    /// branch is dispatched into — which the caller knows and the map need not
    /// hold yet: a dispatch records its route ahead of the write that opens
    /// the run, so the anchor it inherits is held in memory until that write
    /// lands and persists it.
    ///
    /// An issue no human has touched is archived and its planning agent stopped
    /// — it was never anything but a guess, and leaving it would put a second
    /// row on the feed for one piece of work and a session on the primary
    /// checkout planning something nobody will read. Anything else is kept and
    /// stays reachable through the capture's own record, because work already
    /// done is nobody's to discard.
    fn record_routing(
        &mut self,
        capture_id: &str,
        routing: crate::capture::CaptureRouting,
        entity_id: &str,
    ) -> Result<crate::capture::Capture, String> {
        let capture = self
            .captures
            .get(capture_id)
            .ok_or_else(|| format!("unknown capture_id: {capture_id}"))?;
        let previous = capture.routing.clone();
        let routed = capture.routed_to(routing);
        self.save_capture(routed.clone())?;
        // The work keeps the capture's place in the inbox. Said on Monday and
        // routed on Tuesday, it is still Monday's business — and it is ONE
        // entry, so the capture's row leaving and the work's row arriving must
        // not read as the list gaining something new.
        self.inherit_capture_anchor(entity_id, capture_id);
        if let Some(previous) = previous {
            self.release_misrouted_artifact(&previous);
        }
        Ok(routed)
    }

    /// Write down the route a dispatch is the destination of, against the run
    /// it opens. Ahead of the write that settles the run, never after it: the
    /// app mutex was free while the git ran, so the capture may be gone by
    /// now, and a refusal has to come before anything is durable — a run that
    /// exists and a caller told it does not is the one outcome nothing can
    /// reconcile.
    pub(in crate::app) fn record_dispatch_route(
        &mut self,
        routed: Option<RoutedCapture>,
        project_id: &str,
        run_id: &str,
        branch: &str,
    ) -> Result<Option<RouteRecorded>, String> {
        let Some(routed) = routed else {
            return Ok(None);
        };
        let capture = self.record_routing(
            &routed.capture_id,
            crate::capture::CaptureRouting {
                project_id: project_id.to_string(),
                kind: crate::capture::CaptureTarget::Branch,
                target_id: branch.to_string(),
                routed_at: now_rfc3339(),
                rationale: routed.rationale,
            },
            run_id,
        )?;
        Ok(Some(RouteRecorded {
            capture,
            answered_with: routed.answer,
        }))
    }

    /// Take back what a misroute created, when there is anything to take back.
    ///
    /// The route made the issue and started its planning agent, so a reroute
    /// takes back both — in that order, because an agent left running in the
    /// primary checkout would keep planning an archived issue and report `done`
    /// for it. What the route did not make, it does not touch.
    fn release_misrouted_artifact(&mut self, routing: &crate::capture::CaptureRouting) {
        if routing.kind != crate::capture::CaptureTarget::Issue {
            return;
        }
        if !self.issue_is_the_routes_alone(&routing.target_id) {
            return;
        }
        let Ok(mut active) = self.take_plan(&routing.target_id) else {
            return;
        };
        self.retire_issue_session(issue_session(&active));
        active.plan.archived_at = Some(now_rfc3339());
        let persisted = self.finish_plan_mutation(routing.target_id.clone(), active);
        if let Err(error) = persisted {
            eprintln!("reroute: could not archive {}: {error}", routing.target_id);
        }
    }

    /// Whether an issue is still nothing but what the route made of it: the
    /// goal the capture became, whatever its own planning agent has since
    /// written, and no human anywhere in it.
    ///
    /// Not "inert" any more — routing starts the planning agent, so an issue
    /// the router filed a minute ago already has a session, a state past
    /// `Created`, and stage docs its agent drafted. None of that is a claim on
    /// the issue. A human's word is: a message they posted, a comment they left
    /// on a stage, a plan they approved, a branch that implements it, a second
    /// agent they added. Any one of those and the issue is theirs, kept, and
    /// reachable from the capture rather than archived out from under them.
    fn issue_is_the_routes_alone(&self, issue_id: &str) -> bool {
        let Some(active) = self.plans.get(issue_id) else {
            return false;
        };
        let implemented = self
            .runs
            .values()
            .any(|run| run.run.plan_id.as_ref().map(|id| id.0.as_str()) == Some(issue_id));
        // The goal itself is the one message `create_plan` seeds, and it is the
        // capture. A second is somebody having spoken to this issue.
        let said_by_a_human = active
            .agents
            .sole_thread()
            .items
            .iter()
            .filter(|item| {
                matches!(item, crate::thread::ThreadItem::Message(message)
                    if message.role == crate::thread::MessageRole::User)
            })
            .count();
        !matches!(
            active.plan.state,
            PlanState::Approved | PlanState::Abandoned
        ) && active.plan.archived_at.is_none()
            && !implemented
            && active.agents.len() == 1
            && said_by_a_human <= 1
            && active.agents.sole_thread().doc_comments().is_empty()
    }

    /// A router reported. Whatever it said, the session is over — and a capture
    /// with no destination and no question outstanding is a route that failed,
    /// which is the state the feed offers a retry from.
    pub(in crate::app) fn on_router_done(&mut self, capture_id: &str, report: DoneReport) {
        if report.status != DoneStatus::Completed {
            eprintln!("router {capture_id}: {}", report.summary);
        }
        self.settle_router_session(capture_id);
    }

    /// End a router session and tidy up after it: the harness, the scratch
    /// directory, and the capture's state if the router left it undecided.
    pub(in crate::app) fn settle_router_session(&mut self, capture_id: &str) {
        let undecided = self.captures.get(capture_id).is_some_and(|capture| {
            capture.state != crate::capture::CaptureState::Routed && !capture.awaiting_answer()
        });
        if undecided {
            self.mark_routing_failed(capture_id);
        }
        self.abandon_router_session(capture_id);
    }

    /// Stop the router working on a capture and wipe what it was working in,
    /// without judging the capture — the caller owns that.
    pub(in crate::app) fn abandon_router_session(&mut self, capture_id: &str) {
        let Some(session) = self.router_sessions.remove(capture_id) else {
            return;
        };
        let root = Self::canonical_root(session.scratch_dir());
        let writers = self
            .retire_tab(
                &TabKey::agent(&root, session.agent_id()),
                "agent_session_ended",
            )
            .into_iter()
            .collect();
        self.session_registry.revoke_mcp_token(session.agent_id());
        self.projects.unbind_live_entity(capture_id);
        // Bridge-owned, per capture, and holding nothing but what the harness
        // wrote for itself — so it goes with the session that made it.
        crate::reaper::remove_dir_once_reaped(
            writers,
            session.scratch_dir().to_path_buf(),
            crate::orchestrator::CHECKOUT_REAP_WAIT,
            format!("router {capture_id}"),
        );
    }

    /// Settle every router whose harness has stopped without reporting.
    ///
    /// A process that dies mid-decision tells nobody, so the capture would sit
    /// in `routing` for the daemon's whole life with no router behind it. Only
    /// sessions that actually started are considered: one still on its way to a
    /// harness has no process to have lost.
    pub(in crate::app) fn reap_finished_router_sessions(&mut self) -> Vec<String> {
        let finished: Vec<String> = self
            .router_sessions
            .values()
            .filter(|session| session.started())
            .filter(|session| {
                !self.agent_is_live(
                    &Self::canonical_root(session.scratch_dir()),
                    session.agent_id(),
                )
            })
            .map(|session| session.capture_id().to_string())
            .collect();
        for capture_id in &finished {
            self.settle_router_session(capture_id);
        }
        finished
    }
}
