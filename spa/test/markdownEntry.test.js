// One way to render markdown (#229).
//
// The user's rule: "a singular method on the frontend for rendering markdown,
// this way it is consistent across all surfaces". core/markdown.js
// `markdownHtml` is that method; everything behind it — the block renderer,
// the reference syntax, the linker, the preview reducer, the resolver — is
// its own, and a surface that reached one of them directly would render
// markdown its own way again, which is how the chat linked references and a
// plan doc did not. This reads every module under src/ and fails one that
// reaches around the entry point.

import { describe, expect, it } from "vitest";

import { srcJsFiles, srcSourceOf } from "./treeFiles.js";

/// The one module each internal part may be imported by. Everything else in
/// src/ goes through `markdownHtml`.
const OWNERS = {
  "core/markdownRefs.js": ["core/markdownLinks.js"],
  "core/markdownLinks.js": ["core/markdown.js"],
  "core/markdownBlocks.js": ["core/markdown.js"],
  "core/previewText.js": ["core/markdown.js"],
  "core/referenceTargets.js": ["core/referenceIndex.js"],
};

/// What each module may be asked for by anyone. `referenceIndex.js` is filled
/// only by its feed and read only by the renderer; a surface may only listen
/// to it, to repaint when an answer could have changed.
const PUBLIC_NAMES = {
  "core/markdown.js": ["markdownHtml"],
  "core/referenceIndex.js": ["referenceIndexVersion", "subscribeReferenceIndex"],
};
const PRIVILEGED = {
  "core/referenceIndex.js": { "core/markdown.js": ["referenceResolver"], "core/referenceIndexFeed.js": ["holdReferenceSources"] },
};

/** Every static or dynamic import in a source, as `{ target, names }`, with
 *  the target resolved against the importing file's directory. */
function importsOf(file, source) {
  const found = [];
  const here = file.split("/").slice(0, -1);
  const resolve = (specifier) => {
    const parts = [...here];
    for (const segment of specifier.split("/")) {
      if (segment === "..") parts.pop();
      else if (segment !== ".") parts.push(segment);
    }
    return parts.join("/");
  };
  const statics = /import\s*(?:\{([^}]*)\}|\*\s+as\s+\w+|\w+)?\s*(?:from\s*)?["'](\.[^"']+)["']/g;
  for (const match of source.matchAll(statics)) {
    const names = match[1] === undefined ? ["*"] : match[1].split(",").map((name) => name.trim().split(/\s+as\s+/)[0]).filter(Boolean);
    found.push({ target: resolve(match[2]), names });
  }
  for (const match of source.matchAll(/import\(\s*["'](\.[^"']+)["']\s*\)/g)) found.push({ target: resolve(match[1]), names: ["*"] });
  return found;
}

const everyImport = () =>
  srcJsFiles().flatMap((file) => importsOf(file, srcSourceOf(file)).map((one) => ({ file, ...one })));

describe("the one markdown entry point", () => {
  it("reads the imports it guards (the scan itself works)", () => {
    const seen = everyImport().filter((one) => one.target === "core/markdown.js").map((one) => one.file);
    expect(seen).toContain("core/thread.js");
  });

  it("keeps each internal part to the one module that owns it", () => {
    const strays = everyImport()
      .filter(({ file, target }) => OWNERS[target] && !OWNERS[target].includes(file))
      .map(({ file, target }) => `${file} imports ${target}`);
    expect(strays).toEqual([]);
  });

  it("hands every other module only the public names", () => {
    const strays = everyImport().flatMap(({ file, target, names }) => {
      const allowed = [...(PUBLIC_NAMES[target] || []), ...(PRIVILEGED[target]?.[file] || [])];
      if (!PUBLIC_NAMES[target]) return [];
      return names.filter((name) => !allowed.includes(name)).map((name) => `${file} imports ${name} from ${target}`);
    });
    expect(strays).toEqual([]);
  });

  // A renderer of its own starts with the emphasis rule everyone writes first.
  it("has no second renderer hand-rolling emphasis", () => {
    const rolled = srcJsFiles()
      .filter((file) => !["core/markdown.js", "core/previewText.js"].includes(file))
      .filter((file) => /\.replace\(\s*\/\\\*\\\*/.test(srcSourceOf(file)));
    expect(rolled).toEqual([]);
  });
});
