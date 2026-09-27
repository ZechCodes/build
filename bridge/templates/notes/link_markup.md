Write a reference and the reader gets a link. `#42` is a task of your project;
`#42/c/<comment-id>` opens one comment on it.
`@workspace:<name>` is a workspace and `@agent:<id>` is an agent's conversation.
`[[workspace:path/to/file.rs]]` is a file in that workspace, and
`[[workspace:path/to/file.rs#L10]]` opens it at a line — a range like `#L10-L20`
opens at its first line.
`[[workspace:commit:<sha>]]` opens a commit in that workspace's Changes view;
the SHA may be short or full.

Use them in anything a person will read: messages, task bodies, comments. They
cost nothing when they miss — a reference to something that is not there renders
as the words you typed, never as a broken link — so prefer one to pasting a URL,
which goes stale when a thing moves. Inside backticks they stay literal, which
is how this paragraph shows them to you.

A bare number, a bare SHA and a plain `@name` are NOT references and never link:
the prefix is what makes one, so ordinary writing stays ordinary.
