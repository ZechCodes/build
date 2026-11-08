import { expect, it } from "vitest";
import { taskCountText } from "../src/core/taskCountText.js";

it.each([
  [32, 102, "32 open tasks"],
  [1, 102, "1 open tasks"],
  [0, 102, "102 tasks"],
  [0, 1, "1 tasks"],
  [0, 0, "0 tasks"],
])("describes %i open of %i total tasks", (open, total, text) => {
  expect(taskCountText(open, total)).toBe(text);
});
