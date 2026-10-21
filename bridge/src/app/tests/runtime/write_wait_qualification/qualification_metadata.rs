//! Shared actual process metadata readers; unchanged smoke implementations.
use super::trial_complete::Provenance;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

const MAX_BINARY_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub(super) fn binary_hash(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_BINARY_BYTES {
        return Err(
            "actual binary is not a regular file within the declared4GiB metadata bound".into(),
        );
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > MAX_BINARY_BYTES {
            return Err("actual binary changed beyond metadata bound".into());
        }
        hash.update(&buffer[..count]);
    }
    if bytes != metadata.len() {
        return Err("actual binary length changed during metadata capture".into());
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(super) fn actual_affinity() -> Result<Vec<usize>, String> {
    let status = std::fs::read_to_string("/proc/self/status").map_err(|error| error.to_string())?;
    let mask = status
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
        .ok_or("actual process CPU affinity missing")?
        .trim();
    let mut cpus = Vec::new();
    for range in mask.split(',') {
        let (start, end) = range.split_once('-').unwrap_or((range, range));
        let first = start.parse::<usize>().map_err(|error| error.to_string())?;
        let last = end.parse::<usize>().map_err(|error| error.to_string())?;
        if last < first || last > 65_535 {
            return Err("actual CPU affinity outside declared metadata bound".into());
        }
        cpus.extend(first..=last);
    }
    Ok(cpus)
}

const MAX_INPUT_BYTES: u64 = 2 * 1024 * 1024;
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

pub(super) fn read_input<T: serde::de::DeserializeOwned>(
    path: &Path,
    name: &'static str,
    expected_hash: Option<&str>,
    captured: &mut Vec<(&'static str, Vec<u8>)>,
) -> Result<T, String> {
    if !path.is_absolute() {
        return Err(format!("{name}: absolute explicit input path required"));
    }
    let file = std::fs::File::open(path).map_err(|error| format!("{name}: {error}"))?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err(format!("{name}: regular file required"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    captured.push((name, bytes));
    let bytes = &captured.last().unwrap().1;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(format!("{name}: input exceeds 2MiB bound"));
    }
    let actual = format!("{:x}", Sha256::digest(bytes));
    if expected_hash.is_some_and(|expected| expected != actual) {
        return Err(format!("{name}: frozen input hash mismatch"));
    }
    serde_json::from_slice(bytes).map_err(|error| format!("{name}: {error}"))
}

fn validate_source(value: &Value, p: &Provenance) -> Result<(), String> {
    if value["head"] != p.source_revision
        || value["tree"] != p.source_tree
        || value["status"] != ""
        || value["patch_sha256"] != EMPTY_SHA256
        || value["untracked_hashes"] != json!({})
    {
        return Err("build receipt does not bind the exact clean source revision/tree".into());
    }
    Ok(())
}
pub(super) fn validate_build(
    p: &Provenance,
    binary: &Value,
    compiler: &Value,
) -> Result<(), String> {
    validate_source(&binary["source"], p)?;
    validate_source(&compiler["source"], p)?;
    if binary["sha256"] != p.binary_sha256
        || binary["cargo_profile"] != "release"
        || compiler["profile"] != "release"
        || compiler["opt_level"] != 3
        || compiler["debug_assertions"] != false
        || compiler["environment_label_used_as_proof"] != false
    {
        return Err("actual binary/compiler receipt profile mismatch".into());
    }
    let units = compiler["units"]
        .as_array()
        .ok_or("compiler units missing")?;
    if units.len() != 1 {
        return Err("exact pinned build_bridge lib-test compiler unit required".into());
    }
    validate_compiler_unit(&units[0])
}

fn validate_compiler_unit(unit: &Value) -> Result<(), String> {
    let flags: Vec<String> = serde_json::from_value(unit["actual_compile_flags"].clone())
        .map_err(|error| error.to_string())?;
    let cfg: Vec<String> =
        serde_json::from_value(unit["cfg"].clone()).map_err(|error| error.to_string())?;
    if unit["exit_code"] != 0
        || unit["actual_ni"] != 10
        || unit["actual_opt_level"] != 3
        || unit["debug_assertions"] != false
        || cfg.iter().any(|flag| flag == "debug_assertions")
        || !flags
            .windows(2)
            .any(|pair| pair == ["--crate-name", "build_bridge"])
        || !flags.windows(2).any(|pair| pair == ["-C", "opt-level=3"])
        || !flags.iter().any(|flag| flag == "--test")
    {
        return Err("actual successful NI10 opt3 lib-test compiler evidence required".into());
    }
    Ok(())
}

/// Validate only this binary's compiled repository location, never a live
/// bridge/profile location or a whole-host process scan.
pub(super) fn validate_current_source(p: &Provenance) -> Result<(), String> {
    let repository = git2::Repository::discover(env!("CARGO_MANIFEST_DIR"))
        .map_err(|error| error.to_string())?;
    validate_repository_source(&repository, p)
}

fn validate_repository_source(repository: &git2::Repository, p: &Provenance) -> Result<(), String> {
    let commit = repository
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(|error| error.to_string())?;
    let mut options = git2::StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);
    if commit.id().to_string() != p.source_revision
        || commit.tree_id().to_string() != p.source_tree
        || !repository
            .statuses(Some(&mut options))
            .map_err(|error| error.to_string())?
            .is_empty()
    {
        return Err("current source no longer matches the frozen clean compiler source".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
