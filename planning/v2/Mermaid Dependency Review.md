# Mermaid dependency review — #889

Reviewed 2026-10-26 against base `c6b1ae4f973dad19d70d6a58c39513e983878ac8`.

- Add exact production dependency **mermaid 12.2.0**, published
  **2026-10-13T08:42:16.513Z**, over thirteen days before installation.
  Registry release metadata: <https://registry.npmjs.org/mermaid>.
  Reason: render fenced Mermaid diagrams locally using its maintained parser,
  layouts and strict text-label renderer. No CDN, service or bridge dependency.
- Node 22.23.4 / npm 12.0.2. Installed from `spa/` using
  `nice -n 10 npm install mermaid@12.2.0 --save-exact --legacy-peer-deps`.
  Committed `.npmrc` keeps `min-release-age=7`, `ignore-scripts=true` and
  `legacy-peer-deps=true`. **No age exclusions or script exceptions.** npm's
  resolver enforced the age rule on new transitive resolutions as well.
- Lock delta: 116 added package resolutions, **zero existing resolutions changed**.
  Both manifest and portable lock are committed. Mermaid's MIT license permits
  use with this AGPL application; no extra fonts, assets or network services load.
- `npm audit --json` reports the **same three advisory packages as the base**:
  `@vitest/mocker` and `vitest` (moderate), `undici` (high), all existing test
  dependencies. No new advisory packages from this change. No unrelated audit
  fix or lockfile refresh was applied. Audit exit 1 on both base and change;
  raw reports are in `/tmp/build889-gates/audit-{baseline,current}.json`.

Mermaid's own DOMPurify use is defense in depth, **not our SVG boundary**.
Inspection found no existing SVG/DOM sanitizer in the SPA: the shared markdown
renderer escapes source and emits a fixed vocabulary. `mermaidSvg.js` adds a
narrow independent rebuild of SVG geometry/text after Mermaid returns. It
never admits HTML, external resources, links, events, authored classes or CSS.
Selected numeric/enum presentation values become individually validated SVG
attributes. A fixed set of renderer-owned semantic classes preserves filled
shapes, solid/dashed sequence messages, and pie slice/legend distinctions using
application theme tokens. IDs and local marker/clip references are rewritten
per insertion, including copies from the 32-entry source/theme cache.

Because Mermaid uses live measurement nodes before returning output, **source
validation happens before importing/invoking Mermaid**. This release deliberately
rejects config/frontmatter and `%%` directives/comments; HTML, entities, escapes
and markdown/math labels (including Mermaid’s `#word;` entities); authored CSS commands, resource URLs and URL functions;
image/icon `@{…}` metadata; and sequence `properties`/`details`/`links`/`link`
commands (including legacy actor icon forms and relative/protocol-relative
resources). Plain structural arrows such as `<|--` remain supported. Sources
over 16,384 UTF-16 units are refused; Mermaid’s graph edge limit is set to 200.
Limit violations fall back. These
restrictions produce the original escaped source with a small fixed error;
there is no image-consent override or automatic resource loading for diagrams.

The library loads only after a complete Mermaid fence reaches the DOM. Global
initialize/render calls are serialized across themes and controllers; owned
measurement nodes are removed on success and failure. The controller fences
stale source/theme results and detached/disposed mounts. It imports no transport
or connection state and enhances the same cached markdown used on every surface.

Regression evidence includes real pinned-library Chromium rendering of
flowchart, sequence, class inheritance, state and pie diagrams in both themes;
fixed error/source retention; zero hostile-resource requests; and synchronous
image href/src instrumentation through the transient measurement phase.
The actor metadata regression first failed with **two connected image href
assignments to `//evil.test/pixel`** (exit 1), then passed after pre-render rejection.
Logs and exact gate exits are attached to the implementation handoff.

Final verification at implementation `f73d357279029d335658b23b68ebb6c9df267154`:
fresh npm12 locked installs, lint, 372 targeted unit tests, 49 Chromium tests,
build, Semgrep (200 rules/13 targets/no findings), Gitleaks and diff check all
exit 0. Production chunk metadata proves all 68 Mermaid-containing chunks stay
outside the initial static graph. Offline Chromium boots the built app and
renders a diagram through the built lazy chunk with zero page errors. Entry
size is 884,840 bytes versus baseline 883,249 (+1,591 raw; +503 gzip). The
change adds a small controller to initial JS; the library itself remains lazy.
Exact commands and raw logs accompany the handoff and the
[security checklist](Markdown%20Rendering%20Security%20Checklist.md).
