/** @vitest-environment jsdom */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let mountWorkspaceRefPicker;
let writeCached;
let cacheScope;

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const scope = { workspace_id: "ws-1", source_id: "repo" };

const listing = {
  current: { kind: "branch", name: "main", full_ref: "refs/heads/main" },
  refs: [
    {
      kind: "branch",
      name: "main",
      full_ref: "refs/heads/main",
      current: true,
      upstream: "origin/main",
      ahead: 0,
      behind: 2,
    },
    {
      kind: "branch",
      name: "feature/search",
      full_ref: "refs/heads/feature/search",
      current: false,
      upstream: null,
      ahead: 0,
      behind: 0,
    },
    {
      kind: "branch",
      name: "remote-work",
      full_ref: "refs/remotes/origin/remote-work",
      current: false,
      remote: "origin",
      upstream: null,
      ahead: 0,
      behind: 0,
    },
    {
      kind: "branch",
      name: "diverged",
      full_ref: "refs/heads/diverged",
      current: false,
      upstream: "origin/diverged",
      ahead: 1,
      behind: 3,
    },
    { kind: "tag", name: "v2.0.0", full_ref: "refs/tags/v2.0.0", current: false },
    { kind: "tag", name: "v1.9.0", full_ref: "refs/tags/v1.9.0", current: false },
  ],
};

function refNames(host) {
  return [...host.querySelectorAll("[data-ref]")].map((row) => row.textContent);
}

async function mount({ checkoutError, refsResponse = listing, refsAnswers = [] } = {}) {
  const host = document.querySelector("#host");
  const callRpc = vi.fn(async (method) => {
    if (method === "git.refs" && refsAnswers.length) {
      const next = refsAnswers.shift();
      if (next instanceof Error) throw next;
      return next;
    }
    if (method === "git.refs") return refsResponse;
    if (method === "git.checkout_ref" && checkoutError) throw checkoutError;
    return {};
  });
  const onCheckout = vi.fn();
  const mounted = mountWorkspaceRefPicker(host, { scope, callRpc, cacheScope, onCheckout });
  await flush();
  if (refsResponse === listing) await vi.waitFor(() => expect(host.querySelector(".workspace-reftrigger-name").textContent).toBe("main"));
  return { host, callRpc, onCheckout, mounted };
}

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  ({ mountWorkspaceRefPicker } = await import("../src/core/workspaceRefPicker.js"));
  ({ writeCached } = await import("../src/core/localCache.js"));
  cacheScope = { address: ({ entityId, kind }) => ({ deviceId: "dev-1", entityId, kind }) };
  document.body.innerHTML = '<div id="host"></div>';
});

