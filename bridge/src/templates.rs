//! Prompt templates *are* the orchestration logic, so they're data, not code.
//!
//! Each one is a markdown file under `bridge/templates/`, and the notes several
//! of them share are under `bridge/templates/notes/`. The files are compiled in
//! with `include_str!`, so the bridge carries its defaults with nothing to find
//! at runtime. This module only composes them (which notes follow which
//! template) and fills in their placeholders. There is no per-project override
//! yet: every project gets these defaults.
//!
//! The terminal-message instructions live in the templates (harness-agnostic,
//! zero special cases). Variables: `{goal}`, `{plan_path}`, `{docs_dir}`,
//! `{comments}`, `{base_branch}`, `{stage_id}`, `{stage_title}`, `{stage_path}`,
//! `{stage_summary}`, `{next_stage_path}`, `{stage_start_sha}`, `{capture_text}`,
//! `{project_name}`, `{project_base}`, `{project_sources}`, `{user_answer}`.

/// Where a single-document plan lives. A plan whose docs dir holds
/// `STAGES_MANIFEST_PATH` when its agent reports Complete is multi-stage
/// instead, read from that manifest.
pub const DEFAULT_PLAN_PATH: &str = ".build/plan.md";

/// The directory multi-stage plan docs and the manifest live under.
pub const STAGES_DIR: &str = ".build/plan";
/// The multi-stage plan manifest: an ordered JSON array of stage entries.
pub const STAGES_MANIFEST_PATH: &str = ".build/plan/stages.json";

/// A default template: its markdown file under `bridge/templates/`, compiled in.
macro_rules! template {
    ($file:literal) => {
        without_final_newline(include_str!(concat!("../templates/", $file)))
    };
}

