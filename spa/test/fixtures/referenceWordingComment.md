## Agent-instruction text for review (bridge, text only)

This is the full before/after wording of every agent-facing text #229 changes. Nothing on the bridge merges until you sign off here. The SPA work is carrying on in the meantime.

**Why it's needed:** agents started in a workspace over the conversation protocol never see any link syntax today. Only the phase templates and the project agent's template carry it (`templates/notes/link_markup.md`). The MCP tool schemas just say "Markdown.".

---

### 1. Conversation protocol: one new last bullet (`bridge/src/orchestrator/workspace.rs`)

Every new agent process gets this block. The existing bullets are unchanged.

**Before:** no bullet about references.

**After:**
> - Link to Build things by reference instead of pasting ids or URLs: `#42` is task 42 of this project and `#42/c/<comment-id>` one comment on it; `@agent:<agent-id>`, `@workspace:<name or id>` and `@project:<name or id>` name an agent, a workspace and a project; `[[<workspace>:<path>#L10]]` is a file at a line and `[[<workspace>:commit:<sha>]]` a commit. For one source directory of a multi-source workspace, write `[[<workspace>/<directory>:<path>]]`. Use the workspace id when its name has spaces or a colon. Inside backticks a reference stays literal.

---

### 2. Tool `body` descriptions (`bridge/src/mcp.rs`)

The same sentence is added to the end of each of these:
> Link Build things by reference: `#42`, `#42/c/<comment-id>`, `@agent:<agent-id>`, `@workspace:<name or id>`, `@project:<name or id>`, `[[<workspace>:<path>#L10]]`, `[[<workspace>:commit:<sha>]]`.

**`post_thread_message` (workspace agents), `body`**
- Before:
  > The full report of this turn, in markdown, written for a reviewer who will not open the activity log. Lead with the outcome in one sentence, then say what changed and where (the files that carry it and why), how you verified it and what you could not, the decisions a reviewer would otherwise have to reverse-engineer, and what you deliberately left out or that remains at risk. Leave a heading out rather than pad it. If blocked or failed, lead with what is needed instead.
- After:
  > The full report of this turn, in markdown, written for a reviewer who will not open the activity log. Lead with the outcome in one sentence, then say what changed and where (the files that carry it and why), how you verified it and what you could not, the decisions a reviewer would otherwise have to reverse-engineer, and what you deliberately left out or that remains at risk. Leave a heading out rather than pad it. If blocked or failed, lead with what is needed instead. Link Build things by reference: `#42`, `#42/c/<comment-id>`, `@agent:<agent-id>`, `@workspace:<name or id>`, `@project:<name or id>`, `[[<workspace>:<path>#L10]]`, `[[<workspace>:commit:<sha>]]`.

**`post_thread_message` (project agent), `body`**
- Before:
  > What you have to say, in the user's terms. Complete for an outcome or an answer, Waiting when the next step is their call, Working only while a long read on their question is still going, never as a progress report.
- After:
  > What you have to say, in the user's terms. Complete for an outcome or an answer, Waiting when the next step is their call, Working only while a long read on their question is still going, never as a progress report. Link Build things by reference: `#42`, `#42/c/<comment-id>`, `@agent:<agent-id>`, `@workspace:<name or id>`, `@project:<name or id>`, `[[<workspace>:<path>#L10]]`, `[[<workspace>:commit:<sha>]]`.

**`comment_task`, `body`**
- Before:
  > Markdown.
- After:
  > Markdown. Link Build things by reference: `#42`, `#42/c/<comment-id>`, `@agent:<agent-id>`, `@workspace:<name or id>`, `@project:<name or id>`, `[[<workspace>:<path>#L10]]`, `[[<workspace>:commit:<sha>]]`.

**`create_task`, `body`**
- Before:
  > Markdown. What you know: what happens, where you saw it, what you think is behind it.
