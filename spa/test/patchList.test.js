// @vitest-environment jsdom
// Only new, updated, or removed entries redraw.
//
// A list surface re-renders on every poll, and writing the whole thing in with
// innerHTML costs the reader everything the browser hangs off the old nodes: a
// half-typed reply, an open row menu, a selection, the scroll the browser was
// anchoring. So the paint is walked key by key — rows that are still there are
// the same nodes afterwards, rows that moved are moved rather than rebuilt, and
// a paint that says what the last one said writes nothing at all.

import { describe, expect, it, vi } from "vitest";
import { patchList, rekeyEntry } from "../src/core/patchList.js";

const listIn = (document, html = "") => {
  const list = document.createElement("ul");
  list.innerHTML = html;
  document.body.appendChild(list);
  return list;
};

/** Everything the paint moved, in the order the browser saw it. */
function mutationsOf(container, paint) {
  const observer = new MutationObserver(() => {});
  observer.observe(container, { childList: true, subtree: true, attributes: true, characterData: true });
  try {
    paint();
    return observer.takeRecords();
  } finally {
    observer.disconnect();
  }
}

/** Every node the paint attached or detached — the ones that redrew or moved. */
const nodesTouched = (records) =>
  new Set(records.flatMap((record) => [...record.addedNodes, ...record.removedNodes]));

const keysOf = (list) => [...list.children].map((child) => child.getAttribute("data-key"));
const labelsOf = (list) => [...list.querySelectorAll(".label")].map((label) => label.textContent);

const row = (entry) => `<li><span class="label">${entry.label}</span><button data-act>go</button></li>`;
const plan = { keyOf: (entry) => entry.id, render: row };
const entriesFor = (...ids) => ids.map((id) => ({ id, label: id.toUpperCase() }));

describe("painting a keyed list", () => {
  it("writes the entries in order and stamps each one with its key", () => {
    const list = listIn(document);

    patchList(list, entriesFor("a", "b", "c"), plan);

    expect(keysOf(list)).toEqual(["a", "b", "c"]);
    expect(labelsOf(list)).toEqual(["A", "B", "C"]);
  });

  it("hands back the entry elements in order", () => {
    const list = listIn(document);
    const painted = patchList(list, entriesFor("a", "b"), plan);
    expect(painted).toEqual([...list.children]);
  });

  it("takes an element from the render as readily as a string", () => {
    const list = listIn(document);

    patchList(list, entriesFor("a"), {
      ...plan,
      render: (entry) => {
        const item = document.createElement("li");
        item.textContent = entry.label;
        return item;
      },
    });

    expect(list.innerHTML).toBe(`<li data-key="a">A</li>`);
  });

  it("reads a key the render already stamped rather than stamping it twice", () => {
    const list = listIn(document);
    const stamped = { ...plan, render: (entry) => `<li data-key="${entry.id}">${entry.label}</li>` };

    patchList(list, entriesFor("a", "b"), stamped);
    const first = list.firstElementChild;
    const records = mutationsOf(list, () => patchList(list, entriesFor("a", "b"), stamped));

    expect(list.firstElementChild).toBe(first);
    expect(records).toEqual([]);
  });

  it("counts a number key and a string of it as the same entry", () => {
    const list = listIn(document);
    const numbered = { keyOf: (entry) => entry.id, render: (entry) => `<li>${entry.label}</li>` };

    patchList(list, [{ id: 7, label: "seven" }], numbered);
    const only = list.firstElementChild;
    patchList(list, [{ id: 7, label: "seven" }], numbered);

    expect(list.firstElementChild).toBe(only);
  });
});

describe("a paint that says what the last one said", () => {
  it("writes nothing at all", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c"), plan);

    const records = mutationsOf(list, () => patchList(list, entriesFor("a", "b", "c"), plan));

    expect(records).toEqual([]);
  });

  it("leaves every element the one it was", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c"), plan);
    const before = [...list.children];

    patchList(list, entriesFor("a", "b", "c"), plan);

    expect([...list.children]).toEqual(before);
  });
});

