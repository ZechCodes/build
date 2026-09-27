/** @vitest-environment jsdom */
// Three incoming kinds, three looks (#42).
//
// The maintainer, on a workspace agent's conversation: "Messages to agents and
// messages from the user are different things and should never look the same.
// Agents receive 3 kinds of messages: from the user, from other agents, and
// notifications. From the user is right aligned and the dark bluish color,
// messages from agents are left aligned collapsed and green, notifications are
// a single line left aligned."
//
// Two of the three were wearing another's look. Build's restart notice — which
// says "assume nothing you were doing finished" — was drawn as a right-aligned
// bubble in the reader's own colour, because `from_build` was read after a
// special case for one kind of notice and fell through to the bubble every
// instruction wears.
//
// The readings used to be a ladder of separate conditions, each able to be
// true. They are one classifier now, and the table below is the point: it is
// not enough that each kind draws right, no two may draw ALIKE.

import { describe, expect, it } from "vitest";
import { INCOMING_KINDS, incomingKindOf, threadHtml } from "../src/core/thread.js";

const PLACE = { deviceId: "dev-1", projectId: "proj-1", projectName: "Build" };

const SENDER = {
  id: "project-01M2SCB",
  owner: { kind: "project", id: "proj-1", name: "Build" },
  topic: "Verify per-file roll",
};

/** Long enough that an arrival of this length is folded, so the fold is real. */
const LONG = Array.from({ length: 12 }, (_, line) => `line ${line + 1} of the brief`).join("\n");

const KINDS = {
  "from the user": { id: INCOMING_KINDS.user, data: { role: "user", body: "look at the retry path" } },
  "from another agent": { id: INCOMING_KINDS.agent, data: { role: "user", body: LONG, from_agent: SENDER } },
  "a notification": { id: INCOMING_KINDS.notice, data: { role: "user", body: `Build restarted at 2026-09-20T20:09:27Z and brought your session back. ${LONG}`, from_build: true } },
};

const row = (data) => {
  document.body.innerHTML = threadHtml(
    { id: "c-1", items: [{ type: "message", data: { id: "m-1", sequence: 1, ...data } }] },
    { place: PLACE },
  );
  return document.querySelector(".thread-message");
};

const classesOf = (element) => [...element.classList].sort().join(" ");

describe("which kind a message is", () => {
  for (const [name, { id, data }] of Object.entries(KINDS)) {
    it(`reads ${name} as ${id}`, () => {
      expect(incomingKindOf(data)).toBe(id);
    });
  }

  // The bug: a notice carries the reader's own role, and a sender mark of its
  // own would still not make it an arrival.
  it("lets Build's mark outrank the role and the sender both", () => {
    expect(incomingKindOf({ role: "user", from_build: true })).toBe(INCOMING_KINDS.notice);
    expect(incomingKindOf({ role: "user", from_build: true, from_agent: SENDER })).toBe(INCOMING_KINDS.notice);
  });

  it("lets a sender outrank the role", () => {
    expect(incomingKindOf({ role: "user", from_agent: SENDER })).toBe(INCOMING_KINDS.agent);
  });
});

describe("the look each kind gets", () => {
  it("puts the reader's own message right, in the reader's bubble", () => {
    const element = row(KINDS["from the user"].data);
    expect(element.classList.contains("user")).toBe(true);
    expect(element.classList.contains("thread-comment")).toBe(true);
    expect(element.querySelector(".thread-comment-card")).not.toBeNull();
    expect(element.querySelector(".thread-avatar")).not.toBeNull();
  });

  it("puts another agent's message left, named, and folded", () => {
    const element = row(KINDS["from another agent"].data);
    expect(element.classList.contains("from-agent")).toBe(true);
    expect(element.classList.contains("user")).toBe(false);
    expect(element.querySelector(".thread-from")).not.toBeNull();
    expect(element.querySelector(".thread-arrival-folded")).not.toBeNull();
    expect(element.querySelector(".thread-arrival-press")).not.toBeNull();
  });

  it("puts a notification on one left line, with no bubble either side", () => {
    const element = row(KINDS["a notification"].data);
    expect(element.classList.contains("thread-notice")).toBe(true);
    expect(element.classList.contains("thread-task-line")).toBe(true);
    expect(element.classList.contains("user")).toBe(false);
    expect(element.classList.contains("from-agent")).toBe(false);
    expect(element.querySelector(".thread-comment-card")).toBeNull();
    expect(element.querySelector(".thread-avatar")).toBeNull();
    // Its line is a summary; the whole of it is behind the press.
    expect(element.querySelector(".thread-task-notice").textContent).not.toContain("line 12 of the brief");
    expect(element.querySelector(".thread-notice-body").textContent).toContain("line 12 of the brief");
  });
});

// The point of the whole change: not that each is right, but that no two are
// the same. A ladder of conditions can make each case pass on its own and
// still let two kinds land in one look.
describe("no two kinds look alike", () => {
  const looks = Object.entries(KINDS).map(([name, { data }]) => [name, classesOf(row(data))]);

  it("gives each kind a class set of its own", () => {
    const seen = new Map();
    for (const [name, classes] of looks) {
      expect([name, seen.get(classes) ?? null]).toEqual([name, null]);
      seen.set(classes, name);
    }
    expect(seen.size).toBe(3);
  });

  // Alignment and colour come from these two, so sharing either is sharing the
  // look a reader actually sees.
  it("never lets two kinds share the reader's side or the arrival's", () => {
    const onTheRight = looks.filter(([, classes]) => classes.split(" ").includes("user"));
    const asAnArrival = looks.filter(([, classes]) => classes.split(" ").includes("from-agent"));
    expect(onTheRight.map(([name]) => name)).toEqual(["from the user"]);
    expect(asAnArrival.map(([name]) => name)).toEqual(["from another agent"]);
  });
});
