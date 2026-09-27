// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { createChatRepository } from "../src/core/chatRepository.js";
import { createViewingContext } from "../src/core/viewingContext.js";
import { composerHtml } from "../src/core/composer.js";
import { wireThreadComposer } from "../src/core/thread.js";
import { mountGitPane } from "../src/core/gitPane.js";
import { renderFilesTab } from "../src/views/files.js";
import { patchFor, worktreeOf } from "./gitWireFixture.js";

const base64 = (text) => Buffer.from(text, "utf8").toString("base64");
const address = { entityId: "run-1", agentId: "agent-1", conversationId: "conversation-1" };

const settle = async () => {
  for (let index = 0; index < 12; index += 1) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
};

function messaging(viewingContext, call = vi.fn(async () => ({ posted_sequence: 9 }))) {
  let operation = 0;
  const repository = createChatRepository({
    scope: { deviceId: "device-1" },
    call,
    viewingContext,
    createOperationId: () => `operation-${++operation}`,
  });
  repository.configureCapabilities({ api_version: "2.0.0", capabilities: ["messages.context"], message_context: { version: 1 } });
  const controller = repository.controller(address);
  return {
    call,
    repository,
    send: (message) => controller.post(controller.captureSubmission(message)),
  };
}

describe("viewing context across SPA surfaces and outgoing messages", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("sends the open file and selected editor excerpt from Files", async () => {
    const viewingContext = createViewingContext();
    const host = document.createElement("div");
    document.body.appendChild(host);
    const files = renderFilesTab(host, {
      scope: { run_id: "run-1" },
      viewingContext,
      callRpc: async (method, params) => {
        if (method === "fs.tree") return { path: "", entries: [{ name: "README.md", kind: "file", size: 16 }] };
        if (method === "fs.read") return {
          mime: "text/markdown", size: 16, editable: true, encoding: "utf-8", revision: "r1",
          content_b64: base64("# Title\nselected words\n"),
        };
        throw new Error(`unexpected ${method}: ${params.path}`);
      },
    });
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    host.querySelector(".ffile").click();
    await vi.waitFor(() => expect(host.querySelector('[data-file-mode="edit"]')).toBeTruthy());
    host.querySelector('[data-file-mode="edit"]').click();
    const editor = host.querySelector(".file-editor");
    const start = editor.value.indexOf("selected");
    editor.setSelectionRange(start, start + "selected words".length);
    editor.dispatchEvent(new Event("select", { bubbles: true }));

    const chat = messaging(viewingContext);
    await chat.send({ body: "revise this" });

    expect(chat.call).toHaveBeenCalledWith("thread.post", expect.objectContaining({
      body: "revise this",
      viewing_context: {
        version: 1,
        items: [
          { kind: "file", path: "README.md" },
          { kind: "selection", path: "README.md", text: "selected words" },
        ],
      },
    }));
    files.dispose();
    chat.repository.dispose();
  });

  it("freezes commit context for a suggested-option reply before navigation changes it", async () => {
    const viewingContext = createViewingContext();
    const chat = messaging(viewingContext);
    viewingContext.set({ kind: "commit", sha: "a".repeat(40) });
    const controller = chat.repository.controller(address);
    const reply = controller.captureSubmission({ option_reply: { message_id: "message-4", option_ids: ["fix"] } });
    viewingContext.set({ kind: "file", path: "src/later.js" });
    await controller.post(reply);

    expect(chat.call).toHaveBeenCalledWith("thread.post", expect.objectContaining({
      option_reply: { message_id: "message-4", option_ids: ["fix"] },
      viewing_context: { version: 1, items: [{ kind: "commit", sha: "a".repeat(40) }] },
    }));
    chat.repository.dispose();
  });

  it("puts the same visible uncommitted-diff context on every review message", async () => {
    const viewingContext = createViewingContext();
    const tree = worktreeOf({ "src/a.js": "first", "src/b.js": "second" });
    const geometry = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function rect() {
      if (this.classList.contains("cdetail-host")) return { top: 0, bottom: 600, left: 0, right: 800 };
      if (this.classList.contains("file")) return { top: 100, bottom: 300, left: 0, right: 800 };
      return { top: 0, bottom: 0, left: 0, right: 0 };
    });
    const calls = [];
    const callRpc = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "git.status") return tree.status();
      if (method === "git.diff") return tree.diff(params);
      if (method === "git.log") return { branch: "main", commits: [], more: false };
      if (method === "run.request_changes") return { ok: true };
      return {};
    });
    const host = document.createElement("div");
    document.body.appendChild(host);
    const pane = mountGitPane(host, { scope: { run_id: "run-1" }, callRpc, viewingContext });
    await settle();
    host.querySelector(".cdetail-host").dispatchEvent(new Event("scroll"));
    await settle();
    for (const file of host.querySelectorAll(".file")) {
      file.querySelector(".fcmt").click();
      document.querySelector(".cp-input").value = `review ${file.dataset.key}`;
      document.querySelector(".cp-save").click();
      await settle();
    }
    host.querySelector(".csbox-actions .btn:not(.caret)").click();
    await settle();

    const request = calls.find((call) => call.method === "run.request_changes");
    expect(request.params.messages).toHaveLength(2);
    expect(request.params.messages.every((message) =>
      JSON.stringify(message.viewing_context) === JSON.stringify({ version: 1, items: [
        { kind: "diff", path: "src/a.js", mode: "uncommitted" },
        { kind: "diff", path: "src/b.js", mode: "uncommitted" },
      ] }),
    )).toBe(true);
    pane.dispose();
    geometry.mockRestore();
  });

  it("adds context to Ask agent to commit without changing its message text", async () => {
    const viewingContext = createViewingContext();
    const tree = worktreeOf({ "src/a.js": "first" });
    const calls = [];
    const callRpc = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "git.status") return tree.status();
      if (method === "git.diff") return tree.diff(params);
      if (method === "git.log") return { branch: "main", commits: [], more: false };
      return {};
    });
    const host = document.createElement("div");
    document.body.appendChild(host);
    const pane = mountGitPane(host, {
      scope: { run_id: "run-1" }, callRpc, viewingContext,
      agentCommitOptions: [{ id: "agent_commit", menuLabel: "Ask agent to commit" }],
    });
    await settle();
    viewingContext.set({ kind: "file", path: "src/a.js" });
    host.querySelector(".csbox-actions .caret").click();
    host.querySelector('[data-action="agent_commit"]').click();
    await settle();

    const request = calls.find((call) => call.method === "run.message");
    expect(request.params.message).toContain("Commit all outstanding changes");
    expect(request.params.viewing_context).toEqual({ version: 1, items: [{ kind: "file", path: "src/a.js" }] });
    expect(calls.some((call) => call.method === "git.commit")).toBe(false);
    pane.dispose();
  });

  it("preserves diff context during composer-focused polls but refreshes on viewer scroll", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let secondTop = 200;
    const geometry = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function rect() {
      if (this.classList.contains("cdetail-host")) return { top: 0, bottom: 600, left: 0, right: 800 };
      if (this.classList.contains("file")) {
        const top = this.dataset.key.endsWith("b.js") ? secondTop : 100;
        return { top, bottom: top + 100, left: 0, right: 800 };
      }
      return { top: 0, bottom: 0, left: 0, right: 0 };
    });
    const viewingContext = createViewingContext();
    const tree = worktreeOf({ "src/a.js": "first", "src/b.js": "second" });
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.status") return tree.status();
      if (method === "git.diff") return tree.diff(params);
      if (method === "git.log") return { branch: "main", commits: [], more: false };
      return {};
    });
    const host = document.createElement("div");
    const composer = document.createElement("div");
    composer.className = "composer";
    composer.innerHTML = "<textarea></textarea>";
    document.body.append(host, composer);
    const pane = mountGitPane(host, { scope: { run_id: "run-1" }, callRpc, viewingContext });
    await vi.advanceTimersByTimeAsync(100);
    expect(viewingContext.snapshot().items).toHaveLength(2);
    composer.querySelector("textarea").focus();
    secondTop = 900;
    await vi.advanceTimersByTimeAsync(2_000);
    expect(viewingContext.snapshot().items).toHaveLength(2);
    host.querySelector(".cdetail-host").dispatchEvent(new Event("scroll"));
    await vi.advanceTimersByTimeAsync(100);
    expect(viewingContext.snapshot().items).toEqual([{ kind: "diff", path: "src/a.js", mode: "uncommitted" }]);
    pane.dispose();
    geometry.mockRestore();
    vi.useRealTimers();
  });

  it("keeps ordinary sends compatible with a legacy bridge", async () => {
    const viewingContext = createViewingContext();
    viewingContext.set({ kind: "file", path: "src/a.js" });
    const call = vi.fn(async () => ({ posted_sequence: 2 }));
    const repository = createChatRepository({ scope: {}, call, viewingContext });
    const controller = repository.controller(address);
    await controller.post(controller.captureSubmission({ body: "still works" }));

    expect(call).toHaveBeenCalledWith("thread.post", expect.not.objectContaining({ viewing_context: expect.anything() }));
    repository.dispose();
  });
});


