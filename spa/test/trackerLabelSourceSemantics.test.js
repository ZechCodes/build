import { expect, it } from "vitest";
import { filterTasks, taskListParams } from "../src/core/trackerFilters.js";

const tasks = ["API", "api", "É", "é", " API ", "api\0v2", "API\0v2", "api\uFFFDv2"]
  .map((label, id) => ({ id, labels: [label] }));
const cases = [
  ["API", [0], [0, 1]], ["api", [1], [0, 1]],
  ["É", [2], [2]], ["é", [3], [3]],
  [" API ", [4], [4]],
  ["api\0v2", [5], [5, 6]], ["API\0v2", [6], [5, 6]],
  ["api\uFFFDv2", [7], [7]],
];

// Legacy source oracle freezes the original exact includes comparison;
// canonical oracle records the backend's eq_ignore_ascii_case contract at
// bridge/src/app/tracker/read.rs and selections.rs. Neither trims or folds É.
it.each(cases)("matches canonical label bytes for %j and records the previous consumer", (label, previous, canonical) => {
  expect(tasks.filter((task) => task.labels.includes(label)).map(({ id }) => id)).toEqual(previous);
  expect(filterTasks(tasks, { label }).map(({ id }) => id)).toEqual(canonical);
  expect(filterTasks(tasks, { label: [label] }).map(({ id }) => id)).toEqual(canonical);
  expect(taskListParams("project", { label })).toEqual({ project_id: "project", label });
});
