/** @vitest-environment jsdom */

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createSecretComposer,
  openSecretModal,
  secretChipLabel,
  secretChipsHtml,
  secretTtlSeconds,
  validSecretName,
} from "../src/core/secretComposer.js";
import { motionSettled } from "../src/core/motion.js";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("secret composer metadata", () => {
  it("uses the bridge's bounded env-style name grammar", () => {
    expect(validSecretName("TEST_TOKEN")).toBe(true);
    expect(validSecretName("_deploy.token-v2")).toBe(true);
    expect(validSecretName("A".repeat(64))).toBe(true);
    for (const name of ["", "2TOKEN", "HAS SPACE", "A/KEY", "A".repeat(65), "éTOKEN"]) {
      expect(validSecretName(name), name).toBe(false);
    }
  });

  it("accepts no expiry, presets, and bounded custom seconds", () => {
    expect(secretTtlSeconds("none", "")).toBe(null);
    expect(secretTtlSeconds("3600", "")).toBe(3600);
    expect(secretTtlSeconds("custom", "60")).toBe(60);
    expect(secretTtlSeconds("custom", "2592000")).toBe(2592000);
    for (const [choice, custom] of [["custom", ""], ["custom", "59"], ["custom", "2592001"], ["custom", "1.5"], ["bogus", ""]]) {
      expect(() => secretTtlSeconds(choice, custom)).toThrow(/expiry/i);
    }
  });

  it.each([
    [{ name: "TEST_TOKEN", grant: { scope: "project" }, ttl_seconds: null }, "TEST_TOKEN · Project · No expiry"],
    [{ name: "DEPLOY_TOKEN", grant: { scope: "agent" }, ttl_seconds: 3600 }, "DEPLOY_TOKEN · Agent · 1 hour after sending"],
    [{ name: "MINUTE_TOKEN", grant: { scope: "project" }, ttl_seconds: 120 }, "MINUTE_TOKEN · Project · 2 minutes after sending"],
    [{ name: "DAY_TOKEN", grant: { scope: "agent" }, ttl_seconds: 172800 }, "DAY_TOKEN · Agent · 2 days after sending"],
    [{ name: "ODD_TOKEN", grant: { scope: "agent" }, ttl_seconds: 61 }, "ODD_TOKEN · Agent · 61 seconds after sending"],
  ])("formats the exact metadata-only chip %s", (metadata, label) => {
    expect(secretChipLabel(metadata)).toBe(label);
    expect(secretChipsHtml([metadata], { removable: false })).toContain(label);
  });

  it("escapes chip metadata and gives staged chips a revoke control", () => {
    const html = secretChipsHtml([{ secret_id: "sec_1", name: '<img src=x onerror="bad">', grant: { scope: "agent" }, ttl_seconds: null }]);
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img");
    expect(html).toContain('data-secret-remove="sec_1"');
    expect(html).toContain('aria-label="Remove &lt;img');
  });
});

const dialog = () => document.querySelector("[role=dialog]");
const field = (name) => dialog().querySelector(`[name="${name}"]`);