describe("an entry whose contents changed", () => {
  it("is rewritten in place, without losing the element holding it", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c"), plan);
    const [first, second, third] = [...list.children];

    const records = mutationsOf(list, () =>
      patchList(list, [{ id: "a", label: "A" }, { id: "b", label: "BEE" }, { id: "c", label: "C" }], plan),
    );

    expect([...list.children]).toEqual([first, second, third]);
    expect(labelsOf(list)).toEqual(["A", "BEE", "C"]);
    expect(records.map((record) => record.type)).toEqual(["characterData"]);
  });

  it("does not disturb the entries either side of it", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c"), plan);
    const [first, , third] = [...list.children];
    const firstLabel = first.querySelector(".label").firstChild;
    const thirdLabel = third.querySelector(".label").firstChild;

    const records = mutationsOf(list, () =>
      patchList(list, [{ id: "a", label: "A" }, { id: "b", label: "BEE" }, { id: "c", label: "C" }], plan),
    );

    expect(records.every((record) => !record.target.contains(firstLabel))).toBe(true);
    expect(records.every((record) => !record.target.contains(thirdLabel))).toBe(true);
  });

  it("is replaced, and wired again, when the render made it a different kind of element", () => {
    const list = listIn(document);
    const wire = vi.fn();
    patchList(list, entriesFor("a"), { ...plan, wire });
    const wasThere = list.firstElementChild;

    patchList(list, entriesFor("a"), { ...plan, wire, render: (entry) => `<div>${entry.label}</div>` });

    expect(list.firstElementChild).not.toBe(wasThere);
    expect(list.firstElementChild.tagName).toBe("DIV");
    expect(wire).toHaveBeenCalledTimes(2);
  });

  it("lands where it belongs when the render changed its tag and its place", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c"), plan);
    const [a, b] = [...list.children];

    patchList(list, entriesFor("c", "a", "b"), {
      ...plan,
      render: (entry) => (entry.id === "c" ? `<div>${entry.label}</div>` : row(entry)),
    });

    expect(keysOf(list)).toEqual(["c", "a", "b"]);
    expect([list.children[1], list.children[2]]).toEqual([a, b]);
    expect(list.firstElementChild.tagName).toBe("DIV");
  });
});

describe("entries arriving and leaving", () => {
  it("inserts a new entry without touching the ones already there", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "c"), plan);
    const [first, last] = [...list.children];

    const records = mutationsOf(list, () => patchList(list, entriesFor("a", "b", "c"), plan));

    expect(keysOf(list)).toEqual(["a", "b", "c"]);
    expect([list.children[0], list.children[2]]).toEqual([first, last]);
    expect([...nodesTouched(records)]).toEqual([list.children[1]]);
  });

  it("appends a new last entry without touching the ones already there", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b"), plan);
    const before = [...list.children];

    const records = mutationsOf(list, () => patchList(list, entriesFor("a", "b", "c"), plan));

    expect([list.children[0], list.children[1]]).toEqual(before);
    expect([...nodesTouched(records)]).toEqual([list.children[2]]);
  });

  it("removes the entry that left and nothing else", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c"), plan);
    const [first, gone, last] = [...list.children];

    const records = mutationsOf(list, () => patchList(list, entriesFor("a", "c"), plan));

    expect(keysOf(list)).toEqual(["a", "c"]);
    expect([...list.children]).toEqual([first, last]);
    expect([...nodesTouched(records)]).toEqual([gone]);
  });

  // A key finds the first element carrying it, so a second one is a ghost: it
  // would sit there through every later paint with no entry able to reach it.
  it("clears out a second element claiming a key already spoken for", () => {
    const list = listIn(document, `<li data-key="a">first</li><li data-key="a">ghost</li>`);
    const first = list.firstElementChild;

    patchList(list, entriesFor("a"), plan);

    expect([...list.children]).toEqual([first]);
    expect(labelsOf(list)).toEqual(["A"]);
  });

  it("empties the list when there are no entries left", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b"), plan);

    patchList(list, [], plan);

    expect(list.children.length).toBe(0);
  });
});

