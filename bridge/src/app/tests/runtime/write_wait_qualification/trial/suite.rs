//! Keep all six ORIGINAL outcomes; infrastructure/SQL review stop is explicit.
use super::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Attempt {
    Finished(TrialReport),
    StartupFailure { trial: TrialName, error: String },
    NotAttempted { trial: TrialName, reason: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SuiteReport {
    pub schema: String,
    pub attempts: Vec<Attempt>,
    pub qualified: bool,
}
pub struct SuiteRun {
    pub report: SuiteReport,
    pub unfinished: Option<UnfinishedCleanup>,
}
struct SuiteState {
    attempts: Vec<Attempt>,
    blocked: Option<String>,
    unfinished: Option<UnfinishedCleanup>,
}
pub async fn run_all_six(
    factory: &dyn FixtureFactory,
    sink: &dyn ArtifactSink,
) -> Result<SuiteRun, String> {
    let mut suite = SuiteState {
        attempts: Vec::new(),
        blocked: None,
        unfinished: None,
    };
    for name in ORIGINAL_TRIALS {
        suite.original(name, factory, sink).await?;
    }
    let qualified = suite.attempts.len() == 6
        && suite
            .attempts
            .iter()
            .all(|attempt| matches!(attempt, Attempt::Finished(report) if report.qualified));
    let report = SuiteReport {
        schema: "build.qualification.suite.v1-draft".into(),
        attempts: suite.attempts,
        qualified,
    };
    sink.suite(&report)?;
    Ok(SuiteRun {
        report,
        unfinished: suite.unfinished,
    })
}
impl SuiteState {
    async fn original(
        &mut self,
        name: TrialName,
        factory: &dyn FixtureFactory,
        sink: &dyn ArtifactSink,
    ) -> Result<(), String> {
        if let Some(reason) = &self.blocked {
            self.attempts.push(Attempt::NotAttempted {
                trial: name,
                reason: reason.clone(),
            });
            return Ok(());
        }
        let backend = match factory.create(name).await {
            Ok(backend) => backend,
            Err(error) => {
                self.startup_failed(name, error);
                return Ok(());
            }
        };
        match run_trial(name, backend, sink).await {
            Ok(run) => self.record_run(name, run),
            Err(error) => self.runtime_failed(name, error),
        }
        sink.suite(&SuiteReport {
            schema: "build.qualification.suite.v1-draft".into(),
            attempts: self.attempts.clone(),
            qualified: false,
        })
    }
    fn startup_failed(&mut self, name: TrialName, error: String) {
        self.attempts.push(Attempt::StartupFailure {
            trial: name,
            error: error.clone(),
        });
        self.blocked = Some(format!(
            "original {} startup invalid: {error}",
            name.label()
        ));
    }
    fn runtime_failed(&mut self, name: TrialName, error: String) {
        self.attempts.push(Attempt::StartupFailure {
            trial: name,
            error: error.clone(),
        });
        self.blocked = Some(format!("{} infrastructure invalid: {error}", name.label()));
    }
    fn record_run(&mut self, name: TrialName, run: TrialRun) {
        if run.report.coordinator_sql_review_required {
            self.blocked = Some("coordinator SQL review required".into());
        } else if run.unfinished.is_some() || run.report.invalid_infrastructure.is_some() {
            self.blocked = Some(format!(
                "{} infrastructure/cleanup invalid; original result retained",
                name.label()
            ));
        }
        self.unfinished = run.unfinished;
        self.attempts.push(Attempt::Finished(run.report));
        // An ordinary supported gate failure retains the original and allows
        // the remaining originals; never a retry or replacement invocation.
    }
}
