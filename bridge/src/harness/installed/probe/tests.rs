use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::*;
use crate::harness::installed::ListedModel;

/// Written by a child process, so no test worker forking meanwhile can hold
/// the script open for writing when it is run (ETXTBSY).
fn fake_cli(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    crate::isolation::test_fixture::write_executable(&path, &format!("#!/bin/sh\n{body}\n"));
    path
}

fn version(raw: &str) -> Version {
    Version::parse(raw).unwrap()
}

#[test]
fn versions_read_out_of_every_cli_s_own_words() {
    assert_eq!(
        version_in("2.1.280 (Claude Code)"),
        Some(version("2.1.280"))
    );
    assert_eq!(version_in("codex-cli 0.155.1\n"), Some(version("0.155.1")));
    assert_eq!(
        version_in("build_bridge/0.155.1 (Linux Unknown; x86_64) unknown"),
        Some(version("0.155.1"))
    );
    assert_eq!(version_in("0.86.1"), Some(version("0.86.1")));
    assert_eq!(version_in("v1.2.3"), Some(version("1.2.3")));
    assert_eq!(
        version_in("2.2.0-beta.1 (Claude Code)"),
        Some(version("2.2.0-beta.1"))
    );
    assert_eq!(version_in("Claude Code, some build"), None);
    assert_eq!(version_in(""), None);
}

#[test]
fn the_version_flag_reads_the_cli_s_answer() {
    let dir = tempfile::tempdir().unwrap();
    let cli = fake_cli(
        dir.path(),
        "claude",
        r#"[ "$#" -eq 1 ] && [ "$1" = --version ] || exit 7
echo '2.1.280 (Claude Code)'"#,
    );

    let reading = VERSION_FLAG.read(cli.to_str().unwrap());

    assert_eq!(reading.version, Some(version("2.1.280")));
    assert_eq!(reading.listed, None);
}

#[test]
fn a_cli_that_cannot_answer_reads_as_knowing_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let failing = fake_cli(dir.path(), "failing", "echo '2.1.280'; exit 3");
    let garbled = fake_cli(dir.path(), "garbled", "echo 'no version here'");
    let missing = dir.path().join("missing");

    for cli in [&failing, &garbled, &missing] {
        assert_eq!(
            VERSION_FLAG.read(cli.to_str().unwrap()),
            CliReading::default(),
            "{}",
            cli.display()
        );
    }
}

/// A wrapper that hangs, holding a child of its own, is cut off at the
/// deadline with its whole process group: nothing it started outlives the ask.
#[test]
fn a_hanging_cli_is_killed_with_everything_it_started() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("child.pid");
    let cli = fake_cli(
        dir.path(),
        "hanging",
        &format!("sleep 60 & echo $! > '{}'\nwait", pid_file.display()),
    );

    let started = Instant::now();
    let reading = VERSION_FLAG.read(cli.to_str().unwrap());

    assert_eq!(reading, CliReading::default());
    assert!(started.elapsed() < PROBE_DEADLINE + Duration::from_secs(2));
    let child: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_gone(child);
}

fn assert_gone(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        // SAFETY: signal 0 only asks whether the process exists.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("process {pid} outlived the probe");
}

/// Answers `initialize` and `model/list` the way codex 0.155.1 does, over two
/// pages, with a notification between them.
const FAKE_APP_SERVER: &str = r#"[ "$1" = app-server ] || exit 9
while read -r line; do
  case "$line" in
    *'"initialize"'*)
      echo '{"id":1,"result":{"userAgent":"build_bridge_probe/0.155.1 (Linux Unknown; x86_64) unknown (build_bridge_probe; 0)"}}' ;;
    *'"model/list"'*'"cursor":null'*)
      echo '{"method":"remoteControl/status/changed","params":{"status":"disabled"}}'
      echo '{"id":2,"result":{"data":[{"id":"gpt-6-astra","model":"gpt-6-astra","displayName":"GPT-6-Astra","hidden":false,"isDefault":true,"supportedReasoningEfforts":[{"reasoningEffort":"low","description":"Fast"},{"reasoningEffort":"ultra","description":"Max"}]},{"id":"gpt-reserve","model":"gpt-reserve","displayName":"GPT-Reserve","hidden":true,"supportedReasoningEfforts":[]}],"nextCursor":"page-2"}}' ;;
    *'"model/list"'*)
      echo '{"id":3,"result":{"data":[{"id":"gpt-5.5","model":"gpt-5.5","displayName":"GPT-5.5","hidden":false,"supportedReasoningEfforts":[{"reasoningEffort":"xhigh","description":"Extra"}]}],"nextCursor":null}}' ;;
  esac