describe("entries changing places", () => {
  it("moves the one entry that moved", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c", "d"), plan);
    const [a, b, c, d] = [...list.children];

    const records = mutationsOf(list, () => patchList(list, entriesFor("d", "a", "b", "c"), plan));

    expect([...list.children]).toEqual([d, a, b, c]);
    expect([...nodesTouched(records)]).toEqual([d]);
  });

  it("moves the one entry that moved when it went the other way", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c", "d"), plan);
    const [a, b, c, d] = [...list.children];

    const records = mutationsOf(list, () => patchList(list, entriesFor("b", "c", "d", "a"), plan));

    expect([...list.children]).toEqual([b, c, d, a]);
    expect([...nodesTouched(records)]).toEqual([a]);
  });

  it("swaps a neighbouring pair by moving one of them", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c"), plan);
    const [a, b, c] = [...list.children];

    const records = mutationsOf(list, () => patchList(list, entriesFor("a", "c", "b"), plan));

    expect([...list.children]).toEqual([a, c, b]);
    expect(nodesTouched(records).size).toBe(1);
  });

  // The reason for reading the longest run already in order: on a list long
  // enough to matter, pulling one row to the top must cost one move, not one
  // per row after it.
  it("moves one entry to the top of a long list at the cost of one move", () => {
    const list = listIn(document);
    const many = entriesFor(...Array.from({ length: 200 }, (unused, index) => `e${index}`));
    patchList(list, many, plan);
    const pulled = list.lastElementChild;

    const records = mutationsOf(list, () => patchList(list, [many[199], ...many.slice(0, 199)], plan));

    expect(list.firstElementChild).toBe(pulled);
    expect([...nodesTouched(records)]).toEqual([pulled]);
  });

  it("rebuilds nothing when the whole list is reversed", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c", "d"), plan);
    const before = [...list.children];

    patchList(list, entriesFor("d", "c", "b", "a"), plan);

    expect([...list.children]).toEqual([...before].reverse());
  });

  it("moves an entry with the state-preserving move when the browser has one", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c"), plan);
    const [a, , c] = [...list.children];
    list.moveBefore = vi.fn((node, anchor) => Object.getPrototypeOf(list).insertBefore.call(list, node, anchor));

    patchList(list, entriesFor("c", "a", "b"), plan);

    expect(list.moveBefore).toHaveBeenCalledTimes(1);
    expect(list.moveBefore).toHaveBeenCalledWith(c, a);
    expect(keysOf(list)).toEqual(["c", "a", "b"]);
  });
});

describe("wiring an entry's handlers", () => {
  it("runs once, when the element is made", () => {
    const list = listIn(document);
    const wire = vi.fn();

    patchList(list, entriesFor("a", "b"), { ...plan, wire });
    patchList(list, [{ id: "a", label: "CHANGED" }, { id: "b", label: "B" }], { ...plan, wire });
    patchList(list, entriesFor("b", "a"), { ...plan, wire });

    expect(wire).toHaveBeenCalledTimes(2);
    expect(wire.mock.calls.map(([element]) => element.getAttribute("data-key"))).toEqual(["a", "b"]);
  });

  // Wiring after the list has settled means a handler can measure the row it
  // is on, or scroll it into view, the moment it is attached.
  it("runs on an element already standing in its finished list", () => {
    const list = listIn(document);
    const seen = [];
    const wire = (element) => seen.push({ connected: element.isConnected, order: keysOf(list).join("") });

    patchList(list, entriesFor("a", "b"), { ...plan, wire });

    expect(seen).toEqual([
      { connected: true, order: "ab" },
      { connected: true, order: "ab" },
    ]);
  });

  it("is handed the element and the entry it was made from", () => {
    const list = listIn(document);
    const wire = vi.fn();

    patchList(list, [{ id: "a", label: "A" }], { ...plan, wire });

    expect(wire).toHaveBeenCalledWith(list.firstElementChild, { id: "a", label: "A" });
  });

  it("only wires the entries that are new to this paint", () => {
    const list = listIn(document);
    const wire = vi.fn();
    patchList(list, entriesFor("a", "b"), { ...plan, wire });
    wire.mockClear();

    patchList(list, entriesFor("a", "c", "b"), { ...plan, wire });

    expect(wire.mock.calls.map(([element]) => element.getAttribute("data-key"))).toEqual(["c"]);
  });

  // Handlers attached once outlive every later paint, so they must read the
  // state of the moment rather than the entry they were handed — the key is
  // what stays true, and looking the row up by it is what views already do.
  it("leaves a handler working after the entry was patched and moved", () => {
    const list = listIn(document);
    const clicked = [];
    let entries = entriesFor("a", "b");
    const wire = (element, entry) => {
      element.querySelector("[data-act]").onclick = () => {
        clicked.push(entries.find((candidate) => candidate.id === entry.id).label);
      };
    };

    patchList(list, entries, { ...plan, wire });
    entries = [{ id: "b", label: "B" }, { id: "a", label: "LATER" }];
    patchList(list, entries, { ...plan, wire });
    list.querySelector('[data-key="a"] [data-act]').click();

    expect(clicked).toEqual(["LATER"]);
  });
});

