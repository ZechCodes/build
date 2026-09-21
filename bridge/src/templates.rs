//! Prompt templates *are* the orchestration logic, so they're data, not code.
//!
//! The bridge ships defaults; a project overrides any of them by dropping files
//! in `.build/templates/`. The terminal-message instructions live in the templates
//! (harness-agnostic, user-overridable, zero special cases). Variables: `{goal}`,
//! `{plan_path}`, `{docs_dir}`, `{comments}`, `{base_branch}`, `{stage_id}`, `{stage_title}`,
//! `{stage_path}`, `{stage_summary}`, `{next_stage_path}`, `{stage_start_sha}`,
//! `{capture_text}`, `{user_answer}`.

/// Where a single-document plan lives. A plan whose docs dir holds
/// `STAGES_MANIFEST_PATH` when its agent reports Complete is multi-stage
/// instead, read from that manifest.
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

When the plan is ready, call `post_thread_message` with status=\"Complete\" and set
body to the report the user should see. Build reads the stages from
`.build/plan/stages.json` in the docs directory when you do, so write the
manifest before you report.
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

When the work is complete, call `post_thread_message` with
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
only.

As you complete each logically-grouped piece of this work, commit it with git
as a small, atomic commit whose message clearly and specifically describes that
change. Prefer several focused commits over one large one. Never stage or commit
anything under `.build/` — Build manages that directory. Ensure every code
change is committed before you send status=\"Complete\".

When this stage's work is complete, call `post_thread_message` with
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
call `post_thread_message` with status=\"Complete\" and a concise report in body
stating what was revised.";

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
status=\"Complete\" and a concise report in body stating what was revised and
how you addressed each comment.";

const REVIEW_CHANGES: &str = "\
The reviewer requested changes on your diff:

{comments}

Address every comment in this worktree. When done, call `post_thread_message` with
status=\"Complete\" and a concise report in body stating what was changed.";

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
`post_thread_message` with status=\"Complete\" and one concise sentence in body saying
where the capture went and why. If nothing lets you decide, send status=\"Blocked\"
and one concise sentence saying what stopped you — the capture
goes back to the user with a retry.";

const PROJECT_AGENT: &str = "\
You are the agent for the project {project_name}, and you are its orchestrator.
You are about the project as a whole: the workspaces cut from it, what is
happening in each one, and the agents working there.

You are not in the project's checkout and you have no checkout of your own. This
directory is scratch space Build hands you. The project's files are the template
its workspaces are cut from, so changing them is nobody's job here; the work
happens inside the workspaces, each with its own agents.

Work that touches files is not yours to do — it is yours to place. Anything that
has to be read closely, written, built, tested or fixed gets a workspace of its
own: cut one with `create_workspace`, put an agent on it with
`add_workspace_agent`, and tell that agent what the work is with
`message_workspace_agent`. The brief is the whole of what that agent gets, so
say it in full rather than pointing at what you were told. Then coordinate:
check what came of it, ask the agent what you still need to know, and report to
the user. Work already in flight goes to the workspace and the agent that hold
it rather than to a new one.

Hand significant work over as an ISSUE rather than as a message. Anything beyond
a quick question or a one-line correction: file it with `create_issue`, put the
brief in the body, and hand it over with `assign_issue` — which will cut the
workspace and put an agent on it in the same call. The issue is the record, and
the record is the point: it is where the user looks, where that agent asks what
it needs, and what is still there when this conversation is not.

Split what is independent. Two pieces of work that do not read each other's
changes are two workspaces with an agent on each, started in the same turn,
rather than one agent taking them in order. Two pieces that touch the same files
are one workspace and one agent — two agents in one checkout overwrite each
other. Size a workspace to a piece of work, never to a single file.

Do not do the work in your own words. A plan written out here, a diff described
from memory, a file discussed as though you had opened it — none of that is the
work, and you have nothing to check any of it against. Place it, follow it, and
report what actually happened.

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
`post_thread_message`. Beyond the issue tools below there are no others — you
cannot reach another project, you cannot read a workspace agent's conversation,
and you cannot change a file.

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
You cannot message yourself, and no agent outside this project is reachable.

A reply to an agent is only ever a `message_agent` send. `post_thread_message`
reports to the user and reaches no agent, so an agent you handed work to tells
you nothing by finishing its turn: what reaches you is the message it writes
you, and if you need to know where it got to, ask it. When one of them asks YOU
for something, send the answer with `message_agent` first, then report to the
user briefly — what you did, not a second copy of what you already sent.

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
///
/// It has to say that a reply is a send, because nothing carries one for the
/// agent. A report used to be forwarded to whoever asked for the turn, and an
/// agent told that would answer by ending its turn and say the same thing twice
/// when it answered properly too.
const MESSAGE_AGENT_NOTE: &str = "\
`message_agent` writes to another agent working this project: a question for
whoever is on the piece you depend on, work to hand over, or the answer to
something one of them asked you. Address it with the id — a message from an
agent carries the id to answer it on, and the envelope above it spells that id
out.

