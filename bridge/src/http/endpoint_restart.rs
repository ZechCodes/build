use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub(super) fn planned(state: &Path, invocation: &str) -> bool {
    if !valid_invocation(invocation) {
        return false;
    }
    read_marker(&state.with_extension("restart"))
        .is_some_and(|marker| marker.strip_suffix('\n') == Some(invocation))
}

fn read_marker(path: &Path) -> Option<String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let mut marker = String::new();
    file.take(64).read_to_string(&mut marker).ok()?;
    Some(marker)
}

fn valid_invocation(value: &str) -> bool {
    value.len() == 32
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        && value.bytes().any(|byte| byte != b'0')
}

pub(crate) struct RestartIntent(PathBuf);

impl RestartIntent {
    pub(crate) fn publish(state: &Path, invocation: &str) -> io::Result<Self> {
        if !valid_invocation(invocation) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid service invocation",
            ));
        }
        let path = state.with_extension("restart");
        write_marker(&path, &format!("{invocation}\n"))?;
        Ok(Self(path))
    }
}

fn write_marker(path: &Path, text: &str) -> io::Result<()> {
    let temporary = path.with_extension(format!("restart-{}.tmp", uuid::Uuid::new_v4()));
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let written = marker.write_all(text.as_bytes());
    drop(marker);
    let result = written.and_then(|()| std::fs::rename(&temporary, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub(super) fn publish_live_invocation(state: &Path) -> io::Result<String> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    write_marker(
        &state.with_extension("run"),
        &format!("{} {id}\n", std::process::id()),
    )?;
    Ok(id)
}

pub(crate) fn active_invocation(state: &Path, pid: u32) -> Option<String> {
    let marker = read_marker(&state.with_extension("run"))?;
    let (process, id) = marker.strip_suffix('\n')?.split_once(' ')?;
    (process.parse::<u32>().ok()? == pid && valid_invocation(id)).then(|| id.to_owned())
}

impl Drop for RestartIntent {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
