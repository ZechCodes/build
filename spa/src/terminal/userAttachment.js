// Recovery belongs to the terminal resource, so an unchanged cached console
// keeps its pane while the terminal channel is restored.
import { assertAttachmentCurrent } from "./attachmentLifetime.js";
import { isTerminalSocketLost } from "./session.js";

export async function attachUserTerminal(manager, termId, scope, options) {
  for (;;) {
    assertAttachmentCurrent(options);
    try { return await manager.attachTerminal(termId, scope, options); }
    catch (error) {
      if (!isTerminalSocketLost(error) || error.reason === "closed") throw error;
      // The next attach waits on the socket owner's connection promise. A
      // collapsed pane cancels that wait instead of participating in recovery.
    }
  }
}