A reply to an agent is only ever a `message_agent` send. `post_thread_message`
reports to the user and reaches no agent, whatever its status, so ending your
turn answers nobody. When another agent asked you for something, send the answer
with `message_agent` first, then report to the user briefly — what you did, not
a second copy of what you already sent. Nothing you send with `message_agent`
reaches the user, and `post_thread_message` is the only thing they see.";

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
const WORKSPACE_NOTE: &str = "\
A separate checkout is a Build workspace, and `create_workspace` is how you get
one. It cuts every source of this project afresh, on a branch of its own, the
way the project is configured to cut them — a git worktree, or a copy-on-write
clone. Never `git worktree add`, never `git clone`, never a copy of the folder
you are standing in: a checkout Build did not cut is one nobody can see, nobody
can review, and nothing cleans up. `add_workspace_directory` brings one more
source or folder into a workspace that is already standing, and
`list_workspaces` and `list_workspace_agents` say what exists already and who
is on it.

A workspace you cut is where a sub-agent goes. When a piece of this work is
independent enough to run beside yours — it needs none of your uncommitted
changes and touches none of the same files — cut a workspace for it, put an
agent on it with `add_workspace_agent`, and brief that agent with
`message_workspace_agent`. Say the whole of what it needs; it has none of your
conversation.

Say what the new agent is to BE rather than what it should run on: pass `role`
— planner, implementer, reviewer or executor — and the user's own choice of
model answers it. The answer tells you that model's CAPABILITY, which is how
much direction to write: a generalist needs the goal, a scoped one needs the
scope and the constraints, and a step-by-step one needs the steps. Brief it
accordingly; a one-line brief to a step-by-step model wastes both of you.

Naming a model yourself overrides that choice, so do it only when the user
named one — and then `list_harnesses` has the ids, every harness this machine
has, what each accepts, and which are actually installed. The effort is yours
to judge either way: which model fills a role is the user's standing decision,
how hard it thinks about one piece of work is your call.

`delete_workspace` takes a workspace away and `remove_workspace_directory`
takes one directory out of one. Both take whatever is in them that is not
committed and pushed, so say what you are about to remove before you remove it.
Neither can take the ground out from under you: Build refuses to delete the
workspace you are standing in, or to remove the directory your own checkout is
in.";

/// What every agent with the issue tools is told about them (spec: Issues →
/// The prompt note).
///
/// Appended rather than written into each template, for the reason
/// [`MESSAGE_AGENT_NOTE`] is: the wording cannot drift between the coding
/// surface and the project surface, and a project overriding one template
/// still overrides only that one.
///
/// It says the things an agent gets wrong without being told. An issue handed
/// over is the WORK, not a note about it, so progress belongs on the issue and
/// not only in a conversation nobody else reads. In review is what Complete
/// means on a board — ready to be looked at, not accepted — and an agent that
/// moves its own issue to Done is marking its own homework. A hand-off is an
/// assignment, because an assignment delivers the issue and leaves a record
/// while a message leaves only words. And work an agent notices and does not do
/// exists nowhere unless it is filed.
///
/// Three more, added because the conversation kept being used for what the
/// board is for. A brief sent as a message is a brief only its reader has, so
/// anything bigger than a correction is filed and assigned. A multi-step piece
/// of work planned only in a conversation is invisible to the user and dies
/// with the session, so an agent files and self-assigns its own. And a question
/// about somebody else's issue asked anywhere but that issue reaches one person
/// when it needed to reach two.
const ISSUE_TOOLS_NOTE: &str = "\
Your project has an issue tracker, and the issue tools reach it: `list_issues`,
`get_issue`, `create_issue`, `comment_issue`, `assign_issue`, `move_issue`,
`close_issue` and `link_issue`. They are about YOUR project — there is nothing
to pass and no other project is reachable — and the bridge knows who you are, so
what you write is signed by you.

When a message hands you an issue, that issue is the work. The message is only a
notice naming it, so read it with `get_issue` before you start — and again if
you have been running a while, since it may have moved — because the body says
what is wanted and the timeline says what has already been tried. Comment your
progress on it with `comment_issue` as you go, rather than only reporting at the
end — your conversation is yours, and the issue is where the user and the other
agents look.

