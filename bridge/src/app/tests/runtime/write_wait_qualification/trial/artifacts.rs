//! Off-source split of the frozen coordinator; no execution evidence.
use super::*;

/// Concrete artifact writer: refuses existing trial directories so a failed
/// original cannot be overwritten by a replacement under the same name.
pub struct JsonArtifacts {
    root: PathBuf,
    representatives: Mutex<BTreeSet<String>>,
}
impl JsonArtifacts {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir(&root).map_err(|error| error.to_string())?;
        Ok(Self {
            root,
            representatives: Mutex::new(BTreeSet::new()),
        })
    }
    fn trial_dir(&self, name: TrialName) -> Result<PathBuf, String> {
        let path = self.root.join(name.label());
        if !path.exists() {
            std::fs::create_dir(&path).map_err(|error| error.to_string())?;
        }
        Ok(path)
    }
    fn append(&self, path: PathBuf, value: &impl Serialize) -> Result<(), String> {
        use std::io::Write;
        let mut output = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .map_err(|error| error.to_string())?;
        serde_json::to_writer(&mut output, value).map_err(|error| error.to_string())?;
        output.write_all(b"\n").map_err(|error| error.to_string())?;
        output.flush().map_err(|error| error.to_string())
    }
    fn create_json(&self, path: PathBuf, value: &impl Serialize) -> Result<(), String> {
        let output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| error.to_string())?;
        serde_json::to_writer_pretty(output, value).map_err(|error| error.to_string())
    }
}
impl ArtifactSink for JsonArtifacts {
    fn accepted_records_archive(&self, name: TrialName, archive: &Value) -> Result<(), String> {
        self.create_json(
            self.trial_dir(name)?.join("accepted-records-final.json"),
            archive,
        )
    }

    fn transport_archive(&self, name: TrialName, archive: &Value) -> Result<(), String> {
        self.create_json(self.trial_dir(name)?.join("transport-final.json"), archive)
    }
    fn registry_archive(&self, name: TrialName, archive: &Value) -> Result<(), String> {
        self.create_json(
            self.trial_dir(name)?.join("native-registry-final.json"),
            archive,
        )
    }
    fn progress(
        &self,
        name: TrialName,
        at_us: u64,
        counts: &BTreeMap<Traffic, TrafficCounts>,
    ) -> Result<(), String> {
        self.append(
            self.trial_dir(name)?.join("progress.jsonl"),
            &serde_json::json!({"at_us":at_us,"counts":counts}),
        )
    }
    fn failure(&self, name: TrialName, snapshot: &FailureSnapshot) -> Result<(), String> {
        self.append(
            self.trial_dir(name)?.join("failure-snapshots.jsonl"),
            snapshot,
        )
    }
    fn finish(
        &self,
        report: &TrialReport,
        rows: &[Row],
        other_frames: &[FrameTrace],
    ) -> Result<(), String> {
        use std::io::Write;
        let dir = self.trial_dir(report.trial)?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("rows.jsonl"))
            .map_err(|error| error.to_string())?;
        for row in rows {
            serde_json::to_writer(&mut output, row).map_err(|error| error.to_string())?;
            output.write_all(b"\n").map_err(|error| error.to_string())?;
        }
        output.flush().map_err(|error| error.to_string())?;
        let mut extra = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("other-frames.jsonl"))
            .map_err(|error| error.to_string())?;
        for frame in other_frames {
            serde_json::to_writer(&mut extra, frame).map_err(|error| error.to_string())?;
            extra.write_all(b"\n").map_err(|error| error.to_string())?;
        }
        extra.flush().map_err(|error| error.to_string())?;
        self.create_json(dir.join("summary.json"), report)
    }
    fn raw_response(
        &self,
        trial: TrialName,
        original_id: &str,
        case: &str,
        failure: bool,
        decoded: &Value,
    ) -> Result<Option<String>, String> {
        let key = format!("{}/{case}", trial.label());
        let mut examples = self.representatives.lock().unwrap();
        if !failure && examples.contains(&key) {
            return Ok(None);
        }
        let kind = if failure {
            "raw-failures"
        } else {
            "case-examples"
        };
        let dir = self.trial_dir(trial)?.join(kind);
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let identity = if failure {
            format!("{case}/{original_id}")
        } else {
            case.to_string()
        };
        let filename = format!("{:x}.json", Sha256::digest(identity.as_bytes()));
        let path = dir.join(&filename);
        // Stream the full original decoded value; no clone or retained Value.
        let output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| error.to_string())?;
        #[derive(Serialize)]
        struct RawRecord<'a> {
            original_id: &'a str,
            case: &'a str,
            failure: bool,
            decoded_response: &'a Value,
        }
        serde_json::to_writer(
            output,
            &RawRecord {
                original_id,
                case,
                failure,
                decoded_response: decoded,
            },
        )
        .map_err(|error| error.to_string())?;
        if !failure {
            examples.insert(key);
        }
        Ok(Some(format!("{}/{kind}/{filename}", trial.label())))
    }
    fn pipeline_control(
        &self,
        report: &PipelineControlReport,
        rows: &[Row],
        other_frames: &[FrameTrace],
    ) -> Result<(), String> {
        // Control gets a separate artifact root from the six originals. Never
        // writes normal summary.json or a suite attempt/replacement record.
        let dir = self.trial_dir(report.trial_seed)?;
        let row_file = dir.join("control-rows.jsonl");
        let frame_file = dir.join("control-other-frames.jsonl");
        for path in [&row_file, &frame_file] {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(|error| error.to_string())?;
        }
        for row in rows {
            self.append(row_file.clone(), row)?;
        }
        for frame in other_frames {
            self.append(frame_file.clone(), frame)?;
        }
        self.create_json(dir.join("NoQualification.json"), report)
    }
    fn suite(&self, report: &SuiteReport) -> Result<(), String> {
        self.append(self.root.join("suite-history.jsonl"), report)
    }
}
