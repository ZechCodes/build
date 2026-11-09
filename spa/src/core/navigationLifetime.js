// Observers of accepted navigation, after leave guards and before page mounting.
// Surfaces opt in; ordinary sheets keep their existing dismissal behavior.
const listeners = new Set();

export function onNavigationCommitted(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function notifyNavigationCommitted() {
  for (const listener of [...listeners]) listener();
}
