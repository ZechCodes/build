# 04 — Transport & Event Bus

The transport layer speaks the wire protocol. The bus speaks the domain.
Between them sits one small module per transport that translates.

## The bus (`core/bus.js`)

```js
const listeners = new Map();    // type → Set<fn>

export const bus = {
  on(type, fn) {
    let set = listeners.get(type);
    if (!set) { set = new Set(); listeners.set(type, set); }
    set.add(fn);
    return () => set.delete(fn);
  },
  emit(type, payload) {
    const set = listeners.get(type);
    if (!set) return;
    for (const fn of set) {
      try { fn(payload); }
      catch (err) { console.error(`[bus] ${type} listener threw`, err); }
    }
  },
};
```

Characteristics:

- **Synchronous.** Emit returns after every listener ran. Predictable
  ordering, no race conditions between subscribers.
- **Typed payloads.** Each event type has a fixed payload shape documented
  below. No `any`. Add a new event; don't overload an existing one.
- **No wildcard subscriptions.** If you need to listen to many events, list
  them.
- **Errors do not poison the bus.** A throwing subscriber is logged and
  skipped.

## Event vocabulary (domain events)

These are the only events that cross layers. Grouped by domain.

### Device lifecycle
- `device.added`           `{ device }`
- `device.status_changed`  `{ deviceId, status }`
- `device.removed`         `{ deviceId }`
- `device.e2ee_ready`      `{ deviceId }`

### E2EE connection
- `e2ee.connecting`    `{ deviceId }`
- `e2ee.connected`     `{ deviceId }`
- `e2ee.disconnected`  `{ deviceId, reason }`
- `e2ee.error`         `{ deviceId, error }`

### Channels
- `channel.upserted`   `{ deviceId, channel }`
- `channel.removed`    `{ deviceId, channelId }`
- `channel.updated`    `{ channelId, patch }`

### Messages
- `message.received`   `{ channelId, msg }`
- `message.bulk`       `{ channelId, msgs }`
- `message.read`       `{ channelId, msgIds }`

### Agent activity
- `agent.started`       `{ channelId }`
- `agent.stopped`       `{ channelId }`
- `agent.restarted`     `{ channelId }`
- `agent.reasoning`     `{ channelId, text, at }`
- `agent.tool_use`      `{ channelId, toolUseId, name, input, at }`
- `agent.tool_result`   `{ channelId, toolUseId, isError, content, at }`
- `agent.activity_end`  `{ channelId }`
- `agent.error`         `{ channelId, message, fatal }`
- `agent.plan_mode`     `{ channelId, planMode }`
- `agent.todo_write`    `{ channelId, todos }`   (derived from tool.use)
- `agent.file_changes`  `{ channelId, paths }`
- `agent.state_update`  `{ channelId, patch }`

### Interactions
- `interaction.requested`  `{ channelId, interactionId, kind, question, options, allowFreeform, plan }`
- `interaction.resolved`   `{ channelId, interactionId, selection }`

### Harness
- `harness.list`           `{ deviceId, harnesses }`

### Files (device-side)
- `files.list_result`      `{ channelId, path, entries }`
- `files.changes_result`   `{ channelId, changes }`
- `files.read_result`      `{ channelId, path, content, partial }`

### Terminal (device-side)
- `terminal.output`        `{ channelId, text }`
- `terminal.complete`      `{ channelId, exitCode }`
- `terminal.completions`   `{ channelId, candidates }`

### Complications
- `complications.updated`  `{ channelId, complications }`

### SSE
- `sse.connected`          `{}`
- `sse.disconnected`       `{}`

### User intent (emitted by views, consumed by transport)
- `intent.send_message`    `{ channelId, text, attachments }`
- `intent.stop_agent`      `{ channelId }`
- `intent.plan_mode_set`   `{ channelId, planMode }`
- `intent.update_channel`  `{ channelId, patch }`
- `intent.terminal_exec`   `{ channelId, cmd }`
- `intent.files_list`      `{ channelId, path }`
- `intent.files_read`      `{ channelId, path }`
- `intent.resolve_complication` `{ channelId, complicationId, action }`

## E2EE dispatcher

Takes one `BuildE2EE` instance per device. Listens to its 32 wire events.
Translates each to one or more domain bus events. That's it.

