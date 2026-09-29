//! The one check every Git remote a client names passes before git sees it.
//!
//! A remote reaches git as an argument (`git clone`, `git remote add`,
//! `git remote set-url`), never through a shell, so what is guarded against
//! here is git reading the text as something other than a location: an option
//! (`--upload-pack=…`), a transport helper that runs a program (`ext::…`), or
//! a second argument smuggled in with whitespace or a newline.

/// Longer than any real clone url, short enough that nothing is stored or
/// echoed back at an unbounded size.
const MAX_REMOTE_LEN: usize = 2048;

/// The url schemes git is asked to speak. `file` and a bare absolute path are
/// how a local repository is named; everything else is a network transport.
const SCHEMES: &[&str] = &["https", "http", "ssh", "git", "git+ssh", "ssh+git", "file"];

/// The remote, trimmed, when it names a location git can clone from and
/// nothing else; a sentence saying why not otherwise. An empty remote is the
/// caller's to interpret (clearing one), so it is refused here like any other
/// url that names nowhere.
pub fn usable_remote_url(remote: &str) -> Result<String, String> {
    let remote = remote.trim();
    if remote.is_empty() {
        return Err("A Git remote needs a url.".to_string());
    }
    if remote.len() > MAX_REMOTE_LEN {
        return Err("That Git remote is too long.".to_string());
    }
    if remote.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("A Git remote cannot contain spaces or control characters.".to_string());
    }
    if remote.starts_with('-') {
        return Err("A Git remote cannot start with a dash.".to_string());
    }
    if names_a_transport_helper(remote) {
        return Err(
            "Build does not use Git transport helpers (name::address) as remotes.".to_string(),
        );
    }
    if encodes_an_ssh_authority(remote) {
        return Err("A Git remote's ssh user, host or port cannot be percent-encoded.".to_string());
    }
    if names_a_host_git_would_read_as_an_option(remote) {
        return Err("A Git remote's user, host or port cannot start with a dash.".to_string());
    }
    if names_a_location(remote) {
        Ok(remote.to_string())
    } else {
        Err(format!(
            "{remote} is not a Git remote Build can use. Use an https, ssh, git or file url, user@host:path, or an absolute path."
        ))
    }
}

