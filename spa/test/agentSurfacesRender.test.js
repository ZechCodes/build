// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import {
  agentRowHtml,
  checklistViewerHtml,
  shellViewerHtml,
  subagentViewerHtml,
  surfacePillsHtml,
  workflowViewerHtml,
} from "../src/core/agentSurfacesRender.js";
import {
  agentRows,
  surfacePills,
  surfaceRows,
  workflowPhases,
} from "../src/core/agentSurfacesModel.js";

const POISON = "</div><img onerror=x>";

const parse = (html) => {
  const holder = globalThis.document.createElement("div");
  holder.innerHTML = html;
  return holder;
};

const readerEntry = {
  id: "a1",
  label: "Reader",
  model: "haiku",
  state: "running",
  duration_ms: 65000,
  tokens: 1200,
  tool_calls: 4,
  last_tool: { name: "Read", summary: "bridge/src/app.rs" },
};

const freshWorkflow = {
  id: "w1",
  name: "Review",
  description: "Read the branch and report",
  state: "running",
  phases: [
    { title: "Read", agents: [{ label: "Reader", state: "start", model: "haiku" }, { label: "Counter", state: "start", model: "haiku" }] },
    { title: "Write", agents: [] },
  ],
};

const workflowRow = (workflow) => surfaceRows("workflows", { workflows: [workflow] })[0];

describe("agentRowHtml", () => {
  it("is one renderer: a workflow agent and a subagent of the same entry paint the same bytes", () => {
    const fromWorkflow = workflowPhases(
      { phases: [{ title: "Read", agents: [readerEntry] }] },
      0,
    ).agents[0];
    const fromSubagents = surfaceRows("subagents", { subagents: [readerEntry] })[0];
    expect(agentRowHtml(fromWorkflow)).toBe(agentRowHtml(fromSubagents));
    expect(agentRowHtml(fromWorkflow)).toBe(agentRowHtml(agentRows([readerEntry])[0]));
  });

  it("draws the label, the model, the last tool, the tokens, the calls and the duration", () => {
    const html = agentRowHtml(agentRows([readerEntry])[0]);
    expect(html).toContain("Reader");
    expect(html).toContain("haiku");
    expect(html).toContain("Read bridge/src/app.rs");
    expect(html).toContain("1200");
    expect(html).toContain("4");
    expect(html).toContain("1m 05s");
  });

  it("draws the state mark the model named and nothing for a state it does not recognise", () => {
    const marked = parse(agentRowHtml(agentRows([readerEntry])[0]));
    expect(marked.querySelectorAll("[data-outcome]").length).toBe(1);
    const unknown = parse(agentRowHtml(agentRows([{ id: "a1", label: "Reader", state: "banana" }])[0]));
    expect(unknown.querySelectorAll("[data-outcome]").length).toBe(0);
  });

  it("draws the result of a finished agent and the error of a failed one", () => {
    expect(agentRowHtml(agentRows([{ id: "a1", label: "Reader", state: "done", result: "42 files" }])[0])).toContain(
      "42 files",
    );
    expect(agentRowHtml(agentRows([{ id: "a1", label: "Reader", state: "failed", error: "no such path" }])[0])).toContain(
      "no such path",
    );
  });

  it("offers the row's actions through the shared menu markup", () => {
    const row = parse(agentRowHtml(agentRows([readerEntry])[0]));
    expect(row.querySelectorAll(".splitbtn .splitmenu .mi").length).toBeGreaterThan(0);
    expect(row.querySelector(".splitmenu").hasAttribute("hidden")).toBe(true);
  });

  it("emits data-call-sequence for a row that carries one and no such attribute for a row that does not", () => {
    const spawned = agentRowHtml(agentRows([{ ...readerEntry, call_sequence: 12 }])[0]);
    expect(parse(spawned).querySelector("[data-call-sequence]").dataset.callSequence).toBe("12");
    expect(agentRowHtml(agentRows([readerEntry])[0])).not.toContain("data-call-sequence");
  });

  it("escapes everything the model can put in it", () => {
    const html = agentRowHtml(
      agentRows([
        {
          id: POISON,
          label: POISON,
          model: POISON,
          state: "running",
          result: POISON,
          last_tool: { name: POISON, summary: POISON },
        },
      ])[0],
    );
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;/div&gt;");
  });
});

