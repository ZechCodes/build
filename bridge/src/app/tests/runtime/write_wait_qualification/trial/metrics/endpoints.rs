//! Actual observed endpoint differences; completion remains diagnostic.
use super::*;
pub(super) fn append(row: &Row, samples: &mut MetricSamples) {
    for (label, from, to) in [
        ("offer_lag", None, Stage::Offered),
        ("due_to_complete", None, Stage::Completed),
        ("due_to_parsed_reply", None, Stage::ParsedFinalReply),
        (
            "credit_and_local_queue",
            Some(Stage::Offered),
            Stage::DispatchStarted,
        ),
        (
            "sent_to_parsed_reply",
            Some(Stage::WireSent),
            Stage::ParsedFinalReply,
        ),
        (
            "intake_to_encoded_reply",
            Some(Stage::ServerIntake),
            Stage::EncodedFinalReply,
        ),
        ("sent_to_complete", Some(Stage::WireSent), Stage::Completed),
    ] {
        if let Some(value) = measured(row, from, to) {
            push(samples, row, label, value);
        }
    }
}
fn measured(row: &Row, from: Option<Stage>, to: Stage) -> Option<u64> {
    let to = row.milestones_us.get(&to)?;
    let from = match from {
        Some(stage) => *row.milestones_us.get(&stage)?,
        None => row.slot.due_us,
    };
    Some(to.saturating_sub(from))
}
