// What a workspace row says about the workspace's lifecycle (#135).
//
// The bridge's reclaim service measures every workspace and writes its verdict
// onto the row as `lifecycle`: whether the workspace has gone quiet, what still
// holds it, and how big it is. The row reads that verdict as one line, plus a
// Reclaim when nothing holds the workspace. The verdict is the bridge's, and
// `workspace.reclaim` measures again before it removes anything, so the line
// never guesses.
//
// No DOM, no app imports: views/projectView.js renders these.

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB"];

/** A byte count as a person says it: `17.2 GB`, `640 MB`, `512 B`. The same
 *  reading the bridge gives the project agent. */
export function humanBytes(bytes) {
  let value = Number(bytes) || 0;
  let unit = 0;
  while (value >= 1000 && unit < BYTE_UNITS.length - 1) {
    value /= 1000;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  return `${value.toFixed(digits)} ${BYTE_UNITS[unit]}`;
}

const counted = (count, one, many) => `${count} ${count === 1 ? one : many}`;

/** Each hold in the words a row has room for. Counted where the verdict counts. */
const HOLD_WORDS = Object.freeze({
  dirty: (verdict) => counted(verdict.dirty_files, "uncommitted file", "uncommitted files"),
  unpushed: (verdict) => counted(verdict.unpushed_commits, "unpushed commit", "unpushed commits"),
  task_open: () => "a task not Done",
  tasks_unread: () => "tasks unread",
  agent_working: () => "an agent working",
  terminal_open: () => "a terminal open",
  plain_directory: () => "a folder that is not a repository",
  not_ready: () => "not ready",
  unknown: () => "Git state unread",
  unmeasured: () => "not fully measured",
});

/** A hold this build has never heard of reads as the bridge's own word. */
const holdWords = (hold, verdict) => HOLD_WORDS[hold]?.(verdict) ?? String(hold).replace(/_/g, " ");

const sizeWords = (verdict) => [
  verdict.size_bytes ? humanBytes(verdict.size_bytes) : "",
  verdict.pruned_bytes > 0 ? `${humanBytes(verdict.pruned_bytes)} of build output dropped` : "",
];

/**
 * The row's reading of one verdict, or null when there is nothing to say: no
 * verdict yet, or a workspace in use that something still holds.
 *
 * `reclaimable` is what offers Reclaim. `text` is the line under the row.
 */
export function lifecycleView(verdict) {
  if (!verdict) return null;
  const reclaimable = verdict.reclaimable === true;
  if (!reclaimable && !verdict.idle) return null;
  const held = (verdict.holds || []).map((hold) => holdWords(hold, verdict)).join(", ");
  const opening = reclaimable ? "Reclaimable" : "Idle";
  const parts = [opening, held, ...sizeWords(verdict)].filter(Boolean);
  return { reclaimable, text: parts.join(" · ") };
}
