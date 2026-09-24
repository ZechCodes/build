//! The GitHub repositories this machine's `gh` can reach.
//!
//! `github.repos` runs `gh` and answers what it printed: the signed-in
//! account's repositories and each of its organisations', as one list. The
//! bridge keeps nothing — no cache, no filter, no refresh — the SPA stores the
//! list and searches it. This module knows nothing of the app state; it runs
//! with the app mutex released.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Output;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::git_process::run_command_with_deadline;

/// How long listing may take, `gh org list` and every `gh repo list` together.
pub const GH_DEADLINE: Duration = Duration::from_secs(20);

/// The most repositories one owner's listing answers.
const REPO_LIMIT: &str = "1000";

/// The fields `gh repo list --json` is asked for, in the order [`GhRepo`] names them.
const REPO_FIELDS: &str = "nameWithOwner,description,sshUrl,url,isPrivate,pushedAt";

/// How every refusal of `github.repos` begins, which is how the facade knows
/// one for what it is: this machine cannot answer, not a bridge fault.
pub const REFUSAL: &str = "Build cannot list GitHub repositories on ";

/// `gh` exits 4 when a command needs a signed-in account and has none.
const GH_AUTH_REQUIRED: i32 = 4;

/// One repository, as the wire carries it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct GithubRepo {
    pub name_with_owner: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub ssh_url: String,
    pub url: String,
    pub private: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed_at: Option<String>,
}

/// One repository, as `gh repo list --json` prints it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhRepo {
    name_with_owner: String,
    #[serde(default)]
    description: Option<String>,
    ssh_url: String,
    url: String,
    is_private: bool,
    #[serde(default)]
    pushed_at: Option<String>,
}

impl From<GhRepo> for GithubRepo {
    fn from(repo: GhRepo) -> Self {
        GithubRepo {
            name_with_owner: repo.name_with_owner,
            description: repo.description.filter(|text| !text.is_empty()),
            ssh_url: repo.ssh_url,
            url: repo.url,
            private: repo.is_private,
            pushed_at: repo.pushed_at.filter(|text| !text.is_empty()),
        }
    }
}

/// Why `gh` gave no list.
#[derive(Debug, PartialEq)]
enum GhFailure {
    NotInstalled,
    NotSignedIn,
    TimedOut,
    Failed(String),
}

impl GhFailure {
    /// The refusal as the user reads it, naming the machine.
    fn sentence(&self, machine: &str) -> String {
        let cannot = format!("{REFUSAL}{machine} because");
        match self {
            Self::NotInstalled => format!("{cannot} gh is not installed."),
            Self::NotSignedIn => {
                format!("{cannot} gh is not signed in. Run `gh auth login` on {machine}.")
            }
            Self::TimedOut => format!(
                "{cannot} gh did not answer within {} seconds.",
                GH_DEADLINE.as_secs()
            ),
            Self::Failed(detail) => format!("{cannot} gh failed: {detail}"),
        }
    }
}

/// The `gh` a bridge runs, and the name of the machine it runs on.
#[derive(Debug, Clone)]
pub struct GithubCli {
    program: OsString,
    machine: Option<String>,
}

impl Default for GithubCli {
    fn default() -> Self {
        GithubCli {
            program: OsString::from("gh"),
            machine: None,
        }
    }
}

impl GithubCli {
    /// A `gh` that is not there: what the unit tests' app states run, so a
    /// sweep over every verb never reaches the network through this machine's
    /// own `gh`.
    #[cfg(test)]
    pub(crate) fn absent() -> Self {
        GithubCli::at("/nonexistent/gh", "this machine")
    }

    /// A `gh` at `program` on a machine called `machine` — how a test stands a
    /// fake in for the real one.
    pub fn at(program: impl Into<OsString>, machine: impl Into<String>) -> Self {
        GithubCli {
            program: program.into(),
            machine: Some(machine.into()),
        }
    }

