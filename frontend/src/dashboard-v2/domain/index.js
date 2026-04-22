// Domain barrel + init hook.
//
// Wave 1: initStores() is a no-op — stores are module-level singletons and
// have no subscriptions to register yet. Wave 2 will add bus bindings in
// each store file; initStores() stays as the single call site main.js uses
// so the bootstrap sequence is stable across waves.

export { devicesStore } from './devices-store.js';
export { channelsStore } from './channels-store.js';
export { messagesStore } from './messages-store.js';
export { activityStore } from './activity-store.js';
export { presenceStore } from './presence-store.js';
export { unreadStore } from './unread-store.js';
export { filesStore } from './files-store.js';
export { terminalStore } from './terminal-store.js';
export { tasksStore } from './tasks-store.js';
export { complicationsStore } from './complications-store.js';
export { uiStore } from './ui-store.js';
export { currentToolStore } from './current-tool-store.js';

export function initStores() {
  // reserved for Wave 2 wiring
}