/// `<transport>::<address>`: git hands the address to `git-remote-<transport>`,
/// which for `ext` runs an arbitrary command.
fn names_a_transport_helper(remote: &str) -> bool {
    remote.split_once("::").is_some_and(|(transport, _)| {
        !transport.is_empty()
            && transport
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// The ssh url schemes: git hands these to `ssh`.
const SSH_SCHEMES: &[&str] = &["ssh", "git+ssh", "ssh+git"];

/// Git percent-decodes an ssh url's user, host and port before it hands them
/// to `ssh`, so `ssh://%2DoProxyCommand=sh/x` reaches ssh as an option once
/// decoded. No real ssh remote needs a `%` there, so any is refused rather
/// than decoded and checked again.
fn encodes_an_ssh_authority(remote: &str) -> bool {
    remote.split_once("://").is_some_and(|(scheme, rest)| {
        SSH_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str())
            && rest.split('/').next().unwrap_or_default().contains('%')
    })
}

/// Git hands an ssh remote's user, host and port to `ssh` as arguments, so
/// one that starts with a dash (`ssh://-oProxyCommand=sh/x`,
/// `git@-oProxyCommand=sh:x`) would be read as an ssh option. Git refuses
/// these itself these days; Build does not lean on that.
fn names_a_host_git_would_read_as_an_option(remote: &str) -> bool {
    let authority = match remote.split_once("://") {
        Some((_, rest)) => rest.split('/').next().unwrap_or_default(),
        None if remote.starts_with('/') => return false,
        None => remote.split(':').next().unwrap_or_default(),
    };
    let (user, host_port) = authority
        .rsplit_once('@')
        .map_or(("", authority), |(user, host)| (user, host));
    let (host, port) = match host_port.strip_prefix('[') {
        Some(bracketed) => bracketed
            .split_once(']')
            .map_or((bracketed, ""), |(host, rest)| {
                (host, rest.strip_prefix(':').unwrap_or(rest))
            }),
        None => host_port.split_once(':').unwrap_or((host_port, "")),
    };
    [user, host, port].iter().any(|part| part.starts_with('-'))
}

fn names_a_location(remote: &str) -> bool {
    match remote.split_once("://") {
        Some((scheme, rest)) => {
            SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) && !rest.is_empty()
        }
        None => remote.starts_with('/') || is_scp_like(remote),
    }
}

/// `[user@]host:path`, which git reads as ssh. A slash before the first colon
/// makes git read it as a local path instead, which is refused here unless it
/// is absolute.
fn is_scp_like(remote: &str) -> bool {
    remote.split_once(':').is_some_and(|(host, path)| {
        let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
        !host.is_empty() && !host.contains('/') && !path.is_empty()
    })
}

#[cfg(test)]
mod tests {
    use super::usable_remote_url;

    #[test]
    fn the_remotes_people_clone_from_are_accepted_trimmed() {
        for remote in [
            "git@github.com:ZechCodes/Do.git",
            "https://github.com/ZechCodes/Do.git",
            "ssh://git@example.com:2222/org/repo.git",
            "ssh://git@[::1]/repo.git",
            "git://example.com/repo.git",
            "file:///srv/git/repo.git",
            "/home/zech/repos/tokens.git",
            "/srv/git/a@-b:c.git",
            "HTTPS://example.com/repo",
        ] {
            assert_eq!(
                usable_remote_url(&format!("  {remote} ")),
                Ok(remote.to_string())
            );
        }
    }

    #[test]
    fn a_remote_git_would_read_as_an_option_is_refused() {
        for remote in [
            "--upload-pack=touch /tmp/pwned",
            "--upload-pack=sh",
            "-oProxyCommand=sh",
            "--mirror=fetch",
        ] {
            assert!(usable_remote_url(remote).is_err(), "{remote}");
        }
    }

    #[test]
    fn an_ssh_host_user_or_port_git_would_read_as_an_option_is_refused() {
        for remote in [
            "ssh://-oProxyCommand=sh/x",
            "ssh://git@-oProxyCommand=sh/x",
            "ssh://-oProxyCommand=sh@example.com/x",
            "ssh://git@example.com:-oProxyCommand=sh/x",
            "git+ssh://-oProxyCommand=sh/x",
            "ssh://[-oProxyCommand=sh]/x",
            "git@-oProxyCommand=sh:x",
            "git@-host:x",
            "a@b@-oProxyCommand=sh:x",
        ] {
            assert!(usable_remote_url(remote).is_err(), "{remote}");
        }
    }

    #[test]
    fn an_ssh_authority_git_would_percent_decode_is_refused() {
        for remote in [
            "ssh://%2DoProxyCommand=sh/x",
            "ssh://%2doProxyCommand=sh@example.com/x",
            "ssh://%2DoProxyCommand=sh@example.com/x",
            "ssh://git@%2dhost/x",
            "ssh://git@example.com:%2D1/x",
            "git+ssh://git@%2Dhost/x",
            "ssh+git://git@%2Dhost/x",
        ] {
            assert!(usable_remote_url(remote).is_err(), "{remote}");
        }
        assert!(usable_remote_url("ssh://git@example.com/org/a%20b.git").is_ok());
    }

    #[test]
    fn a_transport_helper_that_runs_a_program_is_refused() {
        for remote in ["ext::sh -c touch% /tmp/pwned", "ext::sh", "fd::3", "x+y::z"] {
            assert!(usable_remote_url(remote).is_err(), "{remote}");
        }
    }

    #[test]
    fn a_second_argument_or_line_cannot_be_smuggled_in() {
        for remote in [
            "https://example.com/a.git --upload-pack=sh",
            "https://example.com/a.git\n[core]",
            "https://example.com/a\u{0}.git",
            "https://example.com/a\t.git",
        ] {
            assert!(usable_remote_url(remote).is_err(), "{remote:?}");
        }
    }

    #[test]
    fn a_remote_that_names_nowhere_git_should_go_is_refused() {
        for remote in [
            "",
            "   ",
            "relative/path",
            "ftp://example.com/repo.git",
            "javascript://x",
            "https://",
            ":path",
            "host:",
            "a/b:c",
        ] {
            assert!(usable_remote_url(remote).is_err(), "{remote:?}");
        }
        assert!(usable_remote_url(&format!("https://e.com/{}", "a".repeat(2100))).is_err());
    }
}