describe("what a kept entry gets to hold on to", () => {
  const typing = (list, key) => {
    const field = list.querySelector(`[data-key="${key}"] input`);
    field.value = "half a thought";
    field.focus();
    field.setSelectionRange(4, 4);
    return field;
  };
  const withField = {
    keyOf: (entry) => entry.id,
    render: (entry) => `<li><span class="label">${entry.label}</span><input></li>`,
  };
  const stillTyping = (field) => ({
    focused: document.activeElement === field,
    value: field.value,
    caret: field.selectionStart,
  });
  const untouched = { focused: true, value: "half a thought", caret: 4 };

  it("keeps the caret through a paint that changed the entry's own words", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b"), withField);
    const field = typing(list, "b");

    patchList(list, [{ id: "a", label: "A" }, { id: "b", label: "BEE" }], withField);

    expect(stillTyping(field)).toEqual(untouched);
  });

  it("keeps the caret through an entry arriving above it", () => {
    const list = listIn(document);
    patchList(list, entriesFor("b"), withField);
    const field = typing(list, "b");

    patchList(list, entriesFor("a", "b"), withField);

    expect(stillTyping(field)).toEqual(untouched);
    expect(keysOf(list)).toEqual(["a", "b"]);
  });

  it("keeps the caret through an entry leaving above it", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b"), withField);
    const field = typing(list, "b");

    patchList(list, entriesFor("b"), withField);

    expect(stillTyping(field)).toEqual(untouched);
  });

  it("keeps the caret through a reorder that left it where it was", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c"), withField);
    const field = typing(list, "b");

    patchList(list, entriesFor("c", "a", "b"), withField);

    expect(stillTyping(field)).toEqual(untouched);
    expect(keysOf(list)).toEqual(["c", "a", "b"]);
  });

  // Moving a node is the browser's one chance to drop focus, and only a
  // browser with `moveBefore` keeps it. The element and everything typed into
  // it are ours to keep either way: the entry is moved, never rebuilt.
  it("keeps the element and the words even when the entry itself is moved", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c"), withField);
    const row = list.querySelector('[data-key="c"]');
    const field = typing(list, "c");

    patchList(list, entriesFor("c", "a", "b"), withField);

    expect(list.firstElementChild).toBe(row);
    expect(list.querySelector('[data-key="c"] input')).toBe(field);
    expect({ value: field.value, caret: field.selectionStart }).toEqual({ value: "half a thought", caret: 4 });
  });

  // A row's menu is open because the model says so, the way views already
  // hold it (`openMenuKey`), so the render is what says it is open. What the
  // patch owes it is the same element on the other side of every paint: the
  // menu that is open stays the menu that was open, rather than a rebuilt one
  // the reader's pointer is no longer over.
  it("keeps a row's open menu the element it was", () => {
    const list = listIn(document);
    let openKey = null;
    const withMenu = {
      keyOf: (entry) => entry.id,
      render: (entry) =>
        `<li><details${entry.id === openKey ? " open" : ""}><summary>${entry.label}</summary></details></li>`,
    };
    patchList(list, entriesFor("a", "b"), withMenu);
    openKey = "b";
    patchList(list, entriesFor("a", "b"), withMenu);
    const menu = list.querySelector('[data-key="b"] details');

    patchList(list, [{ id: "a", label: "AY" }, { id: "b", label: "B" }, { id: "c", label: "C" }], withMenu);
    patchList(list, entriesFor("c", "b", "a"), withMenu);

    expect(list.querySelector('[data-key="b"] details')).toBe(menu);
    expect(menu.open).toBe(true);
  });

  it("keeps what a handler hung off the element through every paint", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b"), { ...plan, wire: (element) => (element.uploads = []) });
    const row = list.querySelector('[data-key="a"]');
    row.uploads.push("shot.png");

    patchList(list, [{ id: "c", label: "C" }, { id: "a", label: "CHANGED" }], { ...plan, wire: () => {} });

    expect(list.querySelector('[data-key="a"]').uploads).toEqual(["shot.png"]);
  });
});