describe("surfacePillsHtml", () => {
  const pills = surfacePills({
    shells: [{ id: "s1", state: "running" }],
    checklist: [{ id: "t1", state: "completed" }],
  });

  it("presses exactly the open kind and no other", () => {
    const row = parse(surfacePillsHtml(pills, "shells"));
    const pressed = [...row.querySelectorAll("button")].map((button) => [
      button.dataset.surfaceKind,
      button.getAttribute("aria-pressed"),
    ]);
    expect(pressed).toEqual([
      ["shells", "true"],
      ["checklist", "false"],
    ]);
  });

  it("presses nothing when no pill is open", () => {
    const row = parse(surfacePillsHtml(pills, null));
    expect([...row.querySelectorAll("button")].map((button) => button.getAttribute("aria-pressed"))).toEqual([
      "false",
      "false",
    ]);
  });

  it("gives a live pill one working dot and a settled pill none", () => {
    const row = parse(surfacePillsHtml(pills, null));
    const [live, settled] = [...row.querySelectorAll("button")];
    expect(live.querySelectorAll(".sdot.sdot-working").length).toBe(1);
    expect(settled.querySelectorAll(".sdot-working").length).toBe(0);
  });

  it("says the label and the count of each pill", () => {
    const row = parse(surfacePillsHtml(pills, "shells"));
    expect(row.textContent).toContain("Shells");
    expect(row.textContent).toContain("1");
  });

  it("draws nothing at all when no kind has content", () => {
    expect(surfacePillsHtml([], null)).toBe("");
  });

  it("escapes a pill label", () => {
    const html = surfacePillsHtml([{ kind: "shells", label: POISON, count: 1, live: false }], null);
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;/div&gt;");
  });
});

describe("workflowViewerHtml", () => {
  const view = (workflow, selectedIndex) => {
    const { phases, agents } = workflowPhases(workflow, selectedIndex);
    return workflowViewerHtml(workflowRow(workflow), phases, agents);
  };

  it("paints both of the id-less queued agents a fresh workflow carries", () => {
    const painted = parse(view(freshWorkflow, 0));
    expect(painted.querySelectorAll(".surface-agent").length).toBe(2);
    expect(painted.textContent).toContain("Reader");
    expect(painted.textContent).toContain("Counter");
  });

  it("lists the phases with their done counts and presses the selected one", () => {
    const workflow = {
      id: "w1",
      name: "Review",
      state: "running",
      phases: [
        { title: "Read", agents: [{ id: "a1", label: "Reader", state: "done" }, { id: "a2", label: "Skimmer", state: "running" }] },
        { title: "Write", agents: [{ id: "a3", label: "Writer", state: "queued" }] },
      ],
    };
    const painted = parse(view(workflow, 1));
    const phases = [...painted.querySelectorAll(".surface-phase")];
    expect(phases.map((phase) => phase.getAttribute("aria-pressed"))).toEqual(["false", "true"]);
    expect(phases[0].textContent).toContain("1/2");
    expect(painted.querySelectorAll(".surface-agent").length).toBe(1);
    expect(painted.textContent).toContain("Writer");
  });

  it("draws each agent through the one agent row renderer", () => {
    const { phases, agents } = workflowPhases(freshWorkflow, 0);
    expect(workflowViewerHtml(workflowRow(freshWorkflow), phases, agents)).toContain(agentRowHtml(agents[0]));
  });

  it("names the workflow, marks its state and offers its actions", () => {
    const painted = parse(view(freshWorkflow, 0));
    expect(painted.textContent).toContain("Review");
    expect(painted.querySelector(".surface-workflow-head [data-outcome]")).toBeTruthy();
    expect(painted.querySelectorAll(".surface-workflow-head .splitmenu .mi").length).toBeGreaterThan(0);
  });

  it("escapes the workflow's own name and its agents' labels", () => {
    const poisoned = { id: "w1", name: POISON, state: "running", phases: [{ title: POISON, agents: [{ label: POISON }] }] };
    const html = view(poisoned, 0);
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;/div&gt;");
  });
});

