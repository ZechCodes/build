// Typed pub/sub. See planning/dashboard-v2/04-transport.md for vocabulary.
//
// Synchronous: emit returns after every subscriber ran. One throwing
// subscriber does not poison the rest — errors are logged and skipped.

import { log } from './log.js';

const plog = log('bus');
const listeners = new Map();

export const bus = {
  on(type, fn) {
    let set = listeners.get(type);
    if (!set) {
      set = new Set();
      listeners.set(type, set);
    }
    set.add(fn);
    return () => set.delete(fn);
  },

  emit(type, payload) {
    const set = listeners.get(type);
    if (!set) return;
    for (const fn of set) {
      try {
        fn(payload);
      } catch (err) {
        plog.error(`listener threw for ${type}`, err);
      }
    }
  },
};
