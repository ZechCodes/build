//! Retained evidence and actual bounded process metadata.
use super::*;

use super::super::qualification_metadata::{actual_affinity, binary_hash};

pub(super) fn actual_provenance() -> Result<Provenance, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let hash = binary_hash(&executable)?;
    Ok(Provenance {
        source_revision: UNPROVIDED.into(),
        source_tree: UNPROVIDED.into(),
        binary_sha256: hash,
        profile: format!(
            "actual_test_binary;debug_assertions={};cargo_profile={UNPROVIDED}",
            cfg!(debug_assertions)
        ),
        nice: i64::from(crate::priority::own_nice()),
        cpu_affinity: actual_affinity()?,
        fresh_quiet_release_sha256: UNPROVIDED.into(),
        fixture_counts: json!({"qualification":false,"actual_fixture_count_receipt":UNPROVIDED,
            "declared_control":{"seconds":2,"semantic":80,"exempt":200,"reads":32,"all":312}}),
        fixture_seed: "two-second-native-pipeline-control-NoQualification".into(),
        service_hook_contract_sha256: UNPROVIDED.into(),
    })
}

pub(super) fn load_rows(path: &Path, retained: &Path) -> Vec<Row> {
    let file = std::fs::File::open(path).unwrap_or_else(|error| {
        infrastructure_failure(retained, "row-artifact-open", &error.to_string())
    });
    std::io::BufReader::new(file)
        .lines()
        .map(|line| {
            let line = line.unwrap_or_else(|error| {
                infrastructure_failure(retained, "row-artifact-read", &error.to_string())
            });
            serde_json::from_str(&line).unwrap_or_else(|error| {
                infrastructure_failure(retained, "row-artifact-parse", &error.to_string())
            })
        })
        .collect()
}

pub(super) fn load_native(path: &Path, retained: &Path) -> serde_json::Value {
    let file = std::fs::File::open(path).unwrap_or_else(|error| {
        infrastructure_failure(retained, "native-archive-open", &error.to_string())
    });
    serde_json::from_reader(std::io::BufReader::new(file)).unwrap_or_else(|error| {
        infrastructure_failure(retained, "native-archive-parse", &error.to_string())
    })
}

pub(super) fn persist(root: &Path, name: &str, value: &impl Serialize) {
    let path = root.join(name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap_or_else(|error| {
            panic!(
                "artifact preservation failed at {}: {error}",
                path.display()
            )
        });
    serde_json::to_writer_pretty(&mut file, value).expect("write retained control evidence");
    file.write_all(b"\n")
        .expect("finish retained control evidence");
    file.sync_all().expect("sync retained control evidence");
}

pub(super) fn infrastructure_failure(root: &Path, stage: &str, detail: &str) -> ! {
    persist(
        root,
        "infrastructure-failure.json",
        &json!({"qualification":false,"status":"NoQualification",
        "stage":stage,"detail":detail,"cleanup":"not-proven-unless-actual-returned-report-says-closed",
        "action":"retain artifacts and owned resources; no normal qualification or retry"}),
    );
    panic!(
        "pipeline control infrastructure failure ({stage}): {detail}; artifacts={}",
        root.display()
    );
}

pub(super) fn panic_detail(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).into();
    }
    "non-string panic; actual process artifact retained".into()
}
