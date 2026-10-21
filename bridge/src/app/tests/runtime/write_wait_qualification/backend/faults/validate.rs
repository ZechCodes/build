//! Gates inspect actual persisted observations only after release and joins.
use super::*;
impl FaultRun {
    pub fn validation_errors(&self) -> Vec<String> {
        let mut errors = self.report.errors.clone();
        let report = &self.report;
        require(
            &mut errors,
            !report.qualification,
            "physical control cannot qualify normal trials",
        );
        require(
            &mut errors,
            report.cleanup_joined && self.unfinished.is_none(),
            "actual resource/watch joins must complete before accepting physical evidence",
        );
        require(
            &mut errors,
            report.physical_hold_us >= 6_000_000,
            "actual holder duration was shorter than six seconds",
        );
        let Some(active) = &report.observed_wait_before_release else {
            errors.push("actual active original snapshot missing".into());
            return errors;
        };
        let Some(completed) = &report.completed_original else {
            errors.push("actual completed original snapshot missing".into());
            return errors;
        };
        common_native(&mut errors, report, active, completed);
        if report.case == "post-ack-required" {
            post_ack(&mut errors, report, active);
        } else {
            pre_ack(&mut errors, report, active);
        }
        errors
    }
}
fn require(errors: &mut Vec<String>, condition: bool, reason: &str) {
    if !condition {
        errors.push(reason.into());
    }
}
fn common_native(
    errors: &mut Vec<String>,
    report: &FaultReport,
    active: &Value,
    completed: &Value,
) {
    require(
        errors,
        active["original_id"] == report.original_id
            && completed["original_id"] == report.original_id,
        "actual registry original identity changed",
    );
    require(
        errors,
        active["write_id"].is_string()
            && active["write_id"] == completed["write_id"]
            && active["intake_us"] == completed["intake_us"],
        "original registry intake/write ID changed during recovery",
    );
    require(
        errors,
        active["known_wait_us"]
            .as_u64()
            .is_some_and(|us| us > 5_000_000),
        "physical active known wait did not breach five seconds",
    );
    require(
        errors,
        completed["known_wait_us"]
            .as_u64()
            .is_some_and(|us| us > 5_000_000),
        "final original lost actual charged wait",
    );
    require(
        errors,
        completed["root_finished"] == true
            && completed["completed_us"].is_number()
            && completed["acknowledged_us"].is_number(),
        "actual original never reached terminal completion/ACK",
    );
    require(
        errors,
        empty(&completed["active"])
            && empty(&completed["outstanding"])
            && completed["cancelled"] == false
            && completed["abandoned"] == false,
        "original has unfinished/cancelled/abandoned actual work",
    );
    require(
        errors,
        report.registry_watch_reports.iter().any(|row| {
            row["original_id"] == report.original_id
                && row["known_wait_us"]
                    .as_u64()
                    .is_some_and(|us| us > 5_000_000)
        }),
        "independent registry watchdog did not report this original",
    );
    require(
        errors,
        report
            .actual_decoded_reply
            .as_ref()
            .is_some_and(|reply| reply["ok"] == true),
        "actual positive decoded final missing after release",
    );
}
fn empty(value: &Value) -> bool {
    value.as_array().is_some_and(Vec::is_empty)
}
fn pre_ack(errors: &mut Vec<String>, report: &FaultReport, active: &Value) {
    require(
        errors,
        active["root_finished"] == false && active["acknowledged_us"].is_null(),
        "preACK control crossed ACK before physical snapshot",
    );
    require(
        errors,
        report.client_watch_report["failed"] == true
            && report.client_watch_report["panicked"] == false,
        "actual caller watchdog did not independently breach cleanly",
    );
    let breach = report.client_watch_report["breaches"]
        .as_array()
        .is_some_and(|rows| {
            rows.iter().any(|row| {
                row["original_id"] == report.original_id
                    && row["elapsed_us"].as_u64().is_some_and(|us| us > 5_000_000)
            })
        });
    require(
        errors,
        breach,
        "independent client breach omitted original ID/deadline",
    );
    require(
        errors,
        actual_client_elapsed(report).is_some_and(|us| us > 5_000_000),
        "actual intended-to-decoded-final did not include physical stall",
    );
    require(
        errors,
        report.effect_checks["native_finite_effect_verified"] == true,
        "actual finite filesystem/Git effect oracle did not verify",
    );
}
fn post_ack(errors: &mut Vec<String>, report: &FaultReport, active: &Value) {
    require(
        errors,
        active["root_finished"] == true
            && active["acknowledged_us"].is_number()
            && !empty(&active["outstanding"]),
        "required child was not outstanding under the original after ACK",
    );
    require(
        errors,
        report.client_watch_report["failed"] == false
            && report.client_watch_report["panicked"] == false,
        "postACK child was incorrectly accepted with failed caller watchdog",
    );
    require(
        errors,
        actual_client_elapsed(report).is_some_and(|us| (0..=5_000_000).contains(&us)),
        "actual decoded final did not settle caller within its original five seconds",
    );
    for field in [
        "positive_final",
        "reserved_tab_matches",
        "actual_session_instance_present",
    ] {
        require(
            errors,
            report.effect_checks[field] == true,
            "actual postACK tab/session lineage did not verify",
        );
    }
}
fn actual_client_elapsed(report: &FaultReport) -> Option<i64> {
    let mut matching = report
        .client_rows
        .iter()
        .filter(|row| row["original_id"] == report.original_id);
    let row = matching.next()?;
    if matching.next().is_some() || row["cancelled"] != false {
        return None;
    }
    Some(
        row["decoded_final_us"]
            .as_i64()?
            .checked_sub(row["intended_us"].as_i64()?)?,
    )
}
