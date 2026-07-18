// The pure action model behind a notification card. A calm card carries only its
// single open action; a warn-tone card (a run/plan that is blocked/failed/idle/
// interrupted — every warn state is messageable) also carries act-now controls:
// message the agent, and drop straight into its live PTY. Kept DOM/app-free so
// the view can map these descriptors to buttons and the split is unit-testable.
export function notifActionsFor(kind, event) {
  const actions = [{ kind: "open", label: event.action, primary: !!event.primary }];
  if (event.tone === "warn") {
    actions.push({ kind: "message", label: "Message agent" });
    actions.push({ kind: "agent", label: "Open agent" });
  }
  return actions;
}