describe("a paint with adds, drops, moves and edits in it", () => {
  it("ends up saying the right thing", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c", "d"), plan);

    patchList(
      list,
      [{ id: "d", label: "D" }, { id: "b", label: "BEE" }, { id: "e", label: "E" }, { id: "a", label: "A" }],
      plan,
    );

    expect(keysOf(list)).toEqual(["d", "b", "e", "a"]);
    expect(labelsOf(list)).toEqual(["D", "BEE", "E", "A"]);
  });

  it("rebuilds only the entry that is new to it", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c", "d"), plan);
    const kept = new Map([...list.children].map((child) => [child.getAttribute("data-key"), child]));

    patchList(
      list,
      [{ id: "d", label: "D" }, { id: "b", label: "BEE" }, { id: "e", label: "E" }, { id: "a", label: "A" }],
      plan,
    );

    for (const key of ["a", "b", "d"]) {
      expect(list.querySelector(`[data-key="${key}"]`)).toBe(kept.get(key));
    }
    expect(kept.get("c").isConnected).toBe(false);
  });

  it("settles, so the paint after it writes nothing", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c", "d"), plan);
    const churned = [
      { id: "d", label: "D" },
      { id: "b", label: "BEE" },
      { id: "e", label: "E" },
      { id: "a", label: "A" },
    ];
    patchList(list, churned, plan);

    const records = mutationsOf(list, () => patchList(list, churned, plan));

    expect(records).toEqual([]);
  });
});

describe("the chrome around the entries", () => {
  it("leaves what it does not own alone", () => {
    const list = listIn(document, `<li class="header">Today</li>`);
    const header = list.firstElementChild;

    patchList(list, entriesFor("a", "b"), plan);
    patchList(list, entriesFor("b"), plan);

    expect(list.firstElementChild).toBe(header);
    expect(keysOf(list)).toEqual([null, "b"]);
  });

  it("keeps the entries ahead of a footer that follows them", () => {
    const list = listIn(document, `<li data-key="a"></li><li class="footer">more</li>`);
    patchList(list, entriesFor("a"), plan);
    const footer = list.querySelector(".footer");

    patchList(list, entriesFor("a", "b"), plan);

    expect(keysOf(list)).toEqual(["a", "b", null]);
    expect(list.lastElementChild).toBe(footer);
  });
});

describe("a list it cannot paint honestly", () => {
  it("says so when two entries claim the same key", () => {
    const list = listIn(document);
    expect(() => patchList(list, entriesFor("a", "a"), plan)).toThrow(/same key "a"/);
  });

  it("says so when the render gives back no element", () => {
    const list = listIn(document);
    expect(() => patchList(list, entriesFor("a"), { ...plan, render: () => "   " })).toThrow(/"a"/);
  });
});

describe("renaming an entry to the identity the answer gave it", () => {
  it("renames an entry without rebuilding it", () => {
    const list = listIn(document);
    patchList(list, entriesFor("pending-agent-1"), {
      ...plan,
      render: () => `<li><canvas class="face"></canvas><input class="say"></li>`,
    });
    const item = list.firstElementChild;
    const face = item.querySelector("canvas");
    const say = item.querySelector("input");
    say.focus();

    expect(rekeyEntry(list, "pending-agent-1", "ag-2")).toBe(true);
    expect(keysOf(list)).toEqual(["ag-2"]);

    patchList(list, entriesFor("ag-2"), {
      ...plan,
      render: () => `<li><canvas class="face"></canvas><input class="say"></li>`,
    });

    expect(list.firstElementChild).toBe(item);
    expect(item.querySelector("canvas")).toBe(face);
    expect(item.querySelector("input")).toBe(say);
    expect(document.activeElement).toBe(say);
  });

  it("drops the stale element when the new key is already standing", () => {
    const list = listIn(document);
    patchList(list, entriesFor("pending-agent-1", "ag-2"), plan);
    const real = list.children[1];

    expect(rekeyEntry(list, "pending-agent-1", "ag-2")).toBe(false);

    expect(keysOf(list)).toEqual(["ag-2"]);
    expect(list.firstElementChild).toBe(real);
  });

  it("says nothing happened when the key it was given is not there", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a"), plan);
    expect(rekeyEntry(list, "b", "c")).toBe(false);
    expect(keysOf(list)).toEqual(["a"]);
  });
});

function deferred() {
  let settle;
  const promise = new Promise((resolve) => {
    settle = resolve;
  });
  return { promise, settle };
}

