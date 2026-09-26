//! Test-only executable writer shared by Rift's probe and CLI fixtures.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

const WRITER_TEST: &str = "isolation::test_fixture::fake_executable_writer_child";
const EXECUTABLE_PATH: &str = "BUILD_TEST_EXECUTABLE_PATH";
const EXECUTABLE_BODY: &str = "BUILD_TEST_EXECUTABLE_BODY";

pub(crate) fn write_executable(path: &Path, body: &str) {
    // Another test worker may fork while a writer is open. Its child can keep
    // the descriptor alive after this thread closes it, making exec fail with
    // ETXTBSY. The isolated writer exits before the script is ever executed.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", WRITER_TEST])
        .env(EXECUTABLE_PATH, path)
        .env(EXECUTABLE_BODY, body)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fake executable writer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn fake_executable_writer_child() {
    let Some(path) = std::env::var_os(EXECUTABLE_PATH) else {
        return;
    };
    let body = std::env::var(EXECUTABLE_BODY).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(path)
        .unwrap();
    file.write_all(body.as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);
}
