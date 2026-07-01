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

/** Hide machine noise (task metadata, caches, lockfiles) from review. */
export function filterNoiseFiles(files) {
  return files.filter(
    (f) => !f.path.startsWith(".build/") && !f.path.includes("__pycache__") && f.path !== "uv.lock",
  );
}
