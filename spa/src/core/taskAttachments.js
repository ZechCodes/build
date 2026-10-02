// Task attachments are offered from cached pages before the bridge greets.
// A bridge that does not know the upload verb refuses it; the composer keeps
// its draft and shows a plain sentence instead of protocol wording.

import { commandRefusalMessage } from "./commandRefusal.js";

export const taskAttachmentRefusal = (error) =>
  commandRefusalMessage(error, "This bridge does not support task attachments.");
