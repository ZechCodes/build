# Client language decision

**Recommendation: incrementally adopt TypeScript, keeping the existing vanilla DOM/ES-module application.** Do not rewrite the UI in Rust or Go WASM. TypeScript and a UI framework are separate decisions; this plan proposes no framework. Existing JavaScript keeps running during migration, and architecture improvements are not gated on a whole-SPA conversion.

## Evidence from this codebase

At baseline `6a219d98`, `spa/src` contains **300 JavaScript files / 63,989 physical lines**; `core` accounts for 253 files / 53,391 lines. There are no TS/TSX/JSX source files or TypeScript configuration. `spa/test` contains **374 `*.test.js` files / 95,410 physical lines**; 204 test files request jsdom. Counts include blank lines/comments and are repository measurements, not a complexity or benchmark score.

[package.json](../../spa/package.json) already uses ESM, Vite 8, Vitest 4 and ESLint 9. [vite.config.js](../../spa/vite.config.js) targets ES2022 and writes into `skriftapp/buildapp/static`. The [shared test setup](../../spa/test/setup/localStorage.js) installs fake IndexedDB/localStorage. [CI](../../.github/workflows/ci.yml) runs lint and Vitest, but has no SPA type-check gate. [ESLint](../../spa/eslint.config.js) currently applies complexity 10 to `src/**/*.js` only.

The work is largely DOM, asynchronous records, identities, event variants and cache reconciliation. Those boundaries benefit from explicit types as more product logic moves client-side. Tests remain essential: a type cannot prove that an event was committed before repaint or that a partial page represents the whole tracker.

## Alternatives

| Choice | Fit and tradeoff | Decision |
| --- | --- | --- |
| Keep vanilla JavaScript | Lowest immediate cost, same debugging/tests/deploy, no syntax migration. JSDoc plus opt-in checking can strengthen individual boundaries. At this scale, record variants and callback contracts remain easier to change inconsistently without a required type gate. | Valid short-term state; not the preferred long-term language for the growing client. |
| TypeScript with existing DOM modules | Preserves runtime architecture and JS ecosystem; typed IDs, record unions and command/response contracts improve refactoring feedback. Adds compiler/parser configuration, declarations for dependencies and some migration friction. | **Choose**, beginning at protocol/cache/selector boundaries. |
| Rust compiled to WASM for the whole client | Can share an extracted pure Rust kernel, but cannot reuse device git/process/SQLite services as browser functionality. DOM, storage and JS-library interop still need bindings. Existing JS tests would need wrappers plus Rust/browser-specific coverage; creates another build/debug boundary. | Reject for the application shell and product logic. Revisit an isolated measured CPU bottleneck only. |
| Go compiled to WASM for the whole client | Adds a language absent from the relevant app/bridge implementation, a Go runtime/support artifact and browser bindings, with no direct Rust source reuse. TinyGo is another toolchain choice requiring its own compatibility evaluation. | Reject here; no identified benefit pays for a UI port. |

