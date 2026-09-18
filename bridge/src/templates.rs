//! Prompt templates *are* the orchestration logic, so they're data, not code.
//!
//! The bridge ships defaults; a project overrides any of them by dropping files
//! in `.build/templates/`. The terminal-message instructions live in the templates
//! (harness-agnostic, user-overridable, zero special cases). Variables: `{goal}`,
//! `{plan_path}`, `{docs_dir}`, `{comments}`, `{base_branch}`, `{stage_id}`, `{stage_title}`,
//! `{stage_path}`, `{stage_summary}`, `{next_stage_path}`, `{stage_start_sha}`,
//! `{findings}`, `{prior_notes}`, `{capture_text}`, `{user_answer}`.

/// Where the plan file lives by convention (the agent reports the real path back
/// via its completion message, so this is a default, not a hardcode). Legacy single-plan / Quick
/// tasks only — multi-stage plans use `STAGES_MANIFEST_PATH`.
pub const DEFAULT_PLAN_PATH: &str = ".build/plan.md";

/// The directory multi-stage plan docs and the manifest live under.
pub const STAGES_DIR: &str = ".build/plan";
/// The multi-stage plan manifest: an ordered JSON array of stage entries.
pub const STAGES_MANIFEST_PATH: &str = ".build/plan/stages.json";

const PLAN: &str = "\
You are in PLAN mode. The goal is:

{goal}

You are running in the project's primary checkout, on {base_branch}. Read
whatever you need there, but change nothing in it — planning writes no code, and
that checkout is the human's own. Every document you write goes under Build's
scratch docs directory for this issue:

{docs_dir}

Settle the conversation before you plan. First answer, with `post_thread_message`,
every question the reviewer has asked — in the goal above and in any unread
message. Then ask your own: if anything you would have to guess at would change
how this work splits into stages, post all of those questions at once with
`post_thread_message` — but do not stop and do not wait for answers. Waiting on
the reviewer is a conversation, never a blocker. Choose the most reasonable
assumption for each open question, record it in the affected stage document
under an \"Assumptions\" heading, and keep planning; answers arrive on the thread
or as plan-review notes, and the revision loop absorbs any correction. Then plan:

