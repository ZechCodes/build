//! Fault-only purpose binding. Shared low-level receipt readers never grant
//! permission to run the six normal qualification originals.
use super::super::qualification_metadata::{
    actual_affinity, binary_hash, read_input, validate_build, validate_current_source,
};
use super::super::trial_complete::Provenance;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
mod process;
#[cfg(test)]
mod tests;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HashedInput {
    path: PathBuf,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inputs {
    schema: String,
    run_id: String,
    purpose: String,
    exact_selector: String,
    provenance: Provenance,
    artifact_root: PathBuf,
    binary_receipt: HashedInput,
    compiler_receipt: HashedInput,
}
/// Private fields prevent callers from substituting arbitrary provenance.
pub(in crate::app::tests::runtime::write_wait_qualification) struct BoundFault {
    provenance: Provenance,
    artifact_root: PathBuf,
    pub(super) captured: Vec<(&'static str, Vec<u8>)>,
    pub(super) actual: Value,
}
impl BoundFault {
    pub(in crate::app::tests::runtime::write_wait_qualification) fn provenance(
        &self,
    ) -> Provenance {
        self.provenance.clone()
    }
    pub(super) fn artifact_root(&self) -> &Path {
        &self.artifact_root
    }
}
pub(super) fn load(purpose: &str, exact_selector: &str) -> Result<BoundFault, String> {
    if cfg!(debug_assertions) {
        return Err("physical controls require actual opt3 release".into());
    }
    let mut captured = Vec::new();
    let manifest = std::env::var_os("BUILD_QUALIFICATION_MANIFEST")
        .ok_or("explicit fault manifest required")?;
    let quiet = std::env::var_os("BUILD_QUALIFICATION_QUIET_RELEASE")
        .ok_or("explicit fault quiet receipt required")?;
    let input: Inputs = read_input(
        Path::new(&manifest),
        "fault-manifest.json",
        None,
        &mut captured,
    )?;
    fields(&input, purpose, exact_selector)?;
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
        "fault-quiet-release.json",
        Some(&input.provenance.fresh_quiet_release_sha256),
        &mut captured,
    )?;
    validate_build(&input.provenance, &binary, &compiler)?;
    let current = process::actual()?;
    quiet_binding(&input, &quiet, &current, unix_us()?)?;
    actual_binding(&input, &current)?;
    Ok(BoundFault {
        provenance: input.provenance,
        artifact_root: input.artifact_root,
        captured,
        actual: json!({"process":current,"exact_selector":exact_selector,"purpose":purpose,"qualification":false}),
    })
}
fn fields(input: &Inputs, purpose: &str, exact_selector: &str) -> Result<(), String> {
    if input.schema != "build.qualification.fault-inputs.v1"
        || input.run_id.is_empty()
        || input.purpose != purpose
        || input.exact_selector != exact_selector
    {
        return Err("fault manifest purpose or exact selector mismatch".into());
    }
    for (value, length) in [
        (&input.provenance.source_revision, 40),
        (&input.provenance.source_tree, 40),
        (&input.provenance.binary_sha256, 64),
        (&input.provenance.fresh_quiet_release_sha256, 64),
        (&input.provenance.service_hook_contract_sha256, 64),
        (&input.binary_receipt.sha256, 64),
        (&input.compiler_receipt.sha256, 64),
    ] {
        if value.len() != length || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("exact source/build/quiet hashes required".into());
        }
    }
    if input.provenance.profile != "release-opt3-debug-assertions-off"
        || input.provenance.nice != 10
        || !input.artifact_root.is_absolute()
        || input.artifact_root.exists()
        || input.provenance.fixture_seed.is_empty()
    {
        return Err("NI10 release, fresh absolute artifact root, and fixed seed required".into());
    }
    Ok(())
}
fn actual_binding(input: &Inputs, actual: &process::Actual) -> Result<(), String> {
    let p = &input.provenance;
    if binary_hash(&std::env::current_exe().map_err(|error| error.to_string())?)? != p.binary_sha256
        || actual.nice != 10
        || actual.affinity != p.cpu_affinity
    {
        return Err("actual binary/NI/affinity differs from frozen build manifest".into());
    }
    validate_current_source(p)?;
    Ok(())
}
fn quiet_binding(
    input: &Inputs,
    quiet: &Value,
    actual: &process::Actual,
    now: u64,
) -> Result<(), String> {
    if quiet["schema"] != "build.qualification.fault-quiet.v1"
        || quiet["run_id"] != input.run_id
        || quiet["purpose"] != input.purpose
        || quiet["exact_selector"] != input.exact_selector
        || quiet["artifact_root"] != json!(input.artifact_root)
        || quiet["source_revision"] != input.provenance.source_revision
        || quiet["source_tree"] != input.provenance.source_tree
        || quiet["binary_sha256"] != input.provenance.binary_sha256
    {
        return Err("quiet receipt belongs to another source/run/purpose/selector".into());
    }
    if quiet["whole_host_quiet_authorized"] != true
        || quiet["no_gates_or_builds_during_control"] != true
        || quiet["coordinator_authorization"]
            .as_str()
            .is_none_or(str::is_empty)
    {
        return Err("explicit coordinator fault quiet authorization missing".into());
    }
    let issued = quiet["issued_at_unix_us"]
        .as_u64()
        .ok_or("quiet issue time missing")?;
    let deadline = quiet["start_before_unix_us"]
        .as_u64()
        .ok_or("quiet deadline missing")?;
    if issued > now || now > deadline || deadline < issued {
        return Err("fault quiet release is not current".into());
    }
    let launcher: process::Identity =
        serde_json::from_value(quiet["launcher"].clone()).map_err(|error| error.to_string())?;
    if !actual.lineage.contains(&launcher) {
        return Err(
            "quiet release launcher PID/start identity is not actual owned ancestry".into(),
        );
    }
    Ok(())
}
fn unix_us() -> Result<u64, String> {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_micros(),
    )
    .map_err(|error| error.to_string())
}
