/** @vitest-environment jsdom */
import { expect, it } from "vitest";
import { markdownHtml } from "../src/core/markdown.js";

const hostOf = (source) => {
  const host = document.createElement("div");
  host.innerHTML = markdownHtml(source);
  return host;
};
it("keeps short prose and measurement cells whole, with long prose allowed to wrap", () => {
  const host = hostOf("| Condition | Timing | Details |\n| --- | ---: | --- |\n| A write holding the app lock | 100–149 ms | " + "A longer explanation. ".repeat(10) + " |");
  const cells = host.querySelectorAll("td");
  expect(cells[0].className).toBe("md-cell-short");
  expect(cells[1].className).toBe("md-cell-short");
  expect(cells[1].style.textAlign).toBe("right");
  expect(cells[2].className).toBe("");
});
it("gives scrollable tables a keyboard stop and column headers", () => {
  const host = hostOf("| a | b |\n| --- | --- |\n| 1 | 2 |");
  const scroll = host.querySelector(".mdtable");
  expect(scroll.tabIndex).toBe(0);
  expect(scroll.getAttribute("role")).toBe("region");
  expect(scroll.getAttribute("aria-label")).toBe("Table");
  expect([...host.querySelectorAll("th")].map((th) => th.scope)).toEqual(["col", "col"]);
});
it("requires equal header and delimiter column counts before recognizing a table", () => {
  for (const source of ["| a | b |\n| --- |", "| a |\n| --- | --- |"])
    expect(hostOf(source).querySelector("table")).toBeNull();
});
