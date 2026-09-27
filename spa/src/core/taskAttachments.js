// Whether a bridge can carry files on a task, asked in one place.
//
// Another one-line question with a large blast radius, for the same reason
// core/trackerPush.js is a module rather than an expression: what turns on it
// is whether a surface offers a PRESS, and a press that cannot work is worse
// than no press at all. Somebody drags a screenshot onto the composer, watches
// it sit in the tray, files the task, and finds the picture is not on it. The
// tab told them afterwards (core/taskComposer.js says so out loud), but the
// only good version of that sentence is the one nobody has to read.
//
// `session.hello` answers it outright from 1.8: `tasks.attachments` is stated
// by a bridge that has `tasks.attach`, and absent from one that does not. A
// greeting that states nothing readable is read as carrying nothing, which is
// the safe direction — the paperclip is not drawn, everything else about the
// tab works, and a reader with an older bridge is simply not offered something
// that machine cannot do.
//
// The question is asked at the moment a surface is about to draw the press,
// never cached: a greeting lands after a tab mounts, and a paperclip that stays
// hidden until the next navigation is a capability nobody gets the benefit of.

import { bridgeCapabilities } from "./changeEvents.js";

/**
 * Whether this device's bridge says it can carry files on a task.
 *
 * Read defensively, like the push kinds beside it: a capability object in a
 * shape this build does not expect costs the tab its paperclip and nothing
 * else.
 */
export const carriesTaskAttachments = (deviceId) =>
  bridgeCapabilities(deviceId)?.tasks?.attachments === true;
