// Unified-diff parsing for the review surface. Pure: patch text in, rows out.

/** Parse a `git diff` patch into per-file row lists the diff tab can render. */
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

// ---- file identity ---------------------------------------------------------

/** The name one file of a patch answers to: its path, plus its status, so a
 *  rename's delete and its add are two files rather than one name claimed
 *  twice. It names both the element (`data-key`) and the reader's state for
 *  it, which is what lets a repaint find the file it is holding. */
export function fileKey(file) {
  return `${file.status}:${file.path}`;
}

/** The line a reader lands on when they leave the diff for the file itself:
 *  the first line the patch touches, or the top of the file when it names
 *  none (a pure deletion). */
export function firstLineOf(file) {
  const row = (file.rows || []).find((each) => typeof each.n === "number");
  return row ? row.n : 1;
}

/** The folds of one changeset, as the reader left them.
 *
 *  Three states, two sets: a file the reader opened is in `expanded`, one they
 *  shut is in `collapsed`, and one they have not touched is in neither — the
 *  capped peek every file starts at. The render reads the sets; nothing reads
 *  the class list back. */
export function createFileFolds() {
  const expanded = new Set();
  const collapsed = new Set();
  const open = (key) => {
    expanded.add(key);
    collapsed.delete(key);
  };
  const shut = (key) => {
    collapsed.add(key);
    expanded.delete(key);
  };
  return {
    expanded,
    collapsed,
    /** A press on the capped body: the reader asked for the whole file. */
    open,
    /** The file's header is one control: it shuts what is showing and shows
     *  what is shut. */
    pressedHead: (key) => (collapsed.has(key) ? open(key) : shut(key)),
  };
}

// ---- hunk identity ---------------------------------------------------------
//
// A port of `patch_hunks` in bridge/src/diff.rs. The bridge assigns the ids a
// triage report speaks in; the SPA has to derive the same ids from the same
// patch to render that triage, so the two implementations must agree
// byte-for-byte. bridge/tests/fixtures/hunk_ids.json is the shared fixture both
// suites read — change one implementation, change the other, regenerate it.

const FNV_OFFSET_BASIS = 0xcbf29ce484222325n;
const FNV_PRIME = 0x100000001b3n;
const SIXTY_FOUR_BITS = 0xffffffffffffffffn;

/** FNV-1a 64 over the UTF-8 bytes of `text`, as 16 lowercase hex digits.
 *  Identity, not integrity: the only digest the browser ships (crypto.subtle)
 *  is async, and hunk ids have to be assignable inside a synchronous render. */
function fnv1a64Hex(text) {
  let hash = FNV_OFFSET_BASIS;
  for (const byte of new TextEncoder().encode(text)) {
    hash = ((hash ^ BigInt(byte)) * FNV_PRIME) & SIXTY_FOUR_BITS;
  }
  return hash.toString(16).padStart(16, "0");
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