describe("workspace ref picker", () => {
  it("paints cached refs while the pull is absent", async () => {
    const address = cacheScope.address({ entityId: 'workspace:["ws-1","repo"]', kind: "refs" });
    await writeCached(address, listing);
    const { host, callRpc, mounted } = await mount({ refsResponse: new Promise(() => {}) });
    await vi.waitFor(() => expect(host.querySelector(".workspace-reftrigger-name").textContent).toBe("main"));
    expect(callRpc).not.toHaveBeenCalled();
    host.querySelector("[data-refpicker-toggle]").click();
    expect(callRpc).toHaveBeenCalledWith("git.refs", scope);
    expect(refNames(host).join(" ")).toContain("feature/search");
    mounted.dispose();
  });

  it("defers a stale refs read while hidden and refreshes on reveal", async () => {
    const address = cacheScope.address({ entityId: 'workspace:["ws-1","repo"]', kind: "refs" });
    await writeCached(address, listing);
    const fresh = { ...listing, current: { kind: "branch", name: "feature/search", full_ref: "refs/heads/feature/search" } };
    const { host, callRpc, mounted } = await mount({ refsResponse: fresh });
    mounted.setVisible(false);
    await writeCached(address, { ...listing, stale: true });
    await flush();
    expect(host.querySelector(".workspace-reftrigger-name").textContent).toBe("main");
    expect(callRpc).not.toHaveBeenCalled();

    mounted.setVisible(true);
    await vi.waitFor(() => expect(host.querySelector(".workspace-reftrigger-name").textContent).toBe("feature/search"));
    expect(callRpc.mock.calls.filter(([method]) => method === "git.refs")).toHaveLength(1);
    mounted.dispose();
  });

  it("defers a cold mount read when hidden before the cache answers", async () => {
    const host = document.querySelector("#host");
    const callRpc = vi.fn(async (method) => method === "git.refs" ? listing : {});
    const mounted = mountWorkspaceRefPicker(host, { scope, callRpc, cacheScope });
    mounted.setVisible(false);
    await flush();
    expect(callRpc).not.toHaveBeenCalled();

    mounted.setVisible(true);
    await vi.waitFor(() => expect(host.querySelector(".workspace-reftrigger-name").textContent).toBe("main"));
    expect(callRpc.mock.calls.filter(([method]) => method === "git.refs")).toHaveLength(1);
    mounted.dispose();
  });

  it("redraws only after the real cache announces a new refs record", async () => {
    const { host, mounted } = await mount({ refsResponse: new Promise(() => {}) });
    const address = cacheScope.address({ entityId: 'workspace:["ws-1","repo"]', kind: "refs" });
    await writeCached(address, { current: { kind: "tag", name: "v3", full_ref: "refs/tags/v3" }, refs: [{ kind: "tag", name: "v3", full_ref: "refs/tags/v3", current: true }] });
    await vi.waitFor(() => expect(host.querySelector(".workspace-reftrigger-name").textContent).toBe("v3"));
    host.querySelector("[data-refpicker-toggle]").click();
    host.querySelector('[data-ref-kind="tag"]').click();
    expect(refNames(host).map((name) => name.trim())).toEqual(["v3Current"]);
    mounted.dispose();
  });
  it("separates branches from tags and searches within the active tab", async () => {
    const { host } = await mount();
    host.querySelector("[data-refpicker-toggle]").click();

    expect(host.querySelector('[data-ref-kind="branch"]').getAttribute("aria-selected")).toBe("true");
    expect(refNames(host).join(" ")).toContain("feature/search");
    expect(refNames(host).join(" ")).not.toContain("v2.0.0");

    const search = host.querySelector(".workspace-refsearch");
    search.value = "REMOTE";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    expect(refNames(host)).toHaveLength(1);
    expect(refNames(host)[0]).toContain("remote-work");

    host.querySelector('[data-ref-kind="tag"]').click();
    expect(host.querySelector('[data-ref-kind="tag"]').getAttribute("aria-selected")).toBe("true");
    expect(refNames(host)).toHaveLength(0);

    search.value = "v2";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    expect(refNames(host)).toHaveLength(1);
    expect(refNames(host)[0]).toContain("v2.0.0");
  });

  it("distinguishes remote, pullable, and diverged branches", async () => {
    const { host } = await mount();
    host.querySelector("[data-refpicker-toggle]").click();

    const remote = host.querySelector('[data-ref="refs/remotes/origin/remote-work"]');
    expect(remote.querySelector(".workspace-refremote").textContent).toContain("Remote");

    const behind = host.querySelector('[data-ref="refs/heads/main"]');
    expect(behind.querySelector(".workspace-refpull").textContent).toContain("Pull 2");

    const diverged = host.querySelector('[data-ref="refs/heads/diverged"]');
    expect(diverged.querySelector(".workspace-refdiverged")).not.toBeNull();
    expect(diverged.querySelector(".workspace-refpull")).toBeNull();
  });

  it("keeps the current ref visible and reports a refused checkout", async () => {
    const refusal = new Error("Cannot switch: local changes would be overwritten");
    const { host, callRpc, onCheckout } = await mount({ checkoutError: refusal });
    const toggle = host.querySelector("[data-refpicker-toggle]");
    expect(toggle.textContent).toContain("main");
    toggle.click();

    host.querySelector('[data-ref-kind="tag"]').click();
    host.querySelector('[data-ref="refs/tags/v2.0.0"]').click();
    await flush();

    expect(callRpc).toHaveBeenCalledWith("git.checkout_ref", {
      ...scope,
      full_ref: "refs/tags/v2.0.0",
    });
    expect(toggle.textContent).toContain("main");
    expect(host.querySelector(".workspace-referror").textContent).toContain("local changes would be overwritten");
    expect(onCheckout).not.toHaveBeenCalled();
  });

  it("clears a refused refs read once a later read lands", async () => {
    const address = cacheScope.address({ entityId: 'workspace:["ws-1","repo"]', kind: "refs" });
    await writeCached(address, listing);
    const { host, callRpc } = await mount({ refsAnswers: [new Error("Not a git repository")] });
    host.querySelector("[data-refpicker-toggle]").click();
    const status = host.querySelector('[role="status"]');
    await vi.waitFor(() => expect(status.textContent).toBe("Not a git repository"));

    host.querySelector("[data-refpicker-toggle]").click();
    host.querySelector("[data-refpicker-toggle]").click();
    await vi.waitFor(() => expect(callRpc.mock.calls.filter(([method]) => method === "git.refs")).toHaveLength(2));
    await vi.waitFor(() => expect(status.textContent).toBe(""));
    expect(host.querySelector(".workspace-reftrigger-name").textContent).toBe("main");
  });

  it("clears a refused refs read when the cache takes a new record", async () => {
    const address = cacheScope.address({ entityId: 'workspace:["ws-1","repo"]', kind: "refs" });
    const { host } = await mount({ refsAnswers: [new Error("Not a git repository")], refsResponse: { ...listing } });
    const status = host.querySelector('[role="status"]');
    await vi.waitFor(() => expect(status.textContent).toBe("Not a git repository"));
    expect(host.querySelector(".workspace-reftrigger-name").textContent).toBe("Refs unavailable");

    await writeCached(address, listing);
    await vi.waitFor(() => expect(host.querySelector(".workspace-reftrigger-name").textContent).toBe("main"));
    expect(status.textContent).toBe("");
  });

  it.each([["Device offline"], ["Device not reachable"], ["App is out of date"], ["the channel closed"]])(
    "never writes %s, the machine's state, over the cached refs",
    async (mark) => {
      const address = cacheScope.address({ entityId: 'workspace:["ws-1","repo"]', kind: "refs" });
      await writeCached(address, listing);
      const { host, callRpc } = await mount({ refsAnswers: [new Error(mark)] });
      host.querySelector("[data-refpicker-toggle]").click();
      await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("git.refs", scope));
      await flush();
      expect(host.querySelector(".workspace-reftrigger-name").textContent).toBe("main");
      expect(host.querySelector('[role="status"]').textContent).toBe("");
    },
  );

  it("keeps a checkout refusal when a refs read lands after it", async () => {
    const refusal = new Error("Cannot switch: local changes would be overwritten");
    const { host, callRpc } = await mount({ checkoutError: refusal });
    const toggle = host.querySelector("[data-refpicker-toggle]");
    toggle.click();
    host.querySelector('[data-ref="refs/heads/feature/search"]').click();
    await vi.waitFor(() => expect(host.querySelector('[role="status"]').textContent).toContain("local changes"));

    toggle.click();
    toggle.click();
    await vi.waitFor(() => expect(callRpc.mock.calls.filter(([method]) => method === "git.refs")).toHaveLength(3));
    await flush();
    expect(host.querySelector('[role="status"]').textContent).toContain("local changes");
  });
});
