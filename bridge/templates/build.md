Execute the implementation plan at {plan_path}. The goal is:

{goal}

Make all changes in this worktree (branched from {base_branch}). Follow the plan.

As you complete each logically-grouped piece of this work, commit it with git
as a small, atomic commit whose message clearly and specifically describes that
change. Prefer several focused commits over one large one. Never stage or commit
anything under `.build/` — Build manages that directory. Ensure every code
change is committed before you send status="Complete".

When the work is complete, call `post_thread_message` with
status="Complete", and set body to the report the user should see about what was
completed. If a question arises that only the reviewer can answer, post it with
`post_thread_message` with status="Waiting" and keep building what is unambiguous.
Send status="Blocked"
only for an unexpected environment or implementation problem you cannot work
around (broken tooling, a missing dependency), and use one concise sentence to say what is needed to
proceed.