    /// Every repository the signed-in account and its organisations hold, or
    /// the sentence saying why there is no list. An organisation whose own
    /// listing fails is left out rather than failing the rest.
    pub fn list_repos(&self) -> Result<Vec<GithubRepo>, String> {
        let expiry = Instant::now() + GH_DEADLINE;
        let own = self
            .repo_list(None, expiry)
            .map_err(|failure| failure.sentence(&self.machine()))?;
        let orgs = self.org_list(expiry).unwrap_or_default();
        let theirs: Vec<Vec<GithubRepo>> = std::thread::scope(|scope| {
            let listings: Vec<_> = orgs
                .iter()
                .map(|org| scope.spawn(move || self.repo_list(Some(org), expiry)))
                .collect();
            listings
                .into_iter()
                .filter_map(|listing| listing.join().ok()?.ok())
                .collect()
        });
        Ok(union(std::iter::once(own).chain(theirs)))
    }

    fn machine(&self) -> String {
        self.machine
            .clone()
            .unwrap_or_else(crate::identity::default_device_name)
    }

    fn repo_list(
        &self,
        owner: Option<&str>,
        expiry: Instant,
    ) -> Result<Vec<GithubRepo>, GhFailure> {
        let mut args = vec!["repo", "list"];
        args.extend(owner);
        args.extend(["--limit", REPO_LIMIT, "--json", REPO_FIELDS]);
        parse_repos(&self.run(&args, expiry)?)
    }

    fn org_list(&self, expiry: Instant) -> Result<Vec<String>, GhFailure> {
        let printed = self.run(&["org", "list", "--limit", REPO_LIMIT], expiry)?;
        Ok(printed
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// What `gh <args>` printed on stdout, or why it printed no answer.
    fn run(&self, args: &[&str], expiry: Instant) -> Result<String, GhFailure> {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        let output = self
            .spawn(&args, expiry)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => GhFailure::NotInstalled,
                std::io::ErrorKind::TimedOut => GhFailure::TimedOut,
                _ => GhFailure::Failed(error.to_string()),
            })?;
        answered(output)
    }

    /// Run `gh` until `expiry`. A `gh` being replaced as it starts — an
    /// upgrade, or a test writing its fake while another thread forks — is
    /// busy for a moment, so only that errno is tried again.
    fn spawn(&self, args: &[&OsStr], expiry: Instant) -> std::io::Result<Output> {
        let mut retries = 0;
        loop {
            let left = expiry.saturating_duration_since(Instant::now());
            match run_command_with_deadline(&self.program, Path::new("/"), args, left) {
                Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && retries < 4 => {
                    retries += 1;
                    std::thread::sleep(Duration::from_millis(20));
                }
                outcome => return outcome,
            }
        }
    }
}

/// A finished `gh`: its stdout when it succeeded, otherwise why it did not.
fn answered(output: Output) -> Result<String, GhFailure> {
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(GH_AUTH_REQUIRED) || stderr.contains("gh auth login") {
        return Err(GhFailure::NotSignedIn);
    }
    let detail = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it exited without saying why")
        .to_string();
    Err(GhFailure::Failed(detail))
}

fn parse_repos(printed: &str) -> Result<Vec<GithubRepo>, GhFailure> {
    let repos: Vec<GhRepo> = serde_json::from_str(printed)
        .map_err(|error| GhFailure::Failed(format!("its list did not parse ({error})")))?;
    Ok(repos.into_iter().map(GithubRepo::from).collect())
}

