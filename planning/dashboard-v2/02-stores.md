# 02 — Domain Stores

Each store owns one slice of state, exposes a read API and mutation methods,
and lets consumers subscribe to changes. No store touches the DOM. No store
imports from another store — cross-store effects go through the bus.

## Common store contract

Every store follows this shape:

```js
// domain/<name>-store.js
import { bus } from '../core/bus.js';
import { makeSubscribable } from '../core/store.js';

const { subscribe, notify } = makeSubscribable();

// private state
const data = new Map();

// read API (pure)
export const fooStore = {
  get(id) { return data.get(id); },
  list() { return [...data.values()]; },

  // mutation API — the only way to change state
  upsert(foo) { data.set(foo.id, foo); notify({ kind: 'upsert', id: foo.id, foo }); },
  remove(id)  { data.delete(id); notify({ kind: 'remove', id }); },

  subscribe,
};

// bus bindings — stores REACT to transport events here
bus.on('foo.received', (foo) => fooStore.upsert(foo));
```

Subscribers receive the change event (`{ kind, id, ... }`), not the whole
slice — they decide what to re-read. Keep change events small.

## Stores and their slices

| Store                | Slice                                              | Primary bus inputs                        |
| -------------------- | -------------------------------------------------- | ----------------------------------------- |
| `devicesStore`       | `Map<deviceId, Device>`                            | `device.added`, `device.status_changed`   |
| `channelsStore`      | `Map<channelId, Channel>` + deviceId index         | `channel.upserted`, `channel.removed`     |
| `messagesStore`      | `Map<channelId, Message[]>`                        | `message.received`, `message.bulk`        |
| `activityStore`      | `Map<channelId, ActivityEntry[]>`                  | `agent.tool_use`, `agent.tool_result`, `agent.reasoning` |
| `presenceStore`      | `Map<channelId, {agentActive, planMode, harness}>` | `agent.started`, `agent.stopped`, `agent.plan_mode` |
| `unreadStore`        | `Map<channelId, {count, hasInteraction, lastSeen}>` | `message.received`, `channel.marked_read` |
| `filesStore`         | `Map<channelId, {tree, changes}>`                  | `files.list_result`, `files.changes_result` |
| `terminalStore`      | `Map<channelId, {history, cmds, completions}>`     | `terminal.output`, `terminal.complete`    |
| `tasksStore`         | `Map<channelId, Todo[]>`                           | `agent.todo_write` (derived from tool.use) |
| `complicationsStore` | `Map<channelId, Complication[]>`                   | `complications.updated`                   |
| `uiStore`            | `{tab, overlay, rail, dropdown, theme}`            | (no transport input; user-driven)         |

## Ownership rules

- **One store per slice.** If `messagesStore` also tracked unread, it'd be
  two responsibilities. Separate stores, composed via bus events.
- **Stores hold the source of truth; views hold derived DOM.** Re-rendering
  is cheap; re-deriving is explicit.
- **No computed fields baked into stored data.** If you need `device.channels`,
  compute it on read from `channelsStore` + `devicesStore`.
- **Persistence is a middleware.** A store may persist to IndexedDB/localStorage
  via a subscriber, not inside the store. Keep the store pure.

## `uiStore` is special

It's the only store that holds UI-adjacent state (which tab is visible, is
the rail open, which dropdown is open). It exists because that state
survives channel switches and is shared across views. Per-channel UI state
(scroll, draft) belongs on `Channel.viewState`, not here.

## Testing stores

Stores are pure — test them without DOM, without transport, by calling
mutation methods and asserting subscribers fire.

```js
test('messagesStore.append notifies subscribers', () => {
  const fired = [];
  messagesStore.subscribe(e => fired.push(e));
  messagesStore.append('ch1', { id: 'm1', content: 'hi' });
  expect(fired).toEqual([{ channelId: 'ch1', msg: { id: 'm1', content: 'hi' } }]);
});
```

## Migration from v1 `state.js`

| v1 `state.xxx`                | v2 home                                   |
| ----------------------------- | ----------------------------------------- |
| `devices`                     | `devicesStore`                            |
| `e2eeConnections`             | `transport/e2ee.js` (not a store)         |
| `channelDeviceMap`            | `channelsStore` (indexed on upsert)       |
| `deviceChannels`              | `channelsStore` (indexed on upsert)       |
| `deviceHarnesses`             | `presenceStore`                           |
| `chatCurrentChannel`          | `uiStore.activeChannel` (or router state) |
| `chatChannels`                | `channelsStore`                           |
| `chatMessages`                | `messagesStore`                           |
| `unreadCounts`, `channelLastSeen` | `unreadStore`                         |
| `channelSortTs`               | derived in `channelsStore.list()`         |
| `channelHistory`              | `shell/router.js` (history stack)         |
| `channelAgentActive`          | `presenceStore`                           |
| `channelPlanMode`             | `presenceStore`                           |
| `channelTodos`                | `tasksStore`                              |
| `fileTreeData`, `filesChangesData` | `filesStore`                         |
| `filesCurrentPath`, `filesCurrentView`, `filesChannelId` | **`Channel.viewState`** (NOT a store) |
| `terminalHistoryMap`          | `terminalStore`                           |
| `terminalCurrentBlock`, `terminalRunning`, `terminalCmdHistory`, `terminalCompletions*` | **`Channel.viewState`** |
| `complicationState`           | `complicationsStore`                      |
| `browserTabs`, `activeBrowserTab` | `Channel.viewState` (per-device tabs: revisit) |
| `pendingFiles`                | `Channel.viewState` (per-channel composer) |
| `currentTab`, `consoleState`, `_openDropdown` | `uiStore`                  |
| `deviceDown`                  | derived from `devicesStore` + `transport/e2ee.js` status |

Everything prefixed `_` in v1 (`_pendingChannelId`, `_navigatingHistory`,
`_filesSelfHealed`, `_unreadHighlightLastSeen`) is a smell that gets
designed out: pending channel becomes a router concern; navigation flag
becomes a router-scoped boolean; self-heal disappears once `filesPath` is
per-channel; unread highlight becomes `ConsoleView` instance state.
