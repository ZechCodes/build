/** @vitest-environment jsdom */
// A message that handed over a task.
//
// Assignment is dispatch: the task arrives in the agent's conversation as an
// ordinary message carrying a `from_task` envelope, and this client draws that
// as a card above the body. It is still a message and still reads in sequence;
// the card is how it is drawn, not a separate kind of thing.

import { describe, expect, it } from "vitest";
import { createThreadState, threadHtml, wireThreadArrivals } from "../src/core/thread.js";
import { itemsAtDetailLevel } from "../src/core/conversationDetail.js";
import { taskCardHtml } from "../src/core/trackerMessageCard.js";

const HERE = { deviceId: "dev-1", projectId: "proj-1" };

const envelope = (over = {}) => ({
  task_id: "task-01K5Z",
  number: 12,
  title: "Kanban drag does not persist",
  body: "Dragging a card to In review leaves it where it was after a reload.",
  links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: null },
  ...over,
});

const handedOver = (over = {}) => ({
  type: "message",
  data: {
    id: "message-9",
    sequence: 9,
    role: "user",
    body: "#12 Kanban drag does not persist\n\nDragging a card…",
    from_task: envelope(over),
  },
});

const paint = (items, place = HERE) => {
  document.body.innerHTML = threadHtml({ id: "conversation-3", items }, { place });
  return document.querySelector(".thread-task");
};

describe("the card", () => {
  it("draws the number, the title and a link to the task page", () => {
    const card = paint([handedOver()]);
    expect(card.querySelector(".thread-task-link").textContent).toBe("#12");
    expect(card.querySelector(".thread-task-link").getAttribute("href"))
      .toBe("#/device/dev-1/project/proj-1/tasks/task-01K5Z");
    expect(card.querySelector(".thread-task-title").textContent).toBe("Kanban drag does not persist");
  });

  // It is still a message: the body is the task rendered as prose, and a
  // harness that never learns about `from_task` receives the whole of it.
  it("draws above the body rather than in place of it", () => {
    paint([handedOver()]);
    const message = document.querySelector(".thread-message");
    expect(message.querySelector(".thread-body")).not.toBeNull();
    const nodes = [...message.querySelector(".thread-comment-card").children];
    expect(nodes.indexOf(document.querySelector(".thread-task")))
      .toBeLessThan(nodes.indexOf(message.querySelector(".thread-body")));
  });

  // The card is opt-in on the record, so a conversation that has never carried
  // one reads exactly as it always has.
  it("draws nothing at all on a message with no envelope", () => {
    expect(paint([{ type: "message", data: { id: "m1", sequence: 1, role: "user", body: "hello" } }])).toBeNull();
  });

  // `from_agent` says an agent did the assigning and is absent when the user
  // did; both hand over the same task, so both draw the card.
  it("draws beside the from_agent line rather than instead of it", () => {
    paint([handedOver()].map((item) => ({
      ...item,
      data: { ...item.data, from_agent: { id: "agent-01K5Y", owner: { kind: "project", id: "proj-1", name: "Build" } } },
    })));
    expect(document.querySelector(".thread-from")).not.toBeNull();
    expect(document.querySelector(".thread-task")).not.toBeNull();
  });

  // A conversation rendered without a place has nowhere to send the reader.
  it("draws the number unlinked when the reader is standing nowhere", () => {
    const card = paint([handedOver()], { deviceId: null, projectId: null });
    expect(card.querySelector("a")).toBeNull();
    expect(card.querySelector(".thread-task-link").textContent).toBe("#12");
  });
});

describe("the links on the card", () => {
  // A branch names itself; a workspace and a conversation are minted ids the
  // feed has to name, and the task page one press away has that rail.
  it("carries the branch, and leaves the ids to the task page", () => {
    const card = paint([handedOver({
      links: {
        workspace_ids: ["ws-3f2a91c4"], branches: ["build/tasks-spa"],
        commits: ["c8381faa"], conversation_ids: ["run-5d90b1e7"], parent_task_id: null,
      },
    })]);
    const links = [...card.querySelectorAll(".thread-task-links a")];
    expect(links.map((link) => link.textContent)).toEqual(["build/tasks-spa"]);
    expect(links[0].getAttribute("href")).toBe("#/device/dev-1/project/proj-1/branch/build%2Ftasks-spa/changes");
  });

  it("carries a parent task, which names itself too", () => {
    const card = paint([handedOver({
      links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: "task-01K5A" },
    })]);
    expect(card.querySelector(".thread-task-links a").getAttribute("href"))
      .toBe("#/device/dev-1/project/proj-1/tasks/task-01K5A");
  });

  it("draws no link row at all when the task links nothing", () => {
    expect(paint([handedOver()]).querySelector(".thread-task-links")).toBeNull();
  });
});

