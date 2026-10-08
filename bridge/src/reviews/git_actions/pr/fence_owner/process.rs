//! PID reuse, bridge death and surviving hook descendants are different facts.
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(super) struct Process {
    pid: u32,
    boot: Option<String>,
    started: Option<String>,
    group: i32,
}

struct Status {
    state: String,
    started: String,
}

fn status(pid: u32) -> Option<Status> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<_> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some(Status {
        state: fields.first()?.to_string(),
        started: fields.get(19)?.to_string(),
    })
}

fn boot() -> Option<String> {
    fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|value| value.trim().into())
}

impl Process {
    pub(super) fn capture(pid: u32) -> Result<Self, String> {
        // Linux records boot/start proof. Other Unix systems retain uncertain
        // identities while still allowing an absent dedicated group to settle.
        // SAFETY: getpgid reads process metadata and sends no signal.
        let group = unsafe { libc::getpgid(pid as i32) };
        if group < 0 {
            return Err("cannot observe hook process group".into());
        }
        Ok(Self {
            pid,
            boot: boot(),
            started: status(pid).map(|status| status.started),
            group,
        })
    }

    pub(super) fn child(pid: u32) -> Result<Self, String> {
        let child = Self::capture(pid)?;
        if child.group != pid as i32 {
            return Err("Git hook child has no dedicated process group".into());
        }
        Ok(child)
    }

    pub(super) fn dead(&self) -> bool {
        if self.boot_changed() {
            return true;
        }
        match (self.started.as_ref(), status(self.pid)) {
            (Some(saved), Some(current)) => current.started != *saved || terminal(&current.state),
            _ => absent(self.pid as i32),
        }
    }

    pub(super) fn group_dead(&self) -> bool {
        if self.boot_changed() {
            return true;
        }
        if !self.dead() {
            return false;
        }
        if absent(-self.group) {
            return true;
        }
        group_quiet(self.group)
    }

    fn boot_changed(&self) -> bool {
        matches!((&self.boot, boot()), (Some(saved), Some(current)) if *saved != current)
    }
}

fn absent(pid: i32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: signal zero checks existence without sending a signal.
    unsafe {
        libc::kill(pid, 0) == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}

fn terminal(state: &str) -> bool {
    matches!(state, "Z" | "X")
}

#[cfg(target_os = "linux")]
fn group_quiet(group: i32) -> bool {
    let Ok(processes) = fs::read_dir("/proc") else {
        return false;
    };
    for (index, entry) in processes.enumerate() {
        if index >= 65_536 {
            return false;
        }
        let Ok(entry) = entry else {
            return false;
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        // SAFETY: getpgid only reads process metadata. Processes outside this
        // child's dedicated group cannot still be executing its hooks.
        if unsafe { libc::getpgid(pid as i32) } != group {
            continue;
        }
        let Some(current) = status(pid) else {
            return false;
        };
        if !terminal(&current.state) {
            return false;
        }
    }
    true
}

#[cfg(not(target_os = "linux"))]
fn group_quiet(_group: i32) -> bool {
    false
}
