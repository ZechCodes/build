//! Prompt templates *are* the orchestration logic, so they're data, not code.
//!
//! The bridge ships defaults; a project overrides any of them by dropping files
//! in `.build/templates/`. The `done`-tool instructions live in the templates
//! (harness-agnostic, user-overridable, zero special cases). Variables: `{goal}`,
//! `{plan_path}`, `{comments}`, `{base_branch}`, `{stage_id}`, `{stage_title}`,
//! `{stage_path}`, `{stage_summary}`, `{next_stage_path}`, `{stage_start_sha}`,
//! `{findings}`, `{prior_notes}`.

use crate::task::StageComment;

/// Where the plan file lives by convention (the agent reports the real path back
/// via `done`, so this is a default, not a hardcode). Legacy single-plan / Quick
/// tasks only — multi-stage plans use `STAGES_MANIFEST_PATH`.
pub const DEFAULT_PLAN_PATH: &str = ".build/plan.md";

/// The directory multi-stage plan docs and the manifest live under.
pub const STAGES_DIR: &str = ".build/plan";
/// The multi-stage plan manifest: an ordered JSON array of stage entries.
pub const STAGES_MANIFEST_PATH: &str = ".build/plan/stages.json";

const PLAN: &str = "\
You are in PLAN mode. The goal is:

{goal}

Break the work into sequential stages and write one self-contained markdown plan
document per stage under `.build/plan/`, named `NN-<stage-id>.md` (`01-`, `02-`, …).
Also write the manifest `.build/plan/stages.json`: a JSON array, in execution
order, of {\"id\", \"title\", \"path\", \"summary\"} — `id` is a stable kebab-case slug
that must never change once written. Use as few stages as the goal honestly needs
(one is fine for small goals); each stage must leave the codebase working, and a
cold agent with no memory of this conversation must be able to execute any single
stage document from scratch given only the previous stages' commits. Plan only —
do not implement anything. Write nothing outside `.build/`.

When the plan is ready, call the `done` tool with phase=\"plan\", status=\"completed\",
outputs.plan_path=\".build/plan/stages.json\", outputs.stages set to the exact
contents of the manifest, and a short markdown summary for the reviewer: a
one-line outcome, then one `-` bullet per stage. Use `backticks` for paths and
commands; prefer scannable bullets over one long paragraph. If you cannot proceed,
call `done` with status=\"blocked\" and, in the same markdown format, say what you need.";

const BUILD: &str = "\
Execute the implementation plan at {plan_path}. The goal is:

{goal}

Make all changes in this worktree (branched from {base_branch}). Follow the plan.

When the work is complete, call the `done` tool with phase=\"build\",
status=\"completed\", and a short markdown summary for the reviewer: a one-line
outcome, then a few `-` bullets of the key changes (use `backticks` for paths and
commands). Prefer scannable bullets over one long paragraph. If you get stuck, call
`done` with status=\"blocked\" (you need something) or status=\"failed\" (the approach
did not work) and, in the same markdown format, say what is needed to proceed.";

const BUILD_STAGE: &str = "\
Execute ONE stage of a multi-stage implementation plan. The overall goal is:

{goal}

Your stage is \"{stage_title}\" — its plan document is at {stage_path}. Earlier
stages are already implemented in this worktree (branched from {base_branch});
later stages will be built by other agents afterwards, so implement this stage
only. Notes from the previous stage's validation:

{prior_notes}

When this stage's work is complete, call the `done` tool with phase=\"build\",
status=\"completed\", and a short markdown summary for the reviewer: a one-line
outcome, then a few `-` bullets of the key changes (use `backticks` for paths and
commands). If you get stuck, call `done` with status=\"blocked\" (you need
something) or status=\"failed\" (the approach did not work) and, in the same
markdown format, say what is needed to proceed.";

const REVISE: &str = "\
The reviewer left notes on the plan at {plan_path}:

{comments}

Revise the plan to address every note. Keep writing only inside `.build/`. When done,
call the `done` tool with phase=\"plan\", status=\"completed\", outputs.plan_path=\"{plan_path}\",
and a short markdown summary (a one-line outcome, then `-` bullets of what changed).";

