import { bytesOfBase64 } from "./bodyPages.js";
import { threadGenerationParam } from "./threadSync.js";
import { requestPriorityFields } from "./readRequests.js";

// One mebibyte keeps the JSON/base64 reply comfortably below the 8 MiB
// reassembly limit. A pre-paging bridge gets the original whole-file call.
export const THREAD_ATTACHMENT_PAGE_BYTES = 1_048_576;

const wholeAttachment = async (load, path) => {
  const answer = await load(path);
  return { ...answer, pages: [answer.content_b64 || ""] };
};

function pageProgress(answer, offset) {
  const count = bytesOfBase64(answer.content_b64 || "").length;
  const size = Number(answer.size);
  if (!count && size > offset) throw new Error("Incomplete attachment");
  return { count, size };
}

export async function readThreadAttachment(load, path, canPage) {
  if (!canPage) return wholeAttachment(load, path);
  const pages = [];
  let offset = 0;
  let first;
  for (;;) {
    const answer = await load(path, offset, THREAD_ATTACHMENT_PAGE_BYTES);
    first ||= answer;
    const { count, size } = pageProgress(answer, offset);
    pages.push(answer.content_b64 || "");
    offset += count;
    if (!Number.isFinite(size) || offset >= size || !count) break;
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
