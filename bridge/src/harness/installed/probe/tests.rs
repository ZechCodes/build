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
    assert_eq!(version_in("2.1.280 (Claude Code)"), Some(version("2.1.280")));
    assert_eq!(version_in("codex-cli 0.155.1\n"), Some(version("0.155.1")));
    assert_eq!(
        version_in("build_bridge/0.155.1 (Linux Unknown; x86_64) unknown"),
        Some(version("0.155.1"))
    );
    assert_eq!(version_in("0.86.1"), Some(version("0.86.1")));
    assert_eq!(version_in("v1.2.3"), Some(version("1.2.3")));
    assert_eq!(version_in("2.2.0-beta.1 (Claude Code)"), Some(version("2.2.0-beta.1")));
    assert_eq!(version_in("Claude Code, some build"), None);
    assert_eq!(version_in(""), None);
}

#[test]
fn the_version_flag_reads_the_cli_s_answer() {
    let dir = tempfile::tempdir().unwrap();
    let cli = fake_cli(dir.path(), "claude", "echo '2.1.280 (Claude Code)'");

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
