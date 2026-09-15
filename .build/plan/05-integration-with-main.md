# Integrating main into the multi-device branch

Status: plan (2026-09-14). Branch `build/combined-interface` at `afd34a4d`
(the tip of the multi-device work, `00`–`04` and the three stage docs) takes
`origin/main` at `d76f088c`, 55 commits past the fork point `4a7dc026`.
`git merge-tree --write-tree --messages origin/main HEAD` reports 38 conflicted
files (37 content, one modify/delete). Written after reading both sides of every
one of them, main's new modules and tests, `planning/v2/workspaces.md` and
`planning/v2/Bridge Wire Protocol Spec.md`, and main's HANDOFF/README additions.

## 0. What each side is, in one paragraph each

**Ours.** There is no current device. One context per paired machine
(`core/deviceContexts.js`: `session`, `rpc`, `cacheScope`, `chatRepository`,
model catalog, `offline`), every online device open at once, feeds merged per
device (`core/feedMerge.js`), rows stamped with `deviceId` and a `projectKey`
minted in `core/deviceKey.js`, routes carrying `#/device/<id>/project/<id>/…`,
device-less routes resolved across devices (`views/resolving.js`), the picker
turned into a filter, one Creation device on the account page, per-device
settings pages, and a scan test (`test/noCurrentDevice.test.js`) that fails on
`App.call`/`App.session`/`App.cacheScope`/`App.chatRepository`/`App.offline`/
`App.offlineSince`/`App.modelCatalog` and the retired helpers.

**Main.** Three things landed. (1) *Workspaces* — a durable multi-source
checkout per project: `workspace.list/get/create/finish/retry/…` RPCs, a
`workspace` route (`#/project/<p>/workspace/<w>[/directory/<s>]/<tab>`),
`views/workspaceView.js` with directory tabs in the toolbar, the inbox rail's
two faces now list workspaces (not branches/issues), `createWork.js` creates
only workspaces, issue *creation* is retired everywhere (compose, capture
routing, create dialog), and two redirect views (`retiredBranchView.js`,
`retiredIssueView.js`) exist but are **not** wired into `VIEWS` — `branch` and
`issue` still render `renderBranch`/`renderIssue` ("legacy, kept reachable").
(2) *The wire contract* — a semver `api_version` in the greeting, an adapter
per major (`core/bridgeApi/`) installed on the session (`session.installAdapter`),
push subscriptions (`changes.subscribe`, `type: "changes"` items with keys),
request priority on the envelope, the cache syncer as a background tier, and
two version gates (`views/versionGate.js`) driven from `views/gate.js` through
`connection.onBridgeSelected`. (3) *Chrome* — a new palette, a floating inbox
toggle in a `#global-controls` header, the rail as a popover when collapsed,
`#branch-tabs` as a vertical rail beside `#root`, the global compose box
removed from the shell (its module kept for queued captures), a "New project"
button in the rail head, and the ⋯ toolbar menu (archive / project settings)
deleted. It also moved the bridge-owned panels (agent modes, fallback harness,
isolation, triage) off the account page onto the device page — the same move
we made — and renamed isolation `cow` → `rift`, `ACCOUNT_ISOLATION` →
`DEVICE_ISOLATION`.

The merge's shape: **main's product surfaces are the base text; our device
model is the substrate they sit on.** Where main reads a singleton (`App.call`,
`App.session`) it reads a context; where main keys by a bare bridge id it keys
by a device-qualified key; where main has one of something per app (adapter,
subscriptions, version gate, greeting) it has one per device.

## 1. Decisions

### (a) Workspaces sit on the per-device model

