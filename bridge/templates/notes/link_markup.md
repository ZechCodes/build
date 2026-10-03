Write a reference and the reader gets a link, labelled with what it names. One of each:

| Write | Links to |
| --- | --- |
| `#42` | task 42 of your project |
| `#42/c/<comment-id>` | one comment on task 42 |
| `@agent:<agent-id>` | an agent's conversation |
| `@workspace:<name or id>` | a workspace |
| `@project:<name or id>` | a project |
| `[[<workspace>:path/to/file.rs]]` | a file in that workspace; add `#L10` for a line (`#L10-L20` opens at 10) |
| `[[<workspace>:commit:<sha>]]` | a commit in that workspace's Changes view; short or full SHA |
| `[[<workspace>/<directory>:path/to/file.rs]]` | a file in one source directory of a multi-source workspace; `commit:` takes the same prefix |

A workspace or project is named by its title or its id. Use the id when the
title has spaces or a colon. Workspaces, agents and projects of other projects
resolve too; `#42` is always your own project's task.

Use them in anything a person will read: messages, task bodies, comments. Prefer
one to pasting a URL, which goes stale when a thing moves. A reference that
names nothing reads as the words you typed, marked as not found. Inside
backticks they stay literal, which is how this note shows them to you.

A bare number, a bare SHA and a plain `@name` are NOT references and never link:
the prefix is what makes one, so ordinary writing stays ordinary.

For a checklist, write one `- [ ] item` per line and `- [x] item` for a completed
step. Task body checkboxes can be ticked by the reader and save back into the
body. Comments and messages show the same checklist with read-only boxes.
Put syntax examples inside backticks or a fenced code block to keep them literal.
