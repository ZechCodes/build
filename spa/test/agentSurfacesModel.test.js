import { describe, it, expect } from "vitest";
import { coreSourceOf } from "./coreSource.js";
import { memoryStorage, refusingStorage } from "./memoryStorage.js";
import { recordedSurfaces } from "./recordedSurfaces.js";
import {
  AGENT_ENTRY_KIND,
  WORKFLOW_ENTRY_KIND,
  agentRows,
  openSurfaceKind,
  openWorkflow,
  readOpenSurface,
  rowActions,
  surfacePills,
  surfaceRows,
  surfaceStateMark,
  workflowChoicesWorthOffering,
  workflowPhases,
  writeOpenSurface,
} from "../src/core/agentSurfacesModel.js";
import { outcomeMarkHtml } from "../src/core/outcomeMark.js";

const oneWorkflow = (workflow) => ({ workflows: [workflow] });

const checklistSnapshot = {
  checklist: [
    { id: "t1", subject: "Read the spec", state: "completed" },
    { id: "t2", subject: "Write the test", state: "in_progress" },
    { id: "t3", subject: "Make it pass", state: "pending" },
  ],
};

describe("surfacePills", () => {
  it("gives one pill per kind that has content", () => {
    expect(surfacePills(checklistSnapshot)).toEqual([
      { kind: "checklist", label: "Checklist", count: 3, live: true },
    ]);
  });

  it("gives nothing for a snapshot with no kinds", () => {
    expect(surfacePills({})).toEqual([]);
    expect(surfacePills(null)).toEqual([]);
  });

  it("gives nothing for a kind sent empty and nothing for a kind this client cannot render", () => {
    expect(surfacePills({ shells: [], sonnets: [{ id: "s1" }] })).toEqual([]);
  });

  it("is live while any entry is running and settled once every entry is done", () => {
    const running = { shells: [{ id: "s1", state: "done" }, { id: "s2", state: "running" }] };
    const finished = { shells: [{ id: "s1", state: "done" }, { id: "s2", state: "done" }] };
    expect(surfacePills(running)[0].live).toBe(true);
    expect(surfacePills(finished)[0].live).toBe(false);
  });

  it("counts every kind and orders them the same way every time", () => {
    const pills = surfacePills({
      checklist: [{ id: "t1", state: "pending" }],
      shells: [{ id: "s1", state: "done" }],
      subagents: [{ id: "a1", label: "Reader", state: "done" }],
      workflows: [{ id: "w1", name: "Review", state: "done" }],
    });
    expect(pills.map((pill) => pill.kind)).toEqual(["workflows", "subagents", "shells", "checklist"]);
    expect(pills.map((pill) => pill.count)).toEqual([1, 1, 1, 1]);
  });
});

describe("openSurfaceKind", () => {
  it("is null when the wanted kind has nothing in the snapshot", () => {
    expect(openSurfaceKind(checklistSnapshot, "shells")).toBe(null);
  });

  it("keeps the wanted kind while it still exists", () => {
    expect(openSurfaceKind(checklistSnapshot, "checklist")).toBe("checklist");
  });

  it("never resolves a gone kind to a different pill", () => {
    const snapshot = { workflows: [{ id: "w1", name: "Review", state: "running" }] };
    expect(openSurfaceKind(snapshot, "shells")).toBe(null);
    expect(openSurfaceKind(snapshot, "workflows")).toBe("workflows");
    expect(openSurfaceKind(snapshot, null)).toBe(null);
  });
});

