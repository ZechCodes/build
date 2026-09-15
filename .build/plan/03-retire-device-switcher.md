# Stage 03 — Retire the switcher: the picker filters, the app has no current device

## Goal

Delete the notion of a "current device" from the app. The device picker becomes
a **filter** over the merged rail ("All devices" by default, or one device) plus
the settings cog it already has. The home device is chosen in one obvious place
and used only for creation. The `App.session` / `App.call` / `App.cacheScope`
/ `App.chatRepository` aliases and `switchDevice` are removed, and a test keeps
them out. Docs say what the product now does.

Binding design: `.build/plan/00-multi-device-design.md` §5–§8.

## Context a cold agent needs

- **Picker** (`spa/src/devices.js`): `paintDevicePicker` labels the toggle with
  the session's device ("Which device runs your tasks"), rows have a select
  action (`data-select-device` → `selectDevice` → `switchDevice` /
  `setHomeDevice` after stage 1) and a settings cog. Tests:
  `spa/test/devicePickerDom.test.js`.
- **Remaining alias readers** after stage 2 — run
  `grep -rn "App\.\(session\|call\|cacheScope\|chatRepository\|offline\)\b" spa/src`
  and expect only: `composeView.js` (`canSend`, `flushCaptures`,
  `capture.create`, `capture.get`), `settings.js` (`project.list`,
  `settings.get/set` on ≈100–203, `App.modelCatalog`), `createWork.js` (the
  creation caller), `sheets/newRepo.js`, `sheets/clone.js`, `console.js`, and
  `app.js` (`loadModelCatalog` / `refreshModelCatalog`) — plus
  `views/archive.js:55` (`archived.list`: read every live context, rows
  stamped `deviceId` in the fetch loop since `toRow` at `core/archive.js:66`
  is ratcheted),
  the two prose mentions in `core/adoption.js:95,123`, and `devices.js:62`
  (the picker, rewritten here) (amended per `04-primitives.md` §5.10).
- **Model catalog** (`app.js:85–108`) is per bridge (`models.list`): it belongs
  on the context, not on `App`.
- **Account settings page** (`spa/src/views/settings.js`): the projects folder
  panel, default agent and isolation panels all read one bridge; the per-device
  settings page (`views/deviceSettings.js`) already owns the projects folder.
- **Docs:** `README.md` "Device project folders" / "Adding projects" (≈62–85)
  describe "the selected device"; `planning/v2/UX Redesign Decisions.md:94`
  says the inbox is device-scoped, `:151` says fan-in is a follow-on;
  `HANDOFF.md` has a dated log section pattern to append to.

## What to build

### 1. The picker filters

- State: `App.deviceFilter` (`null` = all, or a device id), persisted under
  `build.deviceFilter`. `paintDevicePicker` labels the toggle "All devices" or
  the device name; the menu's first row is "All devices"; each device row
  selects the filter; the cog stays. Title: "Which devices the inbox shows".
- `inboxView` / `projectBlocks` / `toolbarModel` receive `deviceFilter` and
  show only that device's rows and projects when set. The filter never affects
  routes: an open surface on a filtered-out device stays open.
- `selectDevice` no longer changes the home device.

### 2. The home device has one control

