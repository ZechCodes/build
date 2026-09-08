// The inbox view owns the meaning of its count; the shell owns how that count
// appears while the rail is away. This small channel keeps those two jobs
// separate without making the shell inspect painted rows.

let count = 0;
const subscribers = new Set();

export function publishInboxAttentionCount(next) {
  count = Math.max(0, Math.floor(Number(next) || 0));
  subscribers.forEach((subscriber) => subscriber(count));
}

export function subscribeInboxAttentionCount(subscriber) {
  subscribers.add(subscriber);
  subscriber(count);
  return () => subscribers.delete(subscriber);
}