TypeScript supports [mixed JS/TS projects through `allowJs`](https://www.typescriptlang.org/tsconfig/allowJs.html). [Vite transpiles TypeScript but does not type-check it](https://vite.dev/guide/features#typescript), so add a separate compiler gate. [Vitest supports type assertions/type checking](https://vitest.dev/guide/testing-types), but runtime tests still need to run. These are compatible additions to the current tooling, not reasons to replace the test suite.

Rust can access the DOM through [wasm-bindgen/web-sys bindings](https://wasm-bindgen.github.io/wasm-bindgen/examples/dom.html). Go's browser target uses a [matching `wasm_exec.js` support file](https://go.dev/wiki/WebAssembly). These options are technically viable; the recommendation against a full port is an engineering judgment about this repository's I/O-heavy UI and existing investment, not a claim that WASM cannot build interfaces or must always be slower.

## Sharing with the Rust bridge

Share **wire contracts and fixtures first**, rather than making client and bridge execute identical workflows. Rust's `api/v1` types own today's serialized boundary; TS should describe those wire shapes, not every internal Rust struct. Start with a narrow typed adapter verified against existing `fixtures/api/v1` examples. If generation is introduced, export a schema from the declared wire types and generate/check TS from it, accounting for Serde tagging, optional/null fields, IDs, limits and unknown variants. Do not hand-maintain two competing schemas. Generated types still require runtime decoding and compatibility tests.

The [bridge crate](../../bridge/Cargo.toml) uses native git2, Tokio and SQLite. Extracting a pure algorithm crate for WASM would be separate work; sharing the daemon wholesale is not the migration path. The client already uses a JS [secure-transport binding](../../spa/package.json); keep its reviewed protocol boundary. A Rust implementation on both ends does not remove version skew or make runtime validation unnecessary.

## Build, bundle and deployment

Keep Vite's hashed static assets, build-version stamp, same-origin serving and self-hosted dependencies. The [app Containerfile](../../skriftapp/Containerfile) builds the SPA in a Node stage and copies it into the Python image; TS adds build-time tooling, not another running service. Make type-check a required pre-build/CI step so a transpiling-only build cannot publish unchecked changes. Keep the existing JS `.mjs` desktop wrapper and Node test harness unless a later task justifies changing them.

Use type-only imports and erasable types; avoid introducing runtime enums/decorators merely for typing. Type annotations are erased, so the migration should not require a new browser runtime payload. Runtime validators do add code: keep them focused on boundaries. No candidate bundle-size or speed claim has been measured here. Record compressed transfer size, initial parsing/startup and mobile responsiveness before/after the first slice. WASM would add binaries, loading/glue and boundary conversions; weigh those against measured CPU savings before considering a small optional module. Existing terminal WASM is not a reason to port unrelated UI logic.

## Migration and cost

| Slice | Concrete work | Rough engineering effort |
| --- | --- | --- |
| Tooling pilot (P0) | Add pinned TS and compatible ESLint parser/config; `noEmit`, strict checking for new TS, bundler resolution and ES2022/DOM libraries. Allow existing JS, leave blanket `checkJs` off initially, retain complexity 10 for both extensions. Add compiler gate, one adapter and real test/build integration. | Small/medium: about 2–4 engineer-days. |
| Critical boundary slice (P0–P2) | Type protocol/cache addresses, decoded entity/event unions, missing/partial states and one tracker selector flow. Supply narrow declarations or checked wrappers for untyped JS dependencies; treat incoming data as `unknown`. | Medium: about 1–2 engineer-weeks, including test/mock adjustments and review. |
| Incremental expansion | Migrate conversation/workspace records and touched feature modules/tests. Update imports and source-reading test tools as files change extension; preserve runtime fixtures and assertions. Avoid unrelated layout/behavior changes in conversion patches. | Medium per area; several weeks of distributed work. |
| Whole-client completion, if desired | Convert remaining 300-module surface and affected test helpers; remove temporary shims, enable broader JS checks where still useful. | Large: roughly 4–8 engineer-weeks total, high uncertainty. Re-estimate after the pilot; not a deadline or prerequisite for #86 architecture gains. |

Estimates are planning judgments for one experienced contributor including review, not measured agent throughput; the slices overlap and should not be summed mechanically. Agents gain compiler feedback on shapes, exhaustive variants and broken call sites, but conversions can also waste time on assertions or type gymnastics. Keep readable object types, small discriminated unions and concrete interfaces. Do not use blanket `any`, `@ts-ignore` or weakened complexity gates to claim completion. Test the mixed-JS/TS import resolution under both Vite and Vitest in the pilot.

A retained-JS pilot using checked JSDoc is a lower-cost fallback if the TS pilot reveals disproportionate friction. It does not change the architectural ownership rules. Rust WASM may later be appropriate for a separately profiled pure parser/computation in a worker with a coarse input/output boundary; Go WASM has no current reuse advantage here. Neither choice makes a browser persistent, so always-on services remain bridge Rust under every client-language option.

External documentation checked 2026-09-23; actual dependency pins must be selected and tested when implementing the pilot. No language migration, dependency installation or benchmark is performed by this docs branch.
