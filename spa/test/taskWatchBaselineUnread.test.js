import { describe, expect, it } from "vitest";
import { taskUnreadCount } from "../src/core/taskUnread.js";
import { comment, event, task } from "./trackerWireFixture.js";

const agentSays = (id) => comment({ id, author: { kind: "agent", agent_id: "a1" } });
const watchedAt = (id) => event({ id, kind: "watched", actor: { kind: "user" } });
const reading = (timeline, listed = {}, detail = listed) => taskUnreadCount(task({ watched: true, ...listed }),
  { task: task({ watched: true, ...detail }), timeline });

describe("cached task counts since watching began (#474)", () => {
  it("counts only news after the watch baseline, including the asking comment", () => {
    const timeline = [agentSays("tc-01"), agentSays("tc-02"), watchedAt("te-03"), agentSays("tc-04")];
    expect(reading(timeline, { watch_started_after: "tc-01" })).toBe(2);
  });

  it("uses the most recent legacy Watched event when no baseline was persisted", () => {
    const timeline = [agentSays("tc-01"), watchedAt("te-02"), agentSays("tc-03"),
      watchedAt("te-04"), agentSays("tc-05")];
    expect(reading(timeline)).toBe(1);
  });

  it("takes the newer cached baseline and compares it with the read mark", () => {
    const timeline = [agentSays("tc-01"), agentSays("tc-02"), watchedAt("te-03"), agentSays("tc-04"), agentSays("tc-05")];
    expect(reading(timeline, { watch_started_after: "te-03" }, { watch_started_after: "tc-01" })).toBe(2);
    expect(reading(timeline, { watch_started_after: "tc-01" }, { watch_started_after: "te-03", read_through: "tc-04" })).toBe(1);
  });

  it("preserves watched-from-creation news without a baseline or Watched event", () => {
    expect(reading([agentSays("tc-01")])).toBe(1);
  });

  it("keeps the canonical list count while its cached timeline is older", () => {
    const listed = { watch_started_after: "tc-01", unread_count: 1, updated_at: "2026-10-10T15:00:00Z" };
    expect(reading([agentSays("tc-01")], listed, { ...listed, updated_at: "2026-10-10T14:00:00Z" })).toBe(1);
  });
});