const REVISE_STAGE: &str = "\
The reviewer left comments on the plan document for stage \"{stage_title}\" at
{stage_path}:

{comments}

Revise that stage document to address every comment. You may also update this
stage's \"title\" and \"summary\" fields in `.build/plan/stages.json`, but do not
add, remove, reorder, or re-id stages, and do not touch other stages' documents.
Keep writing only inside `.build/`. When done, call the `done` tool with
phase=\"revise\", status=\"completed\", outputs.comment_resolutions set to one
{\"comment_id\", \"response\"} entry per [c-N] comment above saying how you addressed
it, and a short markdown summary (a one-line outcome, then `-` bullets of what changed).";

const FIX_STAGE: &str = "\
An automated validation pass reviewed stage \"{stage_title}\" (plan document at
{stage_path}) against the changes it produced, and it did not pass. Findings:

{findings}

Reviewer note (may be empty):

{comments}

Address every finding in this worktree — the stage's earlier work is your
starting point (`git diff {stage_start_sha}` shows everything this stage has
changed so far). Implement this stage only. When done, call the `done` tool with
phase=\"build\", status=\"completed\", and a short markdown summary of what changed
(a one-line outcome, then `-` bullets). If you get stuck, call `done` with
status=\"blocked\" or status=\"failed\" and say, in the same markdown format, what is
needed to proceed.";

const REVIEW_CHANGES: &str = "\
The reviewer requested changes on your diff:

{comments}

Address every comment in this worktree. When done, call the `done` tool with
phase=\"revise\", status=\"completed\", and a short markdown summary of what changed
(a one-line outcome, then `-` bullets). Prefer scannable bullets over one long paragraph.";

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
call the `done` tool with phase=\"validate\", status=\"completed\", and
outputs.validation = {\"passed\": true|false, \"findings\": \"...\", \"notes_for_next_stage\": \"...\"}.
`findings` is a short markdown report: a one-line verdict, then `-` bullets of
what was verified and any divergences. `notes_for_next_stage` is markdown the
next stage's builder should know (surprises, renamed symbols, follow-ups) — use
\"\" if there is nothing. Also give a one-line markdown summary with a few `-`
bullets in the summary argument. If you cannot complete the review, call `done`
with status=\"blocked\" or status=\"failed\" and say, with markdown bullets, why.";

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
}

impl Default for Templates {
    fn default() -> Self {
        Templates {
            plan: PLAN.to_string(),
            build: BUILD.to_string(),
            build_stage: BUILD_STAGE.to_string(),
            revise: REVISE.to_string(),
            revise_stage: REVISE_STAGE.to_string(),
            fix_stage: FIX_STAGE.to_string(),
            review_changes: REVIEW_CHANGES.to_string(),
            validate: VALIDATE.to_string(),
        }
    }
}

/// The substitution variables a template can reference.
#[derive(Debug, Default, Clone)]
pub struct Vars<'a> {
    pub goal: &'a str,
    pub plan_path: &'a str,
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
}

/// Render a template by substituting every `{var}` placeholder.
pub fn render(template: &str, vars: &Vars) -> String {
    template
        .replace("{goal}", vars.goal)
        .replace("{plan_path}", vars.plan_path)
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
}

