// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { railStatusLeadClass, railStatusLeadHtml, railWhoHtml } from "../src/core/agentRailRender.js";

describe("the lead of the pinned status row", () => {
  it("gives the working shape a word of its own to collapse and a slot for the clock", () => {
    expect(railStatusLeadHtml("working")).toContain('class="rail-status-working-word"');
    expect(railStatusLeadHtml("working")).toContain('class="rail-status-text"');
    expect(railStatusLeadHtml("starting")).not.toContain("rail-status-working-word");
    expect(railStatusLeadHtml("starting")).toContain('class="rail-status-text"');
    expect(railStatusLeadHtml("quiet")).toBe("");
  });

  it("wears the class of the shape it is saying", () => {
    expect(railStatusLeadClass("working")).toBe("rail-status-lead rail-status-working");
    expect(railStatusLeadClass("starting")).toBe("rail-status-lead rail-status-starting");
    expect(railStatusLeadClass("quiet")).toBe("rail-status-lead");
  });
});

describe("the conversation heading", () => {
  it("keeps the agent name in a large first line and current work below", () => {
    const host = document.createElement("div");
    host.innerHTML = railWhoHtml("Rail <helper>", { text: "Unify <prompt> delivery" });
    expect(host.querySelector(".rail-who-name").textContent).toBe("Rail <helper>");
    expect(host.querySelector(".rail-who-work").textContent).toBe("Unify <prompt> delivery");
    expect(host.querySelector(".rail-who-name").dataset.titleMotion).toBeUndefined();
  });

  it("has one line when the agent has no current work", () => {
    const host = document.createElement("div");
    host.innerHTML = railWhoHtml("Agent 2", null);
    expect(host.querySelector(".rail-who-name").textContent).toBe("Agent 2");
    expect(host.querySelector(".rail-who-work")).toBeNull();
  });

  it("shimmers Starting only on the work line", () => {
    const host = document.createElement("div");
    host.innerHTML = railWhoHtml("Project agent", { text: "Starting", starting: true });
    expect(host.querySelector(".rail-who-work").classList.contains("rail-who-starting")).toBe(true);
    expect(host.querySelector(".rail-who-name").classList.contains("rail-who-starting")).toBe(false);
  });
});
