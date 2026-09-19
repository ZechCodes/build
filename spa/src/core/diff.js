// Unified-diff parsing for the review surface. Pure: patch text in, rows out.

/** Parse a `git diff` patch into per-file row lists the diff tab can render. */
// eslint-disable-next-line complexity -- ratchet: parseDiff is at 16, cap 10 — reduce it, then drop this line
export function parseDiff(patch) {
  const files = [];
  let current = null;
  let oldLine = 0;
  let newLine = 0;
  for (const line of (patch || "").split("\n")) {
    if (line.startsWith("diff --git")) {
      const m = line.match(/ b\/(.+)$/);
      current = { path: m ? m[1] : "?", status: "EDIT", add: 0, del: 0, rows: [] };
      files.push(current);
    } else if (!current) continue;
    else if (line.startsWith("new file")) current.status = "ADD";
    else if (line.startsWith("deleted file")) current.status = "DEL";
    else if (line.startsWith("index ") || line.startsWith("--- ") || line.startsWith("+++ ")) continue;
    else if (line.startsWith("@@")) {
      const m = line.match(/@@ -(\d+).* \+(\d+)/);
      oldLine = m ? +m[1] : 0;
      newLine = m ? +m[2] : 0;
      current.rows.push({ t: "hunk", text: line });
    } else if (line.startsWith("+")) {
      current.add++;
      current.rows.push({ t: "add", n: newLine++, text: line.slice(1) });
    } else if (line.startsWith("-")) {
      current.del++;
      current.rows.push({ t: "del", o: oldLine++, text: line.slice(1) });
    } else {
      current.rows.push({ t: "ctx", o: oldLine++, n: newLine++, text: line.slice(1) });
    }
  }
  return files;
}

export function fileKey(file) {
  return `${file.status}:${file.path}`;
}

export function pathOf(key) {
  const text = String(key);
  return text.slice(text.indexOf(":") + 1);
}

export function firstLineOf(file) {
  const row = (file.rows || []).find((each) => typeof each.n === "number");
  return row ? row.n : 1;
}

const OPEN = "open";
const SHUT = "shut";
const CAPPED = "capped";

export function untouchedFold(key, approved) {
  return approved && approved.has(pathOf(key)) ? SHUT : CAPPED;
}

export function createFileFolds() {
  const moved = new Map();
  const foldOf = (key, { approved = null } = {}) => moved.get(key) || untouchedFold(key, approved);
  return {
    foldOf,
    press: (key, { approved = null } = {}) => moved.set(key, foldOf(key, { approved }) === SHUT ? OPEN : SHUT),
    openBody: (key) => moved.set(key, OPEN),
  };
}

// Machine noise (Build metadata, caches, lockfiles) is no longer filtered out
// of a review: core/changesModel.js's groupNoiseFiles separates it, and
// core/diffRender.js's diffStackHtml renders it as one collapsed group at the
// bottom of the stack. Collapse, never hide.
