// Canonical menu memberships remain independent of the pane's task read and
// filters. The grouped list/board retain their complete legacy source until
// exact group complements and filtered totals have a bounded contract.
import { createTaskFilterChoicesReader, filterChoicesAddress } from "./taskFilterChoices.js";
import { subscribeCache } from "./localCache.js";
import { chosenOf } from "./trackerFilters.js";
import { assigneeFromKey, assigneeKey } from "./trackerModel.js";

const DIMENSIONS = { state: "states", status: "statuses", label: "labels", assignee: "assignees" };
const usable = (record) => record && (record.complete || typeof record.next_cursor === "string");
const unique = (options) => {
  const seen = new Set();
  return options.filter(({ value }) => !seen.has(value) && seen.add(value));
};

function choiceOption(dimension, choice, defaults, labelFor) {
  const value = dimension === "assignee" && typeof choice === "object" ? assigneeKey(choice) : choice;
  const held = defaults.find((option) => option.value === value);
  if (held) return held;
  const label = dimension === "assignee" && typeof choice === "object" ? labelFor(choice) || value : value;
  return { value, label };
}

function chosenOptions(dimension, filters, defaults, labelFor) {
  return chosenOf(filters[dimension]).map((value) => {
    const choice = dimension === "assignee" ? assigneeFromKey(value) || value : value;
    return choiceOption(dimension, choice, defaults, labelFor);
  });
}

/** Cache-backed option paint, independent first pages, and explicit More for
 * each dimension. Live capabilities gate dispatch only; cached memberships
 * and continuations remain paintable without a current greeting. */
export function createTaskPaneChoiceReads({ scope, host, callRpc, capabilities, sessionOf,
  enabled = () => true, active = () => true, onChange = () => {}, onError = () => {} }) {
  const records = new Map();
  let disposed = false;
  let lastSession;
  let requested = false;
  let readSerial = 0;
  const current = () => !disposed && active() && enabled();
  const readers = Object.fromEntries(Object.keys(DIMENSIONS).map((dimension) => [dimension,
    createTaskFilterChoicesReader({ scope, dimension, callRpc, capabilities, sessionOf, active: current }),
  ]));

  async function readCache() {
    const serial = ++readSerial;
    const saved = await Promise.all(Object.entries(readers).map(async ([dimension, reader]) => [dimension, await reader.read()]));
    if (disposed || serial !== readSerial) return;
    for (const [dimension, record] of saved) records.set(dimension, record);
    onChange();
  }
  const unsubscribe = subscribeCache({ ...filterChoicesAddress(scope, "label"), sub: undefined }, () => void readCache());
  const ready = readCache();

  function ask(reader, operation) {
    return reader[operation]().catch((error) => { if (current()) onError(error); return false; });
  }
  function refresh() {
    if (!current() || capabilities()?.filterChoices !== true) return Promise.resolve(false);
    lastSession = sessionOf();
    requested = true;
    return Promise.all(Object.values(readers).map((reader) => ask(reader, "refresh")));
  }
  function ensure() {
    return requested && lastSession === sessionOf() ? Promise.resolve(false) : refresh();
  }

  return {
    ready, ensure, refresh,
    options(defaults, standing, filters, labelFor) {
      if (!enabled()) return defaults;
      const next = { ...defaults };
      for (const [dimension, key] of Object.entries(DIMENSIONS)) {
        const record = records.get(dimension);
        if (!usable(record)) continue;
        next[key] = unique([...standing[key],
          ...record.choices.map((choice) => choiceOption(dimension, choice, defaults[key], labelFor)),
          ...chosenOptions(dimension, filters, defaults[key], labelFor)]);
      }
      return next;
    },
    paintContinuation() {
      for (const [dimension, reader] of Object.entries(readers)) {
        const menu = host.querySelector(`[data-filter-menu="${dimension}"]`);
        if (!menu) continue;
        const record = records.get(dimension);
        let more = menu.querySelector("[data-task-choices-more]");
        if (!more && usable(record) && enabled()) {
          more = host.ownerDocument.createElement("button");
          more.type = "button";
          more.className = "btn mini";
          more.dataset.taskChoicesMore = dimension;
          more.textContent = "Load more choices";
          more.onclick = () => void ask(reader, "more");
          menu.querySelector(".fmenu-foot").append(more);
        }
        if (more) more.hidden = !enabled() || !record?.next_cursor || record.refreshing;
      }
    },
    dispose() {
      disposed = true;
      unsubscribe();
      Object.values(readers).forEach((reader) => reader.dispose());
    },
  };
}