describe("surfaceRows", () => {
  it("keys every workflow row, and never lets two id-less entries collide", () => {
    const rows = surfaceRows("workflows", {
      workflows: [
        { name: "Review", state: "running" },
        { name: "Ship", state: "queued" },
      ],
    });
    expect(rows.map((row) => row.key)).toEqual(["workflows-0", "workflows-1"]);
    expect(rows[0].name).toBe("Review");
  });

  it("falls back to the index key when two entries repeat an id", () => {
    const rows = surfaceRows("shells", {
      shells: [
        { id: "same", description: "npm test" },
        { id: "same", description: "cargo test" },
      ],
    });
    expect(rows.map((row) => row.key)).toEqual(["same", "shells-1"]);
  });

  it("never lets a real id collide with the fallback namespace it hands out", () => {
    const rows = surfaceRows("workflows", { workflows: [{ id: "workflows-1" }, {}] });
    expect(rows.map((row) => row.key)).toEqual(["workflows-1", "workflows-2"]);
    expect(agentRows([{ id: "agent-1" }, {}]).map((row) => row.key)).toEqual(["agent-1", "agent-2"]);
  });

  it("draws a workflow row without handing it a raw phases array", () => {
    const entry = {
      id: "w1",
      name: "Review",
      state: "running",
      phases: [{ title: "Read", agents: [{ label: "Reader" }] }, { title: "Write", agents: [] }],
    };
    const rows = surfaceRows("workflows", oneWorkflow(entry));
    expect(rows[0]).toEqual({
      key: "w1",
      id: "w1",
      name: "Review",
      description: "",
      state: "running",
      stateMark: surfaceStateMark("workflows", "running"),
      subject: "Review",
      actions: rowActions("workflows", entry),
      phaseCount: 2,
    });
  });

  it("stamps every row with the subject and the actions its menu speaks, so no painter recomputes them", () => {
    const shell = { id: "s1", description: "npm test", state: "running" };
    const [row] = surfaceRows("shells", { shells: [shell] });
    expect(row.subject).toBe("npm test");
    expect(row.actions).toEqual(rowActions("shells", shell));
    const nameless = { id: "s2", state: "running" };
    expect(surfaceRows("shells", { shells: [nameless] })[0].subject).toBe("s2");
  });

  it("delegates the subagents arm to agentRows rather than keying them a second way", () => {
    const subagents = [{ id: "a1", label: "Reader", state: "running" }];
    expect(surfaceRows("subagents", { subagents })).toEqual(agentRows(subagents));
    expect(surfaceRows("subagents", { subagents })[0].key).toBe("a1");
  });

  it("gives every shell its tail and exit code, and every checklist item its subject", () => {
    const shells = surfaceRows("shells", {
      shells: [{ id: "s1", description: "npm test", state: "done", exit_code: 0, tail: ["ok"] }],
    });
    expect(shells[0]).toMatchObject({ key: "s1", description: "npm test", exitCode: 0, tail: ["ok"] });
    const checklist = surfaceRows("checklist", checklistSnapshot);
    expect(checklist.map((row) => row.subject)).toEqual(["Read the spec", "Write the test", "Make it pass"]);
  });

  it("gives nothing for a kind the snapshot does not carry and nothing for a kind it cannot render", () => {
    expect(surfaceRows("shells", checklistSnapshot)).toEqual([]);
    expect(surfaceRows("sonnets", { sonnets: [{ id: "x" }] })).toEqual([]);
  });

  it("marks each row's state through the one state table", () => {
    expect(surfaceRows("checklist", checklistSnapshot)[1].stateMark).toEqual(
      surfaceStateMark("checklist", "in_progress"),
    );
  });
});

describe("agentRows", () => {
  it("normalises one agent entry to what one row draws", () => {
    const [row] = agentRows([
      {
        id: "a1",
        label: "Reader",
        model: "haiku",
        state: "running",
        duration_ms: 65000,
        tokens: 1200,
        tool_calls: 4,
        last_tool: { name: "Read", summary: "bridge/src/app.rs" },
        call_sequence: 12,
      },
    ]);
    expect(row).toMatchObject({
      key: "a1",
      id: "a1",
      label: "Reader",
      model: "haiku",
      state: "running",
      tokens: 1200,
      toolCalls: 4,
      lastTool: "Read bridge/src/app.rs",
      callSequence: 12,
      duration: "1m 05s",
    });
    expect(row.stateMark).toEqual(surfaceStateMark("subagents", "running"));
  });

  it("keys two id-less agents apart", () => {
    expect(agentRows([{ label: "one" }, { label: "two" }]).map((row) => row.key)).toEqual([
      "agent-0",
      "agent-1",
    ]);
  });

  it("claims nothing about a state token it does not recognise", () => {
    const [row] = agentRows([{ id: "a1", label: "Reader", state: "banana" }]);
    expect(row.stateMark).toBe(null);
  });

  it("gives nothing for a missing list", () => {
    expect(agentRows(undefined)).toEqual([]);
  });
});

