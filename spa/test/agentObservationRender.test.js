// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { observationPanelHtml } from "../src/core/agentObservationRender.js";

const parse = (html) => {
  const template = document.createElement("template");
  template.innerHTML = html;
  return template.content;
};

describe("the compact goal rendering", () => {
  it("escapes provider text and leaves tasks to the activity viewer", () => {
    const dom = parse(observationPanelHtml({
      goal: { objective: '<img src=x onerror="boom">', status: "Goal blocked", notes: [], stale: false },
      checklist: {
        currentStep: "Test <script>bad()</script>", progress: "0/1", notes: [], stale: false,
        rows: [{ key: "1", subject: "Test", description: "", state: "mystery", stateMark: null }],
      },
    }));

    expect(dom.querySelector("img")).toBe(null);
    expect(dom.querySelector("script")).toBe(null);
    expect(dom.querySelector("details.agent-observation-checklist")).toBe(null);
    expect(dom.textContent).not.toContain("Test");
  });

  it("does not put live motion on stale evidence", () => {
    const dom = parse(observationPanelHtml({
      goal: { objective: "Ship", status: "Goal active", notes: ["Last known"], stale: true },
      checklist: null,
    }));

    expect(dom.querySelector("[data-motion]")).toBe(null);
    expect(dom.querySelector(".agent-observation-note").textContent).toBe("Last known");
  });

  it("renders nothing for a checklist without a goal", () => {
    const dom = parse(observationPanelHtml({
      goal: null,
      checklist: { currentStep: "", progress: "2/2", notes: [], stale: false, rows: [] },
    }));

    expect(dom.childElementCount).toBe(0);
  });

  it("bounds an expanded checklist and wraps its metadata on a phone", () => {
    const css = readFileSync(resolve("src/styles/agentObservation.css"), "utf8");
    expect(css).toMatch(/\.agent-observation-items\s*\{[^}]*max-height:[^;}]+;[^}]*overflow-y:auto/);
    expect(css).toMatch(/@media\s*\(max-width:760px\)[\s\S]*\.agent-observation-checklist-meta\s*\{[^}]*flex-wrap:wrap/);
  });
});