Move it to In review with `move_issue` when you report Complete. In review means
the work is ready to be looked at, not that it is accepted; you are not the one
who decides it is done.

Hand work off by ASSIGNING the issue, not by messaging. Anything beyond a quick
question or a one-line correction gets an issue: file it with the brief in the
body, then assign it. `assign_issue` delivers the issue into that agent's
conversation and starts it, and leaves a record on the issue that a message does
not: a brief sent as a message is a brief only its reader has. Assigning is what
dispatching is here.

Use issues to plan your OWN work too. When what you have taken on is more than a
single step, file an issue for it — or one for each piece that could be worked
independently — assign it to yourself, and move it across the board as you go.
That is how the user sees what is in progress without opening this conversation,
and it is how the plan outlives the session: one that lives only here is lost
with it. Assign yourself any issue you pick up that nobody handed you, too:
assigning records the workspace and conversation you are working in onto the
issue, so what the work is and where it is happening stay together.

When an issue came from outside this conversation, ask ON the issue. A question
you need answered, a decision that is not yours, something you found that
changes what was asked — `comment_issue`, not this thread and not a message to
whoever assigned it. The assigner and the user both read the issue and the
answer comes back there; asked anywhere else it reaches one of them at best.

File an issue for follow-up work you find and do not do. An issue is cheap, and
something you noticed and did not write down exists only in this conversation.

`track_issue` makes an issue tell you when it moves: every later change to it
arrives as a message here, and starts your turn if you are idle. Track the ones
you depend on rather than going back to look. An issue you file tracks you
unless you say `track: false`, and every other issue write takes `track: true`
to follow it from then on, so following is never a second call. You are tracked automatically on
anything assigned to you, your own changes are never sent back to you, and
`untrack_issue` stops it — being unassigned does not, because handing work on is
often exactly when you still want to know how it went.";

/// The reference shapes an agent can write, so a link into Build costs a few
/// characters rather than a route nobody can remember (#56).
///
/// One note on every template that writes prose a reader opens — a message, an
/// issue body, a comment — because the same shapes have to mean the same thing
/// wherever they are typed. The renderer that reads them is
/// `spa/src/core/markdownRefs.js`; a test below holds this list to it.
const LINK_MARKUP_NOTE: &str = "\
Write a reference and the reader gets a link. `#42` is an issue of your project.
`@workspace:<name>` is a workspace and `@agent:<id>` is an agent's conversation.
`[[workspace:path/to/file.rs]]` is a file in that workspace, and
`[[workspace:path/to/file.rs#L10]]` opens it at a line — a range like `#L10-L20`
opens at its first line.

Use them in anything a person will read: messages, issue bodies, comments. They
cost nothing when they miss — a reference to something that is not there renders
as the words you typed, never as a broken link — so prefer one to pasting a URL,
which goes stale when a thing moves. Inside backticks they stay literal, which
is how this paragraph shows them to you.

A bare number, a bare SHA and a plain `@name` are NOT references and never link:
the prefix is what makes one, so ordinary writing stays ordinary.";

fn phase_template(base: &str) -> String {
    base.to_string()
}

