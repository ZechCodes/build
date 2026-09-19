// A message the reader has just sent, between the press and the wire.
//
// The rail draws it the moment send is pressed, and the conversation carries it
// back a round trip later. What happens in between is here: where the message
// is stood up, what the post's receipt does to it, and what a post the browser
// stopped waiting for leaves behind.
//
// There are two places it can be stood up in, and which one depends on whether
// the conversation exists yet. An agent that exists has a record on disk, and
// the message goes into it under the operation carrying it — one conversation,
// one store, every tab and surface on it painting the same thing. An agent
// being created has no record to write to, so its first message is drawn over
// the window out of the optimistic store until `agent.add` answers.

import {
  acknowledgeProvisionalMessage,
  threadCacheAddress,
  withdrawProvisionalMessage,
} from "./conversationCache.js";
import { replyOrNothing } from "./session.js";
import { MUTATION_THREAD_PAGE } from "./thread.js";

/** What the panel says about a message the post has answered for: the daemon's
 *  own word where it gave one, else "on its way" from a bridge that tracks
 *  delivery and "sent" from one that does not. */
const provisionalDeliveryStatus = (submission, posted) => {
  if (posted?.operation_status === "uncertain") return "uncertain";
  return submission.threadPostOperations ? "queued" : "sent";
};

/** The agent that does not exist yet has no record to write to: its first
 *  message is drawn over the window until `agent.add` answers and the
 *  conversation it is in is a real one. */
export const provisionalMessageEntry = (messageKey, message) => ({
  type: "message",
  data: {
    role: "user",
    sequence: messageKey,
    body: message.body || "",
    attachments: message.attachments || [],
    created_at: new Date().toISOString(),
    delivery_status: "queued",
  },
});

/** That drawn-over message, under the sequence the post was written at — or
 *  gone, where no receipt said one. */
export const rekeyPostedMessage = (handle, messageKey, provisional, posted, submission) => {
  const sequence = (posted && posted.posted_sequence) ?? null;
  if (sequence === null) handle.drop(messageKey);
  else handle.rekey(messageKey, String(sequence), {
    ...provisional,
    data: { ...provisional.data, sequence, delivery_status: provisionalDeliveryStatus(submission, posted) },
  });
};

/** Where a submission's conversation is held on disk, or null while there is
 *  no conversation yet — the first message on a work item is the one that
 *  makes it. */
export const conversationRecordAddress = (cacheScope, address) => {
  const scoped = address?.entityId ? cacheScope?.address({ entityId: address.entityId }) : null;
  if (!scoped) return null;
  return threadCacheAddress({ ...scoped, agentId: address.agentId, conversationId: address.conversationId });
};

/** The post was taken: the message the reader is looking at gets the sequence
 *  it was written at, so the item that arrives carrying it replaces the
 *  stand-in rather than joining it. No receipt at all means nothing was
 *  written, and the stand-in goes. */
export const settleProvisional = (address, submission, posted) => {
  const sequence = (posted && posted.posted_sequence) ?? null;
  if (sequence === null) return withdrawProvisionalMessage(address, submission.operationId);
  return acknowledgeProvisionalMessage(
    address,
    submission.operationId,
    sequence,
    provisionalDeliveryStatus(submission, posted),
  );
};

/** Put the message on the conversation, and settle the stand-in under the
 *  sequence the daemon gave it.
 *
 *  A post the browser stopped waiting for is not a refusal: the turn is
 *  durable on the daemon's side, and reverting it here would hand the draft
 *  back and have the human send the same turn twice. The stand-in holds
 *  instead, and the item carrying the real message replaces it — it names the
 *  operation that made it, which is how the record knows it for the reader's
 *  own words coming back (core/thread.js). */
export const postSubmission = async (controller, submission, settle) => {
  let posted;
  try {
    posted = await replyOrNothing(controller.post(submission, MUTATION_THREAD_PAGE));
  } catch (error) {
    if (!error.uncertain) throw error;
    posted = null;
  }
  if (posted) settle(posted);
};