describe("composer context groups", () => {
  it("retains expansion during updates and dismisses every file with one click", () => {
    const viewingContext = createViewingContext();
    viewingContext.set({ version: 1, items: [
      { kind: "diff", path: "a.js", mode: "all" },
      { kind: "diff", path: "b.js", mode: "all" },
      { kind: "diff", path: "c.js", mode: "all" },
    ] });
    document.body.innerHTML = composerHtml({ inputId: "ci", sendId: "cs", hintId: "ch", placeholder: "Message" });
    const control = wireThreadComposer(document.body, {
      ids: { input: "ci", send: "cs", hint: "ch" }, viewingContext,
      readDraft: () => "", writeDraft: () => {}, onSubmit: async () => {},
    });
    const tray = document.querySelector(".composer-context");
    expect(tray.querySelectorAll("button")).toHaveLength(1);
    tray.querySelector("summary").click();
    viewingContext.setSelection([{ kind: "selection", path: "a.js", text: "keep" }]);
    expect(tray.querySelector(".viewing-context-files").open).toBe(true);
    tray.querySelector(".viewing-context-remove").click();
    expect(tray.querySelector(".viewing-context-group")).toBeNull();
    expect(viewingContext.snapshot().items).toEqual([{ kind: "selection", path: "a.js", text: "keep" }]);
    tray.querySelector("button").click();
    expect(viewingContext.snapshot()).toBeUndefined();
    expect(tray.hidden).toBe(true);
    control.dispose();
  });
});
