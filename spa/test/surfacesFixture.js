export const SPAWNING_CALL_SEQUENCE = 12;

export const surfacesSnapshot = (overrides = {}) => ({
  workflows: [
    {
      id: "wf-1",
      name: "Review sweep",
      state: "running",
      phases: [
        { title: "Read", agents: [{ id: "a1", label: "reader", state: "running" }] },
        { title: "Judge", agents: [{ id: "a2", label: "judge", state: "queued" }] },
      ],
    },
  ],
  subagents: [
    { id: "s1", label: "parser reviewer", state: "done", call_sequence: SPAWNING_CALL_SEQUENCE },
    { id: "s2", label: "fixture writer", state: "running" },
  ],
  shells: [{ id: "sh1", description: "cargo test", state: "running", tail: ["running 12 tests"] }],
  checklist: [{ id: "c1", subject: "Land the fold", state: "in_progress" }],
  ...overrides,
});