On the account Settings page, a **Creation device** select ("New projects and
captures go to") listing the paired devices, writing `build.selectedDeviceId`
through `rememberSelectedDevice`. The composer's placeholder names it
("Capture on <device name>"), and the capture decision page's project list is
that device's. `homeDeviceId()` from stage 2 is the only reader; when the
chosen device is offline the composer queues (it already does) and says which
device it is waiting for.

### 3. Delete the aliases

- Move `modelCatalog`, `loadModelCatalog`, `refreshModelCatalog` onto the
  context (`context.modelCatalog()`); `composeView`, `createWork`, `settings`
  call the home or route context's.
- `composeView`: `canSend = () => !!homeContext()?.call && !homeContext().offline`;
  `flushCaptures` / `capture.create` / `capture.get` on the home context.
- `settings.js`: the projects folder and default-agent/isolation panels move to
  `views/deviceSettings.js` (one page per device, which already exists and
  already owns its transport); the account page keeps the passkey/devices/keys
  panels, the Creation device select, and links to each device's page.
- `sheets/newRepo.js`, `sheets/clone.js`, `console.js`: home or route context
  (stage 2 named which).
- Remove `App.session`, `App.call`, `App.cacheScope`, `App.chatRepository`,
  `App.offline`, `App.offlineSince`, `adoptApplicationScope`,
  `disposeApplicationScope` (fold what `appScope.test.js` proves into
  `deviceContexts.test.js`), `switchDevice` (`setHomeDevice` since stage 1),
  `resetFeedScope` if it survived stage 1, the
  `cacheScope.js` singleton compatibility exports (`setCacheDevice`,
  `cacheDeviceId`, `currentCacheScope` — keep `adoptCacheScope` only if the
  registry still uses it).
- `spa/test/noCurrentDevice.test.js`: reads every file under `spa/src`,
  strips comments, and fails on `App.session`, `App.call`, `App.cacheScope`,
  `App.chatRepository`, `App.offline`, `App.offlineSince`, `App.modelCatalog`
  (the same shape as `complexityRatchet.test.js`; amended per
  `04-primitives.md` §5.11).

### 4. Offline, finished

Rows of an offline device: greyed, "offline" mark, verbs disabled, still
present (stage 1). A view open on a device that goes offline: the existing
frozen-view treatment, banner naming the device, resumes silently when its
context returns. A navigation that *arrives* at a work item on a device that
cannot answer — never opened here, or gone since — mounts the device notice
(`core/deviceNotice.js`, stage 2) instead of a surface: there is nothing on
screen to freeze, every verb on the frame would be refused, and the notice
hands the link back the moment that device lands. Changing that to a frozen
mount is this stage's call and needs this stage's two pieces first — the
disabled verbs inside a mounted surface, and the banner naming the device.
When the last context goes: the gate's waiting screen (`renderWaiting`) — not a
banner over a dead app.

### 5. Docs

- `README.md`: the device section says the inbox and projects rail show every
  device; the picker filters; the Creation device setting is where new projects
  and captures go; per-device settings live on each device's page.
- `planning/v2/UX Redesign Decisions.md`: amend `:94` and `:151` with a dated
  line: inbox and projects are account-wide as of this stage; capture routing
  remains device-scoped to the creation device.
- `HANDOFF.md`: a dated section in the existing style: what shipped, what is
  unverified (the two-bridge browser pass, if it could not be run).

## Tests (write first)

- `devicePickerDom.test.js`: "All devices" row; picking a device sets the
  filter and does not touch `selectedDeviceId`; the cog still routes to
  settings; keyboard navigation unchanged.
- `inboxDom.test.js` / `inboxProjectsDom.test.js` / `toolbarDom.test.js`: the
  filter hides the other device's rows and projects and never changes the
  route.
- `settingsDom` test: the Creation device select writes the sticky key; the
  composer placeholder names the device; an offline creation device queues
  with the device's name.
- `noCurrentDevice.test.js` as above.
- `connectionOffline.test.js`: last context lost → waiting screen; one context
  back → app resumes without a reload.

## Verify

`npm run lint && npm test && npm run build`; `semgrep`; `gitleaks`. Browser
pass: two bridges online → both projects lists in one rail; filter to one;
create a project and a capture and confirm both land on the creation device;
stop one bridge → its rows grey, the other keeps working; stop both → waiting
screen; start one → app returns.

## Follow-ons this stage does not do (record them in HANDOFF.md)

Bridge-minted globally unique project ids; account-wide capture routing;
per-device terminal sockets; WebRTC across devices.
