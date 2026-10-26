# Markdown rendering security checklist

**Status:** 100/100 — #889 targeted gates and changed-file scanners passed on 2026-10-26.
**Verified:** 2026-10-26 (#889; evidence below); 2026-10-22 (post-merge scanner refresh); 2026-10-20 (#695 rich markdown rendering; #701 boundary revisions; #700 production-CSP redirect consent policy); earlier desktop link, credential, persistence and API authentication evidence remains recorded below (#262, #615, #616).

## Mermaid boundary amendment — #889 (2026-10-26)

The earlier renderer has **no existing DOM/SVG sanitizer**: its boundary is
escape-first fixed markup. Mermaid fences retain that escaped source and add
only a fixed `md-code-block md-mermaid` wrapper synchronously. A document-level
controller lazily enhances complete fences on every cached surface. The
original code remains available, with a fixed small error on invalid/refused
source. Ordinary/unfinished fences and inline/plain modes keep their old behavior.

`mermaidRenderer.js` pins `securityLevel: "strict"`, global/flowchart
`htmlLabels: false`, no auto-start, suppressed error diagrams, 16,384 source
units and 200 edges. All library initialize/render operations share one serial
queue. The 32-entry source/theme cache holds only independently sanitized SVG;
every mount gets fresh local IDs. Mounts guard source/theme changes, detachment
and disposal; measurements live in an owned host removed in `finally`.

Strict output settings alone cannot protect Mermaid's **transient measurement
DOM**. Before invocation, reject configuration/frontmatter, `%%` directives and
comments, HTML/entities/escapes/markdown and math labels (including Mermaid `#word;`
entities and internal entity placeholders), CSS commands, URL functions and
resource schemes, `@{…}` image/icon metadata, and legacy sequence
properties/details/links commands including relative/protocol-relative icons.
The actor metadata Chromium regression originally recorded two image href
assignments while connected (red exit 1). It now checks zero href/src assignments
and zero requests, including during parser errors and subsequent valid rendering.

`mermaidSvg.js` parses XML off-DOM and rebuilds only SVG geometry, groups, text,
titles/descriptions, markers and clips. No HTML/foreignObject, scripts, events,
images/use, links, CSS stylesheets, arbitrary classes or external references can
survive. Only validated numeric/enum presentation values become attributes;
fixed semantic classes preserve filled shapes, dashed messages and themed pie
palettes. Unique local marker/clip targets are remapped for every insertion;
malformed, oversized or deeply nested SVG fails closed. Thus controls 2–5 and 7
below now cover two explicit boundaries: escaped source plus an independently
rebuilt SVG vocabulary, rather than arbitrary source markup.

Evidence: `markdownMermaid.test.js`, `markdownMermaidMount.test.js`,
`mermaidRenderer.test.js`, `mermaidSvg.test.js`, and real pinned-library
`browser/markdownMermaid.test.js`. The Chromium suite checks common diagram
semantics, both themes, lazy library requests, invalid-source retention and zero
hostile resource requests/assignments. See [dependency review](Mermaid%20Dependency%20Review.md)
for exact version, release-age enforcement, existing audit advisories and syntax
limits. #889 gate exits and scanner results are recorded with the final handoff.

### #889 gate evidence

Base `c6b1ae4f973dad19d70d6a58c39513e983878ac8`; frozen implementation
`f73d357279029d335658b23b68ebb6c9df267154`. All executable gates below ran
under `nice -n 10` with Node 22.23.4/npm 12.0.2, judged by actual process exits.
Full command/exit manifest and raw logs are retained in `/tmp/build889-gates/`
and attached to the handoff. Documentation-only evidence commits follow this
source without changing code/tests/dependencies.

| Gate | Actual exit / evidence |
| --- | --- |
| `npm ci --prefix transport/js --ignore-scripts` | 0; `final-install-transport.log` |
| SPA `npm ci --legacy-peer-deps` | 0; `final-install-spa.log` |
| SPA `npm run lint` | 0; `lint.log` |
| SPA `npx vitest run test/markdown*.test.js test/mermaid*.test.js test/theme.test.js test/themeDom.test.js test/complexityRatchet.test.js` | 0; 26 files, 372 tests; `vitest.log` |
| SPA `npx vitest run test/browser/markdown*.test.js` | 0; 9 files, 49 Chromium tests; `browser.log` |
| SPA `npm run build` | 0; `build.log` |
| Built bundle graph + offline production Chromium probe | 0; `production-graph.log` and `verify-production.mjs`; boot and lazy diagram render, zero page errors; all 68 Mermaid-containing chunks excluded from initial static imports |
| `semgrep --config auto --error --jobs 2` over changed tracked JS/CSS files | 0; 200 rules, 13 targets, zero findings; `semgrep.log`, `semgrep-files.txt` |
| `gitleaks git --no-banner --redact --log-opts='<base>..HEAD' .` | 0; zero leaks; `gitleaks.log`; repeated after final documentation commit |
| `git diff --check <base>..HEAD` | 0; `diff-check.log`; repeated after final documentation commit |

TDD evidence: `red-fence.log` fails the missing Mermaid wrapper assertion,
`red-actor-browser.log` fails because two connected image href assignments occur,
and `red-markers.log` fails the missing hollow UML marker assertion; each exits
1. The final tests above pass their corresponding regressions. Early development
startup/timeout failures remain in their separate logs and are not counted as
assertion-based red evidence. No source changed during the final gates.

The production entry changes from 883,249 to 884,840 raw bytes (+1,591),
293,816 to 294,319 gzip bytes (+503). This small controller cost is separate
from the lazy Mermaid runtime. The large-chunk and existing libsodium/browser
externalization warnings remain non-failing; built runtime behavior was checked.
The full unrelated SPA suite and live deployment/pairing were deliberately not
run; scope was the required affected suites and offline production assets.

## Earlier controls and evidence

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
credential behavior is unchanged. Every authored HTTP(S) image URL requires
activation, regardless of its origin or path. A same-origin route may redirect
externally, so neither origin equality nor a route name confers attachment trust.
There is no HTTP attachment-route exemption. The renderer does not upgrade URLs.

The approved image-consent policy (#700/#701) renders an inert native button:
`type="button" class="md-image-load"` with escaped `data-md-image-src`,
`data-md-image-alt` and optional `data-md-image-title`. Its label identifies the
host and optional literal alt text. HTTP(S) sources produce no `img`, `src` or
`srcset` before activation. The delegated controller validates the stored source
again before replacing an activated button with an image. Loaded images have
only escaped `src`, `alt`, optional
`title`, fixed `loading="lazy"` and `referrerpolicy="no-referrer"`. Existing Build
attachments and cached media keep their byte-backed loader paths, which create
their own trusted blob/data sources. Raw markdown cannot enter those paths by
authoring a blob/data URL. Image placeholders, images, anchors, keyboard
and code output are protected from later emphasis, Build-reference and footnote
expansion. `markdownImages.test.js` checks hostile latent sources, literal alt/title,
per-instance activation, cached repaint reset, cleanup and existing attachments.

The production, development and mail CSP policies permit `https:` in `img-src`,
but not general `http:`. The landing preview renders no markdown and retains
its original `img-src 'self' data:` policy. Desktop uses the app page's response
policy. **CSP no
longer blocks external HTTPS images: the renderer's no-`img`-before-click
invariant is the only consent guard.** The 17 mutation controls below cover that
boundary, including credential rejection, activation URL revalidation and
no-referrer privacy. Additional round-four controls cover same-origin and
redirect-route consent bypasses. The controller permits validated HTTP(S) only
after activation; CSP allows same-origin development HTTP but does not add a
general HTTP allowance. The
`markdownImageCspLayout.test.js` Chromium regression serves the exact configured header and
checks zero requests before activation, a decoded image with positive
`naturalWidth` afterward, and no CSP violation. Plain and linked same-origin
download redirects are tested under that exact header: neither the initial
route nor its external redirect target is requested before activation.

Linked images retain a policy-checked external anchor around the image or
placeholder. Activating a placeholder cancels navigation and propagation in
the capture phase before validating its latent source; following the enclosing
link does not activate the loader. `markdownWebLinks.test.js` covers nested
image/link grammar, literal attributes and refused destinations;
`markdownImagesLayout.test.js` verifies mouse, Enter and Space in Chromium
with separate navigation and image-request counts. The permanent activation
test asserts both `defaultPrevented` and absence of a containing-link handler.

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
| 4 | Every href is a Build route on `a.md-ref`, an external `a.md-link` with HTTP(S) host syntax or mailto plus `target="_blank" rel="noopener noreferrer nofollow"`, or an exact scoped footnote fragment pointing at one matching generated target. Unsafe schemes, protocol-relative and relative links remain text. Image sources are separately narrowed to parsed credential-free HTTP(S) URLs | 10/10 | `markdownSafety.test.js` unsafe links, opener checks, image policy, and forged/duplicated footnote fragments; `markdownWebLinks.test.js` unsafe schemes/whitespace/control probes and positive controls; `markdownNavigation.test.js` local footnote handling. Desktop navigation and credential evidence: `desktop/test/security-policy.test.mjs`, `desktop/test/main-auth.test.mjs`, `desktop/test/token-store.test.mjs` (#262, #615) Approved image policy: every authored HTTP(S) source remains an inert button until activation, including same-origin routes that may redirect. Trusted attachment/cache byte loaders remain separate; raw blob/data sources are refused. Activation revalidates the source and sets no-referrer before src. `markdownSafety.test.js` consent case failed the old emitter (exit 1), then safety/WebLinks/Images passed 68/68 (exit 0); raw `/tmp/build-markdown-695-review/security-image-policy-{red,green}.{log,exit}`. |
| 5 | Reference labels/titles, URLs, image alt/title, fence labels and document scope values cannot leave their text or attribute context. Generated element contents and attributes are protected from later readers | 10/10 | `markdownSafety.test.js` hostile names/references, "escapes image text and protects literal and attribute contents from later syntax", and "encodes hostile footnote scopes and keeps independent documents separate"; `markdownWebLinks.test.js` escaped addresses/titles |
| 6 | A reference that resolves to nothing is never a link; it is the typed text, escaped, with an escaped reason in `title` | 10/10 | `markdownLinks.test.js` "a reference that does not resolve"; `markdownInline.test.js` "protects entire generated unresolved-reference spans from emphasis" |
| 7 | Code and keyboard contents remain literal; references, links, footnotes and emphasis are not expanded there. Highlighted fences may add only constrained Prism token spans inside known code classes; malformed/unknown labels do not create attributes | 10/10 | `markdownSafety.test.js` fenced-language adversarial matrix, including nested markdown fences, plus protected contents test; `markdownInline.test.js` exact backtick runs and keyboard grammar; `markdownRichBlocks.test.js` highlighted and unknown fence labels |
| 8 | Quotes, lists and details share an eight-level bound. Long mixed documents, blank lines, link punctuation, unmatched rich delimiters and large fenced source finish within fixed regression budgets | 10/10 | `markdownSafety.test.js` "nesting that tries to exhaust the reader": 40,000 blank lines, 100,000-character link punctuation, 200 nested details, 10,000 unmatched details openers, malformed keyboard/footnote/emphasis/image delimiters, and a large known-language fence. #238 and #256 earlier regressions remain covered #700: `markdownInline.test.js` has 15 closer-before-unmatched-opener cases, each in a Node 22 child bounded to 2 seconds with SIGKILL. The original two keyboard cases timed out (red suite exit 1, 2 failed/22 passed); after validating closer position and enforcing forward progress, the five-suite focused check passed 108/108, exit 0. Raw logs/exits: `/tmp/build-markdown-695-review/inline-700-{red,green}.{log,exit}`. |
| 9 | Plain mode answers text with no markup, and its callers escape it | 10/10 | `markdownSafety.test.js` "answers plain text with no markup in plain mode"; `agentOverview.js` writes the snippet through `esc` |
| 10 | Repository scanners are clean on the change | 10/10 | Post-merge source head `f57b9258eac2f04f5f83a3b42a4b552c267770a4`, base `origin/main` at `5b13c50d284ac29e02117b00af88cad889ec7b42`: `nice -n 10 semgrep --config auto --error` over all 44 changed JS/MJS/CSS/Python files ran 442 rules, zero findings, exit 0; no ENOMEM fallback was needed. `nice -n 10 gitleaks git --no-banner --redact --log-opts='origin/main..HEAD' .` scanned 38 commits with no leaks, exit 0. `git diff --check origin/main..HEAD` exited 0. Raw logs/exits and the exact scan file list: `/tmp/build695-gates/`. |

Round-two actual mutation controls (#701) ran against a temporary copy of the production
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

The round-three delta added two independent linked-image action controls against code
head `5dd749cd5767e4e1ea28344beed145190cbfafb2` and the permanent assertion added in
`86a10c56786dbc6f03317c758b976a7777e5bb04`: remove `preventDefault`, then independently
remove `stopPropagation`. Each produces two assertion failures and exits 1;
clean before/after baselines pass 2/2 and exit 0. The full image-controller suite
passes 22/22, exit 0. Commands, raw logs, patches and hashes are retained under
`/tmp/build-markdown-695-round3/linked-action-mutations/` and attached to the
handoff. The initial `preventDefault` mutation survived because the old test
only checked propagation; that first probe remains labeled `first-probe/`.
Adding the explicit canceled-event assertion closed the coverage gap before
the final full-suite run.

Historical round-three production-CSP evidence at isolated source commit
`c19912315e94104c0f059839113fb7abc8ac6e74` reran all 17 original mutations and
two new controls: removing the controller's HTTPS-only activation check, and
removing the renderer's external-HTTP link fallback. All 19 failed through
assertions with exit 1; clean before/after baselines passed 96/96 with exit 0.
Source, policy and unit-test hashes stayed unchanged during the batch.
Raw commands, logs, exits and patches are retained under
`/tmp/build-markdown-695-round3/csp-mutations/` and attached to the handoff.
The exact-production-CSP browser case failed the old policy because image
decoding was blocked (exit 1), then passed with a decoded image, one request
only after activation, and no CSP violation (final browser recheck exit 0).
That round added `https:` to four headers. Round four restores the landing
preview header, since it renders no markdown. The HTTPS-only controller and
HTTP-link fallback controls above describe the superseded round-three policy;
the current policy requires consent for all authored HTTP(S) URLs, including
same-origin development URLs. The new controls below replace those two cases.

Round-four consent verification at source commit
`ff67429e15f2e3762435ddac0a609043d1d437b1` requires a button for every authored
HTTP(S) source. The exact-production-CSP Chromium tests first reproduced the
reported download redirect: both plain and linked images requested the
same-origin route and its external target before activation (red exit 1).
Both forms now assert zero requests to either hop and no image element before
activation; afterward both hops occur once, the image decodes with positive
`naturalWidth`, and no CSP violation occurs. CDP intercepts every hop locally
and refuses unknown URLs, so this regression never contacts the real hosts.
The focused ten-suite check passed 182/182 with exit 0. Production attachment
cache-to-Blob and composer FileReader-to-Data previews still paint directly;
the cached attachment revisit makes no transport call.

All 21 round-four mutations failed through assertions with actual exit 1:
the retained 17 controls (using unconditional image auto-load as the universal
consent negative control), both linked-action controls, and separate
same-origin and exact-download-route auto-load exemptions. Each of the last
two fails both plain and linked Chromium cases by making one request to each
hop before consent. Removing activation URL validation fails nine hostile
source assertions; removing either link action guard fails two assertions.
Both clean baselines pass 104/104 with actual exit 0. Source, tests, policy and
trusted-loader hashes remained unchanged throughout. Raw commands, logs,
exits, patches and validation are retained in
`/tmp/build-markdown-695-round4/mutations/` and attached to the handoff. An initial
isolated fixture setup failure is retained separately and is not counted as a
negative control; the corrected harness permits read-only access to linked
local dependency assets without changing production code.

**Total: 100/100.**

The coordinator paused #695 after code/test commit `ae14ab0f8` on 2026-10-20.
The consent fix, exact-CSP redirect regressions, attachment coverage, heading
separation and landing-preview policy revert are implemented. Headless native
frame and click failures were isolated by matched probes to inherited
`WAYLAND_DISPLAY`; the harness now removes only that browser-child variable.
The parent environment, timeouts, RAF polling and existing assertions remain
unchanged. The native-frame regression failed before the fix (exit 1), then
57 tests across ten affected suites passed (exit 0); the default system-browser
probe also passed. The 2026-10-20 full `npm test` was stopped at the coordinator’s
request. The coordinator resumed the task on 2026-10-22, explicitly accepting
the `img-src https:` widening. The post-merge scans above supersede the paused
scanner checkpoint.

Post-merge integration verification (2026-10-22), source head
`f57b9258eac2f04f5f83a3b42a4b552c267770a4`, with Node 22.23.4 and npm 12.0.2
(the pins on merged main). All executable gates below ran under `nice -n 10`;
results are actual process exits. Logs, exit files and the exact scanner target
list are retained in `/tmp/build695-gates/` and attached to the task handoff.

| Gate | Exit / evidence |
| --- | --- |
| `npm ci --prefix transport/js --ignore-scripts` | 0 |
| SPA `npm ci --legacy-peer-deps` | 0; install scripts disabled by `.npmrc` |
| SPA `npm run lint` | 0 |
| SPA `npm test` | 0; 742 files and 11,411 tests passed, including exact-CSP redirect consent regressions; 1,610.98 seconds |
| SPA `npm run test:captures` | 0; all three capture scripts exited 0 |
| SPA `npm run build` | 0 |
| skriftapp `uv run --frozen ruff check buildapp` | 0 |
| skriftapp `uv run --frozen pytest buildapp -q` | 0; 959 passed, 1 skipped, including production CSP tests |
| Semgrep / Gitleaks / `git diff --check origin/main..HEAD` | 0 / 0 / 0; scanner details in control 10 |
| ShellCheck | Not applicable: no changed `.sh` files against `origin/main` |
| Fresh markdown captures (`captureMarkdownPolish.mjs`) | 0; desktop 960px, phone 390px and narrow 320px in light/dark, with consent buttons visible |

Main was integrated in merge commit `f57b9258e`. The only textual conflict was
in `thread.js`: outgoing messages retain both scoped markdown document IDs and
main’s attachment rendering. The README’s runtime pin and both rail tests’
painted-state waits were retained. No new implementation changes, dependency
changes or complexity exemptions were needed during integration.
