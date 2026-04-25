// Subscription plumbing shared by every domain store.
//
// Each store creates its own `{ subscribe, notify }` pair and exposes
// `subscribe` publicly. Errors in subscribers are logged and skipped so one
// broken view does not block others.

import { log } from './log.js';

const plog = log('store');

export function makeSubscribable(scope = 'unknown') {
  const subs = new Set();
  return {
    subscribe(fn) {
      subs.add(fn);
      return () => subs.delete(fn);
    },
    notify(event) {
      for (const fn of subs) {
        try {
          fn(event);
        } catch (err) {
          plog.error(`${scope} subscriber threw`, err);
        }
      }
    },
  };
}
