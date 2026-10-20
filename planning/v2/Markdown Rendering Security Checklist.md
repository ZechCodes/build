# Markdown rendering security checklist

**Status:** 100/100 (10/10 controls verified)
**Verified:** 2026-10-20 (#695 rich markdown rendering); earlier desktop link, credential, persistence and API authentication evidence remains recorded below (#262, #615, #616).

Checklist rendering re-verified 2026-10-03 (#347): checklist list items have the
fixed `task-item` class and a native input with only fixed checkbox type/class,
boolean checked/disabled attributes and numeric source/index metadata. Task
bodies and comments use `aria-labelledby` references to their rendered label
text, with stable document-scoped IDs encoded into a fixed safe vocabulary.
Unscoped read-only surfaces derive escaped accessible names from rendered inline
text. Other markup in an item remains escape-first. Checkboxes
are disabled by default; only task bodies enable saving. The edit helper calls
the same `markdownHtml` entry point to identify a marker and replaces only its
checked character, preserving the source, CRLF, code literals and nesting bound.
`markdownTasks.test.js` verifies nested source mapping and paragraph marker
lookalikes; `markdownTaskLabels.test.js` verifies rendered names and stable,
distinct scopes. `markdownSafety.test.js` includes hostile checklist payloads,
checks every label reference and rejects extra input attributes.

This checklist covers the SPA's markdown renderer, an XSS boundary: everything an agent or a person writes (chat messages, task bodies, comments, plan docs, `.md` file previews, one-line previews) reaches the page through `spa/src/core/markdown.js` `markdownHtml` as `innerHTML`. The renderer is escape-first: source text is escaped before insertion into generated markup. It recognizes explicit markdown productions and two narrow HTML-shaped productions; arbitrary HTML remains visible text. The emitted vocabulary is fixed and independently checked after browser parsing.

Rich rendering (#695) adds emphasis and deletion, exact backtick-run code spans,
text-only attribute-free `<kbd>…</kbd>`, HTTP(S) images, angle URL/email
autolinks, five fixed alerts, fenced Prism highlighting, balanced details with
an immediate one-line summary, and document-scoped footnotes. The HTML-shaped
syntax is exact lowercase `<kbd>` without attributes or nested opening tags,
and standalone attribute-free `<details>` / `</details>` lines with an immediate
attribute-free `<summary>…</summary>` line. Attributes, uppercase lookalikes,
unclosed details and arbitrary HTML stay escaped. Details, quotes and lists
share the eight-level nesting bound.

Images require an HTTP(S) URL with a parsed host and no username/password.
They emit only escaped `src`, `alt`, optional `title`, fixed `loading="lazy"`
and `referrerpolicy="no-referrer"`. They do not accept data, blob, mailto,
relative, protocol-relative or credential-bearing sources. Existing web-link
credential behavior is unchanged. Image, anchor, keyboard and code output is
protected from later emphasis, Build-reference and footnote expansion.

Footnotes use generated positive numbers and hex-codepoint document scopes:
`md-fn:{scope}:{number}` and `md-fnref:{scope}:{number}:{occurrence}`. A caller's
`taskLabelScope` is encoded injectively; the unscoped API uses a deterministic
pair of source hashes. Surfaces rendering multiple documents supply distinct
stable scopes. Only exact generated forward/back fragments may link locally,
and they must identify one matching renderer-owned target. Missing references
stay text; fenced definitions, code/keyboard literals, link labels and image
attributes do not become footnotes. Definition text uses the same renderer
without recursive footnote expansion.

Fences accept only registered language names/extension aliases for code classes.
Unknown labels remain escaped labels with literal code. Prism's core and fixed
local grammars supply only span tokens; no autoloader or DOM highlighting runs.
Sources above 20,000 characters take the escaped path to bound grammar work.
Tables retain only fixed alignment styles; accessible scroll wrappers use the
fixed `.mdtable`, `tabindex="0"`, `role="region"`, `aria-label="Table"`, headers
use `scope="col"`, and short body cells may use `md-cell-short`.

Since #256 the renderer writes web links as well as Build routes (`spa/src/core/markdownWebLinks.js`): `[text](url)`, `[text](url "title")` and bare `http(s)://` URLs in prose. That widened the rule this checklist used to record, "every href is a `#/` route"; control 4 is the rule now. Internal forms (`[text](#42)`, `[text](#/…)`) and relative paths are refused, not routed: a Build reference has its own syntax (`#42`, `@workspace:…`, `[[ws:path]]`), and a relative path has no page to be relative to. A refused link renders as the words typed. In the desktop app, only `https://getbuild.ing/app/desktop` (including query strings and hash routes) stays in the app window. `installNavigationPolicy` in `desktop/src/security-policy.mjs`, wired by `desktop/src/main.mjs`, sends other http, https and mailto links to the system browser with just their URL and denies new windows; the same rule covers navigation and redirects. Landing pages, docs, sibling paths and path-prefix lookalikes are external. The server-side API boundary is pinned by `test_spa_api_guards.py` across every `/api/*` call in SPA source; `test_rtc_controller.py` proves desktop bearer and browser access to ICE credential minting require active alpha membership (#616). Request authentication is separate: only the main-frame desktop document (the server guards `GET /app/desktop`) and same-origin `/api/` fetch/XHR requests receive Bearer headers. The desktop document is the one navigation that carries the Bearer; iframes and subresources aimed at that path, API document navigation, and public pages receive none. `installAuthorizationHeader` registers `onBeforeSendHeaders` with `<all_urls>`: supplied or inherited Authorization headers are stripped case-insensitively on every host before applying that allowlist, including after cross-origin redirects (#262). Since #615, refresh credentials are encrypted in main-process storage, restored only through a token refresh, and cleared by Account → Sign Out; access tokens remain in memory. The scoped [Desktop Security Checklist](Desktop%20Security%20Checklist.md) records the persistence evidence.

| # | Control | Score | Verification |
|---|---|---:|---|
| 1 | One entry point: no module under `spa/src/` renders markdown, parses references or builds a resolver except through `markdownHtml`; the guard names no exceptions | 10/10 | `spa/test/markdownEntry.test.js` |
| 2 | All source text is escaped before insertion into generated markup in every block and mode, including alert bodies, details summaries/bodies, image labels/titles, footnote definitions and highlighted source. Only the exact keyboard/details grammar is recognized as structure | 10/10 | `markdownSafety.test.js` hostile payload/context matrix and "keeps arbitrary HTML inert beside the exact keyboard and details productions"; positive control "accepts the exact generated rich vocabulary" |
| 3 | Only renderer-owned tags, attributes and class values are emitted. No event handlers or arbitrary styles; table alignment is fixed, list starts have at most nine digits, code languages and Prism token names are constrained, and new image/details/footnote attributes use their precise grammar | 10/10 | `markdownSafety.test.js` browser-parsed `violations()` checks every payload/context; "rejects extra rich tags, classes, language names and attributes in the oracle" and checklist/image negative controls prove the oracle rejects forged output |
| 4 | Every href is a Build route on `a.md-ref`, an external `a.md-link` with HTTP(S) host syntax or mailto plus `target="_blank" rel="noopener noreferrer nofollow"`, or an exact scoped footnote fragment pointing at one matching generated target. Unsafe schemes, protocol-relative and relative links remain text. Image sources are separately narrowed to parsed credential-free HTTP(S) URLs | 10/10 | `markdownSafety.test.js` unsafe links, opener checks, image policy, and forged/duplicated footnote fragments; `markdownWebLinks.test.js` unsafe schemes/whitespace/control probes and positive controls; `markdownNavigation.test.js` local footnote handling. Desktop navigation and credential evidence: `desktop/test/security-policy.test.mjs`, `desktop/test/main-auth.test.mjs`, `desktop/test/token-store.test.mjs` (#262, #615) |
| 5 | Reference labels/titles, URLs, image alt/title, fence labels and document scope values cannot leave their text or attribute context. Generated element contents and attributes are protected from later readers | 10/10 | `markdownSafety.test.js` hostile names/references, "escapes image text and protects literal and attribute contents from later syntax", and "encodes hostile footnote scopes and keeps independent documents separate"; `markdownWebLinks.test.js` escaped addresses/titles |
| 6 | A reference that resolves to nothing is never a link; it is the typed text, escaped, with an escaped reason in `title` | 10/10 | `markdownLinks.test.js` "a reference that does not resolve"; `markdownInline.test.js` "protects entire generated unresolved-reference spans from emphasis" |
| 7 | Code and keyboard contents remain literal; references, links, footnotes and emphasis are not expanded there. Highlighted fences may add only constrained Prism token spans inside known code classes; malformed/unknown labels do not create attributes | 10/10 | `markdownSafety.test.js` fenced-language adversarial matrix, including nested markdown fences, plus protected contents test; `markdownInline.test.js` exact backtick runs and keyboard grammar; `markdownRichBlocks.test.js` highlighted and unknown fence labels |
| 8 | Quotes, lists and details share an eight-level bound. Long mixed documents, blank lines, link punctuation, unmatched rich delimiters and large fenced source finish within fixed regression budgets | 10/10 | `markdownSafety.test.js` "nesting that tries to exhaust the reader": 40,000 blank lines, 100,000-character link punctuation, 200 nested details, 10,000 unmatched details openers, malformed keyboard/footnote/emphasis/image delimiters, and a large known-language fence. #238 and #256 earlier regressions remain covered |
| 9 | Plain mode answers text with no markup, and its callers escape it | 10/10 | `markdownSafety.test.js` "answers plain text with no markup in plain mode"; `agentOverview.js` writes the snippet through `esc` |
| 10 | Repository scanners are clean on the change | 10/10 | #695 integrated scans on 2026-10-20: `nice -n 10 semgrep --config auto --error -j 2` over all 25 changed JS/MJS/CSS files, 200 rules, zero findings, exit 0; `nice -n 10 gitleaks git --no-banner --redact --log-opts=main..HEAD .`, zero leaks, exit 0 |

**Total: 100/100.**
