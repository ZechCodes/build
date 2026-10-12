# Navigation storyboard — #523

Proposed design illustrations, not screenshots of implemented product behavior.
The source is [Navigation Storyboard.html](../Navigation%20Storyboard.html), a
self-contained document with no external assets or service calls. Open it in a
browser and use its four scene tabs.

- `01-up-to-project.png`: start, project link, project workspace.
- `02-sibling-workspace.png`: workspace selector, sibling list, new workspace.
- `03-across-projects.png`: Switch, global destinations, another project's workspace.
- `04-phone-toolbar-options.png`: the same chat with 88px and 44px bars and long names.

Each flow shows three 390 × 844 phone frames in each of two rows: recommended
88px toolbar above, alternative 44px toolbar below. Cross-project switching
takes two activations in the first option and three in the second. The intermediate
workspace-menu step in option B is named in the middle-frame caption. Both
options preserve the real project link and lock.

Rendered with an isolated headless Chromium process against the local HTML:
1320 × 2160 for `#up`, `#siblings`, and `#across`; 1320 × 1240 for `#options`.
No bridge, paired client, or user browser was used. All four images were visually
inspected. Zech's toolbar choice remains open; this storyboard changes no spec
decision and starts no implementation.
