use super::*;

// ==== the decision rule, driven end to end ================================
//
// The rule is the router's whole contract (spec: UX Redesign Decisions,
// "Capture and router"): dispatch to a branch only when the capture names an
// existing branch or unambiguously continues work already in flight there;
// otherwise file an inert issue on the best-guess project; ask only when
// even the project is ambiguous.
//
// These tests take the model out and leave everything else in.
// [`ScriptedRouter`] reads the world through the router's own tools, applies
// that rule to what they answered, and writes the decision as MCP frames on
// the real router surface — so each case below runs the frame parse, the
// tool inventory, the surface gate, the destination verb and the capture
// record, and the suite reads as the rule table it is.
//
// What a harness cannot pin is whether a model applies the rule well. That
// half lives in the template, and `templates.rs`'s
// `the_router_template_states_the_decision_rule_verbatim` holds it to these
// same rules word for word.

/// Where the rule sends a capture.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RoutingDecision {
    Dispatch { project_id: String, branch: String },
    FileIssue { project_id: String },
    Ask,
}

/// Words that say nothing about which piece of work a capture is about.
/// Matching on them would make any two sentences look like the same job.
const AMBIENT_WORDS: &[&str] = &[
    "about", "after", "again", "also", "been", "could", "does", "from", "have", "into", "just",
    "make", "more", "much", "must", "need", "over", "should", "some", "still", "than", "that",
    "them", "then", "there", "they", "thing", "this", "were", "what", "when", "with", "work",
    "would", "your",
];

/// The words in a capture that could identify the work it continues.
fn subject_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.len() >= 4 && !AMBIENT_WORDS.contains(word))
        .map(str::to_string)
        .collect()
}

/// A router harness with the model taken out: everything a router session
/// does, decided by the rule instead of by judgement.
struct ScriptedRouter {
    capture_id: String,
    text: String,
    server: crate::mcp::DoneServer,
    /// The tools this session was actually offered. A transcript may use
    /// only these: a rule that needs a tool the router does not have is a
    /// rule it cannot follow.
    offered_tools: Vec<String>,
    next_frame_id: u64,
}

impl ScriptedRouter {
    /// Capture `text` and take the router session Build put on it.
    fn on(state: &mut AppState, text: &str) -> ScriptedRouter {
        let (capture_id, agent_id) = captured(state, text);
        let mut router = ScriptedRouter {
            capture_id,
            text: text.to_string(),
            server: crate::mcp::DoneServer::for_owner(agent_id),
            offered_tools: Vec::new(),
            next_frame_id: 1,
        };
        let listed = router.request("tools/list", json!({}));
        router.offered_tools = listed["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("a router session is offered tools: {listed:?}"))
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect();
        router
    }

    /// One JSON-RPC request from the harness, answered by the real server.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_frame_id;
        self.next_frame_id += 1;
        let message =
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string();
        let handled = self.server.handle_message(&message);
        serde_json::from_str(&handled.reply.expect("a request is answered")).unwrap()
    }

