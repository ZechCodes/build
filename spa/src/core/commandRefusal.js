import { messageOf } from "./text.js";
import { notifyError } from "./notify.js";

/** Controls stay available before a greeting. If an older bridge refuses a
 *  command, explain the missing feature in the surface's words. */
export function commandRefusalMessage(error, unsupportedMessage) {
  const message = messageOf(error);
  const code = error?.code || error?.error_code;
  if (code === "unknown_method" || /unknown method|method not found/i.test(message)) {
    return unsupportedMessage;
  }
  return message;
}

/** Unsupported features are the notice itself, never hidden in its details. */
export function notifyCommandFailure(error, summary, unsupported) {
  const reason = commandRefusalMessage(error, unsupported);
  if (reason === unsupported) notifyError(reason);
  else notifyError(summary, reason);
}
