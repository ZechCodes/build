# Landing screen captures

This isolated capture fixture draws each landing screen in the app's own frame without changing the product client. `app-shell.js` writes the SPA's markup — the inbox rail, the view column with its toolbar, directory rail and console bar, and the agent rail with its bubble strip — and the SPA's shipped stylesheets lay it out, loaded in the order the app loads them, so regions, proportions, row anatomy and type scale are the app's. `workspace-scenes.js` and `project-scenes.js` fill the regions using the SPA's own renderers where it has them (inbox rows, toolbar, directory rail, thread, diff rows, commit rows, highlighting, bubble patterns). The two features the app does not have yet, the workflow builder and the review triage, are drawn as a project page and a workspace tab in that frame, styled in `capture.css` with the app's tokens. Its data (`story.js`) is an explicit demonstration fixture for the landing narrative; controls do not call a repository or merge changes.

To compare a scene with the app, bring up the compose stack (`deploy/README.md`), seed it, and screenshot the matching view at the scene's app bounds (1416×804 on the MacBook, 1138×672 on the iPad, 440×863 on the iPhone; see `screen-manifest.json`).

Run Vite from the repository root with `spa/node_modules/.bin/vite . --port 4178`, then run `node design/landing-captures/capture.mjs`. Set `CAPTURE_ORIGIN` when Vite uses another origin and `CHROMIUM_PATH` when Playwright's managed browser is not installed. The script resolves Playwright from `web/package.json`, writes native-resolution lossless PNG masters to `design/landing-captures/masters/`, asserts the app host stays outside every system exclusion, emits runtime WebP derivatives to `skriftapp/buildapp/landing/assets/screens/`, and records every checked rectangle in `capture-results.json`.

By default the tool captures every state in three exact device profiles. `macbook` captures a 1512×982 logical viewport at 2× for the MacBook Pro 14-inch 3024×1964 panel. `ipad` captures 1210×834 at 2× for the iPad Pro 11-inch 2420×1668 panel. `iphone` captures 440×956 at 3× for the iPhone 17 Pro Max 1320×2868 panel. Comma-separated filters keep targeted refreshes from regenerating unrelated assets. For example, the activity and review iPad maps are captured with:

```sh
CAPTURE_SCENES=ui04,ui05 CAPTURE_PROFILES=ipad \
  CHROMIUM_PATH=/usr/bin/chromium node design/landing-captures/capture.mjs
```

`CAPTURE_STATES` can narrow a multi-state scene such as `ui05` to `approval` or `merged`.
Targeted runs update those entries in `capture-results.json` while retaining validation
records for assets that were not regenerated.

The landing story textures are refreshed without touching reused device states with:

```sh
CAPTURE_SCENES=ui10-editor,ui12-issues,ui13-team,ui14-git,ui16-builder \
  CAPTURE_PROFILES=macbook CHROMIUM_PATH=/usr/bin/chromium \
  node design/landing-captures/capture.mjs
CAPTURE_SCENES=ui15-triage CAPTURE_PROFILES=ipad \
  CHROMIUM_PATH=/usr/bin/chromium node design/landing-captures/capture.mjs
CAPTURE_SCENES=ui05 CAPTURE_STATES=approval CAPTURE_PROFILES=ipad \
  CHROMIUM_PATH=/usr/bin/chromium node design/landing-captures/capture.mjs
CAPTURE_SCENES=ui05 CAPTURE_STATES=merged CAPTURE_PROFILES=macbook \
  CHROMIUM_PATH=/usr/bin/chromium node design/landing-captures/capture.mjs
```

System-composited files keep the existing state stems and use platform suffixes:

- `ui03-question-*`, `ui03-answer-*`, and `ui03-resumed-*` for the decision sequence
- `ui05-approval-*` and `ui05-merged-*` for the review sequence

The `ui01`, `ui02` and `ui04` files and the older `-desktop`, `-tablet`, and `-mobile` assets are archives of the retired storyboard; the landing page draws none of them and the capture command no longer writes them.

The frame is a deterministic reconstruction for marketing capture, not a claim that the demonstration data came from a live host.

The exact workspace, state order, crops, timers, app bounds, native pixel sizes, and system exclusions are recorded in `screen-manifest.json`. The platform treatment follows Apple's current [layout and safe-area guidance](https://developer.apple.com/design/human-interface-guidelines/layout), [status-bar guidance](https://developer.apple.com/design/human-interface-guidelines/status-bars), and [iPadOS 26 window-controls guidance](https://developer.apple.com/videos/play/wwdc2025/208/). Native panel sizes come from Apple's [MacBook Pro](https://www.apple.com/macbook-pro/specs/), [iPad Pro](https://www.apple.com/ipad-pro/specs/), and [iPhone 17 Pro Max](https://www.apple.com/iphone-17-pro/specs/) specifications.
