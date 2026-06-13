//! Prompt templates *are* the orchestration logic, so they're data, not code.
//!
//! The bridge ships defaults; a project overrides any of them by dropping files
//! in `.build/templates/`. The `done`-tool instructions live in the templates
//! (harness-agnostic, user-overridable, zero special cases). Variables: `{goal}`,
//! `{plan_path}`, `{comments}`, `{base_branch}`.

/// Where the plan file lives by convention (the agent reports the real path back
/// via `done`, so this is a default, not a hardcode).
pub const DEFAULT_PLAN_PATH: &str = ".build/plan.md";

const PLAN: &str = "\
You are in PLAN mode. The goal is:

{goal}

Produce a complete, self-contained implementation plan at {plan_path}. Plan only —
do not implement anything. A cold agent with no memory of this conversation should
be able to execute the plan from scratch. Write nothing outside `.build/`.

When the plan is ready, call the `done` tool with phase=\"plan\", status=\"completed\",
outputs.plan_path=\"{plan_path}\", and a short markdown summary for the reviewer: a
one-line outcome, then a few `-` bullets of the approach. Use `backticks` for paths
and commands; prefer scannable bullets over one long paragraph. If you cannot proceed,
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

const REVISE: &str = "\
The reviewer left notes on the plan at {plan_path}:

{comments}

Revise the plan to address every note. Keep writing only inside `.build/`. When done,
call the `done` tool with phase=\"plan\", status=\"completed\", outputs.plan_path=\"{plan_path}\",
and a short markdown summary (a one-line outcome, then `-` bullets of what changed).";

const REVIEW_CHANGES: &str = "\
The reviewer requested changes on your diff:

{comments}

Address every comment in this worktree. When done, call the `done` tool with
phase=\"revise\", status=\"completed\", and a short markdown summary of what changed
(a one-line outcome, then `-` bullets). Prefer scannable bullets over one long paragraph.";

/// The four phase templates. Clone-and-edit to override per project.
#[derive(Debug, Clone)]
pub struct Templates {
    pub plan: String,
    pub build: String,
    pub revise: String,
    pub review_changes: String,
}

impl Default for Templates {
    fn default() -> Self {
        Templates {
            plan: PLAN.to_string(),
            build: BUILD.to_string(),
            revise: REVISE.to_string(),
            review_changes: REVIEW_CHANGES.to_string(),
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
}

/// Render a template by substituting every `{var}` placeholder.
pub fn render(template: &str, vars: &Vars) -> String {
    template
        .replace("{goal}", vars.goal)
        .replace("{plan_path}", vars.plan_path)
        .replace("{comments}", vars.comments)
        .replace("{base_branch}", vars.base_branch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_substitutes_every_placeholder() {
        let out = render(
            "{goal} / {plan_path} / {comments} / {base_branch}",
            &Vars {
                goal: "ship it",
                plan_path: ".build/plan.md",
                comments: "fix x",
                base_branch: "main",
            },
        );
        assert_eq!(out, "ship it / .build/plan.md / fix x / main");
    }

    #[test]
    fn defaults_instruct_the_done_tool_and_plan_scope() {
        let t = Templates::default();
        assert!(t.plan.contains("`done`"));
        assert!(t.plan.contains("{plan_path}"));
        assert!(t.plan.contains(".build/"));
        assert!(t.build.contains("phase=\"build\""));
        assert!(t.review_changes.contains("phase=\"revise\""));
    }

    #[test]
    fn done_summaries_ask_for_markdown_bullets() {
        let t = Templates::default();
        for tmpl in [&t.plan, &t.build, &t.revise, &t.review_changes] {
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
            },
        );
        assert!(!out.contains("{goal}"));
        assert!(!out.contains("{plan_path}"));
        assert!(out.contains("add a greeting"));
    }
}