```js
// transport/e2ee-dispatcher.js
import { bus } from '../core/bus.js';

export function bindE2EEDispatcher(instance, deviceId) {
  instance.addEventListener('channels', (evt) => {
    for (const ch of evt.detail.upserted ?? []) bus.emit('channel.upserted', { deviceId, channel: ch });
    for (const id of evt.detail.removed ?? [])  bus.emit('channel.removed',  { deviceId, channelId: id });
  });

  instance.addEventListener('message', (evt) => {
    bus.emit('message.received', { channelId: evt.detail.channel_id, msg: evt.detail });
  });

  instance.addEventListener('messages', (evt) => {
    bus.emit('message.bulk', { channelId: evt.detail.channel_id, msgs: evt.detail.messages });
  });

  instance.addEventListener('agent_event', (evt) => {
    const { channel_id, event_type, event } = evt.detail;
    switch (event_type) {
      case 'chat.response':      bus.emit('message.received',    { channelId: channel_id, msg: normalizeAgentMsg(event) }); break;
      case 'activity.delta':     bus.emit('agent.reasoning',     { channelId: channel_id, text: event.delta?.text ?? '', at: event.created_at }); break;
      case 'tool.use':           dispatchToolUse(channel_id, event); break;
      case 'tool.result':        bus.emit('agent.tool_result',   { channelId: channel_id, ...event }); break;
      case 'activity.end':       bus.emit('agent.activity_end',  { channelId: channel_id }); break;
      case 'interaction.request': bus.emit('interaction.requested', { channelId: channel_id, ...event }); break;
      case 'agent.error':        bus.emit('agent.error',         { channelId: channel_id, ...event }); break;
      case 'agent.state_update': bus.emit('agent.state_update',  { channelId: channel_id, patch: event }); break;
      case 'agent.file_changes': bus.emit('agent.file_changes',  { channelId: channel_id, paths: event.paths ?? [] }); break;
    }
  });

  // ...one addEventListener per wire event type
}

function dispatchToolUse(channelId, event) {
  bus.emit('agent.tool_use', { channelId, toolUseId: event.tool_use_id, name: event.name, input: event.input, at: event.created_at });
  if (event.name === 'TodoWrite' && event.input?.todos) {
    bus.emit('agent.todo_write', { channelId, todos: event.input.todos });
  }
}
```

The dispatcher has **no state**, **no DOM**, and **no knowledge of stores**.
Stores subscribe to `bus` and react.

## Intent dispatcher (reverse direction)

Views emit `intent.*` events. The intent dispatcher subscribes and calls
the right E2EE method.

```js
// transport/intent-dispatcher.js
import { bus } from '../core/bus.js';
import { e2eeForChannel } from './e2ee-pool.js';

bus.on('intent.send_message', ({ channelId, text, attachments }) => {
  e2eeForChannel(channelId)?.sendMessage(channelId, text, attachments);
});
bus.on('intent.stop_agent', ({ channelId }) => {
  e2eeForChannel(channelId)?.stopAgent(channelId);
});
// ...
```

This means the view code for the send button is two lines:

```js
bus.emit('intent.send_message', { channelId: this.channel.id, text, attachments });
```

No direct E2EE import, no `getActiveE2EE()` lookup, no transport coupling.

## Transport coordinator (reconnect state)

One module owns reconnect state so SSE reconnect and E2EE retry don't
double-fire (v1 bug).

```js
// transport/coordinator.js
let reconnecting = false;
bus.on('sse.connected', async () => {
  if (reconnecting || e2eePool.size === 0) return;
  reconnecting = true;
  try {
    await e2eePool.teardownAll();
    await devicesRest.refresh();
    await e2eePool.connectReady();
  } finally { reconnecting = false; }
});
```

## Testing transport

- **Dispatcher**: feed synthetic wire events to a fake `BuildE2EE`, assert
  bus emissions. (v1 has `window.__test_bindE2EEEvents` — v2 just
  exports `bindE2EEDispatcher` directly.)
- **Intent**: spy on `e2eeForChannel` returned object, emit an intent
  event, assert method called.
- **Coordinator**: mock `e2eePool` and `devicesRest`, emit `sse.connected`,
  assert teardown/reconnect ordered correctly.