describe("workflowPhases", () => {
  const workflow = {
    id: "w1",
    name: "Review",
    state: "running",
    phases: [
      {
        title: "Read",
        agents: [
          { id: "a1", label: "Reader", state: "done" },
          { id: "a2", label: "Skimmer", state: "running" },
        ],
      },
      { title: "Write", agents: [{ label: "Writer" }, { label: "Editor" }] },
    ],
  };

  it("counts the done agents of every phase", () => {
    const { phases } = workflowPhases(oneWorkflow(workflow), 0, 0);
    expect(phases).toEqual([
      { key: "phase-0", index: 0, title: "Read", total: 2, done: 1, selected: true },
      { key: "phase-1", index: 1, title: "Write", total: 2, done: 0, selected: false },
    ]);
  });

  it("hands back the selected phase's agents already through agentRows", () => {
    expect(workflowPhases(oneWorkflow(workflow), 0, 0).agents).toEqual(agentRows(workflow.phases[0].agents));
  });

  it("keys two id-less agents of a phase apart, so patchList never sees a duplicate", () => {
    expect(workflowPhases(oneWorkflow(workflow), 0, 1).agents.map((row) => row.key)).toEqual([
      "agent-0",
      "agent-1",
    ]);
  });

  it("keys a phase through the one keying function rather than a rule of its own", () => {
    const named = workflowPhases(oneWorkflow({ phases: [{ id: "read", title: "Read" }, { title: "Write" }] }), 0, 0);
    expect(named.phases.map((phase) => phase.key)).toEqual(["read", "phase-1"]);
  });

  it("clamps a selection the workflow no longer has", () => {
    expect(workflowPhases(oneWorkflow(workflow), 0, 9).phases[0].selected).toBe(true);
    expect(workflowPhases(oneWorkflow(workflow), 0, 9).agents).toEqual(agentRows(workflow.phases[0].agents));
  });

  it("gives nothing for a workflow carrying no phases", () => {
    expect(workflowPhases(null, 0, 0)).toEqual({ phases: [], agents: [] });
    expect(workflowPhases(oneWorkflow({ id: "w1" }), 0, 0)).toEqual({ phases: [], agents: [] });
  });

  it("reads the chosen workflow's phases itself, so no caller carries a raw phases array", () => {
    const second = { id: "w2", name: "Ship", phases: [{ title: "Tag", agents: [{ id: "b1", label: "Tagger" }] }] };
    const both = { workflows: [workflow, second] };
    expect(workflowPhases(both, 1, 0).phases.map((phase) => phase.title)).toEqual(["Tag"]);
    expect(workflowPhases(both, 9, 0).phases.map((phase) => phase.title)).toEqual(["Read", "Write"]);
  });
});

describe("the workflow the viewer shows", () => {
  const reviewSweep = { id: "w1", name: "Review", state: "running", phases: [{ title: "Read", agents: [] }] };
  const fixtureSweep = { id: "w2", name: "Fixtures", state: "done", phases: [{ title: "Write", agents: [] }] };
  const both = { workflows: [reviewSweep, fixtureSweep] };

  it("is the one the reader chose, and carries no raw phases array", () => {
    expect(openWorkflow(both, 1).name).toBe("Fixtures");
    expect(openWorkflow(both, 1)).not.toHaveProperty("phases");
    expect(workflowPhases(both, 1, 0).phases.map((phase) => phase.title)).toEqual(["Write"]);
  });

  it("falls back to the first when the choice is out of range", () => {
    expect(openWorkflow(both, 9).name).toBe("Review");
    expect(openWorkflow(both).name).toBe("Review");
  });

  it("is nothing at all when no workflow is running", () => {
    expect(openWorkflow({ shells: [] }, 0)).toBe(null);
  });

  it("offers a choice per workflow, pressing the chosen one", () => {
    expect(workflowChoicesWorthOffering(both, 1)).toEqual([
      { ...surfaceRows(WORKFLOW_ENTRY_KIND, both)[0], index: 0, selected: false },
      { ...surfaceRows(WORKFLOW_ENTRY_KIND, both)[1], index: 1, selected: true },
    ]);
    expect(workflowChoicesWorthOffering(both, 9)[0].selected).toBe(true);
  });

  it("offers no choice at all while there is only one workflow to look at", () => {
    expect(workflowChoicesWorthOffering({ workflows: [reviewSweep] }, 0)).toEqual([]);
    expect(workflowChoicesWorthOffering({}, 0)).toEqual([]);
  });
});