/// Every listing's repositories once each, in the order they were listed.
fn union(listings: impl Iterator<Item = Vec<GithubRepo>>) -> Vec<GithubRepo> {
    let mut seen = std::collections::HashSet::new();
    listings
        .flatten()
        .filter(|repo| seen.insert(repo.name_with_owner.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A fake `gh` in `dir`: a shell script answering `script`'s cases.
    fn fake_gh(dir: &Path, script: &str) -> std::path::PathBuf {
        let path = dir.join("gh");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    const OWN: &str = r#"[{"nameWithOwner":"zech/build","description":"Agentic IDE","sshUrl":"git@github.com:zech/build.git","url":"https://github.com/zech/build","isPrivate":true,"pushedAt":"2026-09-24T22:00:00Z"},{"nameWithOwner":"zech/dotfiles","description":"","sshUrl":"git@github.com:zech/dotfiles.git","url":"https://github.com/zech/dotfiles","isPrivate":false,"pushedAt":"2026-09-01T10:00:00Z"}]"#;
    const ORG: &str = r#"[{"nameWithOwner":"smarter-dev/bot","description":"Discord bot","sshUrl":"git@github.com:smarter-dev/bot.git","url":"https://github.com/smarter-dev/bot","isPrivate":false,"pushedAt":"2026-09-20T08:00:00Z"},{"nameWithOwner":"zech/build","description":"Agentic IDE","sshUrl":"git@github.com:zech/build.git","url":"https://github.com/zech/build","isPrivate":true,"pushedAt":"2026-09-24T22:00:00Z"}]"#;

    fn signed_in_gh(dir: &Path) -> std::path::PathBuf {
        fake_gh(
            dir,
            &format!(
                r#"case "$1 $2 $3" in
  "repo list --limit") echo '{OWN}' ;;
  "org list --limit") printf 'smarter-dev\nsso-org\n' ;;
  "repo list smarter-dev") echo '{ORG}' ;;
  "repo list sso-org") echo 'GraphQL: Resource protected by organization SAML enforcement.' >&2; exit 1 ;;
  *) echo "unexpected: $*" >&2; exit 1 ;;
esac"#
            ),
        )
    }

    #[test]
    fn the_account_and_its_organisations_are_listed_once_each() {
        let dir = tempfile::tempdir().unwrap();
        let gh = GithubCli::at(signed_in_gh(dir.path()), "zech-desktop");
        let repos = gh.list_repos().unwrap();
        let names: Vec<&str> = repos
            .iter()
            .map(|repo| repo.name_with_owner.as_str())
            .collect();
        assert_eq!(names, ["zech/build", "zech/dotfiles", "smarter-dev/bot"]);
        assert_eq!(
            repos[0],
            GithubRepo {
                name_with_owner: "zech/build".into(),
                description: Some("Agentic IDE".into()),
                ssh_url: "git@github.com:zech/build.git".into(),
                url: "https://github.com/zech/build".into(),
                private: true,
                pushed_at: Some("2026-09-24T22:00:00Z".into()),
            }
        );
        assert_eq!(repos[1].description, None, "an empty description is none");
    }

    #[test]
    fn a_missing_gh_is_said_in_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let gh = GithubCli::at(dir.path().join("gh"), "zech-desktop");
        assert_eq!(
            gh.list_repos().unwrap_err(),
            "Build cannot list GitHub repositories on zech-desktop because gh is not installed."
        );
    }

    #[test]
    fn a_signed_out_gh_is_said_in_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let gh = fake_gh(
            dir.path(),
            "echo 'To get started with GitHub CLI, please run:  gh auth login' >&2; exit 4",
        );
        assert_eq!(
            GithubCli::at(gh, "zech-desktop").list_repos().unwrap_err(),
            "Build cannot list GitHub repositories on zech-desktop because gh is not signed in. \
             Run `gh auth login` on zech-desktop."
        );
    }

    #[test]
    fn any_other_failure_carries_what_gh_said() {
        let dir = tempfile::tempdir().unwrap();
        let gh = fake_gh(dir.path(), "echo 'HTTP 502: Bad Gateway' >&2; exit 1");
        assert_eq!(
            GithubCli::at(gh, "zech-desktop").list_repos().unwrap_err(),
            "Build cannot list GitHub repositories on zech-desktop because gh failed: HTTP 502: Bad Gateway"
        );
    }

    #[test]
    fn a_listing_that_does_not_parse_is_a_failure_not_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let gh = fake_gh(dir.path(), "echo 'not json'");
        let refusal = GithubCli::at(gh, "zech-desktop").list_repos().unwrap_err();
        assert!(
            refusal.starts_with("Build cannot list GitHub repositories on zech-desktop because gh failed: its list did not parse"),
            "{refusal}"
        );
    }

    #[test]
    fn an_account_without_organisations_is_its_own_list() {
        let dir = tempfile::tempdir().unwrap();
        let gh = fake_gh(
            dir.path(),
            &format!(r#"case "$1" in repo) echo '{OWN}' ;; org) exit 0 ;; esac"#),
        );
        let repos = GithubCli::at(gh, "zech-desktop").list_repos().unwrap();
        assert_eq!(repos.len(), 2);
    }

    #[test]
    fn the_timeout_sentence_names_the_deadline() {
        assert_eq!(
            GhFailure::TimedOut.sentence("zech-desktop"),
            "Build cannot list GitHub repositories on zech-desktop because gh did not answer within 20 seconds."
        );
    }
}
