# Navigation storyboard — #523

Revised design illustrations following Zech's 2026-10-12 feedback, not screenshots
of implemented product behavior. These supersede the original A/B toolbar images.
The source is [Navigation Storyboard.html](../Navigation%20Storyboard.html), a
self-contained document with no external assets or service calls. Open it in a
browser and use its four scene tabs.

- `01-up-to-project.png`: start, project link, project workspace.
- `02-sibling-workspace.png`: workspace selector, sibling list, new workspace.
- `03-across-projects.png`: up-arrow, focused global search, another project's workspace.
- `04-revised-bar-detail.png`: text-width project link, long names, and the 44px bar above chat.

Each flow has three 390 × 844 phone frames. The ordinary toolbar is one 44px
row: up-arrow project switcher, project link sized to its text, workspace
selector, lock. Either switcher focuses search immediately, including touch.
Search frames illustrate typing and the software keyboard; two activations
plus typing reach a sibling or another project's workspace. Project return
remains one activation. A keyboard drawing is illustrative, not a browser test.

Rendered with an isolated headless Chromium process against the local HTML:
1320 × 1240 for `#up`, `#siblings`, `#across`, and `#options`.
No bridge, paired client, or user browser was used. All four images were visually
inspected. The single-row direction is settled; this storyboard starts no
implementation.