describe("secret modal", () => {
  it("is labelled, focuses Name, masks the non-autofilled value, and defaults to Agent with scope explanations", async () => {
    const modal = openSecretModal({ stage: vi.fn() });
    await motionSettled();
    await vi.waitFor(() => expect(document.activeElement).toBe(field("secret-key")));
    expect(dialog().getAttribute("aria-labelledby")).toBe("secret-modal-title");
    expect(field("secret-value").type).toBe("password");
    expect(field("secret-value").autocomplete).toBe("off");
    expect(dialog().querySelector('[name="secret-scope"][value="agent"]').checked).toBe(true);
    expect(dialog().textContent).toContain("Limited to this agent’s conversation");
    expect(dialog().textContent).toContain("Available to current agents in this project");
    expect(dialog().textContent).toContain("starts when the message is sent");

    const value = field("secret-value");
    value.value = "synthetic modal secret";
    const reveal = dialog().querySelector("[data-secret-reveal]");
    reveal.click();
    expect(value.type).toBe("text");
    expect(reveal.getAttribute("aria-pressed")).toBe("true");
    reveal.click();
    expect(value.type).toBe("password");
    expect(reveal.getAttribute("aria-pressed")).toBe("false");
    modal.close();
  });

  it("stages Agent-default metadata, clears the live value, and closes on success", async () => {
    const stage = vi.fn(async () => ({ secret_id: "sec_1" }));
    const changed = vi.fn();
    openSecretModal({ stage, onChange: changed });
    field("secret-key").value = "DEPLOY_TOKEN";
    const value = field("secret-value");
    value.value = "synthetic stage value";
    field("secret-ttl").value = "3600";
    dialog().querySelector("[data-secret-submit]").click();

    await vi.waitFor(() => expect(stage).toHaveBeenCalledOnce());
    const submitted = stage.mock.calls[0][0];
    expect(submitted).toMatchObject({ name: "DEPLOY_TOKEN", scope: "agent", ttlSeconds: 3600 });
    expect(submitted.value).toBe("synthetic stage value");
    expect(value.value).toBe("");
    expect(changed).toHaveBeenCalledOnce();
    await vi.waitFor(() => expect(dialog()).toBeNull());
  });

  it("submits Project scope with a bounded custom TTL", async () => {
    const stage = vi.fn(async () => ({ secret_id: "sec_project" }));
    openSecretModal({ stage });
    field("secret-key").value = "PROJECT_TOKEN";
    field("secret-value").value = "synthetic project value";
    dialog().querySelector('[name="secret-scope"][value="project"]').click();
    field("secret-ttl").value = "custom";
    field("secret-ttl").dispatchEvent(new Event("change"));
    expect(field("secret-custom-ttl").hidden).toBe(false);
    field("secret-custom-ttl").value = "90";
    dialog().querySelector("[data-secret-submit]").click();
    await vi.waitFor(() => expect(stage).toHaveBeenCalledOnce());
    expect(stage.mock.calls[0][0]).toMatchObject({ name: "PROJECT_TOKEN", scope: "project", ttlSeconds: 90 });
  });

  it.each([
    ["invalid name", "synthetic invalid-name value", /letters/i],
    ["VALID_NAME", "", /value/i],
  ])("validates without staging and wipes the submitted value", async (name, rawValue, error) => {
    const stage = vi.fn();
    openSecretModal({ stage });
    field("secret-key").value = name;
    const value = field("secret-value");
    value.value = rawValue;
    dialog().querySelector("[data-secret-submit]").click();
    expect(stage).not.toHaveBeenCalled();
    expect(value.value).toBe("");
    expect(dialog().querySelector("[data-secret-error]").textContent).toMatch(error);
    expect(dialog().querySelector("[data-secret-error]").getAttribute("aria-live")).toBe("polite");
  });

  it("wipes a rejected submission and reports only the safe error", async () => {
    const stage = vi.fn(async () => { throw new Error("Upload could not be confirmed"); });
    openSecretModal({ stage });
    field("secret-key").value = "FAILED_TOKEN";
    const value = field("secret-value");
    value.value = "synthetic rejected value";
    dialog().querySelector("[data-secret-submit]").click();
    await vi.waitFor(() => expect(dialog().querySelector("[data-secret-error]").textContent).toContain("Upload could not be confirmed"));
    expect(value.value).toBe("");
    expect(dialog().textContent).not.toContain("synthetic rejected value");
  });

  it("keeps the modal open and reports a stage that produced no secret", async () => {
    openSecretModal({ stage: vi.fn(async () => null) });
    field("secret-key").value = "COLD_START_TOKEN";
    field("secret-value").value = "synthetic cold-start value";
    dialog().querySelector("[data-secret-submit]").click();

    await vi.waitFor(() => expect(dialog().querySelector("[data-secret-error]").textContent)
      .toContain("could not be attached"));
    expect(dialog()).toBeTruthy();
    expect(field("secret-value").value).toBe("");
  });

  it("wipes the live field when dismissed", async () => {
    openSecretModal({ stage: vi.fn() });
    const value = field("secret-value");
    value.value = "synthetic dismissed value";
    dialog().querySelector("[data-secret-cancel]").click();
    expect(value.value).toBe("");
    await vi.waitFor(() => expect(dialog()).toBeNull());
  });
});

describe("secret composer controller", () => {
  it("waits for revocation before repainting removable staged chips", async () => {
    let resolve;
    const service = {
      entries: vi.fn(() => [{ secret_id: "sec_1", name: "TOKEN", grant: { scope: "agent" }, ttl_seconds: null }]),
      stage: vi.fn(),
      remove: vi.fn(() => new Promise((done) => { resolve = done; })),
    };
    const changed = vi.fn();
    const failed = vi.fn();
    const composer = createSecretComposer({ service, onChange: changed, onError: failed });
    document.body.innerHTML = `<div id="tray">${composer.chipsHtml()}</div>`;
    composer.wire(document.querySelector("#tray"));
    const remove = document.querySelector("[data-secret-remove]");
    remove.click();
    expect(service.remove).toHaveBeenCalledWith("sec_1");
    expect(remove.disabled).toBe(true);
    expect(changed).not.toHaveBeenCalled();
    resolve({ status: "revoked" });
    await vi.waitFor(() => expect(changed).toHaveBeenCalledOnce());
    expect(failed).not.toHaveBeenCalled();
  });

  it("keeps a cached chip and reports when revocation produces no result", async () => {
    const service = {
      entries: vi.fn(() => [{ secret_id: "sec_cached", name: "TOKEN", grant: { scope: "agent" }, ttl_seconds: null }]),
      stage: vi.fn(),
      remove: vi.fn(async () => null),
    };
    const changed = vi.fn();
    const failed = vi.fn();
    const composer = createSecretComposer({ service, onChange: changed, onError: failed });
    document.body.innerHTML = `<div id="tray">${composer.chipsHtml()}</div>`;
    composer.wire(document.querySelector("#tray"));
    const remove = document.querySelector("[data-secret-remove]");

    remove.click();
    await vi.waitFor(() => expect(failed).toHaveBeenCalledOnce());
    expect(failed.mock.calls[0][0].message).toContain("could not be revoked");
    expect(changed).not.toHaveBeenCalled();
    expect(remove.disabled).toBe(false);
    expect(document.querySelector('[data-secret-id="sec_cached"]')).toBeTruthy();
  });
});
