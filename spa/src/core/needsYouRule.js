// Which "Needs you" rule a machine's tasks are read by (#144), held in the
// cache so the Tasks tab and the inbox paint it before the bridge answers.
//
// The rule is a fact about the bridge: one that keeps `notify_user` on
// comments announces `tasks.commentUserNotifies`, and its tasks need the user
// only when assigned to them, or when an unread agent comment or created event
// asks them (core/trackerAttentionModel.js). A greeting writes it here; views
// read it with the records they paint, never from the live greeting, so a
// cold reload draws the same Needs you it will draw once connected.

import { mergeCachedAtomically, readCached } from "./localCache.js";

export const NEEDS_YOU_RULE_KIND = "needs-you-rule";

export const needsYouRuleAddress = (deviceId) => ({ deviceId, entityId: "", kind: NEEDS_YOU_RULE_KIND });

/** Write what a greeted bridge's capabilities say, when that changes the rule. */
export function rememberNeedsYouRule(deviceId, capabilities) {
  if (!deviceId) return Promise.resolve(false);
  const askedOnly = capabilities?.tasks?.commentUserNotifies === true;
  return mergeCachedAtomically(needsYouRuleAddress(deviceId), (held) =>
    (held?.askedOnly === askedOnly ? null : { askedOnly }));
}

/** Whether this machine's tasks are read by the narrow rule. A machine never
 *  greeted in this browser is read by the earlier one. */
export async function readNeedsYouRule(deviceId) {
  const record = await readCached(needsYouRuleAddress(deviceId));
  return record?.value?.askedOnly === true;
}
