import { bytesOfBase64 } from "./bodyPages.js";
import { threadGenerationParam } from "./threadSync.js";
import { requestPriorityFields } from "./readRequests.js";

// One mebibyte keeps the JSON/base64 reply comfortably below the 8 MiB
// reassembly limit. Legacy first reads can be capped too: their full size
// metadata, rather than the greeting, decides whether another range is owed.
export const THREAD_ATTACHMENT_PAGE_BYTES = 1_048_576;

function attachmentSize(answer) {
  if (answer.size === undefined) return null;
  if (!Number.isSafeInteger(answer.size) || answer.size < 0) throw new Error("Invalid attachment size");
  return answer.size;
}

function validatePageMetadata(answer, first, path, offset) {
  if (answer.path !== undefined && answer.path !== path) throw new Error("Unexpected attachment path");
  if (answer.offset !== undefined && answer.offset !== offset) throw new Error("Unexpected attachment offset");
  if (offset > 0 && answer.offset === undefined) throw new Error("Missing attachment offset");
  if (answer.size !== first.size || answer.mime !== first.mime) throw new Error("Attachment metadata changed during read");
}

function pageProgress(answer, offset, size) {
  const count = bytesOfBase64(answer.content_b64 || "").length;
  if (size === null) return count;
  if (offset + count > size) throw new Error("Attachment page exceeds its size");
  if (!count && offset < size) throw new Error("Incomplete attachment");
  return count;
}

export async function readThreadAttachment(load, path, canPage) {
  const first = canPage ? await load(path, 0, THREAD_ATTACHMENT_PAGE_BYTES) : await load(path);
  const size = attachmentSize(first);
  if (canPage && (size === null || first.offset === undefined)) throw new Error("Missing attachment range metadata");
  const pages = [];
  let offset = 0;
  let answer = first;
  for (;;) {
    validatePageMetadata(answer, first, path, offset);
    const count = pageProgress(answer, offset, size);
    pages.push(answer.content_b64 || "");
    offset += count;
    if (size === null || offset === size) break;
    answer = await load(path, offset, THREAD_ATTACHMENT_PAGE_BYTES);
  }
  return { ...first, pages };
}

export function fetchThreadAttachment(call, entityId, path, canPage, identity = {}) {
  return readThreadAttachment((name, offset, length) => call("thread.attachment", {
    entity_id: entityId, path: name, ...threadGenerationParam(identity.threadId),
    ...(identity.agentId ? { agent_id: identity.agentId, conversation_id: identity.conversationId } : {}),
    ...(offset === undefined ? {} : { offset, length }),
  }, requestPriorityFields("background")), path, canPage);
}
