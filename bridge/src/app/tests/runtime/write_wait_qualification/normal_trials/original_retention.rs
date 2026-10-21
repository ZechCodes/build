use super::super::trial_complete::{
    run_all_six, FixtureFactory, JsonArtifacts, TrialBackend, TrialName, ORIGINAL_TRIALS,
};
use super::evidence::{suite_outcome, EntryArtifacts};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

struct StartupFailure {
    attempts: Mutex<Vec<TrialName>>,
}

impl FixtureFactory for StartupFailure {
    fn create(
        &self,
        name: TrialName,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<dyn TrialBackend>, String>> + Send + '_>> {
        self.attempts.lock().unwrap().push(name);
        Box::pin(async { Err("injected original startup failure".into()) })
    }
}

#[tokio::test]
async fn failed_original_preserves_all_six_outcomes_and_never_starts_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("originals");
    let artifacts = EntryArtifacts::claim(&root, &[]).unwrap();
    let sink = JsonArtifacts::new(root.join("traffic")).unwrap();
    let factory = StartupFailure {
        attempts: Mutex::new(Vec::new()),
    };
    let result = suite_outcome(Ok(run_all_six(&factory, &sink).await))
        .map_err(|failure| failure.1)
        .unwrap();
    artifacts
        .write("returned-suite.json", &result.report)
        .unwrap();
    assert!(!result.report.qualified);
    assert!(result.unfinished.is_none());
    assert_eq!(*factory.attempts.lock().unwrap(), vec![ORIGINAL_TRIALS[0]]);
    assert_eq!(
        ORIGINAL_TRIALS.map(TrialName::label),
        [
            "small_trial_1",
            "small_trial_2",
            "small_trial_3",
            "large_trial_1",
            "large_trial_2",
            "large_trial_3",
        ]
    );
    assert_eq!(super::super::TRIAL_SECONDS, 600);
    let returned: Value =
        serde_json::from_slice(&std::fs::read(root.join("returned-suite.json")).unwrap()).unwrap();
    let history = std::fs::read_to_string(root.join("traffic/suite-history.jsonl")).unwrap();
    let history: Vec<Value> = history
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(history, vec![returned.clone()]);
    assert_eq!(returned["attempts"].as_array().unwrap().len(), 6);
    let first = &returned["attempts"][0]["StartupFailure"];
    assert_eq!(
        first["trial"],
        serde_json::to_value(ORIGINAL_TRIALS[0]).unwrap()
    );
    assert_eq!(first["error"], "injected original startup failure");
    for (index, name) in ORIGINAL_TRIALS.iter().enumerate().skip(1) {
        let attempt = &returned["attempts"][index]["NotAttempted"];
        assert_eq!(attempt["trial"], serde_json::to_value(name).unwrap());
        assert_eq!(
            attempt["reason"],
            "original small_trial_1 startup invalid: injected original startup failure"
        );
    }
    assert!(EntryArtifacts::claim(&root, &[]).is_err());
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(root.join("returned-suite.json")).unwrap(),)
            .unwrap(),
        returned
    );
}
