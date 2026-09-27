// #183: which timeline entries count as unread — comments, and changes to who
// holds an issue or where it stands; bookkeeping does not (Zech, "Correct,
// no"). The SPA's fallback count, over a cached timeline, is held to the cases
// the bridge's `unread_count` answers (bridge app::tests::tracker_unread_kinds,
// printed on demand so plain `npx vitest run` needs no prepared file).

import { execFile } from "node:child_process";
import { resolve } from "node:path";
import { promisify } from "node:util";
import { beforeAll, describe, expect, it } from "vitest";
import { issueUnreadCount } from "../src/core/issueUnread.js";
import { comment, issue } from "./trackerWireFixture.js";

const run = promisify(execFile);
const bridgeRoot = resolve(process.cwd(), "../bridge");

/** The SPA's count for a watched issue whose cached timeline is `timeline`:
 *  no list count, so the timeline is what is read. */
const counted = (readThrough, timeline) => {
  const watched = issue({ id: "issue-1", watched: true, read_through: readThrough ?? undefined, unread_count: undefined });
  return issueUnreadCount(watched, { issue: watched, timeline });
};

const agent = { kind: "agent", agent_id: "agent-01K5ZFILER" };
const event = (kind, id) => ({ type: "event", id, issue_id: "issue-1", at: "2026-09-27T02:00:00Z", actor: agent, kind, payload: {} });

describe("the SPA's count", () => {
  const mark = "ie-01K5Z000000000000000000010";

  it("leaves out an agent filing and tracking the issue", () => {
    expect(counted(mark, [event("created", "ie-01K5Z000000000000000000011"), event("tracked", "ie-01K5Z000000000000000000012")])).toBe(0);
  });

  it("counts an agent's comment and move", () => {
    const said = comment({ id: "ic-01K5Z000000000000000000011", author: agent });
    expect(counted(mark, [said, event("moved", "ie-01K5Z000000000000000000012")])).toBe(2);
  });
});

describe("the bridge's cases", () => {
  let cases;

  beforeAll(async () => {
    const { stdout } = await run("cargo", ["test", "--lib", "tracker_unread_kinds", "--", "--nocapture"], {
      cwd: bridgeRoot,
      env: { ...process.env, BUILD_PRINT_UNREAD_KINDS: "1" },
      maxBuffer: 2 * 1024 * 1024,
      timeout: 600_000,
    });
    const line = stdout.split(/\r?\n/).find((candidate) => candidate.startsWith("BUILD_UNREAD_KINDS="));
    expect(line, "Rust must print BUILD_UNREAD_KINDS").toBeDefined();
    ({ cases } = JSON.parse(line.slice("BUILD_UNREAD_KINDS=".length)));
  }, 620_000);

  it("covers filing and tracking, and a comment with a move", () => {
    const unreadOf = (name) => cases.find((one) => one.name === name)?.unread;
    expect(unreadOf("an agent filed and tracked it")).toBe(0);
    expect(unreadOf("an agent commented and moved it")).toBe(2);
    expect(cases.length).toBeGreaterThan(18);
  });

  it("answers every one the way the bridge does", () => {
    for (const one of cases) {
      expect({ name: one.name, unread: counted(one.read_through, one.timeline) }).toEqual({ name: one.name, unread: one.unread });
    }
  });
});
