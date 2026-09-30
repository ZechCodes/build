//! Every git a sync starts, run as nobody's command (#268).
//!
//! Nobody asked for this git, so it asks nobody anything and starts nothing
//! of the user's: the unattended environment ([`run_git_unattended`]: no
//! prompt, stdin closed, killed at its deadline) on every command, not only
//! the fetch, and on each of them
//!
//! - no hooks. The user did not start this merge, so a `post-merge` that runs
//!   `npm install` or a `reference-transaction` that notifies someone is not
//!   theirs to expect. `core.hooksPath` names `/dev/null`, a path no hook can
//!   be found under.
//! - no automatic maintenance. `fetch` and `merge` each start
//!   `maintenance run --auto` when done, which may detach and outlive the
//!   deadline's kill.
//!
//! Filters are left on: a checkout without its smudge filter (git-lfs) would
//! write pointer files where the user's files belong. They run unattended
//! like the rest, so one that needs a credential fails rather than asks.

use crate::git_process::{git_failure, run_git_unattended};
use std::ffi::OsStr;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

/// How long a sync's quick local git (a read, a ref update) may run. The
/// checkout's fast-forward has a cap of its own ([`super::checkout`]).
const LOCAL_DEADLINE: Duration = Duration::from_secs(30);

const NOBODYS_COMMAND: &[&str] = &[
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "gc.auto=0",
    "-c",
    "maintenance.auto=false",
];

/// One git command of a sync's, bounded by `deadline`, its raw output.
pub(super) fn sync_git_within(
    path: &Path,
    args: &[&str],
    deadline: Duration,
) -> std::io::Result<Output> {
    let command: Vec<&OsStr> = NOBODYS_COMMAND.iter().chain(args).map(OsStr::new).collect();
    run_git_unattended(path, &command, deadline)
}

/// One local git command of a sync's: what it printed, or what it said
/// about why it failed.
pub(super) fn sync_git(path: &Path, args: &[&str]) -> Result<String, String> {
    let output = sync_git_within(path, args, LOCAL_DEADLINE).map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        Err(git_failure(&args, &output).to_string())
    }
}