done"#;

#[test]
fn codex_lists_every_model_on_every_page_and_its_version() {
    let dir = tempfile::tempdir().unwrap();
    let cli = fake_cli(dir.path(), "codex", FAKE_APP_SERVER);

    let reading = CODEX_MODEL_LIST.read(cli.to_str().unwrap());

    assert_eq!(reading.version, Some(version("0.155.1")));
    assert_eq!(
        reading.listed,
        Some(vec![
            ListedModel {
                id: "gpt-6-astra".into(),
                label: "GPT-6-Astra".into(),
                hidden: false,
                efforts: vec!["low".into(), "ultra".into()],
            },
            ListedModel {
                id: "gpt-reserve".into(),
                label: "GPT-Reserve".into(),
                hidden: true,
                efforts: vec![],
            },
            ListedModel {
                id: "gpt-5.5".into(),
                label: "GPT-5.5".into(),
                hidden: false,
                efforts: vec!["xhigh".into()],
            },
        ])
    );
}

#[test]
fn a_codex_that_cannot_list_still_says_its_version() {
    let dir = tempfile::tempdir().unwrap();
    let cli = fake_cli(
        dir.path(),
        "codex",
        r#"while read -r line; do
  case "$line" in
    *'"initialize"'*) echo '{"id":1,"result":{"userAgent":"build_bridge_probe/0.150.0 (Linux)"}}' ;;
    *'"model/list"'*) echo '{"id":2,"error":{"code":-32601,"message":"method not found"}}' ;;
  esac
done"#,
    );

    let reading = CODEX_MODEL_LIST.read(cli.to_str().unwrap());

    assert_eq!(reading.version, Some(version("0.150.0")));
    assert_eq!(reading.listed, None);
}

#[test]
fn a_codex_that_never_answers_is_cut_off() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("child.pid");
    let cli = fake_cli(
        dir.path(),
        "codex",
        &format!("sleep 60 & echo $! > '{}'\nwait", pid_file.display()),
    );

    let started = Instant::now();
    let reading = CODEX_MODEL_LIST.read(cli.to_str().unwrap());

    assert_eq!(reading, CliReading::default());
    assert!(started.elapsed() < PROBE_DEADLINE + Duration::from_secs(2));
    let child: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_gone(child);
}

#[test]
fn a_missing_codex_reads_as_knowing_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("codex");

    assert_eq!(
        CODEX_MODEL_LIST.read(missing.to_str().unwrap()),
        CliReading::default()
    );
}

/// A CLI's output is read into memory, so one that says too much is cut off
/// and read as knowing nothing, however it says it.
#[test]
fn a_cli_that_says_too_much_is_cut_off() {
    let dir = tempfile::tempdir().unwrap();
    let endless_lines = fake_cli(dir.path(), "lines", "yes '2.1.280 (Claude Code)'");
    let endless_line = fake_cli(
        dir.path(),
        "line",
        "echo 2.1.280; head -c 2000000 /dev/zero | tr '\\0' a",
    );

    for cli in [&endless_lines, &endless_line] {
        let started = Instant::now();
        assert_eq!(
            VERSION_FLAG.read(cli.to_str().unwrap()),
            CliReading::default(),
            "{}",
            cli.display()
        );
        assert!(
            started.elapsed() < PROBE_DEADLINE,
            "cut off by what it said, not by the clock"
        );
    }
}

