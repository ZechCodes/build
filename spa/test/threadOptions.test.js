// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { createThreadState, threadHtml, wireThreadOptions } from "../src/core/thread.js";

const OFFER = {
  type: "message",
  data: {
    id: "message-2",
    role: "agent",
    body: "Two ways out. Which?",
    created_at: "2026-08-19T09:01:00Z",
    options: [
      { id: "option-1", label: "Revert it", message: "Revert the commit that turned the tests red." },
      { id: "option-2", label: "Fix forward" },
    ],
  },
};

const said = (body, extra = {}) => ({
  type: "message",
  data: { id: "message-3", role: "user", body, created_at: "2026-08-19T09:02:00Z", ...extra },
});

const paint = (items, id = "thread:run-1") => {
  document.body.innerHTML = threadHtml({ id, items });
  return document.body;
};

const chips = () => [...document.querySelectorAll(".thread-option")];
const submit = () => document.querySelector(".thread-options-send");

describe("suggested actions on an agent message", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("offers the agent's options under its message, with nothing to submit yet", () => {
    paint([OFFER]);

    expect(chips().map((chip) => chip.textContent.trim())).toEqual(["Revert it", "Fix forward"]);
    expect(chips().every((chip) => !chip.disabled)).toBe(true);
    expect(submit().disabled).toBe(true);
  });

  it("selecting arms the submit, and deselecting disarms it", () => {
    paint([OFFER]);
    wireThreadOptions(document.body, () => Promise.resolve());

    chips()[0].click();
    expect(chips()[0].classList.contains("chosen")).toBe(true);
    expect(submit().disabled).toBe(false);

    chips()[0].click();
    expect(chips()[0].classList.contains("chosen")).toBe(false);
    expect(submit().disabled).toBe(true);
  });

  it("submits every option picked, and shuts the offer the moment it is pressed", async () => {
    paint([OFFER]);
    const sent = [];
    wireThreadOptions(document.body, (choice) => {
      sent.push(choice);
      return Promise.resolve();
    });

    chips()[0].click();
    chips()[1].click();
    submit().click();

    expect(sent).toEqual([{ messageId: "message-2", optionIds: ["option-1", "option-2"] }]);
    expect(chips().every((chip) => chip.disabled)).toBe(true);
    expect(submit().disabled).toBe(true);
  });

  it("a refused submission gives the offer back", async () => {
    paint([OFFER]);
    wireThreadOptions(document.body, () => Promise.reject(new Error("no")));

    chips()[1].click();
    submit().click();
    await Promise.resolve();
    await Promise.resolve();

    expect(chips().every((chip) => !chip.disabled)).toBe(true);
    expect(chips()[1].classList.contains("chosen")).toBe(true);
  });

  it("keeps what was chosen and dims the rest once the choice is on the record", () => {
    paint([
      { ...OFFER, data: { ...OFFER.data, selected_options: ["option-2"] } },
      said("Fix forward", { answers_options_of: "message-2" }),
    ]);

    expect(chips().every((chip) => chip.disabled)).toBe(true);
    expect(chips()[1].classList.contains("chosen")).toBe(true);
    expect(chips()[0].classList.contains("chosen")).toBe(false);
    expect(submit()).toBe(null);
  });

  /// The choice is shown on the chips, so the message it sends would be the
  /// same words twice.
  it("draws nothing for the message a choice sent", () => {
    paint([
      { ...OFFER, data: { ...OFFER.data, selected_options: ["option-2"] } },
      said("Fix forward", { answers_options_of: "message-2" }),
    ]);

    expect(document.querySelectorAll(".thread-comment")).toHaveLength(1);
    expect(document.querySelector(".thread-title").textContent).toContain("1");
  });

  it("shuts an unanswered offer that anything later was said after", () => {
    paint([OFFER, said("actually, hold on")]);

    expect(chips().every((chip) => chip.disabled)).toBe(true);
    expect(chips().every((chip) => !chip.classList.contains("chosen"))).toBe(true);
    expect(submit().disabled).toBe(true);
  });

  it("leaves an offer open when only an event followed it", () => {
    paint([OFFER, { type: "event", data: { event: "committed", created_at: "2026-08-19T09:02:00Z" } }]);

    expect(chips().every((chip) => !chip.disabled)).toBe(true);
  });

  /// Every conversation numbers its own messages from one, so two of them are
  /// forever offering a "message-2".
  it("keeps one conversation's picks out of another's", () => {
    paint([OFFER]);
    wireThreadOptions(document.body, () => Promise.resolve());
    chips()[0].click();

    paint([OFFER], "thread:run-2");

    expect(chips().every((chip) => !chip.classList.contains("chosen"))).toBe(true);
    expect(submit().disabled).toBe(true);
  });

  it("keeps identical conversation ids isolated by their owning scope", () => {
    const firstScope = createThreadState({ ownerId: "device-a" });
    const secondScope = createThreadState({ ownerId: "device-b" });
    document.body.innerHTML = threadHtml({ id: "thread:run-1", items: [OFFER] }, { threadState: firstScope });
    wireThreadOptions(document.body, () => Promise.resolve(), firstScope);
    chips()[0].click();

    document.body.innerHTML = threadHtml(
      { id: "thread:run-1", items: [OFFER] },
      { threadState: secondScope },
    );

    expect(chips().every((chip) => !chip.classList.contains("chosen"))).toBe(true);
    expect(submit().disabled).toBe(true);
  });

  it("ignores a late offer completion after its owning scope is disposed", async () => {
    const state = createThreadState({ ownerId: "device-a" });
    let resolveSend;
    document.body.innerHTML = threadHtml({ id: "thread:run-1", items: [OFFER] }, { threadState: state });
    wireThreadOptions(document.body, () => new Promise((resolve) => { resolveSend = resolve; }), state);
    chips()[0].click();
    submit().click();

    state.dispose();
    resolveSend();
    await Promise.resolve();

    expect(state.snapshot()).toEqual({ choiceState: "", sending: "" });
  });

  it("escapes what the agent wrote on a chip", () => {
    paint([
      {
        ...OFFER,
        data: { ...OFFER.data, options: [{ id: "option-1", label: "<img src=x onerror=alert(1)>" }] },
      },
    ]);

    expect(document.querySelector(".thread-option img")).toBe(null);
    expect(chips()[0].textContent).toContain("<img src=x onerror=alert(1)>");
  });
});
