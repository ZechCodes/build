# Markdown rendering security checklist

**Status:** 100/100
**Verified:** 2026-10-20 (#695 rich markdown rendering; #701 boundary revisions and approved external-image consent policy); earlier desktop link, credential, persistence and API authentication evidence remains recorded below (#262, #615, #616).

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

Images require an HTTP(S) URL with a parsed host, no username/password and no
credential delimiter in the authority. Data, blob, mailto, relative,
protocol-relative and credential-bearing sources are refused. Existing web-link
credential behavior is unchanged.

The approved external-image policy (#701) renders an inert native button:
`type="button" class="md-image-load"` with escaped `data-md-image-src`,
`data-md-image-alt` and optional `data-md-image-title`. Its label identifies the
host and optional literal alt text. External sources produce no `img`, `src` or
`srcset` before activation. The delegated controller validates the stored source
again before replacing an activated button with an image. Same-origin markdown
images may load directly. Loaded images have only escaped `src`, `alt`, optional
`title`, fixed `loading="lazy"` and `referrerpolicy="no-referrer"`. Existing Build
attachments keep their own path. Image placeholders, images, anchors, keyboard
and code output are protected from later emphasis, Build-reference and footnote
expansion. `markdownImages.test.js` checks hostile latent sources, literal alt/title,
per-instance activation, cached repaint reset, cleanup and existing attachments.

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
Sources above 4,096 characters take the escaped path to bound grammar work.
Tables retain only fixed alignment styles; accessible scroll wrappers use the
fixed `.mdtable`, `role="region"`, `aria-label="Table"`; measured overflow adds
`tabindex="0"` only while the wrapper actually scrolls horizontally. Headers use
`scope="col"`; short numeric/measurement cells and right-aligned columns may use
`md-cell-short`, while short prose can wrap.

Since #256 the renderer writes web links as well as Build routes (`spa/src/core/markdownWebLinks.js`): `[text](url)`, `[text](url "title")` and bare `http(s)://` URLs in prose. That widened the rule this checklist used to record, "every href is a `#/` route"; control 4 is the rule now. Internal forms (`[text](#42)`, `[text](#/…)`) and relative paths are refused, not routed: a Build reference has its own syntax (`#42`, `@workspace:…`, `[[ws:path]]`), and a relative path has no page to be relative to. A refused link renders as the words typed. In the desktop app, only `https://getbuild.ing/app/desktop` (including query strings and hash routes) stays in the app window. `installNavigationPolicy` in `desktop/src/security-policy.mjs`, wired by `desktop/src/main.mjs`, sends other http, https and mailto links to the system browser with just their URL and denies new windows; the same rule covers navigation and redirects. Landing pages, docs, sibling paths and path-prefix lookalikes are external. The server-side API boundary is pinned by `test_spa_api_guards.py` across every `/api/*` call in SPA source; `test_rtc_controller.py` proves desktop bearer and browser access to ICE credential minting require active alpha membership (#616). Request authentication is separate: only the main-frame desktop document (the server guards `GET /app/desktop`) and same-origin `/api/` fetch/XHR requests receive Bearer headers. The desktop document is the one navigation that carries the Bearer; iframes and subresources aimed at that path, API document navigation, and public pages receive none. `installAuthorizationHeader` registers `onBeforeSendHeaders` with `<all_urls>`: supplied or inherited Authorization headers are stripped case-insensitively on every host before applying that allowlist, including after cross-origin redirects (#262). Since #615, refresh credentials are encrypted in main-process storage, restored only through a token refresh, and cleared by Account → Sign Out; access tokens remain in memory. The scoped [Desktop Security Checklist](Desktop%20Security%20Checklist.md) records the persistence evidence.

| # | Control | Score | Verification |
|---|---|---:|---|
| 1 | One entry point: no module under `spa/src/` renders markdown, parses references or builds a resolver except through `markdownHtml`; the guard names no exceptions | 10/10 | `spa/test/markdownEntry.test.js` |
| 2 | All source text is escaped before insertion into generated markup in every block and mode, including alert bodies, details summaries/bodies, image labels/titles, footnote definitions and highlighted source. Only the exact keyboard/details grammar is recognized as structure | 10/10 | `markdownSafety.test.js` hostile payload/context matrix and "keeps arbitrary HTML inert beside the exact keyboard and details productions"; positive control "accepts the exact generated rich vocabulary" |
| 3 | Only renderer-owned tags, attributes and class values are emitted. No event handlers or arbitrary styles; table alignment is fixed, list starts have at most nine digits, code languages and Prism token names are constrained, and new image/details/footnote attributes use their precise grammar | 10/10 | `markdownSafety.test.js` browser-parsed `violations()` checks every payload/context; "rejects extra rich tags, classes, language names and attributes in the oracle" and checklist/image negative controls prove the oracle rejects forged output |
| 4 | Every href is a Build route on `a.md-ref`, an external `a.md-link` with HTTP(S) host syntax or mailto plus `target="_blank" rel="noopener noreferrer nofollow"`, or an exact scoped footnote fragment pointing at one matching generated target. Unsafe schemes, protocol-relative and relative links remain text. Image sources are separately narrowed to parsed credential-free HTTP(S) URLs | 10/10 | `markdownSafety.test.js` unsafe links, opener checks, image policy, and forged/duplicated footnote fragments; `markdownWebLinks.test.js` unsafe schemes/whitespace/control probes and positive controls; `markdownNavigation.test.js` local footnote handling. Desktop navigation and credential evidence: `desktop/test/security-policy.test.mjs`, `desktop/test/main-auth.test.mjs`, `desktop/test/token-store.test.mjs` (#262, #615) Approved image policy: external sources remain an inert button until activation, same-origin images may load directly, activation revalidates the source and sets no-referrer before src. `markdownSafety.test.js` consent case failed the old emitter (exit 1), then safety/WebLinks/Images passed 68/68 (exit 0); raw `/tmp/build-markdown-695-review/security-image-policy-{red,green}.{log,exit}`. |
| 5 | Reference labels/titles, URLs, image alt/title, fence labels and document scope values cannot leave their text or attribute context. Generated element contents and attributes are protected from later readers | 10/10 | `markdownSafety.test.js` hostile names/references, "escapes image text and protects literal and attribute contents from later syntax", and "encodes hostile footnote scopes and keeps independent documents separate"; `markdownWebLinks.test.js` escaped addresses/titles |
| 6 | A reference that resolves to nothing is never a link; it is the typed text, escaped, with an escaped reason in `title` | 10/10 | `markdownLinks.test.js` "a reference that does not resolve"; `markdownInline.test.js` "protects entire generated unresolved-reference spans from emphasis" |
| 7 | Code and keyboard contents remain literal; references, links, footnotes and emphasis are not expanded there. Highlighted fences may add only constrained Prism token spans inside known code classes; malformed/unknown labels do not create attributes | 10/10 | `markdownSafety.test.js` fenced-language adversarial matrix, including nested markdown fences, plus protected contents test; `markdownInline.test.js` exact backtick runs and keyboard grammar; `markdownRichBlocks.test.js` highlighted and unknown fence labels |
| 8 | Quotes, lists and details share an eight-level bound. Long mixed documents, blank lines, link punctuation, unmatched rich delimiters and large fenced source finish within fixed regression budgets | 10/10 | `markdownSafety.test.js` "nesting that tries to exhaust the reader": 40,000 blank lines, 100,000-character link punctuation, 200 nested details, 10,000 unmatched details openers, malformed keyboard/footnote/emphasis/image delimiters, and a large known-language fence. #238 and #256 earlier regressions remain covered #700: `markdownInline.test.js` has 15 closer-before-unmatched-opener cases, each in a Node 22 child bounded to 2 seconds with SIGKILL. The original two keyboard cases timed out (red suite exit 1, 2 failed/22 passed); after validating closer position and enforcing forward progress, the five-suite focused check passed 108/108, exit 0. Raw logs/exits: `/tmp/build-markdown-695-review/inline-700-{red,green}.{log,exit}`. |
| 9 | Plain mode answers text with no markup, and its callers escape it | 10/10 | `markdownSafety.test.js` "answers plain text with no markup in plain mode"; `agentOverview.js` writes the snippet through `esc` |
| 10 | Repository scanners are clean on the change | 10/10 | #701 integration scan at code head `8bb0eff859d3760129a9af2a9b0968957778f80e`: Semgrep checked all 35 changed JS/MJS/CSS files with 200 rules, zero findings, exit 0; Gitleaks scanned `main..HEAD`, zero leaks, exit 0. Raw output and exit footers are attached to the task handoff (`semgrep.log`, `gitleaks.log`); Gitleaks runs again after this evidence-only update and before push. |

Actual mutation controls (#701) run against a temporary copy of the production
source, with each mutation applied separately and restored before the next run.
Raw Vitest output, the exact source patch, the actual process exit and the
unmutated baselines are retained in
`/tmp/build-markdown-695-review/security-mutations/`. These controls must fail
through assertions (exit 1); a timeout, transform failure or import failure is
not accepted as evidence. All 17 independent mutations failed through assertions
with actual exit 1. Both unmutated baselines passed 43/43 tests with actual
exit 0. `results.json` records each full command and exit; each named `.log`
contains raw test output and `ACTUAL_EXIT`, and each `.patch` records the exact
mutation. No production files were mutated in the shared checkout.

| Mechanism | Deliberate mutation | Fresh result |
| --- | --- | --- |
| Prose escaping | Remove `esc` from prose output | Assertion failure, exit 1 |
| Scheme allowlist | Return every address as allowed | Assertion failure, exit 1 |
| Address normalization | Keep C0/whitespace characters in accepted hrefs | Assertion failure, exit 1 |
| Host requirement | Permit HTTP(S) without `//` and a host | Assertion failure, exit 1 |
| Refused relative addresses | Permit an address with no scheme | Assertion failure, exit 1 |
| External link opener protection | Remove `rel`; independently remove `target` | Assertion failure, exit 1 |
| Attribute escaping | Remove href escaping; independently remove title escaping | Assertion failure, exit 1 |
| Keyboard grammar | Accept attributes on the `<kbd>` opener | Assertion failure, exit 1 |
| Details grammar | Accept attributes on the `<details>` opener | Assertion failure, exit 1 |
| Footnote scope | Replace every provided scope with one fixed value | Assertion failure, exit 1 |
| Fence language allowlist | Return unknown labels as language classes | Assertion failure, exit 1 |
| Image policy | Remove credential/authority validation from a parsed source | Assertion failure, exit 1 (final click-to-load implementation) |
| External image consent | Treat an external origin as same-origin and emit an image before activation | Assertion failure, exit 1 |
| Activation URL validation | Bypass source validation of a modified placeholder dataset | Eight hostile-source assertion failures, exit 1 |
| Activated-image privacy | Omit `referrerpolicy="no-referrer"` from the activated image | Assertion failure, exit 1 |

**Total: 100/100.**