const refused = () => {
  let refuse;
  const promise = new Promise((resolve, reject) => {
    refuse = reject;
  });
  return { promise, refuse };
};

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

const liveKeysOf = (list) =>
  [...list.children].filter((child) => !child.hasAttribute("data-exiting")).map((child) => child.getAttribute("data-key"));

describe("entries arriving and leaving under the caller's own motion", () => {
  it("calls onEnter for the entries that were not there before, and no others", () => {
    const list = listIn(document);
    const entered = [];
    const withEnter = { ...plan, onEnter: (element) => entered.push(element.getAttribute("data-key")) };

    patchList(list, entriesFor("a", "b"), withEnter);
    expect(entered).toEqual(["a", "b"]);

    entered.length = 0;
    patchList(list, entriesFor("c", "a", "b"), withEnter);

    expect(entered).toEqual(["c"]);
    expect(keysOf(list)).toEqual(["c", "a", "b"]);
  });

  it("hands a departed entry to onExit and keeps it until the exit is over", async () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c"), plan);
    const leaving = deferred();
    const exited = [];

    patchList(list, entriesFor("a", "c"), {
      ...plan,
      onExit: (element) => {
        exited.push(element.getAttribute("data-key"));
        return leaving.promise;
      },
    });

    expect(exited).toEqual(["b"]);
    expect(keysOf(list)).toEqual(["a", "b", "c"]);
    expect(liveKeysOf(list)).toEqual(["a", "c"]);

    leaving.settle();
    await flush();

    expect(keysOf(list)).toEqual(["a", "c"]);
  });

  it("takes the entry out at once when onExit promises nothing", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b"), plan);

    patchList(list, entriesFor("a"), { ...plan, onExit: () => {} });

    expect(keysOf(list)).toEqual(["a"]);
  });

  it("keeps the same element when a key comes back in the middle of its exit", async () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c"), plan);
    const leaving = deferred();
    const entered = [];
    const exited = [];
    const hooks = {
      ...plan,
      onEnter: (element) => entered.push(element.getAttribute("data-key")),
      onExit: (element) => {
        exited.push(element.getAttribute("data-key"));
        return leaving.promise;
      },
    };
    patchList(list, entriesFor("a", "c"), hooks);
    const b = list.children[1];

    patchList(list, entriesFor("a", "b", "c"), hooks);

    expect(list.children[1]).toBe(b);
    expect(entered).toEqual([]);

    leaving.settle();
    await flush();

    expect(keysOf(list)).toEqual(["a", "b", "c"]);
    expect(list.children[1]).toBe(b);
    expect(exited).toEqual(["b"]);
  });

  it("paints around an entry that is still leaving without moving the others", async () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b", "c"), plan);
    const leaving = deferred();
    const entered = [];
    const hooks = {
      ...plan,
      onEnter: (element) => entered.push(element.getAttribute("data-key")),
      onExit: () => leaving.promise,
    };
    patchList(list, entriesFor("a", "c"), hooks);
    const a = list.children[0];

    patchList(list, entriesFor("d", "a", "c"), hooks);

    expect(liveKeysOf(list)).toEqual(["d", "a", "c"]);
    expect(entered).toEqual(["d"]);
    expect(list.querySelector('[data-key="a"]')).toBe(a);

    leaving.settle();
    await flush();

    expect(keysOf(list)).toEqual(["d", "a", "c"]);
  });

  it("takes the entry out when its exit fell over, and lets the failure through", async () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b"), plan);
    const exit = refused();
    const failures = [];
    const watchFailures = (error) => failures.push(error);
    process.on("unhandledRejection", watchFailures);

    patchList(list, entriesFor("a"), { ...plan, onExit: () => exit.promise });
    expect(keysOf(list)).toEqual(["a", "b"]);

    const fell = new Error("the exit fell over");
    exit.refuse(fell);
    await flush();
    process.off("unhandledRejection", watchFailures);

    expect(keysOf(list)).toEqual(["a"]);
    expect(failures).toEqual([fell]);
  });

  it("leaves a departed entry out at once when the caller gave no onExit", () => {
    const list = listIn(document);
    patchList(list, entriesFor("a", "b"), plan);

    patchList(list, entriesFor("a"), plan);

    expect(keysOf(list)).toEqual(["a"]);
  });
});
