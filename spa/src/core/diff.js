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

// ---- hunk identity ---------------------------------------------------------
//
// A port of `patch_hunks` in bridge/src/diff.rs. The bridge assigns the ids a
// triage report speaks in; the SPA has to derive the same ids from the same
// patch to render that triage, so the two implementations must agree
// byte-for-byte. bridge/tests/fixtures/hunk_ids.json is the shared fixture both
// suites read — change one implementation, change the other, regenerate it.

const FNV_OFFSET_HIGH = 0xcbf29ce4;
const FNV_OFFSET_LOW = 0x84222325;
const FNV_PRIME_LOW = 0x1b3;
const TWO_TO_32 = 0x100000000;

/** FNV-1a 64 over the UTF-8 bytes of `text`, as 16 lowercase hex digits.
 *  Identity, not integrity: the only digest the browser ships (crypto.subtle)
 *  is async, and hunk ids have to be assignable inside a synchronous render. */
export function fnv1a64Hex(text) {
  let high = FNV_OFFSET_HIGH;
  let low = FNV_OFFSET_LOW;
  for (const byte of new TextEncoder().encode(text)) {
    low = (low ^ byte) >>> 0;

    // FNV_PRIME is 2^40 + 0x1b3. The low product is still below 2^53,
    // so its carry is exact; Math.imul supplies each wrapping 32-bit half.
    const carry = Math.floor((low * FNV_PRIME_LOW) / TWO_TO_32);
    high = (Math.imul(high, FNV_PRIME_LOW) + carry + (low << 8)) >>> 0;
    low = Math.imul(low, FNV_PRIME_LOW) >>> 0;
  }
  return high.toString(16).padStart(8, "0") + low.toString(16).padStart(8, "0");
}

/** A patch's lines, without the empty tail a trailing newline leaves behind. */
function patchLines(patch) {
  const lines = (patch || "").split("\n");
  if (lines.length && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** The path a `diff --git a/x b/x` line is about: its `b/` side. */
function diffHeaderPath(line) {
  const at = line.indexOf(" b/");
  if (at < 0) return null;
  const path = line.slice(at + 3);
  return path ? path : null;
}

const HUNK_HEADER = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/** A hunk header reduced to its line counts: `@@ -12,7 +14,9 @@ fn x()` becomes
 *  `-7 +9`, so a hunk that only moved keeps its id. Null when the line is not a
 *  well-formed hunk header. */
function normalizedHunkHeader(line) {
  const m = line.match(HUNK_HEADER);
  if (!m) return null;
  return `-${m[2] ?? "1"} +${m[4] ?? "1"}`;
}

/** Split a patch into its hunks, in patch order, before ids are assigned. */
function rawHunks(patch) {
  const hunks = [];
  let path = null;
  let inHunk = false;
  for (const line of patchLines(patch)) {
    if (line.startsWith("diff --git")) {
      path = diffHeaderPath(line);
      inHunk = false;
    } else if (line.startsWith("@@")) {
      inHunk = false;
      const normalized = normalizedHunkHeader(line);
      if (path && normalized) {
        hunks.push({ path, header: line, normalized, body: [] });
        inHunk = true;
      }
    } else if (inHunk) {
      hunks[hunks.length - 1].body.push(line);
    }
  }
  return hunks;
}

/** Assign every hunk in `patch` a stable id: `h` + 12 hex digits of FNV-1a 64
 *  over the file path, the count-normalized hunk header, and the hunk's body
 *  lines. Repeats of one hunk within a file fold their occurrence number into
 *  the hashed material, so ids are unique within a patch.
 *  Returns [{ hunk_id, path, header }] in patch order. */
export function patchHunks(patch) {
  const occurrences = new Map();
  return rawHunks(patch).map(({ path, header, normalized, body }) => {
    const material = `${path}\n${normalized}\n${body.join("\n")}`;
    const seen = occurrences.get(material) || 0;
    const hashed = seen === 0 ? material : `${material}\n#${seen}`;
    occurrences.set(material, seen + 1);
    return { hunk_id: `h${fnv1a64Hex(hashed).slice(0, 12)}`, path, header };
  });
}

/** patchHunks' ids alone, in patch order. */
export function hunkIds(patch) {
  return patchHunks(patch).map((hunk) => hunk.hunk_id);
}

// Machine noise (Build metadata, caches, lockfiles) is no longer filtered out
// of a review: core/changesModel.js's groupNoiseFiles separates it, and
// core/diffRender.js's diffStackHtml renders it as one collapsed group at the
// bottom of the stack. Collapse, never hide.