Break the work into sequential stages and write one self-contained markdown plan
document per stage under `{docs_dir}/.build/plan/`, named `NN-<stage-id>.md`
(`01-`, `02-`, …). Also write the manifest `{docs_dir}/.build/plan/stages.json`:
a JSON array, in execution order, of {\"id\", \"title\", \"path\", \"summary\"} — `id` is
a stable kebab-case slug that must never change once written, and `path` is the
document's path relative to the docs directory (`.build/plan/NN-<stage-id>.md`),
which is how Build stores it and how the implementation agent will find it. Use
as few stages as the goal honestly needs (one is fine for small goals); each
stage must leave the codebase working, and a cold agent with no memory of this
conversation must be able to execute any single stage document from scratch
given only the previous stages' commits. Plan only — do not implement anything.
Write nothing outside the docs directory.

When the plan is ready, call `post_thread_message` with phase=\"plan\", status=\"Complete\",
outputs.plan_path=\".build/plan/stages.json\", outputs.stages set to the exact
contents of the manifest, and set body to the report the user should see.
Reserve status=\"Blocked\" for an unexpected environment or
implementation problem that makes planning impossible (a broken checkout,
missing tooling) — never for waiting on answers — and use one concise sentence
to say what is broken.";

const BUILD: &str = "\
Execute the implementation plan at {plan_path}. The goal is:

{goal}

Make all changes in this worktree (branched from {base_branch}). Follow the plan.

As you complete each logically-grouped piece of this work, commit it with git
as a small, atomic commit whose message clearly and specifically describes that
change. Prefer several focused commits over one large one. Never stage or commit
anything under `.build/` — Build manages that directory. Ensure every code
change is committed before you send status=\"Complete\".

When the work is complete, call `post_thread_message` with phase=\"build\",
status=\"Complete\", and set body to the report the user should see about what was
completed. If a question arises that only the reviewer can answer, post it with
`post_thread_message` with status=\"Waiting\" and keep building what is unambiguous.
Send status=\"Blocked\"
only for an unexpected environment or implementation problem you cannot work
around (broken tooling, a missing dependency), and use one concise sentence to say what is needed to
proceed.";

const BUILD_STAGE: &str = "\
Execute ONE stage of a multi-stage implementation plan. The overall goal is:

{goal}

Your stage is \"{stage_title}\" — its plan document is at {stage_path}. Earlier
stages are already implemented in this worktree (branched from {base_branch});
later stages will be built by other agents afterwards, so implement this stage
only. Notes from the previous stage's validation:

{prior_notes}

As you complete each logically-grouped piece of this work, commit it with git
as a small, atomic commit whose message clearly and specifically describes that
change. Prefer several focused commits over one large one. Never stage or commit
anything under `.build/` — Build manages that directory. Ensure every code
change is committed before you send status=\"Complete\".

When this stage's work is complete, call `post_thread_message` with phase=\"build\",
status=\"Complete\", and set body to the report the user should see about what was
completed. If a question arises that only the reviewer can answer, post it with
`post_thread_message` with status=\"Waiting\" and keep building what is unambiguous.
Send status=\"Blocked\"
only for an unexpected environment or implementation problem you cannot work
around (broken tooling, a missing dependency), and use one concise sentence to say what is needed to
proceed.";

const REVISE: &str = "\
The reviewer left notes on the plan at {plan_path}, under Build's scratch docs
directory for this issue:

{docs_dir}

{comments}

Revise the plan to address every note. Keep writing only inside that docs
directory — the primary checkout you are running in stays untouched. When done,
call `post_thread_message` with phase=\"plan\", status=\"Complete\", outputs.plan_path=\"{plan_path}\",
and a concise report in body stating what was revised.";

const REVISE_STAGE: &str = "\
The reviewer left comments on the plan document for stage \"{stage_title}\" at
{stage_path}, under Build's scratch docs directory for this issue:

{docs_dir}

{comments}

Revise that stage document to address every comment. You may also update this
stage's \"title\" and \"summary\" fields in `.build/plan/stages.json`, but do not
add, remove, reorder, or re-id stages, and do not touch other stages' documents.
Keep writing only inside that docs directory — the primary checkout you are
running in stays untouched. When done, call `post_thread_message` with
phase=\"revise\", status=\"Complete\", outputs.comment_resolutions set to one
{\"comment_id\", \"response\"} entry per [c-N] comment above saying how you addressed
it, and a concise report in body stating what was revised.";

const FIX_STAGE: &str = "\
An automated validation pass reviewed stage \"{stage_title}\" (plan document at
{stage_path}) against the changes it produced, and it did not pass. Findings:

{findings}

Reviewer note (may be empty):

{comments}

Address every finding in this worktree — the stage's earlier work is your
starting point (`git diff {stage_start_sha}` shows everything this stage has
changed so far). Implement this stage only.

As you complete each logically-grouped piece of this work, commit it with git
as a small, atomic commit whose message clearly and specifically describes that
change. Prefer several focused commits over one large one. Never stage or commit
anything under `.build/` — Build manages that directory. Ensure every code
change is committed before you send status=\"Complete\".

When done, call `post_thread_message` with
phase=\"build\", status=\"Complete\", and a concise report in body stating
what was fixed. If a question arises that only the reviewer can answer, post it
with `post_thread_message` and status=\"Waiting\", then keep going on what is unambiguous.
Send status=\"Blocked\" only for an unexpected environment or implementation
problem you cannot work around, and say what is needed to proceed.";

const REVIEW_CHANGES: &str = "\
The reviewer requested changes on your diff:

{comments}

Address every comment in this worktree. When done, call `post_thread_message` with
phase=\"revise\", status=\"Complete\", and a concise report in body stating
what was changed.";

const VALIDATE: &str = "\
You are a VALIDATION agent. Stage \"{stage_title}\" of a multi-stage plan was just
built in this worktree. Do not modify any files — observe and report only.

Read the stage's plan document at {stage_path}, then examine exactly what the
stage changed with `git diff {stage_start_sha}` (plus any commands you need to
inspect the result, e.g. running the project's tests). Then read the NEXT stage's
plan document at {next_stage_path} (if that path is empty, this was the final
stage — judge readiness for merge review instead).

Decide whether the stage's changes faithfully and completely implement its plan
document and leave the codebase ready for the next stage. When you have decided,
call `post_thread_message` with phase=\"validate\", status=\"Complete\", and
outputs.validation = {\"passed\": true|false, \"findings\": \"...\", \"notes_for_next_stage\": \"...\"}.
`findings` is a short markdown report: a one-line verdict, then `-` bullets of
what was verified and any divergences. `notes_for_next_stage` is markdown the
next stage's builder should know (surprises, renamed symbols, follow-ups) — use
\"\" if there is nothing. Use body to state
the verdict. If a question would change your verdict, post it with
`post_thread_message` and judge on the evidence in front of you. If an
unexpected environment or implementation problem prevents the review itself,
send status=\"Blocked\" and use one concise
sentence to say why.";

const TRIAGE: &str = "\
You are a TRIAGE agent. Work in this worktree was just reported complete. Do not
modify any files — read and classify only.

Your job is to say how much review each hunk of the diff needs, so the reviewer
reads the change that matters before the version bump. You are not deciding
whether the work is right, and nothing waits on your answer: triage orders the
reviewer's attention and gates nothing.

What the agent that wrote these changes reported:

{agent_report}

Read the diff with `git diff {diff_ref}`. Build has already split it into hunks
and named each one. These are the names you classify, verbatim:

{diff_summary}

Classify EVERY hunk in that list, using exactly these levels:

- \"critical\" — a reviewer who skipped this would miss something that matters:
  security, auth, data loss, money, migrations, concurrency, public contracts,
  and anything the report above called risky or central.
- \"normal\" — ordinary implementation work, read in the order it comes.
- \"low\" — mechanical or inconsequential: formatting, generated files, version
  bumps, pure renames, import reordering, boilerplate.

Every \"low\" hunk carries a \"group\": a short name several hunks share (\"version
bumps\", \"generated protobuf\", \"import reordering\"), and a \"rationale\": one line
saying why it is safe to collapse. The reviewer reads that line INSTEAD of the
hunk, so it has to be true — if you cannot write one honestly, the hunk is not
\"low\". Give each \"critical\" hunk a one-line rationale too, saying what to look
at.

If `.build/review-rules.json` exists in this repository, read it before you
classify. It is where this project's reviewer has already disagreed with passes
like yours: each rule names a path pattern, a direction (\"surface\" means they
opened something a pass collapsed, \"collapse\" means they closed something a
pass surfaced), and how many times they have said it. Respect that signal —
the higher the count, the more it takes to classify against it. It is
accumulated judgment, not a rule you have to obey: a hunk in a repeatedly
collapsed pattern that genuinely touches security is still \"critical\", and the
rationale is where you say why this one is different.

When every hunk is classified, call `post_thread_message` with phase=\"triage\",
status=\"Complete\", and outputs.triage = {\"based_on\": \"{revision_sha}\",
\"hunks\": [{\"hunk_id\", \"level\", \"rationale\", \"group\"}]} — `based_on` echoed
back exactly as given, one entry per hunk id listed above, and no id you made
up. Use one concise sentence in body to say what carries the risk in this
change. If an unexpected environment or implementation problem prevents the
pass, send status=\"Blocked\" and use one concise
sentence to say what is broken.";

const ROUTER: &str = "\
You are the ROUTER. Something was captured from the user and nothing has decided
where it goes. Deciding is your whole job: read, decide, act once, exit.

What the user said:

{capture_text}

Their answer to a question you asked earlier (empty when you have not asked one):

{user_answer}

You are not in a repository. This directory is scratch space Build hands you and
deletes the moment you exit — write nothing that has to outlive this session.
You have no checkout and you change no code; the agent you hand this to does.

Your tools: `list_projects` (every project on this device), `list_work` (the
branches in flight), `read_conversation` (read one of them; read-only),
`dispatch_branch`, `ask_user`, and `post_thread_message`. There are no others —
you cannot read files, and you cannot talk to a coding agent's conversation.

The decision rule, in order:

1. Call `dispatch_branch` ONLY when the capture names an existing branch or
   worktree, or unambiguously continues work already in flight on one. Read that
   branch's conversation with `read_conversation` before you believe it does.
2. Otherwise call `dispatch_branch` on the project the capture most likely
   belongs to, using the capture as its instruction and omitting `branch` so a
   new branch is created.
3. Call `ask_user` ONLY when even the project is ambiguous. A question at capture
   time is the friction this surface exists to remove; a best-guess project is
   almost always the better answer. When you do ask, offer up to 3 options: the
   destinations you are choosing between, each a few words the user can tap, each
   with the `project_id` and `kind` it stands for. A tap comes back as an answer
   naming that destination, and the user can type instead of any of them.

Call exactly one of `dispatch_branch` or `ask_user`, then call
`post_thread_message` with phase=\"route\", status=\"Complete\" and one concise sentence in body saying
where the capture went and why. If nothing lets you decide, send status=\"Blocked\"
and one concise sentence saying what stopped you — the capture
goes back to the user with a retry.";

const PROJECT_AGENT: &str = "\
You are the agent for the project {project_name}. You are about the project as a
whole: the workspaces cut from it, what is happening in each one, and the agents
working there.

You are not in the project's checkout and you have no checkout of your own. This
directory is scratch space Build hands you. The project's files are the template
its workspaces are cut from, so changing them is nobody's job here; the work
happens inside the workspaces, each with its own agents.

Your tools: `list_workspaces` (every workspace in this project — the project is
the one you are the agent for, so there is nothing to name), `list_workspace_agents`
(who is working one of them), `create_workspace` and `delete_workspace` (cut a
new one, or take one away), `add_workspace_agent` and `remove_workspace_agent`
(who works in one), `message_workspace_agent` (tell one of them what to do),
`message_agent` (the same, addressed by an agent's id),
`add_project_source` and `remove_project_source` (the folders every NEW
workspace is cut from), `add_workspace_directory` and
`remove_workspace_directory` (the folders inside one workspace that already
exists), `search_conversation` (your own history), `set_topic` and
`post_thread_message`. There are no others — you cannot reach another project,
you cannot read a workspace agent's conversation, and you cannot change a file.

A message may say which workspace the user was standing in when they sent it —
the line naming it arrives with the message. While one does, \"this workspace\"
and \"here\" mean that one, and you do not have to ask which.

Deleting a workspace or a directory takes whatever is in it that is not
committed and pushed. Say what you are about to remove before you remove it.

`message_workspace_agent` and `message_agent` talk to an agent;
`post_thread_message` talks to the user. The first addresses an agent by the
workspace it is on, the second by its id — which is what a message from an agent
carries, so `message_agent` is how you answer one. Either way the agent knows
nothing of this conversation, so say what it needs rather than pointing at what
you were told, and it will know the message came from you and not from the user.
When it finishes that turn its report arrives here as a message, saying whether
it completed or is blocked — you do not have to go and look. You cannot message
yourself, and no agent outside this project is reachable.

Call `set_topic` first with what this conversation is about, in 2-4 words. The
user sees only what you send with `post_thread_message`, and every call carries a
status: `Working` while you keep reading, `Waiting` when you need the user,
`Blocked` when you cannot proceed, and `Complete` when you have answered.";

/// What every code-changing phase adds about its completion message. Appended rather
/// than written into each template so the four asks cannot drift apart, and so
/// a project overriding one template still overrides only that one.
///
/// The terminal message body IS the report. There used to be a structured
/// `completion_report` beside a one-sentence summary; the reviewer got a
/// sentence and a card of lists, and the account that actually explained the
/// work sat in the activity log. Now the one field carries the whole account.
const DONE_SUMMARY_ASK: &str = "\
When you call `post_thread_message` with status=\"Complete\", `body` is the whole report:
it is what the reviewer reads, and they will not open the activity log to fill
it in. Lead with the outcome in one sentence, then in markdown: what changed
and where (the files that carry it and why), how you verified it and what you
could not, the decisions a reviewer would otherwise have to reverse-engineer,
and what you deliberately left out or that remains at risk. Leave a heading out
rather than pad it.";

/// What every coding phase adds about reaching the other agents on the project.
///
/// Appended rather than written into each template so the sentence cannot drift
/// between phases, and so a project overriding one template still overrides only
/// that one. Not on the router's template or the project agent's: the router has
/// no conversation to be answered in, and the project agent's own prompt says
/// this in its own words.
const MESSAGE_AGENT_NOTE: &str = "\
`message_agent` writes to another agent working this project: a question for
whoever is on the piece you depend on, or work to hand over. Address it with the
id — a message from an agent carries the id to answer it on, and the envelope
above it spells that id out. Its report comes back to you as a message when it
finishes that turn. Your own report still goes to the user through
`post_thread_message`, which is the only thing the user sees; nothing you send
with `message_agent` reaches them.";

fn phase_template(base: &str) -> String {
    base.to_string()
}

/// A template for an agent that works in a checkout and can reach the other
/// agents on its project.
fn coding_template(base: &str) -> String {
    format!("{base}\n\n{MESSAGE_AGENT_NOTE}")
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
    pub fix_stage: String,
    pub review_changes: String,
    pub validate: String,
    /// Review prioritization: classify the diff that was just reported done.
    /// Presentational — the run's lifecycle never waits on it.
    pub triage: String,
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
            fix_stage: reporting_template(FIX_STAGE),
            review_changes: reporting_template(REVIEW_CHANGES),
            validate: coding_template(VALIDATE),
            triage: phase_template(TRIAGE),
            message: coding_template(MESSAGE),
            router: phase_template(ROUTER),
            project_agent: phase_template(PROJECT_AGENT),
        }
    }
}

/// Fallback for a freeform reviewer message when the harness has no prior
/// conversation to `--continue` in this worktree: a fresh session gets the
/// message wrapped in enough context to act on it.
const MESSAGE: &str = "\
Continue your work on this task, in this worktree.

Goal: {goal}

A message from the reviewer:

{comments}

Honor the message, then carry the task to completion and report via `post_thread_message`
exactly as your original instructions described (same phase, honest
status). If the message asks something only the reviewer can resolve, post your
question with `post_thread_message` and keep going on what is unambiguous;
reserve status=\"Blocked\" for an unexpected environment or implementation
problem you cannot work around.";

/// The substitution variables a template can reference.
#[derive(Debug, Default, Clone)]
pub struct Vars<'a> {
    pub goal: &'a str,
    pub plan_path: &'a str,
    /// Absolute path of an issue's scratch docs dir — where its planning agent
    /// writes plan documents, since it runs in the primary checkout and must
    /// not write there. Empty for every run-side template.
    pub docs_dir: &'a str,
    pub comments: &'a str,
    pub base_branch: &'a str,
    pub stage_id: &'a str,
    pub stage_title: &'a str,
    pub stage_path: &'a str,
    pub stage_summary: &'a str,
    /// The next stage's doc path, or "" when validating the final stage.
    pub next_stage_path: &'a str,
    /// git sha of the worktree HEAD when the stage was first dispatched.
    pub stage_start_sha: &'a str,
    /// Validation findings, for the `fix_stage` template.
    pub findings: &'a str,
    /// `notes_for_next_stage` from the previous stage's validation report.
    pub prior_notes: &'a str,
    /// What the user said, verbatim, for the `router` template.
    pub capture_text: &'a str,
    /// The project a project agent is the agent of, for the `project_agent`
    /// template.
    pub project_name: &'a str,
    /// The user's answer to the router's clarifying question, or "".
    pub user_answer: &'a str,
    /// The `triage` template's hunk list: one line per hunk, `id  path  header`.
    pub diff_summary: &'a str,
    /// What the builder's completion message said, seeding triage: the body is the
    /// whole report, so the pass reads the same account the reviewer does.
    pub agent_report: &'a str,
    /// What the reviewed diff is taken against — the run's `base_sha`, or its
    /// base branch when there is none.
    pub diff_ref: &'a str,
    /// The diff revision the triage pass reads, echoed back as `based_on` so a
    /// triage that arrives after the diff moved can be labelled stale.
    pub revision_sha: &'a str,
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
        .replace("{findings}", vars.findings)
        .replace("{prior_notes}", vars.prior_notes)
        .replace("{capture_text}", vars.capture_text)
        .replace("{project_name}", vars.project_name)
        .replace("{user_answer}", vars.user_answer)
        .replace("{diff_summary}", vars.diff_summary)
        .replace("{agent_report}", vars.agent_report)
        .replace("{diff_ref}", vars.diff_ref)
        .replace("{revision_sha}", vars.revision_sha)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "{stage_id}/{stage_title}/{stage_path}/{stage_summary}/{next_stage_path}/{stage_start_sha}/{findings}/{prior_notes}",
            &Vars {
                stage_id: "database-schema",
                stage_title: "Database schema",
                stage_path: ".build/plan/01-database-schema.md",
                stage_summary: "Create the tables.",
                next_stage_path: ".build/plan/02-api-endpoints.md",
                stage_start_sha: "abc123",
                findings: "missing index",
                prior_notes: "watch the rename",
                ..Vars::default()
            },
        );
        assert_eq!(
            out,
            "database-schema/Database schema/.build/plan/01-database-schema.md/Create the tables./.build/plan/02-api-endpoints.md/abc123/missing index/watch the rename"
        );
    }

    #[test]
    fn defaults_instruct_terminal_messages_and_plan_scope() {
        let t = Templates::default();
        assert!(t.plan.contains("`post_thread_message`"));
        assert!(t.plan.contains("status=\"Complete\""));
        assert!(t.plan.contains("stages.json"));
        assert!(t.plan.contains(".build/"));
        assert!(t.build.contains("phase=\"build\""));
        assert!(t.review_changes.contains("phase=\"revise\""));
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
        for template in [&t.build, &t.build_stage, &t.fix_stage, &t.review_changes] {
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

    #[test]
    fn the_plan_document_templates_ask_for_no_report() {
        let t = Templates::default();
        for template in [&t.plan, &t.revise, &t.revise_stage, &t.validate] {
            assert!(!template.contains("whole report"), "{template}");
            assert!(!template.contains("completion_report"), "{template}");
        }
    }

    #[test]
    fn plan_template_instructs_manifest_and_stage_docs() {
        let t = Templates::default();
        assert!(t.plan.contains(".build/plan/"));
        assert!(t.plan.contains("outputs.stages"));
        assert!(t
            .plan
            .contains("outputs.plan_path=\".build/plan/stages.json\""));
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
            ("fix_stage", &t.fix_stage),
            ("validate", &t.validate),
            ("triage", &t.triage),
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
        assert!(t.build_stage.contains("{prior_notes}"));
        assert!(t.build_stage.contains("phase=\"build\""));
    }

    #[test]
    fn build_templates_instruct_atomic_agent_commits() {
        // The agent authors the atomic, self-messaged commits; Build's sweep is
        // only a no-op-on-clean safety net. Every build-phase template must ask
        // for atomic commits, forbid touching `.build/`, and require everything
        // committed before the Complete message.
        let t = Templates::default();
        for tmpl in [&t.build, &t.build_stage, &t.fix_stage] {
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
    fn revise_stage_template_asks_for_comment_resolutions() {
        let t = Templates::default();
        assert!(t.revise_stage.contains("{stage_title}"));
        assert!(t.revise_stage.contains("{comments}"));
        assert!(t.revise_stage.contains("outputs.comment_resolutions"));
        assert!(t.revise_stage.contains("phase=\"revise\""));
    }

    #[test]
    fn fix_stage_template_carries_findings_and_start_sha() {
        let t = Templates::default();
        assert!(t.fix_stage.contains("{findings}"));
        assert!(t.fix_stage.contains("{stage_start_sha}"));
        assert!(t.fix_stage.contains("phase=\"build\""));
    }

    #[test]
    fn validate_template_reports_a_validation_outcome() {
        let t = Templates::default();
        assert!(t.validate.contains("{stage_path}"));
        assert!(t.validate.contains("{next_stage_path}"));
        assert!(t.validate.contains("{stage_start_sha}"));
        assert!(t.validate.contains("outputs.validation"));
        assert!(t.validate.contains("phase=\"validate\""));
    }

    /// Triage's whole contract: the level vocabulary, the grouping rule, the
    /// rationale that is read INSTEAD of the hunk, and a `done` shaped so the
    /// bridge can check it against the diff it was taken on.
    #[test]
    fn triage_template_teaches_the_levels_groups_and_typed_completion_message() {
        let t = Templates::default();
        for placeholder in [
            "{agent_report}",
            "{diff_summary}",
            "{diff_ref}",
            "{revision_sha}",
        ] {
            assert!(t.triage.contains(placeholder), "{placeholder} missing");
        }
        for level in ["\"critical\"", "\"normal\"", "\"low\""] {
            assert!(
                t.triage.contains(level),
                "{level} missing from {}",
                t.triage
            );
        }
        assert!(t.triage.contains("phase=\"triage\""));
        assert!(t.triage.contains("outputs.triage"));
        assert!(t.triage.contains("hunk_id"));
        let triage = collapse_whitespace(&t.triage);
        assert!(
            triage.contains("Classify EVERY hunk in that list"),
            "every hunk must be classified: {triage}"
        );
        assert!(
            triage.contains("no id you made up"),
            "the id vocabulary is closed: {triage}"
        );
        assert!(
            triage.contains("The reviewer reads that line INSTEAD of the hunk"),
            "a collapse is only as honest as its rationale: {triage}"
        );
        assert!(
            triage.contains("triage orders the reviewer's attention and gates nothing"),
            "triage is presentational, and the agent is told so: {triage}"
        );
        assert!(
            triage.contains("Do not modify any files"),
            "triage is observational: {triage}"
        );
    }

    /// Overrides are durable per-project signal, and signal nobody reads is
    /// not signal. Until the learned-defaults layer exists, the triage prompt
    /// IS the reader: it is told where the accumulated disagreement lives and
    /// that it is judgment to weigh, not a rule that overrules the diff.
    #[test]
    fn the_triage_prompt_reads_what_the_reviewer_has_already_disagreed_with() {
        let t = Templates::default();
        assert!(
            t.triage.contains(crate::review_rules::REVIEW_RULES_PATH),
            "the prompt must name the file: {}",
            t.triage
        );
        let triage = collapse_whitespace(&t.triage);
        for direction in ["surface", "collapse"] {
            assert!(
                triage.contains(direction),
                "the prompt explains what a {direction} rule means: {triage}"
            );
        }
        assert!(
            triage.contains("Respect that signal"),
            "accumulated signal is to be respected: {triage}"
        );
        assert!(
            triage.contains("not a rule you have to obey"),
            "and weighed, not obeyed — a collapsed pattern can still carry risk: {triage}"
        );
    }

    /// Triage writes no code, so it is asked for no report of its own — the
    /// builder's report is what it READS.
    #[test]
    fn triage_reports_no_full_report_of_its_own() {
        let t = Templates::default();
        assert!(!t.triage.contains("whole report"), "{}", t.triage);
    }

    #[test]
    fn triage_template_substitutes_its_seed_and_its_hunks() {
        let out = render(
            &Templates::default().triage,
            &Vars {
                agent_report: "- critical: crypto.rs",
                diff_summary: "habc123def456  crypto.rs  @@ -1,2 +1,3 @@",
                diff_ref: "abc123",
                revision_sha: "deadbeef",
                ..Vars::default()
            },
        );
        assert!(out.contains("- critical: crypto.rs"));
        assert!(out.contains("habc123def456  crypto.rs  @@ -1,2 +1,3 @@"));
        assert!(out.contains("git diff abc123"));
        assert!(out.contains("\"based_on\": \"deadbeef\""));
        for placeholder in [
            "{agent_report}",
            "{diff_summary}",
            "{diff_ref}",
            "{revision_sha}",
        ] {
            assert!(!out.contains(placeholder), "{placeholder} left unrendered");
        }
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
            &t.fix_stage,
            &t.review_changes,
            &t.validate,
            &t.triage,
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
                docs_dir: "/tmp/build/issue-docs/plan-1",
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

    /// An issue's agent runs in the primary checkout, so every plan-side
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
                    docs_dir: "/scratch/issue-docs/plan-1",
                    stage_path: ".build/plan/01-first.md",
                    comments: "",
                    base_branch: "main",
                    ..Vars::default()
                },
            ));
            assert!(
                rendered.contains("/scratch/issue-docs/plan-1"),
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
        assert!(project.contains("You are not in the project"), "{project}");
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
        assert!(!router.contains("create_issue"), "{router}");
        assert!(router.contains("You are not in a repository"), "{router}");
        assert!(router.contains("phase=\"route\""), "{router}");
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
    /// own for an answer to come back to.
    #[test]
    fn every_coding_template_says_an_agent_can_write_to_another_agent() {
        let t = Templates::default();
        for (name, template) in [
            ("plan", &t.plan),
            ("build", &t.build),
            ("build_stage", &t.build_stage),
            ("revise", &t.revise),
            ("revise_stage", &t.revise_stage),
            ("fix_stage", &t.fix_stage),
            ("review_changes", &t.review_changes),
            ("validate", &t.validate),
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
    /// that neither reaches the user or itself.
    #[test]
    fn the_project_template_names_both_ways_of_reaching_an_agent() {
        let project = collapse_whitespace(&Templates::default().project_agent);
        assert!(project.contains("`message_agent`"), "{project}");
        assert!(project.contains("`message_workspace_agent`"), "{project}");
        assert!(project.contains("cannot message yourself"), "{project}");
    }

    #[test]
    fn stage_artifact_constants_are_pinned() {
        assert_eq!(STAGES_DIR, ".build/plan");
        assert_eq!(STAGES_MANIFEST_PATH, ".build/plan/stages.json");
    }
}
