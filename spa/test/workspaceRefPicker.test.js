/** @vitest-environment jsdom */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { mountWorkspaceRefPicker } from "../src/core/workspaceRefPicker.js";

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

async function mount({ checkoutError } = {}) {
  const host = document.querySelector("#host");
  const callRpc = vi.fn(async (method) => {
    if (method === "git.refs") return listing;
    if (method === "git.checkout_ref" && checkoutError) throw checkoutError;
    return {};
  });
  const onCheckout = vi.fn();
  const mounted = mountWorkspaceRefPicker(host, { scope, callRpc, onCheckout });
  await flush();
  return { host, callRpc, onCheckout, mounted };
}

beforeEach(() => {
  document.body.innerHTML = '<div id="host"></div>';
});

describe("workspace ref picker", () => {
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
});
