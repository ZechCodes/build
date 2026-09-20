// Where the conversation sits relative to the work, as a width.
//
// One number, and the two questions asked of it. It lives here rather than in
// core/agentRail.js because the rail is not the only thing that needs it: the
// shell asks whether the chat is covering the page before it puts it away on a
// navigation (#62), and a shell that reached into the rail for a layout fact
// would be importing a conversation to ask about a stylesheet.

/** The width the panel stops sitting beside the work and is laid over it
 *  instead (styles/shell.css, `@media (max-width: 760px)`). */
const PANEL_OVERLAYS_BELOW = 761;

/**
 * Whether the chat is laid OVER the page rather than beside it.
 *
 * Over it, a press that changes the page changes something the reader cannot
 * see — which is why a navigation puts the chat away at this width and leaves
 * it alone at any other.
 */
export const chatOverlaysPage = () => window.innerWidth < PANEL_OVERLAYS_BELOW;

/** Whether the panel is docked before anyone has said. Beside the work it is;
 *  over the work it is a card, because a docked panel there is the whole
 *  screen. */
export const panelDocksByDefault = () => window.innerWidth >= PANEL_OVERLAYS_BELOW;
