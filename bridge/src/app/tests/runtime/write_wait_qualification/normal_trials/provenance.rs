//! OFFSOURCE. Only the two explicitly forwarded input paths are consulted.
//! Runner receipts bind the actual binary to a clean source tree/compiler run;
//! a quiet receipt is an external authorization/attestation, not a host audit.
use super::super::qualification_metadata::{
    actual_affinity, binary_hash, read_input, validate_build, validate_current_source,
};
use super::super::trial_complete::Provenance;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RunInputs {
    pub schema: String,
    pub run_id: String,
    pub provenance: Provenance,
    pub artifact_root: PathBuf,
    pub expected_envelopes: [ExpectedEnvelope; 2],
    pub binary_receipt: HashedInput,
    pub compiler_receipt: HashedInput,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HashedInput {
    pub path: PathBuf,
    pub sha256: String,
}
#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExpectedEnvelope {
    label: String,
    projects: usize,
    tracker_tasks: usize,
    registry_workspaces: usize,
    active_workspaces: usize,
    canonical_histories: usize,
    thread_items: u64,
}
impl ExpectedEnvelope {
    fn is_expected(
        &self,
        label: &str,
        tasks: usize,
        workspaces: usize,
        histories: usize,
        items: u64,
    ) -> bool {
        self.label == label
            && self.projects == 4
            && self.tracker_tasks == tasks
            && self.registry_workspaces == workspaces
            && self.active_workspaces == 8
            && self.canonical_histories == histories
            && self.thread_items == items
    }
}

pub(super) struct LoadAttempt {
    pub result: Result<RunInputs, String>,
    // Exact bytes read once, retained even when a later validation fails.
    pub captured: Vec<(&'static str, Vec<u8>)>,
}

pub(super) fn load() -> LoadAttempt {
    let mut captured = Vec::new();
    let result = (|| {
        let manifest = std::env::var_os("BUILD_QUALIFICATION_MANIFEST")
            .ok_or("explicit qualification manifest required")?;
        let quiet = std::env::var_os("BUILD_QUALIFICATION_QUIET_RELEASE")
            .ok_or("explicit root quiet receipt required")?;
        let input: RunInputs = read_input(
            Path::new(&manifest),
            "input-manifest.json",
            None,
            &mut captured,
        )?;
        validate_fields(&input, cfg!(debug_assertions))?;
        let binary: Value = read_input(
            &input.binary_receipt.path,
            "binary-receipt.json",
            Some(&input.binary_receipt.sha256),
            &mut captured,
        )?;
        let compiler: Value = read_input(
            &input.compiler_receipt.path,
            "compiler-receipt.json",
            Some(&input.compiler_receipt.sha256),
            &mut captured,
        )?;
        let quiet: Value = read_input(
            Path::new(&quiet),
            "quiet-release.json",
            Some(&input.provenance.fresh_quiet_release_sha256),
            &mut captured,
        )?;
        validate_build(&input.provenance, &binary, &compiler)?;
        validate_current_source(&input.provenance)?;
        validate_quiet(&input, &quiet, unix_us()?)?;
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        validate_runtime(
            &input.provenance,
            &binary_hash(&executable)?,
            i64::from(crate::priority::own_nice()),
            &actual_affinity()?,
        )?;
        Ok(input)
    })();
    LoadAttempt { result, captured }
}

fn validate_fields(input: &RunInputs, debug_assertions: bool) -> Result<(), String> {
    let p = &input.provenance;
    if input.schema != "build.qualification.inputs.v1" || input.run_id.is_empty() {
        return Err("named six-original qualification manifest required".into());
    }
    if !hex(&p.source_revision, 40)
        || !hex(&p.source_tree, 40)
        || !hex(&p.binary_sha256, 64)
        || !hex(&p.fresh_quiet_release_sha256, 64)
        || !hex(&p.service_hook_contract_sha256, 64)
        || !hex(&input.binary_receipt.sha256, 64)
        || !hex(&input.compiler_receipt.sha256, 64)
    {
        return Err("exact source/tree and receipt hashes required".into());
    }
    if p.profile != "release-opt3-debug-assertions-off" || debug_assertions {
        return Err("pinned release opt3 without debug assertions required".into());
    }
    if !input.artifact_root.is_absolute() || p.fixture_seed.is_empty() {
        return Err("absolute original artifact root and fixed fixture seed required".into());
    }
    if !input.expected_envelopes[0].is_expected("small", 100, 10, 16, 1000)
        || !input.expected_envelopes[1].is_expected("large", 10000, 200, 206, 100000)
    {
        return Err("exact Small/Large starting envelope declarations required".into());
    }
    let expected = json!({"qualification":true,"expected_envelopes":input.expected_envelopes});
    if p.fixture_counts != expected {
        return Err("factory fixture-count contract differs from the manifest expectations".into());
    }
    Ok(())
}

fn validate_quiet(input: &RunInputs, quiet: &Value, now: u64) -> Result<(), String> {
    let p = &input.provenance;
    if quiet["schema"] != "build.qualification.quiet-release.v1"
        || quiet["purpose"] != "six-original-qualification"
        || quiet["exact_selector"] != "app::tests::runtime::write_wait_qualification::normal_trials::qualify_small_and_large_three_original_ten_minute_trials"
        || quiet["run_id"] != input.run_id
        || quiet["source_revision"] != p.source_revision
        || quiet["source_tree"] != p.source_tree
        || quiet["binary_sha256"] != p.binary_sha256
        || quiet["artifact_root"] != serde_json::to_value(&input.artifact_root).unwrap()
        || quiet["whole_host_quiet_authorized"] != true
        || quiet["no_gates_or_builds_during_suite"] != true
        || quiet["coordinator_authorization"]
            .as_str()
            .is_none_or(str::is_empty)
    {
        return Err("purpose-bound coordinator quiet authorization required".into());
    }
    let issued = quiet["issued_at_unix_us"]
        .as_u64()
        .ok_or("quiet issue time missing")?;
    let deadline = quiet["start_before_unix_us"]
        .as_u64()
        .ok_or("quiet start deadline missing")?;
    if issued > now || deadline < now || deadline < issued {
        return Err("quiet authorization is not current at original suite startup".into());
    }
    Ok(())
}
fn hex(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn unix_us() -> Result<u64, String> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    u64::try_from(elapsed.as_micros()).map_err(|error| error.to_string())
}

fn validate_runtime(
    provenance: &Provenance,
    binary_sha256: &str,
    nice: i64,
    affinity: &[usize],
) -> Result<(), String> {
    if binary_sha256 != provenance.binary_sha256 {
        return Err("actual test binary hash differs from frozen build receipts".into());
    }
    if nice != 10 || provenance.nice != 10 {
        return Err("actual and manifest NI10 required".into());
    }
    if affinity != provenance.cpu_affinity {
        return Err("actual inherited CPU affinity differs from frozen manifest".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