describe("surfaceStateMark", () => {
  it("names a mark and labels it for every kind", () => {
    expect(surfaceStateMark("checklist", "blocked")).toEqual({ mark: "blocked", label: "Blocked" });
    expect(surfaceStateMark("workflows", "failed").mark).toBe("error");
    expect(surfaceStateMark("shells", "running").mark).toBe("running");
    expect(surfaceStateMark("subagents", "queued").mark).toBe("pending");
  });

  it("returns a mark NAME the glyph table knows, never a glyph", () => {
    const { mark, label } = surfaceStateMark("checklist", "completed");
    expect(outcomeMarkHtml(mark, label)).not.toBe("");
    expect(mark).not.toContain("✓");
  });

  it("claims nothing about an unrecognised state token", () => {
    expect(surfaceStateMark("shells", "banana")).toBe(null);
    expect(surfaceStateMark("shells", undefined)).toBe(null);
  });

  it("claims nothing about a kind it does not know", () => {
    expect(surfaceStateMark("sonnets", "running")).toBe(null);
  });
});

describe("rowActions", () => {
  it("shapes each action the way menuButtonMarkup takes it, and carries the message to send", () => {
    const actions = rowActions("workflows", { id: "w1", name: "Review", state: "running" });
    expect(actions.length).toBeGreaterThan(0);
    for (const action of actions) {
      expect(Object.keys(action).sort()).toEqual(["description", "id", "label", "message"]);
      expect(typeof action.message).toBe("string");
      expect(action.message.length).toBeGreaterThan(0);
    }
    expect(actions[0].message).toContain("Review");
  });

  it("names the agent kind once, so the row renderer and the state marks pick the same one", () => {
    expect(AGENT_ENTRY_KIND).toBe("subagents");
    const agents = [{ id: "a1", label: "Reader", state: "running" }];
    expect(surfaceRows(AGENT_ENTRY_KIND, { [AGENT_ENTRY_KIND]: agents })).toEqual(agentRows(agents));
    expect(agentRows(agents)[0].stateMark).toEqual(surfaceStateMark(AGENT_ENTRY_KIND, "running"));
    expect(rowActions(AGENT_ENTRY_KIND, agents[0]).length).toBeGreaterThan(0);
  });

  it("calls an agent row an agent, since one renderer serves the workflow and subagent viewers", () => {
    for (const action of rowActions(AGENT_ENTRY_KIND, { id: "a1", label: "Reader" })) {
      expect(`${action.id} ${action.label} ${action.description} ${action.message}`).not.toContain("subagent");
      expect(`${action.description} ${action.message}`).toContain("agent");
    }
  });

  it("offers an action for every kind, naming what the row is", () => {
    expect(rowActions("subagents", { id: "a1", label: "Reader" })[0].message).toContain("Reader");
    expect(rowActions("shells", { id: "s1", description: "npm test" })[0].message).toContain("npm test");
    expect(rowActions("checklist", { id: "t1", subject: "Write the test" })[0].message).toContain(
      "Write the test",
    );
  });

  it("offers nothing for a kind it does not know", () => {
    expect(rowActions("sonnets", { id: "x" })).toEqual([]);
    expect(rowActions("workflows", null)).toEqual([]);
  });
});

