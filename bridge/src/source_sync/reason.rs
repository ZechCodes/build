//! What a sync says when git refused: one or two lines of git's own words,
//! with nothing in them that was a secret.

use super::{Failure, SyncOutcome};

/// How much of what git said a reason keeps. Enough to say why; not a log.
const MAX_LINES: usize = 3;
const MAX_CHARS: usize = 400;

/// What git says when a remote wanted something only a person can give.
const NEEDS_YOU: &[&str] = &[
    "Authentication failed",
    "could not read Username",
    "could not read Password",
    "terminal prompts disabled",
    "Permission denied",
    "Host key verification failed",
    "passphrase",
    "401",
    "403",
];

/// `text` with the user and password taken out of every url in it. A fetch
/// can echo the url it was given, and a url can carry a token
/// (`https://user:token@host/…`), so nothing git said is stored or shown
/// before passing through here.
pub fn without_credentials(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(scheme_end) = rest.find("://") {
        let (head, tail) = rest.split_at(scheme_end + 3);
        kept.push_str(head);
        let authority_end = tail
            .find(|c: char| c == '/' || c == '\'' || c == '"' || c.is_whitespace())
            .unwrap_or(tail.len());
        let authority = &tail[..authority_end];
        kept.push_str(
            authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host),
        );
        rest = &tail[authority_end..];
    }
    kept.push_str(rest);
    kept
}

/// The first few non-empty lines of `text`, bounded.
pub(super) fn first_lines(text: &str) -> String {
    let joined = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(MAX_LINES)
        .collect::<Vec<_>>()
        .join(" ");
    match joined.char_indices().nth(MAX_CHARS) {
        Some((cut, _)) => format!("{}…", &joined[..cut]),
        None => joined,
    }
}

/// What a fetch git ran and refused comes to: a remote that has no such
/// branch is a skip, anything else a failure, marked as needing you when the
/// remote wanted a secret.
pub(super) fn fetch_failure(said: &str, remote: &str, base: &str) -> SyncOutcome {
    if said.contains("couldn't find remote ref") {
        return SyncOutcome::Skipped(format!("{remote} has no branch {base}."));
    }
    let said = without_credentials(said);
    SyncOutcome::Failed(Failure {
        needs_you: NEEDS_YOU.iter().any(|sign| said.contains(sign)),
        reason: format!("The fetch from {remote} failed: {}", first_lines(&said)),
    })
}