describe("the five-line fold", () => {
  // The same rule, the same length and the same press a long arrival folds by
  // (core/thread.js): one reading of "does this bury the conversation".
  const long = Array.from({ length: 12 }, (_, line) => `line ${line + 1}`).join("\n");

  const pressIn = () => document.querySelector(".thread-task .thread-arrival-press");
  const bodyIn = () => document.querySelector(".thread-task-body");

  // A "Show more" that reveals nothing is worse than no fold.
  it("offers no press on a body that is already whole", () => {
    paint([handedOver({ body: "one line" })]);
    expect(pressIn()).toBeNull();
    expect(bodyIn().classList.contains("thread-arrival-folded")).toBe(false);
  });

  it("comes up folded, and unfolds on a press", () => {
    const state = createThreadState();
    paint([handedOver({ body: long })]);
    wireThreadArrivals(document.body, state);
    expect([pressIn().getAttribute("aria-expanded"), bodyIn().classList.contains("thread-arrival-folded")])
      .toEqual(["false", true]);
    pressIn().click();
    expect([pressIn().getAttribute("aria-expanded"), bodyIn().classList.contains("thread-arrival-folded")])
      .toEqual(["true", false]);
  });

  // The press says what it controls, which is how one wiring serves the
  // arrival's card and the task's body without either knowing about the other.
  it("names the body it controls", () => {
    paint([handedOver({ body: long })]);
    expect(pressIn().getAttribute("aria-controls")).toBe(bodyIn().id);
  });

  // What a reader opened stays open across the next repaint, on the same
  // thread-state memory a long arrival uses.
  it("draws open again on the next repaint", () => {
    const state = createThreadState();
    paint([handedOver({ body: long })]);
    wireThreadArrivals(document.body, state);
    pressIn().click();
    document.body.innerHTML = threadHtml({ id: "conversation-3", items: [handedOver({ body: long })] }, { place: HERE, threadState: state });
    expect(bodyIn().classList.contains("thread-arrival-folded")).toBe(false);
    expect(pressIn().getAttribute("aria-expanded")).toBe("true");
  });

  // An agent-assigned task is an arrival AND carries a card: the report folds
  // and the task's body folds, and neither press moves the other.
  it("folds independently of the arrival that carried it", () => {
    const state = createThreadState();
    const item = handedOver({ body: long });
    item.data.body = long;
    item.data.from_agent = { id: "agent-01K5Y", owner: { kind: "project", id: "proj-1", name: "Build" } };
    paint([item]);
    wireThreadArrivals(document.body, state);
    const presses = [...document.querySelectorAll(".thread-arrival-press")];
    expect(presses).toHaveLength(2);
    presses[0].click();
    expect(presses[1].getAttribute("aria-expanded")).toBe("false");
  });
});

describe("the card on its own", () => {
  it("says nothing for an envelope that names no task", () => {
    expect(taskCardHtml(null)).toBe("");
    expect(taskCardHtml({ number: 12 })).toBe("");
  });
});

// How a handed-over task reads at each of the three detail levels
// (core/conversationDetail.js).
//
// The levels do not know about `from_task` and should not: what they read is
// whether a message is this conversation's DIALOGUE or correspondence with
// somewhere else, and `from_agent`/`sent_to` is the whole of that question. A
// task hand-off falls on either side of it depending on who did the
// assigning, which is the right answer for both — so this is the contract
// between the two, pinned from the tracker's side.
describe("a handed-over task at each detail level", () => {
  /** The user assigned it: an instruction arriving on the user's side of this
   *  conversation, from the person reading it. This agent's dialogue. */
  const assignedByUser = handedOver();

  /** Another agent assigned it: the hand-off carries `from_agent`, and it is
   *  drawn as an arrival — correspondence with a conversation elsewhere. */
  const assignedByAgent = () => {
    const item = handedOver();
    item.data.id = "message-10";
    item.data.sequence = 10;
    item.data.from_agent = { id: "agent-01K5Y", owner: { kind: "project", id: "proj-1", name: "Build" } };
    return item;
  };

  const both = () => [assignedByUser, assignedByAgent()];
  const idsAt = (level) => itemsAtDetailLevel(both(), level).map((item) => item.data.id);

  it("keeps both at All", () => {
    expect(idsAt("all")).toEqual(["message-9", "message-10"]);
  });

  // Both are messages, whoever sent them; only the activity goes.
  it("keeps both at All messages", () => {
    expect(idsAt("messages")).toEqual(["message-9", "message-10"]);
  });

  // The one the reader assigned is the reader talking to this agent. Hiding it
  // at the level called "this agent's messages and yours" would hide the
  // instruction the agent is working from.
  it("keeps the one the user assigned at Agent only", () => {
    expect(idsAt("agent")).toEqual(["message-9"]);
  });

  // The one another agent assigned is correspondence, and goes with the rest
  // of it.
  it("drops the one another agent assigned at Agent only", () => {
    expect(idsAt("agent")).not.toContain("message-10");
  });

  it("still draws the card on what Agent only kept", () => {
    const card = paint(itemsAtDetailLevel(both(), "agent"));
    expect(card).not.toBeNull();
    expect(card.querySelector(".thread-task-link").textContent).toBe("#12");
    expect(document.querySelectorAll(".thread-task")).toHaveLength(1);
  });

  it("draws both cards at All messages, one of them as an arrival", () => {
    paint(itemsAtDetailLevel(both(), "messages"));
    expect(document.querySelectorAll(".thread-task")).toHaveLength(2);
    expect(document.querySelectorAll(".thread-message.from-agent")).toHaveLength(1);
  });
});