describe("the remembered open pill", () => {
  it("is null for a key nothing was written under", () => {
    expect(readOpenSurface("issue-1:agent-1", memoryStorage())).toBe(null);
  });

  it("round-trips the kind under the same key", () => {
    const storage = memoryStorage();
    writeOpenSurface("issue-1:agent-1", "shells", storage);
    expect(readOpenSurface("issue-1:agent-1", storage)).toBe("shells");
    expect(readOpenSurface("issue-1:agent-2", storage)).toBe(null);
  });

  it("forgets a stored value this client cannot render", () => {
    const storage = memoryStorage();
    writeOpenSurface("issue-1:agent-1", "sonnets", storage);
    expect(readOpenSurface("issue-1:agent-1", storage)).toBe(null);
  });

  it("survives a storage that refuses to answer", () => {
    const refusing = refusingStorage();
    expect(readOpenSurface("k", refusing)).toBe(null);
    expect(() => writeOpenSurface("k", "shells", refusing)).not.toThrow();
    expect(() => writeOpenSurface("k", null, refusing)).not.toThrow();
  });

  it("clears the memory itself when no kind is open, so no caller spells the empty value", () => {
    const storage = memoryStorage();
    writeOpenSurface("issue-1:agent-1", "shells", storage);
    writeOpenSurface("issue-1:agent-1", null, storage);
    expect(readOpenSurface("issue-1:agent-1", storage)).toBe(null);
    expect([...storage.entries.keys()]).toEqual([]);
  });

  it("reaches storage only through the injected object", () => {
    const storage = memoryStorage();
    writeOpenSurface("issue-1:agent-1", "shells", storage);
    expect([...storage.entries.keys()].length).toBe(1);
  });
});

describe("the wire the bridge actually builds", () => {
  const recorded = recordedSurfaces();

  it("counts a pill for every kind the recorded streams carry", () => {
    expect(surfacePills(recorded).map((pill) => [pill.kind, pill.count])).toEqual([
      ["workflows", 1],
      ["subagents", 1],
      ["shells", 1],
      ["checklist", 3],
    ]);
    expect(surfacePills(recorded).every((pill) => pill.live === false)).toBe(true);
  });

  it("reads every field name the bridge writes on a subagent", () => {
    const [row] = surfaceRows(AGENT_ENTRY_KIND, recorded);
    expect(row.label).toBe(recorded.subagents[0].label);
    expect(row.state).toBe("done");
    expect(row.stateMark).toEqual(surfaceStateMark(AGENT_ENTRY_KIND, "done"));
    expect(row.tokens).toBe(recorded.subagents[0].tokens);
    expect(row.toolCalls).toBe(recorded.subagents[0].tool_calls);
    expect(row.callSequence).toBe(recorded.subagents[0].call_sequence);
    expect(row.lastTool).toBe("Read Reading README.md");
    expect(row.duration).not.toBe("");
  });

  it("reads every field name the bridge writes on a shell and on a checklist item", () => {
    const [shell] = surfaceRows("shells", recorded);
    expect(shell.description).toBe(recorded.shells[0].description);
    expect(shell.exitCode).toBe(0);
    expect(shell.stateMark).toEqual(surfaceStateMark("shells", "done"));
    const items = surfaceRows("checklist", recorded);
    expect(items.map((item) => item.subject)).toEqual(recorded.checklist.map((item) => item.subject));
    expect(items.map((item) => item.description)).toEqual(recorded.checklist.map((item) => item.description));
    expect(items.every((item) => item.stateMark !== null)).toBe(true);
  });

  it("reads a workflow's phases and its agents' field names", () => {
    const [workflow] = surfaceRows(WORKFLOW_ENTRY_KIND, recorded);
    expect(workflow.name).toBe("readme-analysis");
    expect(workflow.phaseCount).toBe(2);
    const { phases, agents } = workflowPhases(recorded, 0, 0);
    expect(phases.map((phase) => [phase.title, phase.done, phase.total])).toEqual([
      ["Read", 2, 2],
      ["Summarize", 1, 1],
    ]);
    expect(agents.map((agent) => agent.label)).toEqual(["line-counter", "char-counter"]);
    expect(agents[0].model).toBe(recorded.workflows[0].phases[0].agents[0].model);
    expect(agents[0].tokens).toBe(recorded.workflows[0].phases[0].agents[0].tokens);
  });
});

describe("the model is pure", () => {
  it("touches no DOM, no bridge and no storage of its own", () => {
    const source = coreSourceOf("agentSurfacesModel.js");
    expect(source).not.toContain("document");
    expect(source).not.toContain("App.call");
    expect(source).not.toContain("fetch(");
    expect(source.match(/localStorage/g) || []).toHaveLength(2);
  });
});
