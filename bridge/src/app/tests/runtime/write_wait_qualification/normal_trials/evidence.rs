//! OFFSOURCE retained inputs/errors. Never overwrite an original attempt.
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) struct EntryArtifacts {
    pub root: PathBuf,
}
impl EntryArtifacts {
    pub fn claim(root: &Path, captured: &[(&str, Vec<u8>)]) -> Result<Self, String> {
        std::fs::create_dir(root).map_err(|error| format!("{}: {error}", root.display()))?;
        let output = Self {
            root: root.to_owned(),
        };
        for (name, bytes) in captured {
            output.write_bytes(name, bytes)?;
        }
        Ok(output)
    }
    fn write_bytes(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.root.join(name))
            .map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())
    }
    pub fn write(&self, name: &str, value: &impl Serialize) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
        self.write_bytes(name, &bytes)
    }
    pub fn fail(&self, stage: &str, detail: &str) -> ! {
        let result = self.write("entry-failure.json", &serde_json::json!({
            "schema":"build.qualification.entry-failure.v1","stage":stage,"detail":detail,
            "qualified":false,"replacement_permitted":false,
            "cleanup":"not proven unless the actual returned report explicitly says closed",
            "original_outcomes":"traffic/suite-history.jsonl and returned-suite.json when available; missing outcomes are not successes"
        }));
        panic!(
            "qualification failed at {stage}: {detail}; artifacts={}; failure_artifact={result:?}",
            self.root.display()
        );
    }
}
pub(super) fn preflight_failure(captured: &[(&str, Vec<u8>)], stage: &str, detail: &str) -> ! {
    let root = tempfile::Builder::new()
        .prefix("build-qualification-preflight-failure-")
        .tempdir()
        .expect("retain preflight failure directory")
        .keep();
    let artifacts = EntryArtifacts { root };
    for (name, bytes) in captured {
        artifacts
            .write_bytes(name, bytes)
            .expect("retain exact captured preflight input");
    }
    artifacts.fail(stage, detail)
}
pub(super) fn panic_detail(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|text| (*text).into()))
        .unwrap_or_else(|| "non-string panic; original evidence retained".into())
}

#[cfg(test)]
mod tests;

/// Classify the actual caught suite return before the caller retains ownership.
pub(super) fn suite_outcome(
    outcome: std::thread::Result<Result<super::super::trial_complete::SuiteRun, String>>,
) -> Result<super::super::trial_complete::SuiteRun, (&'static str, String)> {
    match outcome {
        Ok(Ok(report)) => Ok(report),
        Ok(Err(detail)) => Err(("suite-returned-error", detail)),
        Err(payload) => Err(("suite-panicked", panic_detail(&*payload))),
    }
}
