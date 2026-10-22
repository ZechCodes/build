import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

/** Exercise the production reader and tile wiring against supplied RPC frames. */
export async function mountPagedAgentAttachments(page, basePath, payload, canPage) {
  await mountLayout(page, '<main id="attachment-thread"></main>', {
    basePath,
    styles: "body{display:block;overflow:auto}#attachment-thread{max-width:800px;margin:32px auto;padding:16px}",
  });
  await loadBrowserModules(page, {
    thread: "src/core/thread.js", pages: "src/core/threadAttachmentPages.js", cache: "src/core/localCache.js",
  }, basePath);
  await page.evaluate(async ({ payload, canPage }) => {
    const { thread, pages, cache } = window.__layoutModules;
    document.documentElement.dataset.theme = "dark";
    const address = { deviceId: "paged-attachments", entityId: payload.entity_id, kind: "thread", sub: payload.agent_id };
    await cache.writeCached(address, payload.thread);
    const state = thread.createThreadState({ ownerId: payload.entity_id });
    const root = document.querySelector("#attachment-thread");
    root.innerHTML = thread.threadHtml((await cache.readCached(address)).value, { threadState: state });
    window.__attachmentReadCalls = [];
    window.__pagedAttachmentState = state;
    const call = async (method, params, options) => {
      window.__attachmentReadCalls.push({ method, params, options });
      const file = payload.files.find((file) => file.path === params.path);
      const reply = params.offset === undefined ? file.default_reply
        : file.ranged_replies.find((reply) => reply.result.offset === params.offset);
      if (!reply?.ok) throw new Error("Unexpected attachment range in fixture");
      return reply.result;
    };
    thread.wireThreadAttachments(root, (path) => pages.fetchThreadAttachment(
      call, payload.entity_id, path, canPage, { agentId: payload.agent_id },
    ), state);
  }, { payload, canPage });
}