- After:
  > Markdown. What you know: what happens, where you saw it, what you think is behind it. Link Build things by reference: `#42`, `#42/c/<comment-id>`, `@agent:<agent-id>`, `@workspace:<name or id>`, `@project:<name or id>`, `[[<workspace>:<path>#L10]]`, `[[<workspace>:commit:<sha>]]`.

**`message_agent`, `body`**
- Before:
  > What to say, in full. The other agent has none of your conversation, so say what it needs rather than pointing at what you were told.
- After:
  > What to say, in full. The other agent has none of your conversation, so say what it needs rather than pointing at what you were told. Link Build things by reference: `#42`, `#42/c/<comment-id>`, `@agent:<agent-id>`, `@workspace:<name or id>`, `@project:<name or id>`, `[[<workspace>:<path>#L10]]`, `[[<workspace>:commit:<sha>]]`.

**`message_workspace_agent`, `body`**
- Before:
  > What to say, in full. The agent has none of your conversation, so say what it needs rather than pointing at what you were told.
- After:
  > What to say, in full. The agent has none of your conversation, so say what it needs rather than pointing at what you were told. Link Build things by reference: `#42`, `#42/c/<comment-id>`, `@agent:<agent-id>`, `@workspace:<name or id>`, `@project:<name or id>`, `[[<workspace>:<path>#L10]]`, `[[<workspace>:commit:<sha>]]`.

---

### 3. The template note (`bridge/templates/notes/link_markup.md`)

The phase templates and the project agent carry this note.

**Before:**
> Write a reference and the reader gets a link. `#42` is a task of your project; `#42/c/<comment-id>` opens one comment on it. `@workspace:<name>` is a workspace and `@agent:<id>` is an agent's conversation. `[[workspace:path/to/file.rs]]` is a file in that workspace, and `[[workspace:path/to/file.rs#L10]]` opens it at a line — a range like `#L10-L20` opens at its first line. `[[workspace:commit:<sha>]]` opens a commit in that workspace's Changes view; the SHA may be short or full.
>
> Use them in anything a person will read: messages, task bodies, comments. They cost nothing when they miss — a reference to something that is not there renders as the words you typed, never as a broken link — so prefer one to pasting a URL, which goes stale when a thing moves. Inside backticks they stay literal, which is how this paragraph shows them to you.
>
> A bare number, a bare SHA and a plain `@name` are NOT references and never link: the prefix is what makes one, so ordinary writing stays ordinary.

**After:**
> Write a reference and the reader gets a link, labelled with what it names. One of each:
>
> | Write | Links to |
> | --- | --- |
> | `#42` | task 42 of your project |
> | `#42/c/<comment-id>` | one comment on task 42 |
> | `@agent:<agent-id>` | an agent's conversation |
> | `@workspace:<name or id>` | a workspace |
> | `@project:<name or id>` | a project |
> | `[[<workspace>:path/to/file.rs]]` | a file in that workspace; add `#L10` for a line (`#L10-L20` opens at 10) |
> | `[[<workspace>:commit:<sha>]]` | a commit in that workspace's Changes view; short or full SHA |
> | `[[<workspace>/<directory>:path/to/file.rs]]` | a file in one source directory of a multi-source workspace; `commit:` takes the same prefix |
>
> A workspace or project is named by its title or its id. Use the id when the title has spaces or a colon. Workspaces, agents and projects of other projects resolve too; `#42` is always your own project's task.
>
> Use them in anything a person will read: messages, task bodies, comments. Prefer one to pasting a URL, which goes stale when a thing moves. A reference that names nothing reads as the words you typed, marked as not found. Inside backticks they stay literal, which is how this note shows them to you.
>
> A bare number, a bare SHA and a plain `@name` are NOT references and never link: the prefix is what makes one, so ordinary writing stays ordinary.

---

The existing drift test (`the_reference_note_names_every_shape_the_renderer_reads`) will be extended to the new shapes and to the protocol bullet, and a new test will hold the tool descriptions to the same sentence. A bridge roll restarts agents; it's to be batched with #228's.
