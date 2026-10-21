//! Inspect only the current process and its bounded direct launcher ancestry.
use super::*;
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub pid: u32,
    pub start_ticks: u64,
}
#[derive(serde::Serialize)]
pub(super) struct Actual {
    pub lineage: Vec<Identity>,
    pub nice: i64,
    pub affinity: Vec<usize>,
}
pub(super) fn actual() -> Result<Actual, String> {
    let mut pid = std::process::id();
    let mut lineage = Vec::new();
    for _ in 0..4 {
        let (identity, parent) = identity(pid)?;
        lineage.push(identity);
        if parent == 0 || parent == pid {
            break;
        }
        pid = parent;
    }
    Ok(Actual {
        lineage,
        nice: i64::from(crate::priority::own_nice()),
        affinity: actual_affinity()?,
    })
}
fn identity(pid: u32) -> Result<(Identity, u32), String> {
    let text =
        std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|error| error.to_string())?;
    let tail = text
        .rsplit_once(") ")
        .ok_or("process stat malformed")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let parent = tail
        .get(1)
        .ok_or("parent pid missing")?
        .parse::<u32>()
        .map_err(|error| error.to_string())?;
    let start_ticks = tail
        .get(19)
        .ok_or("process start ticks missing")?
        .parse::<u64>()
        .map_err(|error| error.to_string())?;
    Ok((Identity { pid, start_ticks }, parent))
}