    /// One tool call, all the way through: the frame the harness writes, the
    /// surface that parses it, and the daemon that executes it.
    fn call(
        &mut self,
        state: &mut AppState,
        tool: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        assert!(
            self.offered_tools.iter().any(|offered| offered == tool),
            "{tool} is not on the router's surface: {:?}",
            self.offered_tools
        );
        let id = self.next_frame_id;
        self.next_frame_id += 1;
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments }
        })
        .to_string();
        let handled = self.server.handle_message(&message);
        // A terminal message carries the routing report; other calls are
        // actions for the daemon.
        if let Some(report) = handled.report {
            state.on_router_done(&self.capture_id, report);
            return Ok(Value::Null);
        }
        let action = handled
            .action
            .unwrap_or_else(|| panic!("{tool} reached no daemon action: {:?}", handled.reply));
        state.router_action(&self.capture_id, action)
    }

    /// A read tool's answer, as the list it promises.
    fn read_list(&mut self, state: &mut AppState, tool: &str, key: &str) -> Vec<Value> {
        let answer = self
            .call(state, tool, json!({}))
            .unwrap_or_else(|error| panic!("{tool}: {error}"));
        answer[key]
            .as_array()
            .unwrap_or_else(|| panic!("{tool} answers with {key}: {answer:?}"))
            .clone()
    }

    /// One router session's whole life: read the world, decide, act once,
    /// report, exit — in the order the template lays it out.
    fn route(&mut self, state: &mut AppState) -> RoutingDecision {
        let projects = self.read_list(state, "list_projects", "projects");
        let work = self.read_list(state, "list_work", "work");
        let decision = self.decide(state, &projects, &work);
        let said = self.text.clone();
        match &decision {
            RoutingDecision::Dispatch { project_id, branch } => {
                self.call(
                    state,
                    "dispatch_branch",
                    json!({
                        "project_id": project_id,
                        "branch": branch,
                        "name": "Route work",
                        "instruction": said,
                        "rationale": format!("continues the work in flight on {branch}"),
                    }),
                )
                .expect("a dispatch the rule reached is a dispatch Build allows");
            }
            RoutingDecision::FileIssue { project_id } => {
                self.call(
                    state,
                    "create_issue",
                    json!({
                        "project_id": project_id,
                        "goal": said,
                        "rationale": "no branch in flight is doing this work",
                    }),
                )
                .expect("an issue the rule reached is an issue Build allows");
            }
            RoutingDecision::Ask => {
                self.call(
                    state,
                    "ask_user",
                    json!({ "question": "which project is this about?" }),
                )
                .expect("a question the rule reached is a question Build allows");
            }
        }
        self.call(
            state,
            "post_thread_message",
            json!({
                "phase": "route",
                "status": "Complete",
                "body": "routed the capture",
            }),
        )
        .expect("the router's report is accepted");
        decision
    }

    /// The decision rule, in the order the template states it, applied to
    /// exactly what the router's tools answered and nothing else.
    fn decide(
        &mut self,
        state: &mut AppState,
        projects: &[Value],
        work: &[Value],
    ) -> RoutingDecision {
        let said = self.text.to_lowercase();
        let branches: Vec<Value> = work
            .iter()
            .filter(|row| row["kind"] == "branch")
            .cloned()
            .collect();
        let dispatch_to = |row: &Value| RoutingDecision::Dispatch {
            project_id: row["project_id"].as_str().unwrap().to_string(),
            branch: row["branch"].as_str().unwrap().to_string(),
        };

        // Rule 1, first half: the capture names a branch that exists.
        if let Some(row) = branches.iter().find(|row| {
            row["branch"]
                .as_str()
                .is_some_and(|branch| said.contains(branch))
        }) {
            return dispatch_to(row);
        }

        // Rule 1, second half: it continues what a branch is already doing.
        // Believed only after reading that branch's conversation — a branch
        // whose conversation is about something else is not the destination.
        let subject = subject_words(&self.text);
        for row in &branches {
            // A checkout nobody has started work on — the bare primary
            // branch is one — has no entity and so no conversation. There
            // is nothing there to continue.
            let Some(entity_id) = row["entity_id"].as_str().map(str::to_string) else {
                continue;
            };
            let conversation = self
                .call(
                    state,
                    "read_conversation",
                    json!({ "entity_id": entity_id }),
                )
                .expect("the router reads any conversation on the device");
            let transcript = conversation["transcript"].as_str().unwrap().to_lowercase();
            if subject.iter().any(|word| transcript.contains(word)) {
                return dispatch_to(row);
            }
        }

        // Rule 2: the best-guess project — the one the capture names, or
        // the only one there is to mean.
        let named = projects.iter().find(|project| {
            project["name"]
                .as_str()
                .is_some_and(|name| said.contains(&name.to_lowercase()))
        });
        if let Some(project) = named.or_else(|| projects.first().filter(|_| projects.len() == 1)) {
            return RoutingDecision::FileIssue {
                project_id: project["project_id"].as_str().unwrap().to_string(),
            };
        }

        // Rule 3: not even the project is guessable.
        RoutingDecision::Ask
    }
}

/// A branch already in flight, with an agent working the instruction that
/// opened it — the work a later capture is checked against.
fn branch_in_flight(state: &mut AppState, instruction: &str) -> (String, String) {
    let project_id = state.project_at(0).id.clone();
    let dispatched = state.handle(req(
        "branch.dispatch",
        json!({ "project_id": project_id, "instruction": instruction }),
    ));
    assert_eq!(dispatched["ok"], true, "{dispatched:?}");
    (
        dispatched["result"]["branch"].as_str().unwrap().to_string(),
        dispatched["result"]["run_id"].as_str().unwrap().to_string(),
    )
}

/// A device with two projects and nothing in flight — the world where a
/// capture can honestly be about either one.
fn two_project_state(dir: &tempfile::TempDir) -> (AppState, String, String) {
    let storefront = init_repo_named(dir.path(), "storefront");
    let billing = init_repo_named(dir.path(), "billing");
    let mut state = qa_state(&storefront, dir.path());
    let storefront_id = state.project_at(0).id.clone();
    let billing_id = state.add_project(billing, "main".to_string());
    (state, storefront_id, billing_id)
}

