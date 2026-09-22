# Landing screen captures

This isolated capture fixture uses the current SPA visual tokens, bundled Inter files, and approved harness artwork without changing the product client. Its data is an explicit demonstration fixture for the landing narrative; controls do not call a repository or merge changes.

The capture header uses the canonical mint Build mark from the official brand package and the word `build` without the retired underscore treatment.

Run Vite from the repository root with `spa/node_modules/.bin/vite . --port 4178`, then run `node design/landing-captures/capture.mjs`. Set `CAPTURE_ORIGIN` when Vite uses another origin and `CHROMIUM_PATH` when Playwright's managed browser is not installed. The script resolves Playwright from `web/package.json`, writes native-resolution lossless PNG masters to `design/landing-captures/masters/`, asserts the app host stays outside every system exclusion, emits runtime WebP derivatives to `skriftapp/buildapp/landing/assets/screens/`, and records every checked rectangle in `capture-results.json`.

By default the tool captures every state in three exact device profiles. `macbook` captures a 1512×982 logical viewport at 2× for the MacBook Pro 14-inch 3024×1964 panel. `ipad` captures 1210×834 at 2× for the iPad Pro 11-inch 2420×1668 panel. `iphone` captures 440×956 at 3× for the iPhone 17 Pro Max 1320×2868 panel. Comma-separated filters keep targeted refreshes from regenerating unrelated assets. For example, the activity and review iPad maps are captured with:

```sh
CAPTURE_SCENES=ui04,ui05 CAPTURE_PROFILES=ipad \
  CHROMIUM_PATH=/usr/bin/chromium node design/landing-captures/capture.mjs
```

`CAPTURE_STATES` can narrow a multi-state scene such as `ui05` to `approval` or `merged`.
Targeted runs update those entries in `capture-results.json` while retaining validation
records for assets that were not regenerated.

System-composited files keep the existing state stems and use platform suffixes:

- `ui01-macbook.webp`, `ui01-ipad.webp`, and `ui01-iphone.webp`
- `ui03-question-*`, `ui03-answer-*`, and `ui03-resumed-*` for the decision sequence
- `ui05-approval-*` and `ui05-merged-*` for the review sequence

The older `-desktop`, `-tablet`, and `-mobile` assets remain app-only fixture archives. The capture command does not overwrite them.

The fixture imports the current `inboxRowHtml`, `threadHtml`, `agentSurfacesRender`, `gitToolbarHtml`, `diffRowsHtml`, and directory-rail renderers. Its surrounding frame is a deterministic reconstruction for marketing capture, not a claim that the demonstration data came from a live host.

The exact workspace, state order, crops, timers, app bounds, native pixel sizes, and system exclusions are recorded in `screen-manifest.json`. The platform treatment follows Apple's current [layout and safe-area guidance](https://developer.apple.com/design/human-interface-guidelines/layout), [status-bar guidance](https://developer.apple.com/design/human-interface-guidelines/status-bars), and [iPadOS 26 window-controls guidance](https://developer.apple.com/videos/play/wwdc2025/208/). Native panel sizes come from Apple's [MacBook Pro](https://www.apple.com/macbook-pro/specs/), [iPad Pro](https://www.apple.com/ipad-pro/specs/), and [iPhone 17 Pro Max](https://www.apple.com/iphone-17-pro/specs/) specifications.