`workspace.list` is a per-bridge RPC, exactly like `project.list`, so it is
read where `project.list` is read: `taskFeed.tick(context)` asks its device
for `board.list`, `project.list` and `workspace.list` (the last caught to
`{ workspaces: [] }` as main does, so a bridge without the verb still feeds
the board), and `feedMerge.liveFeedSnapshot(board, projectList, workspaceList,
deviceId)` stamps every workspace with `deviceId`, `projectKey` and
`workspaceKey`, joins `board.workspace_summaries` as main does, and
`WIRE_FIELDS`/`FEED_COLLECTIONS` gain `workspaces` — which is what makes
`mergeFeeds` concatenate them in device order and `filterByDevice` narrow them
with no further code. A workspace's identity across the account is the pair
(device, workspace id); the bridge's workspace ids look uuid-shaped today but
nothing on the client relies on that, so the key is minted the way the project
key is and in the same file: `core/deviceKey.js` gains
`export const workspaceKey = (deviceId, workspaceId) => deviceKey(deviceId,
workspaceId)` and `routeWorkspaceKey(route)`; nothing else concatenates the
two halves. Every place main keys a workspace by `workspace.id` — the inbox
entry key `workspace:<id>`, `workspacesBeingFinished`, `workspaceProjectBlocks`
(which keys by bare project id), the toolbar's `workspacesByProject` map, the
console key `workspace:<id>` — keys by `workspaceKey`/`projectKey` instead;
wire params stay bare (`workspace_id`, `project_id`), as the project rule
already says. Workspace routes carry the device segment like project routes:
main's `workspaceRoute(projectId, parts)` parser sits under `surfaceFromSegments`
untouched, `peelDevice` already strips `device/<d>` in front of `project`, so
`#/device/<d>/project/<p>/workspace/<w>/directory/<s>/<tab>` parses for free;
`stampDevice` stamps it because it has a `projectId`; `HASH_WRITERS` gains a
`workspace` writer that uses `projectPrefix(route)`; `WORK_SURFACES` gains
`"workspace"` so a device-less workspace link parks on `resolve/project` and
`routeResolve.REFERENCE_KINDS.project` resolves it across devices (candidates:
rows and projects *and workspaces* with that project id; the picked device is
stamped onto the inner route). `routeFromHash`'s Files-tab `tabPlace` check
includes `workspace` (main's change). `projectModel.workspaceRoute(workspace)`
stamps `deviceId: workspace.deviceId`, like `projectRoute`. The workspace view
takes everything from `routeContext(App.route)`: `state.callRpc = context.rpc`,
`cacheScope: context.cacheScope` handed to `renderFilesTab`/`mountGitPane` (as
`branchView` does), `chatRepository`/`cacheScope` into the agent rail context,
`consoleKey`/`mountConsole` given `deviceId`, and every `App.call !==
state.callRpc` ownership guard (workspaceView.js:168,178;
workspaceGitInitialization.js:106,155) becomes `!context.active() || context
!== routeContext(App.route)`. A workspace link whose device cannot answer
mounts `mountDeviceNotice` exactly as `renderBranch` does (`views/branchView.js`
is the model; copy its first twenty lines). The retired redirect views (see e)
read `routeContext(App.route).rpc` and stamp `deviceId` on the workspace route
they redirect to.

### (b) The bridge API facade is per device

Each context greets its own bridge and holds its own adapter and api version.
In `connection.greetLiveBridge(context)` the `greetBridge` call gains main's
`install` option, and this is where `selectAdapter` lands: `greetBridge(session
.call, { deviceId, isCurrent, onGreeting, install: (selection) => { const
adapter = session.installAdapter(selection); adoptBridgeSelection(context,
selection, adapter); return adapter; } })` — `changeEvents.greetBridge` keeps
calling `selectAdapter(greeting)` itself (main's shape, one selection per
greeting) and hands the selection to `install`; nothing in `connection.js`
selects. `adoptBridgeSelection` is a new export of `core/deviceContexts.js`
that writes `context.adapter` (the adapter or null), `context.apiVersion`
(`selection.version`) and `context.unsupported` (`"app" | "bridge" | null`),
then `announceDeviceState()`. `adoptDeviceSession(session)` clears all three
(a reconnect re-greets and re-selects, as main's comment demands). Every RPC
from a context already goes through `context.rpc`, which reads
`context.call = session.call`, and `session.call` routes through the installed
adapter — so error normalisation and the raw/adapter split stay inside
`core/session.js` untouched. `canAnswer(context)` becomes `Boolean(context &&
context.call && !context.offline && !context.unsupported)`: an unsupported
bridge is a machine that cannot be asked anything, so its rows grey, its verbs
shut and a surface about it mounts the notice — with the version wording, not
the offline one (`core/text.js` gains `deviceAppBehindText(name, version)`
and `deviceBridgeBehindText(name, version)`; `deviceNotice.js` picks by
`context.unsupported`). `changeEvents` keeps main's `bridgeCapabilities()` /
`bridgeAdapter()` / `bridgeApiVersion()` but each takes a `deviceId` and reads
that device's entry of a per-device map (see c); `NO_CAPABILITIES` for an
unknown or unsupported device. The version gate is per device: main's
`onBridgeSelected` listener and `App.session?.deviceId` go; `views/gate.js`'s
`holdAppWhileNoDeviceAnswers` already re-evaluates on every
`onDeviceStateChanged`, and `holdForDevices()` chooses the screen — when
`liveContexts()` is empty and some known context has `unsupported`, it renders
`renderAppBehindBridgeGate`/`renderBridgeBehindAppGate` for the first such
device (app-behind wins when both kinds exist, since a reload fixes that one
at no cost), else `renderWaiting`. The whole app gates only when NO device is
usable; one unsupported bridge among usable ones is a device-level notice in
its rows and surfaces. `App.updateAvailable` (main's, set by the served-version
watcher in `main.js`) survives as the reload gate's input.

### (c) Push subscriptions belong to the session that holds them

Main's subscription manager (`sessionCall`, `subscriptionsMode`,
`liveSubscriptions`, `lastBoardRevision`, `syncChain`, `wantSubscriptions`) is
one bundle of per-session state. It becomes a `Map<deviceId, bridgeState>`
inside `changeEvents.js`, one entry per greeted device, created by
`adoptGreetedSession(call, deviceId)` and deleted by `disarmChangeEvents
(deviceId)`; `armed` stays our per-device map. A watcher registered with
`deviceId` subscribes on that device only; a watcher that spans devices
(`deviceId: null`) is subscribed on every device whose bridge serves
subscriptions, under the same `subscription_id` on each — the id is minted per
watcher-and-scope as main does, the bridge namespaces ids per session, so no
collision. `desiredSubscriptions(deviceId)` filters by `watcher.hears
(deviceId)`; `scheduleSync(deviceId)` queues one diff per device;
`subscriptionsSettled()` awaits every chain. `dispatchChangeEvent(payload,
deviceId)` routes `type: "changes"` items to watchers that `hears(deviceId)`
(board item news is judged against that device's `lastBoardRevision`), and
the legacy `board.changed`/`entity.changed` as ours does today. Reconnect:
`greetLiveBridge(context)` runs on every landed session and on every carrier
change, so the whole desired map for that device is replayed there, per device,
as main replays it per app. Cadence is per device: `pollIntervalMs(fastMs,
deviceId)` and `changeEventsArmed(deviceId)` are ours already; `subscriptions
Active(deviceId)` and `onSubscriptionsChange(fn)` (called with `(deviceId,
active)`) are main's, widened. `cacheSync.js` is the background tier per
device: `syncDeviceSnapshot(deviceId, view)` registers that device's two
all-scope background watchers on first sight (`watchBackground(deviceId)`,
kept in a `Map<deviceId, watcher[]>`), `forgetDevicesMissingFrom` disposes
them, `applyChanges(items, deviceId)` looks rows up by `rowKey(deviceId,
entity_id)`, `syncEntityWatchers`/`sweep` consult `subscriptionsActive
(deviceId)` per device, and every read rides `context.rpc(method, params,
BACKGROUND)` — our `rpc` forwards a third argument untouched, so main's
envelope reaches `session.call`. Focus-tier registrations main added to
`branchView`, `agentRail` and the feed's board watcher (`kinds`, `mode`) are
kept and gain the `deviceId` they already have on our side.

### (d) `deviceBootstrap.js` does not survive; its two behaviours do

Main's `openFirstReachableDevice` exists so that one stalled device does not
strand the others while the app opens exactly one session. We open every
online device concurrently with `preferDeviceId` pinned per socket
(`openDeviceSessions`), so no device consumes another's relay snapshot and
there is nothing to rotate through: `core/deviceBootstrap.js`,
`connection.openBootSession` and `test/deviceBootstrap.test.js` are deleted.
Two behaviours are carried over. First, `securityCritical` errors (main's
`relayLink.js` change, which auto-merges) must not be treated as "offline,
back off and retry": `connection.connectDevice` rethrows them without
`setContextOffline`/`scheduleResume`, `resume()` stops on them, and the gate's
`enterApp` surfaces the message on the waiting screen's `#oerr` — a pinned-key
mismatch is a stop, never a reconnect loop (`connectionOffline.test.js` gains
the case). Second, main's "eventually tries devices whose API status is stale
offline": we open only devices the list calls online, but the relay's
`device_key` push calls `markDeviceOnline → openDeviceSessions()` and the
waiting screen re-reads the list every three seconds, so a device the api
called offline is opened the moment either says otherwise; the plan records
this rather than adding a rotation. The creation device is untouched:
`App.selectedDeviceId` is where creation goes, never which device the app
boots on.

### (e) Retired branch/issue views

What main did: it did **not** retire the branch and issue surfaces — `VIEWS`
still maps `branch → renderBranch` and `issue → renderIssue`, and
`shellSkeleton.test.js` pins "keeps a legacy branch conversation reachable".
It retired issue *creation* (compose manual panel, capture routing, create
dialog and the rail's + all make branches or workspaces now) and added two
views nothing imports: `renderRetiredBranch` (looks the branch up in
`workspace.list` and redirects to the workspace whose directory carries it,
else says "This checkout has no workspace") and `renderRetiredIssue` (follows
`issue.workspace_id` to a workspace, else "Issues are read-only in this
version"). Decision: our device-aware `renderBranch`/`renderIssue` stay the
views the routes render (they carry `deviceId`, the notice, the frozen strip,
per-route context); main's two retired views are kept as files, migrated to
`routeContext(App.route).rpc` with the device notice when the context cannot
answer, made to stamp `deviceId` on the workspace route they `go` to, and
stay unwired — wiring them is a product switch main has not thrown. If the
integrator finds they are dead beyond doubt, deleting them with their test
(`issueRetirement.test.js` does not import them) is acceptable; keeping them
readable costs one migration each. The retirement of issue creation is taken
whole (main's `compose.js`, `captureDecision.js`, `captureDecisionView.js`,
`createWork.js` text and shapes; `CREATE_KINDS = ["workspace"]`), and our
device wiring re-lands on it: `openCreateWork({ projectId, deviceId,
projectName, navigate })` asks `deviceCall(deviceId)`, and the created
workspace's route carries `deviceId`.

### (f) Main's chrome and directory tabs are the base

`spa/index.html` (not conflicted, main's lands whole), `spa/src/styles.css`,
`spa/src/styles/shell.css` and `core/inboxShell.js` (main's, not conflicted)
are taken as main wrote them: the palette, `#global-controls` with the
floating `#inbox-open`, the rail as a popover, `#inbox-new-project` in the rail
head, `#branch-tabs` beside `#root`, the toolbar as workspace switcher +
directory tabs with the `@container toolbar-shell` collapse into a directory
menu, no compose box in the shell, no ⋯ menu. Our four contributions are
re-applied onto it, not merged around it: (1) the device tag — `dimDeviceHtml`
after a clashing project name on rows, block heads and the toolbar's project
menu entries, and on workspace rows the same rule via the row's `projectKey`;
(2) the offline word and greys — `.inbox-offline`/`.inbox-away`/`.inbox-muted`
rules and `--inbox-actions-room` from our `shell.css` hunk, `.device-strip` and
`.device-away` from ours, replacing main's `body.offline` rules which are gone
on both sides; (3) the picker as a filter — our `devices.js` (not conflicted)
and `#devpick` styles, minus the deleted `.device-picker-error`; (4) the
Creation device control on the account page (ours, in `views/settings.js`).
Main's `#inbox-new-project` head button calls `openNewProject()` from
`inboxView` — that export is ours (`creationTarget` → `openNewRepo(onDone,
{ callRpc, deviceName })`), so the button creates on the creation device.
The compose module stays as main left it (guarded on `#compose` being absent)
with our creation-device placeholder and notes intact; `composeDom.test.js`
keeps main's fixture that inserts a `#compose` host, so the module is still
exercised. Main's `.logo` click handler removal and `initCompose()` in
`main.js` land as main wrote them.

### (g) What main added on the aliases we deleted

`git grep -n "App\.\(call\|session\|cacheScope\|chatRepository\|offline\)"
origin/main -- spa/src` lists 105 lines; most are base text we already
migrated and git's auto-merge keeps our version. The auto-merged tree (checked
with `git merge-tree --write-tree origin/main HEAD` then `git grep` on it)
leaves exactly these, all of which package P2–P4 must move:

| file:line (merged tree) | main added | becomes |
| --- | --- | --- |
| `core/createWork.js:72,284` | `canCreate` reads `App.call`; `workspace.create` via `App.call` | `askDevice = deviceCall(deviceId)`; `canCreate = state.projectId && canAnswer(contextFor(deviceId))` |
| `core/inboxView.js:672` | `workspace.finish` via `App.call` | `verbCall(entry)("workspace.finish", …)` |
| `core/toolbar.js:137,142,158` | `workspace.list`/`workspace.get` via `App.call`, `!App.call` guard | `contextFor(project.deviceId).rpc` for a project, `routeContext(App.route).rpc` for the route's workspace; guard `canAnswer(context)` |
| `core/workspaceGitInitialization.js:106,155` | `App.call === callRpc` ownership guard | `context.active() && context === routeContext(App.route)` (pass `context`, not `callRpc`) |
| `views/workspaceView.js:168,178,288` | `state.callRpc = App.call`; two `App.call !== state.callRpc` guards | `state.context = routeContext(App.route)`, `state.callRpc = state.context.rpc`; guards as above |
| `views/retiredBranchView.js:27`, `views/retiredIssueView.js:11` | `App.call` | `routeContext(App.route).rpc` behind `canAnswer`, else `mountDeviceNotice` |
| `views/gate.js:349` | `gatedDeviceName = () => deviceName(App.session?.deviceId)` | the gated device's id comes from the context that is unsupported (b); `deviceNameOf(App.devices, id)` |
| `views/deviceSettings.js:159` | `App.session?.deviceId === device.id` → `App.modelCatalog = null` | ours already: `contextFor(device.id)?.refreshModelCatalog()` via `devicePanels.refreshAccountCatalog` — drop main's line |
| `sheets/newRepo.js:9,14,71` | `session = App.session, call = App.call` guard | ours: `openNewRepo(onDone, { callRpc, deviceName })`, no guard — the caller's `rpc` refuses when its machine goes |
| `sheets/projectSettings.js:22` | default `callRpc = App.call` | ours: `callRpc` required (no caller on main; keep the sheet, keep main's `sourcesHtml`) |
| `views/settings.js:204` | `project.list` via `App.call` (account-page projects panel) | drop: the projects panel lives on the device page on both sides; ours wins the file |
| `core/taskFeed.js:69,99,100` | conflict-marker residue of main's `ownsFeedContext`/`tick` | ours (per-context `tick`) |

Also surviving the auto-merge and to be fixed in the same pass:
`views/devicePanels.js` imports `ACCOUNT_ISOLATION`, which main renamed to
`DEVICE_ISOLATION` (rollup fails the build until it is renamed);
`views/files.js:14` imports `currentCacheScope` (ours removed it — main's
`directoryCacheId` hunk is kept, the import line is dropped);
`core/inboxView.js:59` imports `newProjectButtonHtml`, which main deleted (the
button moved to the rail head). `sheets/clone.js` was deleted on our side and
main did not touch it: the deletion stands. `core/adoption.js` comments were
reworded on our side; main did not touch them.

## 2. Conflict rules, one per file

"Base" is the side whose text the file starts from in the editor; "carry"
is what is taken from the other side; "delete" is what neither keeps.

1. **`README.md`** — base: main (its "Projects and workspaces" and "Work
   isolation" sections replace "Adding projects"). Carry: our "Devices" section
   whole, and our rewording of "Device project folders" (folder chooser on the
   device's own page). Delete: our "Adding projects" paragraphs (main's
   multi-source "Add project" supersedes them), keeping only the one sentence
   that **New project** in the rail and captures go to the **Creation device**,
   appended to main's "Projects and workspaces". Where main says "the selected
   device" it now says "that device" (there is no selected device).
2. **`spa/src/connection.js`** — base: ours. Carry from main: the
   `install` option in `greetLiveBridge` (b), the `securityCritical` handling
   into `connectDevice`/`resume` (d). Delete: main's `openBootSession`,
   `onBridgeSelected`/`bridgeSelectedListener`, `openAppSession`,
   `adoptSession`, global `goOffline`/`resume`/`switchDevice`, module
   `peerLink`, the `deviceBootstrap` import. `setConn` is gone on our side and
   main's gate no longer needs it once the gate is ours.
3. **`spa/src/core/cacheSync.js`** — base: ours (per-device
   `syncDeviceSnapshot`, `rowKey`, `retuneWatchers`). Carry from main: the
   module comment's "Two tiers" section, `BACKGROUND = requestPriorityFields
   ("background")` on every `context.call`/`rpc`, `listTrees`, `applyFiles`/
   `applyGit`/`threadBehind`/`rowState`/`stateMoved`/`applyDetail`/`applyItem`
   /`applyChanges`, `sweep`, `watchBackground`, the `warmed` set, `syncEntity
   Watchers` gating on `subscriptionsActive`, `onSubscriptionsChange`,
   `visibilitychange` sweep — all keyed by `rowKey(deviceId, entityId)` and
   registered per device (c). Delete: main's single `syncContext()` over
   `App.*`, main's `takeActiveRows`/`evictLeavers`/`warmNewcomers` in their
   app-wide form (their bodies fold into `keepActiveRows`/`evictUnnamed`/a
   per-device `warmNewcomers`).
4. **`spa/src/core/changeEvents.js`** — base: main (the subscription
   manager, adapter selection, `changes` dispatch, `WATCHER_DEFAULTS`). Carry
   from ours: `armed` as a `Map`, `deviceKeyOf`/`spansDevices`/`hearsFor`,
   `watcher.deviceId`/`hears`, `armChangeEvents(greeting, deviceId)`,
   `disarmChangeEvents`, `changeEventsArmed(deviceId)`, `pollIntervalMs(fastMs,
   deviceId)`, `dispatchChangeEvent(payload, deviceId)` with `AUDIENCE_FOR`,
   `refetchEverything(deviceId)`, `greetBridge(call, { deviceId, … })`. Then
   widen main's per-session state into the per-device map (c). Delete: the
   module-level `sessionCall`, `subscriptionsMode`, `lastApiVersion`,
   `installedAdapter`, `lastBoardRevision` singletons.
5. **`spa/src/core/composeView.js`** — base: ours (creation device, `homeCall`,
   `deviceCatalog(null)`, `heldCaptureRow`). Carry from main: branch-only
   manual panel (no `kindButton`, `kind: "branch"`, "Dispatch to the branch",
   the single `fail("Say what the agent should do first.")`), the `if (!$
   ("#compose")) return` guards in `openCompose` and `initCompose`, and
   `paintPrompt()` moved after the feed subscription. Delete: our issue-kind
   branches of `advancedHtml`/`wireAdvanced`/`submitManual`.
6. **`spa/src/core/createWork.js`** — base: main (workspace-only dialog,
   `workspaceCreateParams`, `CREATE_KINDS = ["workspace"]`). Carry from ours:
   `deviceId` in `openCreateWork`'s bag, `askDevice = deviceCall(deviceId)` for
   the one RPC, `canCreate` on `canAnswer(contextFor(deviceId))`, `navigate({
   ...route, deviceId })`. Delete: our branch/issue tabs, `readBranches`,
   `deviceCatalog`/agent choice (a workspace carries no agent), and main's
   `App.call` reads.
7. **`spa/src/core/inbox.js`** — base: ours (`OPENS_AT`, `STANDS_ON`,
   `projectKey` keys, `projectTagHtml`/`dimDeviceHtml`, `inboxRowHtml`
   un-ratcheted). Carry from main: `workspaceFacts`, `toWorkspaceEntry`,
   `workspaceEntries(workspaces, projects, items)` (stamping `deviceId`,
   `projectKey`, `workspaceKey`; key `workspace:${workspaceKey}`; conversation
   lookup keyed by `JSON.stringify([projectKey, entityId])`; `workspaceRun`
   narrowed to the same device), `workspaceDoneHtml` and its slot in
   `inboxRowHtml`, `menuHtml`'s workspace early return, `activeEntryKey`'s
   workspace case as a `STANDS_ON.workspace` entry matching `deviceId` and
   `workspaceId`, `rerouteMenuHtml` branch-only. `OPENS_AT.workspace` is
   `workspaceRoute` with the device stamped. Delete: nothing else.
8. **`spa/src/core/inboxProjects.js`** — base: ours (`projectNameOf`,
   `clashingProjectNames`, `deviceTags`, `deviceTagHtml`, `rowDeviceNames`,
   `blockFor`, blocks keyed by `projectKey`). Carry from main:
   `workspaceProjectBlocks(entries, projects, activeWorkspaceKey)` keyed by
   `projectKey` (groups by `entry.projectKey`, adds device tags through
   `deviceTags`, `workspaceGroup: true`, route = the active workspace's else
   the first), `projectHeadHtml`'s workspace wording ("New workspace in …",
   "Open …'s workspace") and the create button only on `workspaceGroup`
   blocks; main's `projectHeadHtml` ratchet score is 12 as ours already says.
   Delete: `newProjectButtonHtml` (main).
9. **`spa/src/core/inboxView.js`** — base: main (workspace faces:
   `drawWorkspaceList`, `drawProjects` over `workspaceProjectBlocks`,
   `finishWorkspace`, `workspaceClicked`, `openNewProject` export, the
   `workspaces` state, attention count from workspace entries). Carry from
   ours: the snapshot filtering (`filterByDevice`, `snapshot`,
   `onlyDeviceRows`), `withDeviceNames`, `indexRowsByEntity`, `verbCall` on
   every RPC (`workspace.finish` included), `paintDeviceState(list, { entryFor,
   blockFor })` after every paint, `onDeviceStateChanged(draw)`, the control
   tables (`ROW_CONTROLS`/`CAPTURE_CONTROLS`/`BLOCK_CONTROLS`/`pressed`) with
   main's `data-workspace-done` added as the first `ROW_CONTROLS` entry,
   `blocksPainted` keyed by `projectKey`, `createInBlock` passing `deviceId`,
   `openNewProject` built on `creationTarget`, `markSeen`/`noteSelfAction`
   re-exported from `inboxSeen.js`, `inboxCaptures.js` wiring. Delete: main's
   `App.call` lines, main's inline `messageOf`, `newProjectButtonHtml` import,
   `projectsFrame`'s new-project element (main).
10. **`spa/src/core/routeResolve.js`** — base: ours (`pickDevice`,
    `REFERENCE_KINDS`, `onNamedDevice`, `onItsDevice`). Carry from main: a row
    with `workspace_id` resolves to a workspace route (`branchRouteFor` gains
    main's first branch, plus `sourceId`); `REFERENCE_KINDS.project.rows` also
    offers `feed.workspaces` with that project id. Delete: main's function-form
    `branchRouteFor` body.
11. **`spa/src/core/router.js`** — base: ours (`peelDevice`, `stampDevice`,
    `projectPrefix`, `HASH_WRITERS`, `withDeviceOrResolve`). Carry from main:
    `workspaceRoute(projectId, parts)` and its `case "project"` line in
    `surfaceFromSegments`, the `workspace` branch of `routeFromHash`'s
    `tabPlace`, and a `workspace` entry in `HASH_WRITERS` (`${projectPrefix
    (route)}/workspace/${encode(workspaceId)}${source}/${tab}${tabPlaceSuffix}`).
    `WORK_SURFACES` adds `"workspace"`. Delete: main's inline `hashFromRoute`
    chain and its ratchet line.
12. **`spa/src/core/taskFeed.js`** — base: ours. Carry from main: the third
    read `workspace.list` (caught to `{ workspaces: [] }`) in `tick`, passed to
    `liveFeedSnapshot`; `kinds: ["state"], mode: "realtime"` on the board
    watcher in `joinFeed`. `EMPTY_SCOPED_FEED`/`workspaces` lives in
    `feedMerge.js` (`WIRE_FIELDS`/`FEED_COLLECTIONS` gain `workspaces`,
    `liveFeedSnapshot` takes `workspaceList` and joins `workspace_summaries`).
    Delete: main's `ownsFeedContext`, `last`, `App.*` reads.
13. **`spa/src/core/toolbar.js`** — base: main (workspace switcher, directory
    tabs, `loadWorkspaces`, `workspaceRows`, `directoryMenu*`, `observeToolbar`,
    `openWorkspaceDirectory`, no ⋯ menu). Carry from ours: `shownFeed =
    filterByDevice(next, App.deviceFilter)`, `scopedProjectKey` in
    `SCOPE_KEY`, `projectFor`/`scopedProject`/`nameOf`, `projectMenuEntries`
    with `deviceTagHtml` and `devices: App.devices`, `workMenuEntries` via
    `scopedWork()` (legacy routes keep it), `openCreate` passing `deviceId`.
    Rewrite main's `workspaceRows`/`loadWorkspaces` on `contextFor(project
    .deviceId).rpc` and key `workspacesByProject` by `projectKey`; `identity()`
    passes `workspaces: workspacesByProject.get(routeProjectKey(App.route))`.
    Delete: our `openSurfaceMenu`/`openSettingsFor` (the ⋯ is gone on main),
    main's `App.call` lines.
14. **`spa/src/core/toolbarModel.js`** — base: ours (`STANDING`, `NOWHERE`,
    `projectKey` everywhere, `deviceTags`). Carry from main:
    `workspaceMenuModel` (filter by `projectKey`, `current` by `workspaceKey`),
    `workspaceDirectoryModel`, and a `STANDING.workspace` entry whose `rowIs`
    matches nothing and whose identity carries `workspaceId`, `workspace`,
    `directories` — `toolbarIdentity` takes `{ items, projects, workspaces }`.
    Delete: main's `workspaceIdentity`/`branchIdentity`/`issueIdentity`
    function-per-kind (ours is the table form of the same thing).
15. **`spa/src/sheets/newRepo.js`** — base: main (multi-source form,
    `inferredName`, `uniqueName`, `browseFor` with `allowCreateDirectory`).
    Carry from ours: the signature `openNewRepo(onDone, { callRpc, deviceName
    })`, no `App.session`/`App.call`, the device name in the sub line
    ("Add Git remotes or folders from <deviceName>."). Delete: main's
    session-change guard and its message.
16. **`spa/src/sheets/projectSettings.js`** — base: ours (`callRpc`
    required). Carry from main: `sourcesHtml(project)` and its slot. Delete:
    main's `App.call` default.
17. **`spa/src/styles.css`** — base: main. Carry from ours: the
    `.device-away` rules (replacing main's `body.offline` block, which main
    still has — delete it), the removal of `#offbar` and
    `.device-picker-error`, and our body comment ("version banner and the
    shell are the only in-flow children"). Delete: main's `#offbar` rule and
    `body.offline …` rules.
18. **`spa/src/views/deviceSettings.js`** — base: ours (`standUpDevicePanels`,
    `deviceOfflineText` refusal, `device-projects-panel`). Carry from main:
    nothing of its inline panel mounting (ours does the same through
    `devicePanels.js`); main's status wording "Bring this device online, then
    retry to configure it." may replace ours. Delete: main's `devicePreferences
    Html`, `scopedCall`, `invalidateActiveCatalog`. Non-conflicted follow-up:
    `views/devicePanels.js` renames `ACCOUNT_ISOLATION` → `DEVICE_ISOLATION`.
19. **`spa/src/views/files.js`** — base: ours (`cacheScope` from the
    options bag). Carry from main: `directoryCacheId(scope)` for
    `cacheEntityId`. Delete: main's `currentCacheScope` import.
20. **`spa/src/views/gate.js`** — base: ours (`openDeviceSessions().first`,
    `holdAppWhileNoDeviceAnswers`, `renderWaiting` with `waitingText`). Carry
    from main: `paintWaiting`'s `#waitintro` sentence choosing "report online
    but Build could not reach one yet" vs "None of your devices are online",
    the version-gate rendering (`showAppBehindGate`/`showBridgeBehindGate`
    with `mintInstallCommand`) called from `holdForDevices()` per (b), and
    `gateGeneration`/`connectingPromise` de-duplication of overlapping boots
    (main's four "overlapping boots" tests are worth keeping in the
    `enterApp` we have). Delete: `openBootSession`, `adoptSession`,
    `greetLiveBridge()` from the gate, `onBridgeSelected`, `setConn`,
    `gatedDeviceName`, `leaveVersionGate` (ours `leaveHold` does it).
21. **`spa/src/views/settings.js`** — base: ours (Creation device panel,
    `deviceRowHtml` with the Settings… link, `retireDevice` on revoke,
    `deviceCatalog(null)` for defaults). Carry from main: the heading
    "🤖 Browser agent defaults" and its sentence. Delete: main's projects
    panel, `#newrepo` → `openCreateWork`, `App.call("project.list")`.
22. **`spa/test/archiveDom.test.js`** — base: ours (per-device `archived.list`
    merge, `App.devices` fixtures). Carry from main: the `finished` workspace
    row and "opens a finished workspace from the archive" (the route it
    expects gains `deviceId`).
23. **`spa/test/branchViewDom.test.js`** — base: ours (route context, other
    device, unopened device cases). Carry from main: the deletion of "brings
    the row back to the inbox when the close-out is refused" (main removed it;
    the inbox no longer lists branch rows). Delete: any of our cases that
    assert a branch row in `#inbox-list`.
24. **`spa/test/cacheSync.test.js`** — base: ours (per-device fixtures,
    `contextFor` mock). Carry from main: `BACKGROUND` on every expected call,
    the `subscriptionsActive`/`onSubscriptionsChange` mock entries, the
    `truncated`-and-tree expectations. Main's new `cacheSyncChanges.test.js`
    (not conflicted) is rewritten to register a device (P1).
25. **`spa/test/changeEvents.test.js`** — base: ours (per-device suites).
    Carry from main: `keepPolling`, the `session.hello` client declaration
    expectations (`expect.objectContaining({ client })`), `bridgeApiVersion`
    cases — each given the device the greeting names.
26. **`spa/test/complexityRatchet.test.js`** — base: ours (`treeFiles.js`
    helpers). Carry from main: its two comment lines (cacheSync's four steps,
    capture routing branch-only). Constant: **66** — 70 at the fork, minus
    `onSnapshot` (both sides), `manualHtml` (main), `hashFromRoute` and
    `inboxRowHtml` (ours); verify by counting after resolution, and the
    number may only go down.
27. **`spa/test/composeDom.test.js`** — base: ours (creation-device cases,
    `deviceSessionFixture`). Carry from main: the `#compose` host inserted
    into the fixture (the shell no longer carries one), branch-only advanced
    panel expectations (`branch.dispatch`, no issue kind). Delete: our
    issue-filing expectations.
28. **`spa/test/createWork.test.js`** — base: main (workspace creation
    cases). Carry from ours: "routes to the created work with the device it
    was scoped to" and "calls the scoped device's bridge, not the home alias"
    rewritten for `workspace.create`; the "blocks creation without a
    connected project" case registers no context for the device instead of
    `App.call = null`.
29. **`spa/test/defaultHarness.test.js`** — base: ours (device page mounts,
    account page defaults from `deviceCatalog`). Carry from main: "keeps
    bridge-owned controls out and labels the browser default" (the heading
    text). Delete: main's `App.session`-based cases.
30. **`spa/test/deviceSettingsDom.test.js`** — base: ours
    (`deviceSettingsFixture`, `devicePanels` cases). Carry from main: the
    Rift capability case ("uses the named device's Rift capability rather
    than the active application session") and "does not let a detached
    preference control call its old device", rewritten on the fixture and on
    `rift`. Delete: main's `App.session?.deviceId`/`App.modelCatalog`
    expectations.
31. **`spa/test/gateOnboardingDom.test.js`** — base: ours (`openDeviceSessions`
    mock, `unmountView`). Carry from main: the overlapping-boot cases and
    "does not claim none are online after an online device's handshake fails"
    (`#waitintro`), rewritten so the mock is `openDeviceSessions` returning
    `{ first, settled }`. Delete: `openBootSession`/`adoptSession`/`render`
    mocks in main's form.
32. **`spa/test/inboxDom.test.js`** — base: main (workspace rows and
    blocks). Carry from ours: a feed fixture that carries `devices`, `deviceId`
    /`projectKey`/`workspaceKey` on every row and workspace (through the real
    `liveFeedSnapshot`/`mergeFeeds` or an equivalent fixture), our
    `deviceSessionFixture` contexts instead of `App.call`, and our device
    suites — read cursor to the row's device, the offline word and greyed
    verbs, the filter, the device tag on a clashing project name — re-stated
    over workspace rows; block selectors become `data-project="<projectKey>"`.
    Delete: our branch/issue-row assertions (the rail no longer lists them).
33. **`spa/test/inboxProjects.test.js`** — base: ours (`projectKey` blocks,
    device tags). Carry from main: the two `workspaceProjectBlocks` cases (ids
    become keys) and "heads the legacy block without branch or issue creation
    controls". Delete: our `newProjectButtonHtml` case.
34. **`spa/test/inboxProjectsDom.test.js`** — modify/delete: **delete**
    (main's deletion wins). Our block-level device-tag and offline assertions
    from it move into `inboxDom.test.js`'s projects-face cases (rule 32).
35. **`spa/test/isolation.test.js`** — base: main (`rift`, `DEVICE_ISOLATION`,
    device wording). Carry from ours: the device-page mount cases, re-spelled
    on `rift`. Delete: our `cow`/`ACCOUNT_ISOLATION` spellings.
36. **`spa/test/newProjectDom.test.js`** — base: main (multi-source form
    cases). Carry from ours: opening through `{ callRpc, deviceName }` and
    the case that a refused `callRpc` shows its message. Delete: main's
    `App.session`/`App.call` guard case ("guards device changes and newer
    sheets" keeps only its newer-sheet half).
37. **`spa/test/shellSkeleton.test.js`** — base: main (`#branch-tabs`,
    `--inbox-space`, popover rail, gate hides the rail). Carry from ours: the
    `App.devices`/context fixture the render-dispatch cases need, and
    `deviceId` on every route they set.
38. **`spa/test/toolbarDom.test.js`** — base: main (workspace toolbar
    cases). Carry from ours: the device-tag-in-project-menu case, the filter
    case, `deviceId` on every route, contexts instead of `App.call`
    (`workspace.list` answered per device through the fixture). Delete: our
    ⋯-menu cases.

Not conflicted but touched by the rules above: `spa/src/core/feedMerge.js`
(`workspaces`), `spa/src/core/deviceKey.js` (`workspaceKey`),
`spa/src/core/deviceContexts.js` (`adoptBridgeSelection`, `canAnswer`),
`spa/src/views/devicePanels.js` (`DEVICE_ISOLATION`),
`spa/src/views/workspaceView.js`, `spa/src/core/workspaceGitInitialization.js`,
`spa/src/views/retiredBranchView.js`, `spa/src/views/retiredIssueView.js`,
`spa/src/core/projectModel.js` (`workspaceRoute` stamps `deviceId`),
`spa/src/core/consoleModel.js` (`consoleKey` for a workspace carries the
device), `spa/src/core/deviceBootstrap.js` and its test (deleted),
`spa/vite.config.js` (both sides' additions coexist: our `setupFiles` and
main's `execArgv: ["--no-experimental-webstorage"]` and `validateDeployRelay`;
keep both), `spa/test/noCurrentDevice.test.js` (unchanged; it is what turns
the (g) table red until P2–P4 land).

## 3. Work packages after the merge commit

The merge commit resolves every file per section 2, passes `npm run lint` and
`npm run build`, and lands with tests red. Then, in order:

### P1 — Primitives: keys, per-device adapter and greeting, feed merge of workspaces

Goal: the substrate every later package stands on — one key minting place,
one adapter/api version/subscription state per device, workspaces in the
merged feed.

Files: `spa/src/core/deviceKey.js`, `spa/src/core/feedMerge.js`,
`spa/src/core/taskFeed.js`, `spa/src/core/deviceContexts.js`,
`spa/src/connection.js`, `spa/src/core/changeEvents.js`,
`spa/src/core/cacheSync.js`, `spa/src/core/text.js`,
`spa/src/core/deviceNotice.js`, `spa/src/core/deviceBootstrap.js` (delete);
tests `deviceKey`, `feedMerge`, `taskFeed`, `deviceContexts`,
`connectionOffline`, `changeEvents`, `adapterGreeting`, `changeSubscriptions`,
`cacheSync`, `cacheSyncChanges`, `session`, `bridgeApi`, `apiContract`,
`deviceBootstrap` (delete).

Acceptance:
- `workspaceKey`/`routeWorkspaceKey` exist in `deviceKey.js`; `git grep
  '\${deviceId}/' spa/src` matches only `deviceKey.js`.
- `mergeFeeds` output carries `workspaces` stamped with `deviceId`,
  `projectKey`, `workspaceKey`; `filterByDevice` narrows it with no change to
  `deviceFilter.js`.
- After `greetLiveBridge(context)` resolves, `context.adapter`,
  `context.apiVersion`, `context.unsupported` are set; `canAnswer` is false for
  an unsupported bridge; `adoptDeviceSession` clears them.
- `changeEvents`: two devices, one 1.1 and one 1.0 — subscriptions are issued
  only on the 1.1 device, a `changes` push from it wakes only watchers hearing
  it, `subscriptionsActive("A")` and `("B")` differ.
- `connectDevice` rethrows `securityCritical` errors without scheduling a
  resume.
- Suites green: `deviceKey`, `feedMerge`, `taskFeed`, `deviceContexts`,
  `connectionOffline`, `changeEvents`, `adapterGreeting`, `changeSubscriptions`,
  `cacheSync`, `cacheSyncChanges`, `session`, `bridgeApi`, `apiContract`.
- No new `eslint-disable`; `complexityRatchet` at 66.

### P2 — Routes and the workspace surfaces

Goal: workspace routes carry the device, device-less ones resolve, and the
workspace view, the retired views and the create dialog read their context
from the route.

Files: `spa/src/core/router.js`, `spa/src/core/routeResolve.js`,
`spa/src/core/projectModel.js`, `spa/src/views/workspaceView.js`,
`spa/src/core/workspaceGitInitialization.js`, `spa/src/views/retiredBranchView.js`,
`spa/src/views/retiredIssueView.js`, `spa/src/core/createWork.js`,
`spa/src/core/consoleModel.js`, `spa/src/views/resolving.js`; tests `router`,
`routeNormalize`, `routeResolve`, `resolvingDom`, `workspaceViewDom`,
`workspaceModel`, `workspaceRefPicker`, `createWork`, `issueRetirement`,
`consoleModel`, `routeTerminals`.

Acceptance:
- `#/device/d1/project/p1/workspace/w1/directory/s1/files?path=a.js`
  round-trips through `routeFromHash`/`hashFromRoute`; the device-less form
  parses to `resolve/project` carrying the inner workspace route and resolves
  to the device whose feed lists `w1`.
- `renderWorkspace` on a route whose device has no context mounts the device
  notice and makes no RPC; on a live one every RPC goes through that context's
  `rpc` (a second device's `rpc` is never called).
- `openCreateWork({ deviceId })` calls that device's `rpc` for
  `workspace.create` and navigates to a route carrying that `deviceId`.
- `noCurrentDevice.test.js` no longer names `workspaceView.js`,
  `workspaceGitInitialization.js`, `createWork.js`, `retired*View.js`.
- Suites green: the ones listed; `complexityRatchet` at 66; no new
  `eslint-disable`.

### P3 — The rail and the toolbar on main's chrome

Goal: main's workspace faces and toolbar with our device tags, offline
marks, filter and per-device verbs.

Files: `spa/src/core/inbox.js`, `spa/src/core/inboxProjects.js`,
`spa/src/core/inboxView.js`, `spa/src/core/inboxDevices.js`,
`spa/src/core/toolbar.js`, `spa/src/core/toolbarModel.js`,
`spa/src/styles/shell.css`, `spa/src/styles.css`; tests `inbox`,
`workspaceInbox`, `inboxProjects`, `inboxDom`, `inboxPeek`, `toolbarDom`,
`toolbarModel`, `deviceFilter`, `devicePickerDom`, `shellSkeleton`,
`agentRailDom`, `agentSurfaceClearance`.

Acceptance:
- Two devices each with a project named "app" and one workspace: the rail
  shows two blocks and two rows, each name followed by its device; the
  toolbar's project menu shows the tag; with the filter set to one device the
  other's rows, blocks and menu entries are absent and the route is untouched.
- A device going offline greys its workspace rows, adds the offline word, and
  shuts `data-workspace-done` and `data-project-create` with the offline
  title; `workspace.finish` on a row calls that row's device.
- `#inbox-new-project` opens `openNewRepo` on the creation device's `rpc`.
- The toolbar on a workspace route lists that project's workspaces from that
  device's `rpc` and its directory tabs; on a legacy branch route it reads as
  main's test "keeps legacy deep-link identities readable".
- Suites green: the ones listed; `complexityRatchet` at 66; no new
  `eslint-disable`.

### P4 — Gate, settings and the remaining chrome

Goal: the per-device version gate, the account/device settings pages,
sheets, compose and the last alias holdouts.

Files: `spa/src/views/gate.js`, `spa/src/views/versionGate.js`,
`spa/src/views/settings.js`, `spa/src/views/deviceSettings.js`,
`spa/src/views/devicePanels.js`, `spa/src/sheets/newRepo.js`,
`spa/src/sheets/projectSettings.js`, `spa/src/sheets/browser.js`,
`spa/src/core/composeView.js`, `spa/src/core/compose.js`,
`spa/src/views/archive.js`, `spa/src/devices.js`; tests `gateOnboardingDom`,
`gateVersionDom`, `versionGate`, `settingsDom`, `settingsDownloadsDom`,
`deviceSettingsDom`, `defaultHarness`, `isolation`, `newProjectDom`,
`browserScopeDom`, `projectSettings`, `composeDom`, `compose`, `archiveDom`,
`devicePickerDom`, `noCurrentDevice`.

Acceptance:
- One device answering 2.0.0 among a live 1.x device: the app is not gated,
  that device's rows wear the version notice, its settings page says the
  bridge is behind. Only that device known: the app shows main's
  app-behind gate naming the device, with the reload only when
  `App.updateAvailable`.
- `newRepo` opened from the device page creates on that device's `rpc` and
  never reads `App.*`; a refused `rpc` shows its message.
- `noCurrentDevice.test.js` green — every file under `spa/src`, comments
  stripped.
- Suites green: the ones listed plus the full run (`npx vitest run`);
  `complexityRatchet` at 66; no new `eslint-disable`; `npm run lint`,
  `npm run build`, semgrep, gitleaks clean.

### P5 — Docs

Goal: the repo's own account of what the integration did, where main's
words and ours disagreed.

Files: `HANDOFF.md`, `README.md`, `planning/v2/UX Redesign Decisions.md`
(dated line at the workspace decision saying workspaces are per device).

Acceptance:
- `HANDOFF.md` gains a "2026-09-14 — Integration with main" section under the
  multi-device one: the four decisions in (a)–(d) in one paragraph each, the
  deleted `deviceBootstrap`, the per-device version gate, the ratchet at 66,
  and a "Verified" note that the two-bridge browser pass is the orchestrator's
  to run after this workflow (do not claim it).
- `README.md`: main's "Projects and workspaces" and "Work isolation" sections
  stand; our "Devices" section stands; every "selected device" is "that
  device"; the one sentence that New project and captures go to the Creation
  device is present; the device-settings page paragraph names Rift isolation
  the way main does.
- `git grep -n "selected device" README.md HANDOFF.md` matches nothing; gitleaks
  and semgrep clean; committed.

## 4. What this plan does not do

It does not merge; it does not rewrite main's workspace product decisions
(the rail listing workspaces rather than branches, issue creation retired,
the ⋯ menu gone); it does not add bridge-minted unique ids, account-wide
capture routing or per-device terminal sockets (still the follow-ons in
`HANDOFF.md`); and it does not run the two-bridge browser pass.