/// What is true of every finished routing session, whatever it decided: the
/// router is gone, and the scratch it worked in went with it.
fn assert_router_session_settled(state: &AppState, capture_id: &str) {
    assert!(
        !state.router_sessions.contains_key(capture_id),
        "the session outlived the decision it was spawned for"
    );
    assert!(
        !state
            .state_root
            .join(crate::router::ROUTER_SCRATCH_DIR_NAME)
            .join(capture_id)
            .exists(),
        "the scratch outlived the session that owned it"
    );
}

/// Rule 1, first half. A capture that names a branch is that branch's work:
/// it joins the checkout already there as an agent of its own, and nothing
/// is filed beside it.
#[test]
fn a_capture_naming_a_branch_in_flight_is_dispatched_to_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (branch, run_id) = branch_in_flight(&mut state, "stop the toast firing twice on login");
    let agents_before = state.runs[&run_id].agents.len();
    let runs_before = state.runs.len();

    let mut router = ScriptedRouter::on(
        &mut state,
        &format!("on {branch}, cover the empty case too"),
    );
    let decision = router.route(&mut state);

    assert_eq!(
        decision,
        RoutingDecision::Dispatch {
            project_id,
            branch: branch.clone()
        }
    );
    let record = capture_record(&mut state, &router.capture_id);
    assert_eq!(record["state"], "routed");
    assert_eq!(record["routing"]["kind"], "branch");
    assert_eq!(record["routing"]["target_id"], branch.as_str());
    assert_eq!(
        state.runs.len(),
        runs_before,
        "the branch that was already there is the branch it went to"
    );
    assert_eq!(
        state.runs[&run_id].agents.len(),
        agents_before + 1,
        "the capture arrives as an agent of its own on that branch"
    );
    assert!(
        state.plans.is_empty(),
        "work this clearly placed is never also filed as an issue"
    );
    assert_router_session_settled(&state, &router.capture_id);
}

/// Rule 1, second half. The capture names no branch, but says what a branch
/// in flight is already doing — and the conversation on that branch is what
/// makes it believable. The branch doing something else gets nothing.
#[test]
fn a_capture_continuing_work_in_flight_is_dispatched_to_the_branch_carrying_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (toast_branch, toast_run) =
        branch_in_flight(&mut state, "stop the toast firing twice on login");
    let (_, invoice_run) = branch_in_flight(&mut state, "widen the invoice pdf footer");
    let toast_agents = state.runs[&toast_run].agents.len();
    let invoice_agents = state.runs[&invoice_run].agents.len();

    let mut router = ScriptedRouter::on(&mut state, "the toast is doubling up again");
    let decision = router.route(&mut state);

    assert_eq!(
        decision,
        RoutingDecision::Dispatch {
            project_id: state.project_at(0).id.clone(),
            branch: toast_branch.clone()
        }
    );
    assert_eq!(
        capture_record(&mut state, &router.capture_id)["routing"]["target_id"],
        toast_branch.as_str()
    );
    assert_eq!(
        state.runs[&toast_run].agents.len(),
        toast_agents + 1,
        "the branch already doing this work took it"
    );
    assert_eq!(
        state.runs[&invoice_run].agents.len(),
        invoice_agents,
        "the branch doing something else was left alone"
    );
    assert!(state.plans.is_empty());
    assert_router_session_settled(&state, &router.capture_id);
}

/// Rule 3. Two projects, and neither the capture nor the work in flight
/// says which — the one case where a question beats a guess. Nothing is
/// created, and the capture goes back to the user saying what it needs.
#[test]
fn a_capture_whose_project_is_ambiguous_is_asked_about_rather_than_guessed() {
    let dir = tempfile::tempdir().unwrap();
    let (mut state, _storefront, _billing) = two_project_state(&dir);

    let mut router = ScriptedRouter::on(&mut state, "the thing could be faster");
    let decision = router.route(&mut state);

    assert_eq!(decision, RoutingDecision::Ask);
    let record = capture_record(&mut state, &router.capture_id);
    assert_eq!(record["state"], "unrouted", "a question is not a route");
    assert_eq!(record["routing"], Value::Null);
    assert_eq!(record["question"]["text"], "which project is this about?");
    assert!(state.plans.is_empty(), "nothing was filed on a coin flip");
    assert!(state.runs.is_empty(), "and nothing was dispatched on one");

    let row = capture_rows(&mut state)
        .into_iter()
        .find(|row| row["capture_id"] == router.capture_id.as_str())
        .expect("an unanswered question keeps the capture on the feed");
    assert_eq!(row["unread"], true);
    assert_eq!(row["unread_reason"], "router_question");
    assert_router_session_settled(&state, &router.capture_id);
}