describe("subagentViewerHtml", () => {
  it("draws the same rows through the same agent row renderer", () => {
    const rows = surfaceRows("subagents", { subagents: [readerEntry, { id: "a2", label: "Writer", state: "done" }] });
    const html = subagentViewerHtml(rows);
    expect(html).toContain(agentRowHtml(rows[0]));
    expect(html).toContain(agentRowHtml(rows[1]));
    expect(parse(html).querySelectorAll(".surface-agent").length).toBe(2);
  });

  it("escapes a subagent label", () => {
    const html = subagentViewerHtml(surfaceRows("subagents", { subagents: [{ id: "a1", label: POISON }] }));
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;/div&gt;");
  });
});

describe("shellViewerHtml", () => {
  const rows = (shells) => surfaceRows("shells", { shells });

  it("folds each shell's tail into a pre inside a details", () => {
    const painted = parse(
      shellViewerHtml(rows([{ id: "s1", description: "npm test", state: "running", tail: ["one", "two"] }])),
    );
    const fold = painted.querySelector("details");
    expect(fold).toBeTruthy();
    expect(fold.open).toBe(false);
    expect(fold.querySelector("pre").textContent).toContain("one");
    expect(fold.querySelector("pre").textContent).toContain("two");
  });

  it("escapes a tail line", () => {
    const html = shellViewerHtml(rows([{ id: "s1", description: "npm test", state: "running", tail: ["<script>alert(1)</script>"] }]));
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
  });

  it("says each shell's description, its state mark and its exit code", () => {
    const painted = parse(
      shellViewerHtml(rows([{ id: "s1", description: "npm test", state: "done", exit_code: 1, tail: ["boom"] }])),
    );
    expect(painted.textContent).toContain("npm test");
    expect(painted.textContent).toContain("1");
    expect(painted.querySelectorAll("[data-outcome]").length).toBe(1);
  });

  it("offers no fold onto a shell that has printed nothing", () => {
    const painted = parse(shellViewerHtml(rows([{ id: "s1", description: "npm test", state: "running" }])));
    expect(painted.querySelector("details")).toBe(null);
  });

  it("escapes a shell description", () => {
    const html = shellViewerHtml(rows([{ id: "s1", description: POISON, state: "running" }]));
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;/div&gt;");
  });
});

describe("checklistViewerHtml", () => {
  const rows = (checklist) => surfaceRows("checklist", { checklist });

  it("marks every state it recognises", () => {
    const painted = parse(
      checklistViewerHtml(
        rows([
          { id: "t1", subject: "one", state: "pending" },
          { id: "t2", subject: "two", state: "in_progress" },
          { id: "t3", subject: "three", state: "completed" },
          { id: "t4", subject: "four", state: "blocked" },
        ]),
      ),
    );
    expect(painted.querySelectorAll(".surface-checklist-item").length).toBe(4);
    expect(painted.querySelectorAll("[data-outcome]").length).toBe(4);
  });

  it("marks nothing for a state it does not recognise", () => {
    const painted = parse(checklistViewerHtml(rows([{ id: "t1", subject: "one", state: "banana" }])));
    expect(painted.querySelectorAll(".surface-checklist-item").length).toBe(1);
    expect(painted.querySelectorAll("[data-outcome]").length).toBe(0);
  });

  it("says each item's subject", () => {
    expect(parse(checklistViewerHtml(rows([{ id: "t1", subject: "Read the spec", state: "pending" }]))).textContent).toContain(
      "Read the spec",
    );
  });

  it("escapes an item's subject", () => {
    const html = checklistViewerHtml(rows([{ id: "t1", subject: POISON, state: "pending" }]));
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;/div&gt;");
  });
});

describe("the renderer is pure markup", () => {
  it("queries no page, wires no handler and calls no bridge", () => {
    const source = readFileSync(resolve("src/core/agentSurfacesRender.js"), "utf8");
    for (const forbidden of ["document", "window", "App", "localStorage"]) {
      expect(source).not.toContain(forbidden);
    }
  });
});
