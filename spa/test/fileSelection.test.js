// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { captureFileSelection } from "../src/core/fileSelection.js";
import { renderDotenvSourceHtml, SPOILER_DOTS } from "../src/core/secrets.js";

function select(startNode, startOffset, endNode = startNode, endOffset = startOffset) {
  const range = document.createRange();
  range.setStart(startNode, startOffset);
  range.setEnd(endNode, endOffset);
  const selection = document.getSelection();
  selection.removeAllRanges();
  selection.addRange(range);
  return selection;
}

function sourceFixture() {
  document.body.innerHTML = `<section id="viewer"><div class="fsrc"><table><tbody>
    <tr data-new-line="7"><td class="fsrc-ln">7</td><td class="fsrc-code"><code>const <span>answer</span> = 42;</code></td></tr>
    <tr data-new-line="8"><td class="fsrc-ln">8</td><td class="fsrc-code"><code>return answer;</code></td></tr>
  </tbody></table></div></section>`;
  return document.querySelector("#viewer");
}

afterEach(() => {
  document.getSelection().removeAllRanges();
  document.body.replaceChildren();
});

describe("captureFileSelection", () => {
  it("captures the exact partial source characters and line number", () => {
    const root = sourceFixture();
    const answer = root.querySelector(".fsrc-code span").firstChild;

    expect(captureFileSelection(root, "src/app.js", select(answer, 1, answer, 5))).toEqual([
      { kind: "selection", path: "src/app.js", text: "nswe", line_start: 7, line_end: 7 },
    ]);
  });

  it("joins selected source rows with newlines and excludes line-number gutters", () => {
    const root = sourceFixture();
    const first = root.querySelectorAll(".fsrc-code code")[0];
    const second = root.querySelectorAll(".fsrc-code code")[1];

    expect(captureFileSelection(root, "src/app.js", select(first.firstChild, 2, second.firstChild, 6))).toEqual([
      { kind: "selection", path: "src/app.js", text: "nst answer = 42;\nreturn", line_start: 7, line_end: 8 },
    ]);
  });

  it("ignores a gutter-only selection", () => {
    const root = sourceFixture();
    const gutter = root.querySelector(".fsrc-ln").firstChild;
    expect(captureFileSelection(root, "src/app.js", select(gutter, 0, gutter, 1))).toEqual([]);
  });

  it("captures the displayed mask from the real dotenv source renderer", () => {
    const rendered = renderDotenvSourceHtml("API_TOKEN=raw-secret\nPUBLIC=visible");
    document.body.innerHTML = `<section id="viewer">${rendered.html}</section>`;
    const root = document.querySelector("#viewer");
    const code = root.querySelector("code");

    expect(captureFileSelection(root, ".env", select(code, 0, code, code.childNodes.length))).toEqual([
      { kind: "selection", path: ".env", text: `API_TOKEN=${SPOILER_DOTS}\nPUBLIC=visible` },
    ]);
    expect(document.body.textContent).not.toContain("raw-secret");
  });

  it("preserves an empty selected source row between non-empty lines", () => {
    document.body.innerHTML = `<section id="viewer"><div class="fsrc"><table><tbody>
      <tr data-new-line="1"><td class="fsrc-ln">1</td><td class="fsrc-code"><code>alpha</code></td></tr>
      <tr data-new-line="2"><td class="fsrc-ln">2</td><td class="fsrc-code"><code></code></td></tr>
      <tr data-new-line="3"><td class="fsrc-ln">3</td><td class="fsrc-code"><code>omega</code></td></tr>
    </tbody></table></div></section>`;
    const root = document.querySelector("#viewer");
    const codes = root.querySelectorAll("code");

    expect(captureFileSelection(root, "notes.txt", select(codes[0].firstChild, 2, codes[2].firstChild, 3))).toEqual([
      { kind: "selection", path: "notes.txt", text: "pha\n\nome", line_start: 1, line_end: 3 },
    ]);
  });

  it("captures exact rendered Markdown prose without fabricating source lines", () => {
    document.body.innerHTML = `<section id="viewer"><div class="plan"><p>Hello <strong>rendered world</strong>.</p></div></section>`;
    const root = document.querySelector("#viewer");
    const prose = root.querySelector("strong").firstChild;

    expect(captureFileSelection(root, "README.md", select(prose, 3, prose, 11))).toEqual([
      { kind: "selection", path: "README.md", text: "dered wo" },
    ]);
  });

  it("safely ignores collapsed, external, and unrelated selections", () => {
    const root = sourceFixture();
    const outside = document.createElement("p");
    outside.textContent = "elsewhere";
    document.body.append(outside);
    expect(captureFileSelection(root, "src/app.js", select(outside.firstChild, 0, outside.firstChild, 4))).toEqual([]);

    const code = root.querySelector(".fsrc-code code").firstChild;
    expect(captureFileSelection(root, "src/app.js", select(code, 2))).toEqual([]);

    root.innerHTML = '<div class="unrelated">nothing selectable here</div>';
    const unrelated = root.firstChild.firstChild;
    expect(captureFileSelection(root, "src/app.js", select(unrelated, 0, unrelated, 7))).toEqual([]);
    expect(captureFileSelection(null, "src/app.js", null)).toEqual([]);
  });
});
