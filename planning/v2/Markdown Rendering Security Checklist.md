# Markdown rendering security checklist

**Status:** 100/100 (10/10 controls verified)
**Verified:** 2026-09-29 (#229)

This checklist covers the SPA's markdown renderer, an XSS boundary: everything an agent or a person writes (chat messages, task bodies, comments, plan docs, `.md` file previews, one-line previews) reaches the page through `spa/src/core/markdown.js` `markdownHtml` as `innerHTML`. The renderer is escape-first: every character of the input is HTML-escaped, and the only markup that comes out is the renderer's own fixed tags.

| # | Control | Score | Verification |
|---|---|---:|---|
| 1 | One entry point: no module under `spa/src/` renders markdown, parses references or builds a resolver except through `markdownHtml`; the guard names no exceptions | 10/10 | `spa/test/markdownEntry.test.js` |
| 2 | Every input character is escaped before any tag is added, in every block (paragraph, heading, list item, quote, table cell, fence) and every mode (block, inline) | 10/10 | `markdownSafety.test.js` "emits only the renderer's own tags and attributes, in every block and mode"; negative control: removing `esc` from the inline path fails it |
| 3 | Only an allow-listed set of tags and attributes is emitted; no `on*` attribute, no `src`, no free `style` (a table cell's `text-align` only), and an ordered list's `start` is a number of at most nine digits the renderer parsed | 10/10 | `markdownSafety.test.js` `violations()` over every payload and context |
| 4 | No URL from the input becomes a link: `javascript:`, `data:` and remote URLs stay text, images are never emitted; every `href` is a route written by `core/router.js` `hashFromRoute` and starts `#/` | 10/10 | `markdownSafety.test.js` "writes no link for a javascript: or data: URL", `href` check in `violations()` |
| 5 | Reference labels and titles (workspace, agent, project names, task titles, file paths, the text an agent typed) are escaped in both text and attribute context | 10/10 | `markdownSafety.test.js` "names and references that try to leave their attribute"; negative control: an unescaped workspace label fails it |
| 6 | A reference that resolves to nothing is never a link; it is the typed text, escaped, with an escaped reason in `title` | 10/10 | `markdownLinks.test.js` "a reference that does not resolve" |
| 7 | Code spans and fences are literal: references and markup inside them are not expanded | 10/10 | `markdown.test.js` "leaves a fenced block entirely alone", "leaves a code span alone while linking beside it"; `markdownSafety.test.js` fence contexts |
| 8 | Nesting is bounded (eight levels of quotes and lists); deeper input is read as escaped text, and rendering time stays linear on long mixed documents and on long runs of blank lines | 10/10 | `markdownSafety.test.js` "nesting that tries to exhaust the reader", including "reads a long run of blank lines after a list item in linear time, plain and quoted" (40,000 blank lines under 500 ms; 5.8 s before the fix). Re-scored after review #238: the first verification scored this 10/10 without testing blank-line runs, and the list reader was quadratic on them (11.8 s on the review's input); it was 0/10 until the fix |
| 9 | Plain mode answers text with no markup, and its callers escape it | 10/10 | `markdownSafety.test.js` "answers plain text with no markup in plain mode"; `agentOverview.js` writes the snippet through `esc` |
| 10 | Repository scanners are clean on the change | 10/10 | `semgrep --config auto --error` on the changed files (0 blocking findings); `gitleaks git --log-opts=main..HEAD` (0 leaks) |

**Total: 100/100.**
