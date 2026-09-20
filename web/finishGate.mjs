// What stands between a workspace and Done, as the harness has to reason about
// it.
//
// `workspace.finish` REMOVES the workspace, so the bridge refuses one whose
// work is only in it (bridge/src/workspace.rs `workspace_directory_blockers`,
// landed 2026-09-17). The harness predates that gate and used to call finish on
// a fixture it had just committed into and that held a plain folder — two
// blockers at once.
//
// The gate is right and is not worked around here. This module is the harness's
// half: the sentences the bridge refuses in, so a check can say WHICH blocker
// it expected, and the reading of a workspace that says which directories would
// stand in the way.

/// The bridge's own sentences, from `blocker_sentence` in
/// bridge/src/workspace.rs. Mirrored rather than imported because the harness
/// talks to a bridge over a wire and has no Rust to ask — so if these drift,
/// the checks that use them fail loudly rather than passing on a refusal that
/// says something else.
export const FINISH_BLOCKERS = Object.freeze({
  agentWorking: "an agent is working in it",
  dirty: "it has uncommitted changes",
  unpushed: "it has commits no remote has",
  plainDirectory: "it holds a directory that is not a repository",
  unknown: "its Git state could not be read",
});

/// How the refusal opens, from `finish_refusal`.
export const FINISH_REFUSAL_PREFIX = "workspace.finish is not available yet";

/** Whether this error is the Done gate refusing, rather than any other failure
 *  — a missing workspace, a dead session, a method that does not exist. */
export const isFinishRefusal = (error) => String(error?.message || error || "").includes(FINISH_REFUSAL_PREFIX);

/** Whether the gate refused for this reason. Both halves are asked: a refusal
 *  is only this blocker's if it is a refusal at all. */
export function refusedBecause(error, blocker) {
  return isFinishRefusal(error) && String(error?.message || error).includes(blocker);
}

/** The directories that are not repositories — each one a blocker in itself,
 *  because nothing measures a plain folder and no remote holds a copy of it, so
 *  Done cannot say its files are anywhere else.
 *
 *  A directory whose `is_git` the bridge did not state is read as plain: the
 *  safe direction, since the consequence of guessing wrong is asking Done to
 *  remove something nothing else has. */
export function plainDirectoriesOf(workspace) {
  return (workspace?.directories || []).filter((directory) => directory?.is_git !== true);
}

/** Whether every directory this workspace holds is a repository — the half of
 *  the gate the harness can satisfy by dropping what is not. */
export const everyDirectoryIsARepository = (workspace) => plainDirectoriesOf(workspace).length === 0;

/** The Git directories, which are the ones a finish reports on. */
export const gitDirectoriesOf = (workspace) =>
  (workspace?.directories || []).filter((directory) => directory?.is_git === true);