/// A template file ends in one newline, as every text file does; the text it
/// holds does not, because composition puts its own blank line between texts.
const fn without_final_newline(text: &'static str) -> &'static str {
    match text.as_bytes() {
        [body @ .., b'\n'] => match std::str::from_utf8(body) {
            Ok(body) => body,
            Err(_) => panic!("a template file is not UTF-8"),
        },
        _ => text,
    }
}

const PLAN: &str = template!("plan.md");

const BUILD: &str = template!("build.md");

const BUILD_STAGE: &str = template!("build_stage.md");

const REVISE: &str = template!("revise.md");

const REVISE_STAGE: &str = template!("revise_stage.md");

const REVIEW_CHANGES: &str = template!("review_changes.md");

const ROUTER: &str = template!("router.md");

const PROJECT_AGENT: &str = template!("project_agent.md");

/// What every code-changing phase adds about its completion message. Appended rather
/// than written into each template so the four asks cannot drift apart, and so
/// a project overriding one template still overrides only that one.
///
/// The terminal message body IS the report. There used to be a structured
/// `completion_report` beside a one-sentence summary; the reviewer got a
/// sentence and a card of lists, and the account that actually explained the
/// work sat in the activity log. Now the one field carries the whole account.
const DONE_SUMMARY_ASK: &str = template!("notes/done_summary_ask.md");

/// What every coding phase adds about reaching the other agents on the project.
///
/// Appended rather than written into each template so the sentence cannot drift
/// between phases, and so a project overriding one template still overrides only
/// that one. Not on the router's template or the project agent's: the router has
/// no conversation to be answered in, and the project agent's own prompt says
/// this in its own words.
///
/// It has to say that a reply is a send, because nothing carries one for the
/// agent. A report used to be forwarded to whoever asked for the turn, and an
/// agent told that would answer by ending its turn and say the same thing twice
/// when it answered properly too.
const MESSAGE_AGENT_NOTE: &str = template!("notes/message_agent.md");

/// What every coding phase adds about getting a second checkout.
///
/// Appended rather than written into each template for the same reason
/// [`MESSAGE_AGENT_NOTE`] is: an agent that reaches for a worktree does it in
/// whichever phase it happens to be in, so the sentence cannot be allowed to
/// live in some of them and not others.
///
/// It has to name `git worktree add` to forbid it. A model that has been told
/// only what to use instead reads `create_workspace` as the Build-flavoured way
/// and its own `git worktree add` as the quick one — and a checkout Build did
/// not cut is one nobody can see, review or clean up. The tool descriptions say
/// this too (`mcp.rs`), because they are re-sent on every tools/list and so
/// outlive this prompt's compaction.
const WORKSPACE_NOTE: &str = template!("notes/workspace.md");

/// What every agent with the task tools is told about them (spec: Tasks →
/// The prompt note).
///
/// Appended rather than written into each template, for the reason
/// [`MESSAGE_AGENT_NOTE`] is: the wording cannot drift between the coding
/// surface and the project surface, and a project overriding one template
/// still overrides only that one.
///
/// It says the things an agent gets wrong without being told. A task handed
/// over is the WORK, not a note about it, so progress belongs on the task and
/// not only in a conversation nobody else reads. In review is what Complete
/// means on a board — ready to be looked at, not accepted — and an agent that
/// moves its own task to Done is marking its own homework. A hand-off is an
/// assignment, because an assignment delivers the task and leaves a record
/// while a message leaves only words. And work an agent notices and does not do
/// exists nowhere unless it is filed.
///
/// Three more, added because the conversation kept being used for what the
/// board is for. A brief sent as a message is a brief only its reader has, so
/// anything bigger than a correction is filed and assigned. A multi-step piece
/// of work planned only in a conversation is invisible to the user and dies
/// with the session, so an agent files and self-assigns its own. And a question
/// about somebody else's task asked anywhere but that task reaches one person
/// when it needed to reach two.
const TASK_TOOLS_NOTE: &str = template!("notes/task_tools.md");

/// The reference shapes an agent can write, so a link into Build costs a few
/// characters rather than a route nobody can remember (#56).
///
/// One note on every template that writes prose a reader opens — a message, a
/// task body, a comment — because the same shapes have to mean the same thing
/// wherever they are typed. The renderer that reads them is
/// `spa/src/core/markdownRefs.js`; a test below holds this list to it.
const LINK_MARKUP_NOTE: &str = template!("notes/link_markup.md");

fn phase_template(base: &str) -> String {
    base.to_string()
}

/// A template for an agent that works in a checkout: it can reach the other
/// agents on its project, and it can cut a checkout of its own to put one of
/// them on.
fn coding_template(base: &str) -> String {
    format!("{base}\n\n{MESSAGE_AGENT_NOTE}\n\n{WORKSPACE_NOTE}\n\n{TASK_TOOLS_NOTE}\n\n{LINK_MARKUP_NOTE}")
}

/// A template whose phase ends in changed code, so its terminal message body is the
/// full report of it.
fn reporting_template(base: &str) -> String {
    format!("{}\n\n{DONE_SUMMARY_ASK}", coding_template(base))
}

/// The phase templates. Clone-and-edit to override per project.
#[derive(Debug, Clone)]
pub struct Templates {
    pub plan: String,
    pub build: String,
    pub build_stage: String,
    pub revise: String,
    pub revise_stage: String,
    pub review_changes: String,
    pub message: String,
    /// The one template that belongs to no phase of a piece of work: routing
    /// decides which piece of work the capture is.
    pub router: String,
    /// The other one: a project agent is about a project rather than about any
    /// piece of work inside it.
    pub project_agent: String,
}

impl Default for Templates {
    fn default() -> Self {
        Templates {
            plan: coding_template(PLAN),
            build: reporting_template(BUILD),
            build_stage: reporting_template(BUILD_STAGE),
            revise: coding_template(REVISE),
            revise_stage: coding_template(REVISE_STAGE),
            review_changes: reporting_template(REVIEW_CHANGES),
            message: coding_template(MESSAGE),
            router: phase_template(ROUTER),
            // The project agent gets the task note and not the message note:
            // its own prompt already says what messaging another agent means,
            // in its own words, and the tracker is the one thing it has that
            // that prompt predates.
            project_agent: format!("{PROJECT_AGENT}\n\n{TASK_TOOLS_NOTE}\n\n{LINK_MARKUP_NOTE}"),
        }
    }
}

/// Fallback for a freeform reviewer message when the harness has no prior
/// conversation to `--continue` in this worktree: a fresh session gets the
/// message wrapped in enough context to act on it.
const MESSAGE: &str = template!("message.md");

/// The substitution variables a template can reference.
#[derive(Debug, Default, Clone)]
pub struct Vars<'a> {
    pub goal: &'a str,
    pub plan_path: &'a str,
    /// Absolute path of a task's scratch docs dir — where its planning agent
    /// writes plan documents, since it runs in the primary checkout and must
    /// not write there. Empty for every run-side template.
    pub docs_dir: &'a str,
    pub comments: &'a str,
    pub base_branch: &'a str,
    pub stage_id: &'a str,
    pub stage_title: &'a str,
    pub stage_path: &'a str,
    pub stage_summary: &'a str,
    /// The next stage's doc path, or "" on the final stage.
    pub next_stage_path: &'a str,
    /// git sha of the worktree HEAD when the stage was first dispatched.
    pub stage_start_sha: &'a str,
    /// What the user said, verbatim, for the `router` template.
    pub capture_text: &'a str,
    /// The project a project agent is the agent of, for the `project_agent`
    /// template.
    pub project_name: &'a str,
    /// The directory a project agent stands in, for the `project_agent`
    /// template.
    pub project_base: &'a str,
    /// The sentence naming a project's other sources, or "" for a project
    /// with one, for the `project_agent` template.
    pub project_sources: &'a str,
    /// The user's answer to the router's clarifying question, or "".
    pub user_answer: &'a str,
}

