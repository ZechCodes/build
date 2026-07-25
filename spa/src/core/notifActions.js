// The pure action model behind a notification card. A calm card carries only its
// single open action; a warn-tone card (a run/plan that is blocked/failed/idle/
// interrupted) also offers a jump straight into its live PTY.
//
// It does NOT offer to message the agent. The Build UI is a layer over a running
// agent session: a page that touches an agent owns a conversation with it, and a
// page that does not has no business pretending to. This list touches none — it
// says which entities need you — so its job is to take you to the page that does.
// Kept DOM/app-free so the view maps descriptors to buttons and the split is
// unit-testable.
export function notifActionsFor(kind, event) {
  const actions = [{ kind: "open", label: event.action, primary: !!event.primary }];
  if (event.tone === "warn") actions.push({ kind: "agent", label: "Open agent" });
  return actions;
}