/// What codex lists is kept only where it could be a model id, with its words
/// clipped: a listed id reaches a picker, and a picked one reaches argv.
#[test]
fn codex_keeps_only_what_could_be_a_model() {
    let dir = tempfile::tempdir().unwrap();
    let long_label = "L".repeat(500);
    let long_id = "g".repeat(65);
    let cli = fake_cli(
        dir.path(),
        "codex",
        &format!(
            r#"while read -r line; do
  case "$line" in
    *'"initialize"'*) echo '{{"id":1,"result":{{"userAgent":"build_bridge_probe/0.155.1"}}}}' ;;
    *'"model/list"'*) printf '%s\n' '{{"id":2,"result":{{"data":[{{"id":"--effort","displayName":"flag"}},{{"id":"gpt 6; rm -rf ~","displayName":"spaced"}},{{"id":"{long_id}"}},{{"id":"gpt-6-sol","displayName":"{long_label}\u0007"}}],"nextCursor":null}}}}' ;;
  esac
done"#
        ),
    );

    let listed = CODEX_MODEL_LIST.read(cli.to_str().unwrap()).listed.unwrap();

    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].id, "gpt-6-sol");
    assert_eq!(listed[0].label, "L".repeat(80));
}

/// A probe asks what the machine runs, not what a checkout pins: it starts in
/// the home directory, and inherits no agent identity from the bridge (which
/// may itself have been started by an agent).
#[test]
fn a_probe_runs_in_the_home_directory_as_nobody_s_agent() {
    let dir = tempfile::tempdir().unwrap();
    let cli = fake_cli(
        dir.path(),
        "claude",
        r#"[ "$(pwd -P)" = "$(cd "$HOME" && pwd -P)" ] || exit 5
echo 2.1.284"#,
    );
    let command = child::command(cli.to_str().unwrap(), &["--version"], false);
    let envs: Vec<_> = command.get_envs().collect();

    assert_eq!(
        VERSION_FLAG.read(cli.to_str().unwrap()).version,
        Some(version("2.1.284"))
    );
    for marker in crate::harness::INHERITED_AGENT_MARKERS {
        assert!(
            envs.contains(&(std::ffi::OsStr::new(marker), None)),
            "{marker} is removed: {envs:?}"
        );
    }
}

/// A probe never installs a CLI: a mise wrapper run with nothing installed
/// would download it, and the deadline would kill that download halfway.
#[test]
fn a_probe_tells_mise_to_stay_offline() {
    let dir = tempfile::tempdir().unwrap();
    let cli = fake_cli(
        dir.path(),
        "claude",
        r#"[ "$MISE_OFFLINE" = 1 ] || exit 7
echo 2.1.284"#,
    );

    assert_eq!(
        VERSION_FLAG.read(cli.to_str().unwrap()).version,
        Some(version("2.1.284"))
    );
}

/// A list with nothing in it Build could start says nothing about what codex
/// runs, whether codex listed nothing or nothing it listed could be a model.
#[test]
fn a_codex_that_lists_nothing_usable_reads_as_listing_nothing() {
    let dir = tempfile::tempdir().unwrap();
    for (name, data) in [
        ("empty", "[]"),
        (
            "unusable",
            r#"[{"id":"--effort"},{"id":"gpt 6; rm -rf ~"}]"#,
        ),
    ] {
        let cli = fake_cli(
            dir.path(),
            name,
            &format!(
                r#"while read -r line; do
  case "$line" in
    *'"initialize"'*) echo '{{"id":1,"result":{{"userAgent":"build_bridge_probe/0.155.1"}}}}' ;;
    *'"model/list"'*) printf '%s\n' '{{"id":2,"result":{{"data":{data},"nextCursor":null}}}}' ;;
  esac
done"#
            ),
        );

        let reading = CODEX_MODEL_LIST.read(cli.to_str().unwrap());

        assert_eq!(reading.version, Some(version("0.155.1")), "{name}");
        assert_eq!(reading.listed, None, "{name}");
    }
}

/// The real CLIs on this machine, asked the way the bridge asks them. Needs
/// them installed, so it is run by hand: `cargo test --lib -- --ignored
/// the_installed_clis_answer`.
#[test]
#[ignore = "asks the claude and codex installed on this machine"]
fn the_installed_clis_answer() {
    let claude = VERSION_FLAG.read("claude");
    let codex = CODEX_MODEL_LIST.read("codex");
    println!("claude: {claude:?}");
    println!("codex: {codex:?}");
    assert!(claude.version.is_some());
    assert!(codex.version.is_some());
    assert!(codex.listed.is_some_and(|listed| !listed.is_empty()));
}