/// Render a template by substituting every `{var}` placeholder.
pub fn render(template: &str, vars: &Vars) -> String {
    template
        .replace("{goal}", vars.goal)
        .replace("{plan_path}", vars.plan_path)
        .replace("{docs_dir}", vars.docs_dir)
        .replace("{comments}", vars.comments)
        .replace("{base_branch}", vars.base_branch)
        .replace("{stage_id}", vars.stage_id)
        .replace("{stage_title}", vars.stage_title)
        .replace("{stage_path}", vars.stage_path)
        .replace("{stage_summary}", vars.stage_summary)
        .replace("{next_stage_path}", vars.next_stage_path)
        .replace("{stage_start_sha}", vars.stage_start_sha)
        .replace("{capture_text}", vars.capture_text)
        .replace("{project_name}", vars.project_name)
        .replace("{project_base}", vars.project_base)
        .replace("{project_sources}", vars.project_sources)
        .replace("{user_answer}", vars.user_answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Collapse every run of whitespace to a single space and trim the ends,
    /// so a template's wording can be asserted across its line wrapping.
    fn collapse_whitespace(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn render_substitutes_every_placeholder() {
        let out = render(
            "{goal} / {plan_path} / {comments} / {base_branch}",
            &Vars {
                goal: "ship it",
                plan_path: ".build/plan.md",
                comments: "fix x",
                base_branch: "main",
                ..Vars::default()
            },
        );
        assert_eq!(out, "ship it / .build/plan.md / fix x / main");
    }

    #[test]
    fn render_substitutes_every_stage_placeholder() {
        let out = render(
            "{stage_id}/{stage_title}/{stage_path}/{stage_summary}/{next_stage_path}/{stage_start_sha}",
            &Vars {
                stage_id: "database-schema",
                stage_title: "Database schema",
                stage_path: ".build/plan/01-database-schema.md",
                stage_summary: "Create the tables.",
                next_stage_path: ".build/plan/02-api-endpoints.md",
                stage_start_sha: "abc123",
                ..Vars::default()
            },
        );
        assert_eq!(
            out,
            "database-schema/Database schema/.build/plan/01-database-schema.md/Create the tables./.build/plan/02-api-endpoints.md/abc123"
        );
    }

    #[test]
    fn defaults_instruct_terminal_messages_and_plan_scope() {
        let t = Templates::default();
        assert!(t.plan.contains("`post_thread_message`"));
        assert!(t.plan.contains("status=\"Complete\""));
        assert!(t.plan.contains("stages.json"));
        assert!(t.plan.contains(".build/"));
        for template in [&t.plan, &t.build, &t.review_changes] {
            assert!(!template.contains("`done`"), "retired tool in {template}");
            assert!(
                !template.contains("read_unread_messages"),
                "retired tool in {template}"
            );
        }
    }

    /// The terminal body is the report, so every code-changing phase is told what a
    /// whole one holds — and nothing asks for the structured report that used
    /// to ride beside it.
    #[test]
    fn every_code_changing_template_asks_for_the_full_report_in_the_terminal_body() {
        let t = Templates::default();
        for template in [&t.build, &t.build_stage, &t.review_changes] {
            assert!(
                template.contains("`body` is the whole report"),
                "{template}"
            );
            for asked in ["what changed", "verified", "reverse-engineer", "left out"] {
                assert!(template.contains(asked), "{asked} missing from {template}");
            }
            assert!(!template.contains("completion_report"), "{template}");
        }
    }

    /// The report note itself carries the whole report's one exception
    /// (#232): the templates around it also carry the task-tools note, which
    /// says it too, so asserting on a whole template cannot tell them apart.
    #[test]
    fn the_report_note_says_a_result_on_the_users_task_is_one_line() {
        use crate::test_support::user_involvement::{says, RESULT_ON_TASK};
        assert!(says(DONE_SUMMARY_ASK, RESULT_ON_TASK), "{DONE_SUMMARY_ASK}");
    }

    #[test]
    fn the_plan_document_templates_ask_for_no_report() {
        let t = Templates::default();
        for template in [&t.plan, &t.revise, &t.revise_stage] {
            assert!(!template.contains("whole report"), "{template}");
            assert!(!template.contains("completion_report"), "{template}");
        }
    }

    #[test]
    fn plan_template_instructs_manifest_and_stage_docs() {
        let t = Templates::default();
        assert!(t.plan.contains(".build/plan/"));
        assert!(t.plan.contains(".build/plan/stages.json"));
        assert!(
            collapse_whitespace(&t.plan).contains("write the manifest before you report"),
            "Build reads the manifest from disk on Complete: {}",
            t.plan
        );
    }

    #[test]
    fn plan_template_asks_questions_without_blocking_on_them() {
        let t = Templates::default();
        let converse = t
            .plan
            .find("post_thread_message")
            .expect("the plan agent reaches the reviewer through the thread");
        let write_stages = t
            .plan
            .find("Break the work into sequential stages")
            .expect("the stage-writing instructions");
        assert!(
            converse < write_stages,
            "questions are posted before any stage doc is written: {}",
            t.plan
        );
        assert!(
            t.plan.contains("Assumptions"),
            "open questions become recorded assumptions, not a parked plan: {}",
            t.plan
        );
        assert!(
            t.plan.contains("never for waiting on answers"),
            "blocked is reserved for environment/implementation problems: {}",
            t.plan
        );
        assert!(
            t.plan.contains("status=\"Blocked\""),
            "the escape hatch for a genuinely broken environment remains: {}",
            t.plan
        );
    }

    #[test]
    fn blocked_means_environment_or_implementation_everywhere() {
        // Waiting on the reviewer is a conversation, not a blocker: every
        // template that teaches terminal `Blocked` must scope it to unexpected
        // environment/implementation problems, never to open questions.
        let t = Templates::default();
        for (name, template) in [
            ("plan", &t.plan),
            ("build", &t.build),
            ("build_stage", &t.build_stage),
            ("message", &t.message),
        ] {
            assert!(
                template.contains("status=\"Blocked\""),
                "{name} lost its escape hatch: {template}"
            );
            assert!(
                collapse_whitespace(template)
                    .contains("unexpected environment or implementation problem"),
                "{name} must scope blocked to environment/implementation problems: {template}"
            );
        }
    }

    #[test]
    fn build_stage_template_scopes_to_one_stage() {
        let t = Templates::default();
        assert!(t.build_stage.contains("{stage_title}"));
        assert!(t.build_stage.contains("{stage_path}"));
        assert!(t.build_stage.contains("status=\"Complete\""));
    }

    #[test]
    fn build_templates_instruct_atomic_agent_commits() {
        // The agent authors the atomic, self-messaged commits; Build's sweep is
        // only a no-op-on-clean safety net. Every build-phase template must ask
        // for atomic commits, forbid touching `.build/`, and require everything
        // committed before the Complete message.
        let t = Templates::default();
        for tmpl in [&t.build, &t.build_stage] {
            assert!(
                tmpl.contains("atomic commit"),
                "template must ask for atomic commits: {tmpl}"
            );
            assert!(
                tmpl.contains("`.build/`"),
                "template must forbid committing under `.build/`: {tmpl}"
            );
            assert!(
                tmpl.contains("before you send status=\"Complete\""),
                "template must require committing before Complete: {tmpl}"
            );
        }
    }

    #[test]
    fn revise_stage_template_carries_its_stage_and_comments() {
        let t = Templates::default();
        assert!(t.revise_stage.contains("{stage_title}"));
        assert!(t.revise_stage.contains("{comments}"));
        assert!(t.revise_stage.contains("status=\"Complete\""));
    }

    #[test]
    fn terminal_reports_use_body_and_never_the_retired_summary_field() {
        let t = Templates::default();
        for tmpl in [
            &t.plan,
            &t.build,
            &t.build_stage,
            &t.revise,
            &t.revise_stage,
            &t.review_changes,
        ] {
            assert!(
                tmpl.contains("body"),
                "terminal report must use body: {tmpl}"
            );
            assert!(
                !tmpl.contains("`summary`"),
                "retired report field in {tmpl}"
            );
            assert!(!tmpl.contains("`done`"), "retired tool in {tmpl}");
        }
    }

    #[test]
    fn rendered_plan_has_no_leftover_placeholders() {
        let out = render(
            &Templates::default().plan,
            &Vars {
                goal: "add a greeting",
                plan_path: DEFAULT_PLAN_PATH,
                docs_dir: "/tmp/build/task-docs/plan-1",
                comments: "",
                base_branch: "main",
                ..Vars::default()
            },
        );
        assert!(!out.contains("{goal}"));
        assert!(!out.contains("{plan_path}"));
        assert!(!out.contains("{docs_dir}"));
        assert!(out.contains("add a greeting"));
    }

    /// A task's agent runs in the primary checkout, so every plan-side
    /// template has to say where its documents go instead — and say that the
    /// checkout itself is not to be written to.
    #[test]
    fn the_plan_templates_send_every_document_to_the_scratch_docs_dir() {
        let t = Templates::default();
        for template in [&t.plan, &t.revise, &t.revise_stage] {
            let rendered = collapse_whitespace(&render(
                template,
                &Vars {
                    goal: "add a greeting",
                    plan_path: DEFAULT_PLAN_PATH,
                    docs_dir: "/scratch/task-docs/plan-1",
                    stage_path: ".build/plan/01-first.md",
                    comments: "",
                    base_branch: "main",
                    ..Vars::default()
                },
            ));
            assert!(
                rendered.contains("/scratch/task-docs/plan-1"),
                "the agent is told where its docs go: {rendered}"
            );
            assert!(
                rendered.contains("docs directory"),
                "the agent is told to write nowhere else: {rendered}"
            );
        }
    }

    /// The decision rule is the router's whole contract, so the template has to
    /// state it in the order it is applied.
    #[test]
    fn the_router_template_carries_the_decision_rule_in_order() {
        let router = collapse_whitespace(&Templates::default().router);
        let dispatch = router.find("`dispatch_branch` ONLY when").expect("rule 1");
        let fallback = router
            .find("Otherwise call `dispatch_branch`")
            .expect("rule 2");
        let ask = router.find("`ask_user` ONLY when").expect("rule 3");
        assert!(dispatch < fallback && fallback < ask, "{router}");

        assert!(
            router.contains("names an existing branch or worktree")
                && router.contains("unambiguously continues work already in flight"),
            "the branch test must be stated, not implied: {router}"
        );
        assert!(
            router.contains("even the project is ambiguous"),
            "a question is reserved for an ambiguous project: {router}"
        );
        assert!(router.contains("omitting `branch`"), "{router}");
    }

    /// Every row of the decision rule, in the template's own words.
    ///
    /// The scripted-router suite in `app.rs` drives this same table through the
    /// bridge (`a_capture_naming_a_branch_in_flight_is_dispatched_to_it` and the
    /// three cases beside it), but a harness can only pin what Build does with a
    /// decision once it is made. The decision itself is taught here and nowhere
    /// else, so the sentences that teach it are asserted whole: an edit that
    /// softens one of them is an edit to the contract.
    #[test]
    fn the_router_template_states_the_decision_rule_verbatim() {
        let router = collapse_whitespace(&Templates::default().router);
        for rule in [
            // Rule 1: the only case that dispatches, and the check that has to
            // happen before the router believes it is in that case.
            "Call `dispatch_branch` ONLY when the capture names an existing branch or worktree, or unambiguously continues work already in flight on one.",
            "Read that branch's conversation with `read_conversation` before you believe it does.",
            // Rule 2: new work starts a new branch on the likeliest project.
            "Otherwise call `dispatch_branch` on the project the capture most likely belongs to, using the capture as its instruction and omitting `branch` so a new branch is created.",
            // Rule 3: the one case a question beats a guess, and the shape of
            // the offer that goes with it.
            "Call `ask_user` ONLY when even the project is ambiguous.",
            "a best-guess project is almost always the better answer.",
            "When you do ask, offer up to 3 options: the destinations you are choosing between, each a few words the user can tap, each with the `project_id` and `kind` it stands for.",
            "the user can type instead of any of them.",
        ] {
            assert!(
                router.contains(rule),
                "the decision rule no longer says, verbatim: {rule}\n\nthe template says: {router}"
            );
        }
    }

    /// A project agent says what it is and what it can do. It has no checkout,
    /// no phases and no work verbs, so the template must not imply any.
    #[test]
    fn the_project_template_names_what_the_agent_is_and_its_whole_tool_inventory() {
        let project = collapse_whitespace(&render(
            &Templates::default().project_agent,
            &Vars {
                project_name: "Build",
                ..Vars::default()
            },
        ));
        assert!(
            project.contains("agent for the project Build"),
            "the agent is told which project it is the agent of: {project}"
        );
        assert!(
            project.contains("workspaces") && project.contains("agents"),
            "{project}"
        );
        for tool in [
            "list_workspaces",
            "list_workspace_agents",
            "post_thread_message",
            "search_conversation",
            "set_topic",
        ] {
            assert!(project.contains(tool), "{tool} missing from {project}");
        }
        for elsewhere in ["dispatch_branch", "list_projects", "read_unread_messages"] {
            assert!(
                !project.contains(elsewhere),
                "{elsewhere} is not on the project surface: {project}"
            );
        }
        assert!(
            !project.contains("phase"),
            "a project agent runs no phases: {project}"
        );
        assert!(
            project.contains("You stand in the project's base"),
            "{project}"
        );
    }

    /// The project agent orchestrates: work that touches files is placed on a
    /// workspace with an agent on it, independent pieces run side by side, and
    /// the agent itself neither does the work nor narrates it.
    #[test]
    fn the_project_template_tells_the_agent_to_place_work_rather_than_do_it() {
        let project = collapse_whitespace(&render(
            &Templates::default().project_agent,
            &Vars {
                project_name: "Build",
                ..Vars::default()
            },
        ));
        assert!(
            project.contains("its orchestrator and chief of staff, standing in for the user"),
            "the agent is told what it is for: {project}"
        );
        for sentence in [
            "are delegated through assigned tasks",
            "assigning one cuts the workspace and starts the agent",
            "The task is the record",
            "Parallelize independent work in separate workspaces",
            "give shared files one writer",
            "Your reach ends at this project, and you cannot read an agent's conversation.",
        ] {
            assert!(
                project.contains(sentence),
                "the orchestration rule no longer says, verbatim: {sentence}\n\nthe template says: {project}"
            );
        }
        for tool in [
            "`create_workspace`",
            "`add_workspace_agent`",
            "`message_workspace_agent`",
            "`create_task`",
            "`assign_task`",
            "`compact_agent`",
            "`compact_self`",
        ] {
            assert!(
                project.contains(tool),
                "the inventory names every tool the surface has, {tool} too: {project}"
            );
        }
        assert!(
            project.contains("Your tools: the task tools below;"),
            "the inventory cannot leave out the tools appended under it: {project}"
        );

        // Every rule the surface had before orchestration arrived is still here.
        for kept in [
            "Never change it: do not edit, check out, build or commit there",
            "Every change goes through a workspace",
            "Deleting a workspace destroys whatever in it is not committed and pushed",
            "Call `set_topic` first",
            "`Working`",
            "`Waiting`",
            "`Blocked`",
            "`Complete`",
        ] {
            assert!(project.contains(kept), "{kept} was dropped from {project}");
        }
    }

    /// A router has no checkout and no coding tools, and its terminal message reports the
    /// one phase it has.
    #[test]
    fn the_router_template_names_its_whole_tool_inventory_and_its_scratch() {
        let router = collapse_whitespace(&Templates::default().router);
        for tool in [
            "list_projects",
            "list_work",
            "read_conversation",
            "dispatch_branch",
            "ask_user",
            "post_thread_message",
        ] {
            assert!(router.contains(tool), "{tool} missing from {router}");
        }
        for coding_tool in ["read_unread_messages", "done"] {
            assert!(
                !router.contains(coding_tool),
                "{coding_tool} is not on the router's surface: {router}"
            );
        }
        assert!(!router.contains("create_task"), "{router}");
        assert!(router.contains("You are not in a repository"), "{router}");
        assert!(router.contains("status=\"Complete\""), "{router}");
        assert!(!router.contains("phase="), "{router}");
        assert!(router.contains("status=\"Blocked\""), "{router}");
    }

    #[test]
    fn the_router_template_substitutes_the_capture_and_the_answer() {
        let out = render(
            &Templates::default().router,
            &Vars {
                capture_text: "fix the login redirect",
                user_answer: "the bridge",
                ..Vars::default()
            },
        );
        assert!(out.contains("fix the login redirect"));
        assert!(out.contains("the bridge"));
        assert!(!out.contains("{capture_text}"));
        assert!(!out.contains("{user_answer}"));
    }

    /// Every agent that works in a checkout is told it can write to the other
    /// agents on its project, what for, and that its own report still goes to
    /// the user. The router is told none of it: it has no conversation of its
    /// own to be answered in.
    #[test]
    fn every_coding_template_says_an_agent_can_write_to_another_agent() {
        let t = Templates::default();
        for (name, template) in [
            ("plan", &t.plan),
            ("build", &t.build),
            ("build_stage", &t.build_stage),
            ("revise", &t.revise),
            ("revise_stage", &t.revise_stage),
            ("review_changes", &t.review_changes),
            ("message", &t.message),
        ] {
            let text = collapse_whitespace(template);
            assert!(text.contains("`message_agent`"), "{name}: {text}");
            assert!(
                text.contains("`post_thread_message`"),
                "{name} still reports to the user: {text}"
            );
        }
        assert!(
            !collapse_whitespace(&t.router).contains("message_agent"),
            "the router has no conversation to be answered in"
        );
    }

    /// The project agent is told about both ways of addressing an agent, and
    /// that neither reaches the user.
    #[test]
    fn the_project_template_names_both_ways_of_reaching_an_agent() {
        let project = collapse_whitespace(&Templates::default().project_agent);
        assert!(
            project.contains("`message_workspace_agent` and `message_agent`"),
            "{project}"
        );
        assert!(
            project.contains("A reply to an agent is a `message_agent` send"),
            "{project}"
        );
    }

    /// The project agent reads the context line on what agents send it, and
    /// before it hands a long-running agent its next task it compacts that
    /// agent — naming what the task needs kept — or starts a fresh one when
    /// the task's resume notes are enough (#68).
    #[test]
    fn the_project_template_says_to_compact_or_replace_a_full_agent_before_its_next_task() {
        let project = collapse_whitespace(&Templates::default().project_agent);
        for words in [
            "how full their context is",
            "before handing a long-running agent its next task",
            "`compact_agent`",
            "naming what to keep",
            "the task's notes",
            "fresh agent",
        ] {
            assert!(project.contains(words), "{words} missing from {project}");
        }
    }

    /// Every template that has `message_agent` says a reply to an agent is a
    /// send and that ending a turn reports to the user and reaches nobody. The
    /// coding templates say it in the shared note, and add that an agent asked
    /// for something answers it and then tells the user briefly; the project
    /// agent says it in its own words, and keeps its answers off the thread
    /// unless the call was the user's (#152).
    #[test]
    fn every_template_with_message_agent_says_a_reply_is_a_send() {
        let t = Templates::default();
        for (name, template) in templates_that_reach_another_agent(&t) {
            let text = collapse_whitespace(template);
            let sentences: &[&str] = if name == "project_agent" {
                &[
                    "`post_thread_message` is how the user hears from you in your thread, and it reaches no agent.",
                    "A reply to an agent is a `message_agent` send",
                    "Nothing is forwarded",
                ]
            } else {
                &[
                    "A reply to an agent is only ever a `message_agent` send.",
                    "`post_thread_message` reports to the user and reaches no agent",
                    "report to the user briefly",
                ]
            };
            for sentence in sentences {
                assert!(text.contains(sentence), "{name} does not say it: {text}");
            }
        }
    }

    /// The project agent's playbook (#152), carried by its role rather than
    /// prescribed step by step: an orchestrator and chief of staff that places
    /// work, decides the routine and brings the user only outcomes and their
    /// own calls. What the role cannot imply is pinned: what a decision of the
    /// user's is, and what each status is for. The old progress-feed wording
    /// stays gone.
    #[test]
    fn the_project_template_reports_only_outcomes_and_the_users_calls() {
        let project = collapse_whitespace(&Templates::default().project_agent);
        for words in [
            "orchestrator",
            "chief of staff",
            "standing in for the user",
            "You place work, decide the routine yourself",
            "only outcomes and the calls that are theirs",
            "one briefing as `Complete`",
            "Report outcomes as `Complete`",
            "Bring unresolved choices about irreversible actions, user-facing behavior or taste to the user as `Waiting`",
            "one question with the options and your recommendation",
            "Everything else stays on the task.",
            "`Working` is only for",
            "`Blocked` for when you cannot proceed",
        ] {
            assert!(
                project.contains(words),
                "the playbook no longer says: {words}\n\nthe template says: {project}"
            );
        }
        for feed in [
            "`Working` while you keep reading",
            "and report to the user.",
            "Say what you are about to remove before you remove it.",
        ] {
            assert!(
                !project.contains(feed),
                "the project agent is told to narrate again: {feed}"
            );
        }
    }

    /// And none of them promises the answer travels on its own, because no
    /// answer does: the bridge forwards nothing.
    #[test]
    fn no_template_promises_a_report_comes_back_on_its_own() {
        let t = Templates::default();
        for (name, template) in templates_that_reach_another_agent(&t) {
            let text = collapse_whitespace(template);
            for promise in [
                "arrives here as a message",
                "Its report comes back",
                "its report comes back",
                "comes back to you",
                "you do not have to go and look",
            ] {
                assert!(
                    !text.contains(promise),
                    "{name} promises a report travels: {text}"
                );
            }
        }
    }

    /// Every agent that works in a checkout is told that a second checkout is
    /// a Build workspace and that git's own worktrees are not an option. One
    /// note, appended by `coding_template`, so no phase can be missing it and
    /// no two phases can say it differently.
    #[test]
    fn every_coding_template_says_a_second_checkout_is_a_build_workspace() {
        let t = Templates::default();
        for (name, template) in [
            ("plan", &t.plan),
            ("build", &t.build),
            ("build_stage", &t.build_stage),
            ("revise", &t.revise),
            ("revise_stage", &t.revise_stage),
            ("review_changes", &t.review_changes),
            ("message", &t.message),
        ] {
            let text = collapse_whitespace(template);
            for sentence in [
                "A separate checkout is a Build workspace, and `create_workspace` is how you get one.",
                "Never `git worktree add`, never `git clone`, never a copy of the folder you are standing in",
                "A workspace you cut is where a sub-agent goes.",
                "say what you are about to remove before you remove it",
                "Build refuses to delete the workspace you are standing in",
            ] {
                assert!(text.contains(sentence), "{name} does not say it: {text}");
            }
            for tool in [
                "`create_workspace`",
                "`add_workspace_directory`",
                "`add_workspace_agent`",
                "`message_workspace_agent`",
                "`delete_workspace`",
                "`remove_workspace_directory`",
            ] {
                assert!(text.contains(tool), "{name} never names {tool}: {text}");
            }
        }
        // One string, carried whole: a phase cannot hold a softened copy of it.
        for (name, template) in [
            ("plan", &t.plan),
            ("build", &t.build),
            ("message", &t.message),
        ] {
            assert!(
                template.contains(WORKSPACE_NOTE),
                "{name} carries something other than the one note: {template}"
            );
        }
    }

    /// Every template that carries the task tools carries the same three rules
    /// about when to reach for them: file and assign rather than message, file
    /// and self-assign to plan your own work, and ask on the task you were
    /// handed. One note, appended by `coding_template` and to the project
    /// agent's prompt, so the coding surface and the project surface cannot
    /// come to say different things.
    #[test]
    fn every_template_with_the_task_tools_says_when_to_reach_for_them() {
        let t = Templates::default();
        for (name, template) in templates_with_the_task_tools(&t) {
            let text = collapse_whitespace(template);
            for sentence in [
                // Rule 1: a brief is a task, not a message.
                "Anything beyond a quick question or a one-line correction gets a task",
                "a brief sent as a message is a brief only its reader has",
                // Rule 2: plan your own work on the board.
                "Use Build tasks to plan your OWN work too.",
                "file a Build task for it with `create_task`",
                "assign it to yourself, and move it across the board as you go",
                // Rule 3: ask where both readers are.
                "When a task came from outside this conversation, ask ON the task.",
                "not this thread and not a message to whoever assigned it",
            ] {
                assert!(text.contains(sentence), "{name} does not say it: {text}");
            }
            // And the rules that were there before these three arrived.
            for kept in [
                "read it with `get_task` before you start",
                "Comment meaningful progress on it with `comment_task` as you go",
                "Move it to In review with `move_task` when you report Complete.",
                "Moving a task with an open review to Done with `move_task` also",
                "records the review as “Marked done”",
                "Hand work off by ASSIGNING the task, not by messaging.",
                "File a Build task for follow-up work you find and do not do.",
            ] {
                assert!(text.contains(kept), "{name} dropped an older rule: {text}");
            }
        }
    }

    /// Every template with the task tools says when the user is involved on a
    /// task (#232), in the one wording the tool descriptions use; the old
    /// advice to watch whatever the user asked for is gone.
    #[test]
    fn every_template_with_the_task_tools_says_when_to_involve_the_user() {
        use crate::test_support::user_involvement::*;
        let t = Templates::default();
        for (name, template) in templates_with_the_task_tools(&t) {
            for rule in [
                FLAG_SPLIT,
                REPORTED,
                ON_A_TASK,
                REPLY_THERE,
                ASKED_IN_THREAD,
                RESULT_ON_TASK,
                BUILD_ON,
                COMMENT_NOTIFY,
            ] {
                assert!(says(template, rule), "{name} does not say {rule:?}");
            }
            assert!(
                !says(template, "Use it when the user asked for the task"),
                "{name} still watches what the user asked for"
            );
            for stale in [
                "is the only thing they see",
                "is the only thing the user sees",
            ] {
                assert!(!says(template, stale), "{name} still says {stale:?}");
            }
        }
    }

    /// A Build task is a card on the project's board, and the harness has task
    /// tools of its own (#190). The note says which is which before anything
    /// else, so an agent never files steps on the board or thinks TaskCreate
    /// put something there.
    #[test]
    fn the_task_tools_say_a_build_task_is_not_the_harness_own_list() {
        let text = collapse_whitespace(TASK_TOOLS_NOTE);
        assert!(
            text.starts_with("Your project has a task board, and Build's task tools reach it:"),
            "{text}"
        );
        assert!(
            text.contains(
                "A Build task is a card on that board. It is not your harness's own task or \
                 todo list (TaskCreate, TodoWrite, update_plan): that one tracks the steps of \
                 this conversation, and nothing in it reaches the board."
            ),
            "{text}"
        );
    }

    /// Every template the task note is appended to: the coding phases and the
    /// project agent. The router has no project and gets none of it.
    /// Every agent that writes prose a reader will open carries the reference
    /// syntax (#56): the same one note, so a message, a task body and a
    /// comment cannot come to use different shapes for the same link.
    #[test]
    fn every_template_that_writes_prose_carries_the_reference_syntax() {
        let t = Templates::default();
        for (name, template) in templates_with_the_task_tools(&t) {
            assert!(
                template.contains(LINK_MARKUP_NOTE),
                "{name} carries something other than the one reference note: {template}"
            );
        }
    }

    /// The shapes themselves, named once here so a silent edit to the prompt
    /// cannot drift from what spa/src/core/markdownRefs.js actually parses.
    #[test]
    fn the_reference_note_names_every_shape_the_renderer_reads() {
        let text = collapse_whitespace(LINK_MARKUP_NOTE);
        for shape in [
            "`#42`",
            "`#42/c/<comment-id>`",
            "`@agent:<agent-id>`",
            "`@workspace:<name or id>`",
            "`@project:<name or id>`",
            "`[[<workspace>:path/to/file.rs]]`",
            "`#L10`",
            "`[[<workspace>:commit:<sha>]]`",
            "`[[<workspace>/<directory>:path/to/file.rs]]`",
        ] {
            assert!(text.contains(shape), "the note never shows {shape}: {text}");
        }
    }

    fn templates_with_the_task_tools(t: &Templates) -> Vec<(&'static str, &String)> {
        vec![
            ("plan", &t.plan),
            ("build", &t.build),
            ("build_stage", &t.build_stage),
            ("revise", &t.revise),
            ("revise_stage", &t.revise_stage),
            ("review_changes", &t.review_changes),
            ("message", &t.message),
            ("project_agent", &t.project_agent),
        ]
    }

    /// The two templates it is deliberately NOT on: the router has no project
    /// and no checkout, and the project agent says all of this in its own
    /// words, addressed to an agent that never stands in a workspace itself.
    #[test]
    fn the_router_and_project_templates_carry_no_workspace_note() {
        let t = Templates::default();
        for (name, template) in [("router", &t.router), ("project_agent", &t.project_agent)] {
            assert!(
                !template.contains("A separate checkout is a Build workspace"),
                "{name} carries the coding phases' note: {template}"
            );
        }
        assert!(
            !collapse_whitespace(&t.router).contains("create_workspace"),
            "the router cuts nothing: {}",
            t.router
        );
    }

    /// Every template whose agent can write to another agent — the coding
    /// phases and the project agent, which is all of them but the router.
    fn templates_that_reach_another_agent(t: &Templates) -> Vec<(&'static str, &String)> {
        vec![
            ("plan", &t.plan),
            ("build", &t.build),
            ("build_stage", &t.build_stage),
            ("revise", &t.revise),
            ("revise_stage", &t.revise_stage),
            ("review_changes", &t.review_changes),
            ("message", &t.message),
            ("project_agent", &t.project_agent),
        ]
    }

    /// A terminal message carries no phase and no structured outputs: the
    /// session already is its phase, and the plan's stages are read from disk.
    #[test]
    fn no_template_asks_for_a_phase_or_structured_outputs() {
        let t = Templates::default();
        for template in [
            &t.plan,
            &t.build,
            &t.build_stage,
            &t.revise,
            &t.revise_stage,
            &t.review_changes,
            &t.message,
            &t.router,
            &t.project_agent,
        ] {
            assert!(!template.contains("phase="), "{template}");
            assert!(!template.contains("outputs."), "{template}");
        }
    }

    #[test]
    fn stage_artifact_constants_are_pinned() {
        assert_eq!(STAGES_DIR, ".build/plan");
        assert_eq!(STAGES_MANIFEST_PATH, ".build/plan/stages.json");
    }

    /// Every template file ends in exactly one newline and no line ends in
    /// whitespace, so an editor that trims or pads a file cannot drift the
    /// prompt it compiles into (#155).
    #[test]
    fn every_template_file_ends_in_one_newline_with_no_trailing_whitespace() {
        let files = template_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("templates"));
        assert!(
            files.len() >= 14,
            "the template files went missing: {files:?}"
        );
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            let name = file.display();
            assert!(
                text.ends_with('\n') && !text.ends_with("\n\n"),
                "{name} must end in exactly one newline"
            );
            for (number, line) in text.lines().enumerate() {
                assert_eq!(
                    line,
                    line.trim_end(),
                    "{name}:{} ends in whitespace",
                    number + 1
                );
            }
        }
    }

    fn template_files(dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(template_files(&path));
            } else {
                files.push(path);
            }
        }
        files
    }
}