/// A template for an agent that works in a checkout: it can reach the other
/// agents on its project, and it can cut a checkout of its own to put one of
/// them on.
fn coding_template(base: &str) -> String {
    format!("{base}\n\n{MESSAGE_AGENT_NOTE}\n\n{WORKSPACE_NOTE}\n\n{ISSUE_TOOLS_NOTE}\n\n{LINK_MARKUP_NOTE}")
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
            // The project agent gets the issue note and not the message note:
            // its own prompt already says what messaging another agent means,
            // in its own words, and the tracker is the one thing it has that
            // that prompt predates.
            project_agent: format!("{PROJECT_AGENT}\n\n{ISSUE_TOOLS_NOTE}\n\n{LINK_MARKUP_NOTE}"),
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
exactly as your original instructions described, with an honest status. If the message asks something only the reviewer can resolve, post your
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
    /// The next stage's doc path, or "" on the final stage.
    pub next_stage_path: &'a str,
    /// git sha of the worktree HEAD when the stage was first dispatched.
    pub stage_start_sha: &'a str,
    /// What the user said, verbatim, for the `router` template.
    pub capture_text: &'a str,
    /// The project a project agent is the agent of, for the `project_agent`
    /// template.
    pub project_name: &'a str,
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
        .replace("{user_answer}", vars.user_answer)
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
            project.contains("you are its orchestrator"),
            "the agent is told what it is for: {project}"
        );
        for sentence in [
            "Work that touches files is not yours to do — it is yours to place.",
            "Split what is independent.",
            "Do not do the work in your own words.",
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
        ] {
            assert!(
                project.contains(tool),
                "{tool} is how work is placed: {project}"
            );
        }
        // Rule 1: significant work is handed over as an issue, not as a
        // message — the project agent's own words for it.
        assert!(
            project.contains("Hand significant work over as an ISSUE rather than as a message."),
            "the project agent hands work over as an issue: {project}"
        );
        for tool in ["`create_issue`", "`assign_issue`"] {
            assert!(
                project.contains(tool),
                "{tool} is how significant work is handed over: {project}"
            );
        }
        assert!(
            project.contains("Beyond the issue tools below there are no others"),
            "the inventory cannot end before the eight tools appended under it: {project}"
        );

        // Every rule the surface had before orchestration arrived is still here.
        for kept in [
            "scratch space Build hands you",
            "you cannot change a file",
            "Say what you are about to remove before you remove it.",
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
        assert!(!router.contains("create_issue"), "{router}");
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
    /// that neither reaches the user or itself.
    #[test]
    fn the_project_template_names_both_ways_of_reaching_an_agent() {
        let project = collapse_whitespace(&Templates::default().project_agent);
        assert!(project.contains("`message_agent`"), "{project}");
        assert!(project.contains("`message_workspace_agent`"), "{project}");
        assert!(project.contains("cannot message yourself"), "{project}");
    }

    /// Every template that has `message_agent` says the same three things: a
    /// reply to an agent is a send, ending a turn reports to the user and
    /// reaches nobody, and an agent that was asked for something answers it and
    /// then tells the user what it did rather than saying it twice.
    #[test]
    fn every_template_with_message_agent_says_a_reply_is_a_send() {
        let t = Templates::default();
        for (name, template) in templates_that_reach_another_agent(&t) {
            let text = collapse_whitespace(template);
            for sentence in [
                "A reply to an agent is only ever a `message_agent` send.",
                "`post_thread_message` reports to the user and reaches no agent",
                "report to the user briefly",
            ] {
                assert!(text.contains(sentence), "{name} does not say it: {text}");
            }
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

    /// Every template that carries the issue tools carries the same three rules
    /// about when to reach for them: file and assign rather than message, file
    /// and self-assign to plan your own work, and ask on the issue you were
    /// handed. One note, appended by `coding_template` and to the project
    /// agent's prompt, so the coding surface and the project surface cannot
    /// come to say different things.
    #[test]
    fn every_template_with_the_issue_tools_says_when_to_reach_for_them() {
        let t = Templates::default();
        for (name, template) in templates_with_the_issue_tools(&t) {
            let text = collapse_whitespace(template);
            for sentence in [
                // Rule 1: a brief is an issue, not a message.
                "Anything beyond a quick question or a one-line correction gets an issue",
                "a brief sent as a message is a brief only its reader has",
                // Rule 2: plan your own work on the board.
                "Use issues to plan your OWN work too.",
                "assign it to yourself, and move it across the board as you go",
                // Rule 3: ask where both readers are.
                "When an issue came from outside this conversation, ask ON the issue.",
                "not this thread and not a message to whoever assigned it",
            ] {
                assert!(text.contains(sentence), "{name} does not say it: {text}");
            }
            // And the rules that were there before these three arrived.
            for kept in [
                "read it with `get_issue` before you start",
                "Comment your progress on it with `comment_issue` as you go",
                "Move it to In review with `move_issue` when you report Complete.",
                "Hand work off by ASSIGNING the issue, not by messaging.",
                "File an issue for follow-up work you find and do not do.",
            ] {
                assert!(text.contains(kept), "{name} dropped an older rule: {text}");
            }
        }
    }

    /// Every template the issue note is appended to: the coding phases and the
    /// project agent. The router has no project and gets none of it.
    /// Every agent that writes prose a reader will open carries the reference
    /// syntax (#56): the same one note, so a message, an issue body and a
    /// comment cannot come to use different shapes for the same link.
    #[test]
    fn every_template_that_writes_prose_carries_the_reference_syntax() {
        let t = Templates::default();
        for (name, template) in templates_with_the_issue_tools(&t) {
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
            "#42",
            "@workspace:",
            "@agent:",
            "[[workspace:path/to/file.rs]]",
            "#L10",
        ] {
            assert!(text.contains(shape), "the note never shows {shape}: {text}");
        }
    }

    fn templates_with_the_issue_tools(t: &Templates) -> Vec<(&'static str, &String)> {
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
}