/// Render the `{comments}` block for a `revise_stage` prompt from a stage's
/// open comments, in insertion order. Pure — the orchestrator filters to
/// `Open` comments for one stage before calling this.
pub fn assemble_stage_comments(comments: &[StageComment]) -> String {
    comments
        .iter()
        .enumerate()
        .map(|(index, comment)| render_one_stage_comment(index + 1, comment))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_one_stage_comment(number: usize, comment: &StageComment) -> String {
    let heading = match &comment.anchor {
        Some(anchor) if anchor.heading_path.is_empty() => format!(
            "{number}. [{id}] On the passage: \"{snippet}\"",
            id = comment.id,
            snippet = collapse_whitespace(&anchor.snippet),
        ),
        Some(anchor) => format!(
            "{number}. [{id}] Under \"{heading_path}\", on the passage: \"{snippet}\"",
            id = comment.id,
            heading_path = anchor.heading_path.join(" > "),
            snippet = collapse_whitespace(&anchor.snippet),
        ),
        None => format!("{number}. [{id}] (general)", id = comment.id),
    };
    format!("{heading}\n   Comment: {body}", body = comment.body)
}

/// Collapse every run of whitespace to a single space and trim the ends.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{CommentAnchor, CommentState, StageComment};

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
    fn defaults_instruct_the_done_tool_and_plan_scope() {
        let t = Templates::default();
        assert!(t.plan.contains("`done`"));
        assert!(t.plan.contains("stages.json"));
        assert!(t.plan.contains(".build/"));
        assert!(t.build.contains("phase=\"build\""));
        assert!(t.review_changes.contains("phase=\"revise\""));
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
    fn build_stage_template_scopes_to_one_stage() {
        let t = Templates::default();
        assert!(t.build_stage.contains("{stage_title}"));
        assert!(t.build_stage.contains("{stage_path}"));
        assert!(t.build_stage.contains("{prior_notes}"));
        assert!(t.build_stage.contains("phase=\"build\""));
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

    #[test]
    fn done_summaries_ask_for_markdown_bullets() {
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
        ] {
            let lower = tmpl.to_lowercase();
            assert!(
                lower.contains("markdown"),
                "template should ask for markdown: {tmpl}"
            );
            assert!(
                lower.contains("bullet"),
                "template should ask for bullets: {tmpl}"
            );
            assert!(
                !lower.contains("one-paragraph"),
                "no 'one-paragraph' guidance: {tmpl}"
            );
        }
    }

    #[test]
    fn rendered_plan_has_no_leftover_placeholders() {
        let out = render(
            &Templates::default().plan,
            &Vars {
                goal: "add a greeting",
                plan_path: DEFAULT_PLAN_PATH,
                comments: "",
                base_branch: "main",
                ..Vars::default()
            },
        );
        assert!(!out.contains("{goal}"));
        assert!(!out.contains("{plan_path}"));
        assert!(out.contains("add a greeting"));
    }

    #[test]
    fn stage_artifact_constants_are_pinned() {
        assert_eq!(STAGES_DIR, ".build/plan");
        assert_eq!(STAGES_MANIFEST_PATH, ".build/plan/stages.json");
    }

    fn comment(
        id: &str,
        anchor: Option<CommentAnchor>,
        body: &str,
        state: CommentState,
    ) -> StageComment {
        StageComment {
            id: id.to_string(),
            stage_id: "database-schema".to_string(),
            anchor,
            body: body.to_string(),
            state,
            agent_reply: None,
        }
    }

    #[test]
    fn assemble_stage_comments_renders_anchored_and_general_entries() {
        let comments = vec![
            comment(
                "c-3",
                Some(CommentAnchor {
                    heading_path: vec!["Database schema".to_string(), "Tables".to_string()],
                    snippet: "users table gets a soft-delete column".to_string(),
                }),
                "use a deleted_at timestamp, not a boolean",
                CommentState::Open,
            ),
            comment(
                "c-4",
                None,
                "this stage feels too big, split the migration from the model changes",
                CommentState::Open,
            ),
        ];
        let out = assemble_stage_comments(&comments);
        assert_eq!(
            out,
            "1. [c-3] Under \"Database schema > Tables\", on the passage: \"users table gets a soft-delete column\"\n   Comment: use a deleted_at timestamp, not a boolean\n\n2. [c-4] (general)\n   Comment: this stage feels too big, split the migration from the model changes"
        );
    }

    #[test]
    fn assemble_stage_comments_handles_empty_heading_path() {
        let comments = vec![comment(
            "c-1",
            Some(CommentAnchor {
                heading_path: vec![],
                snippet: "  a passage   with   extra   whitespace  ".to_string(),
            }),
            "tighten this up",
            CommentState::Open,
        )];
        let out = assemble_stage_comments(&comments);
        assert_eq!(
            out,
            "1. [c-1] On the passage: \"a passage with extra whitespace\"\n   Comment: tighten this up"
        );
    }

    #[test]
    fn assemble_stage_comments_empty_slice_is_empty_string() {
        assert_eq!(assemble_stage_comments(&[]), "");
    }
}
