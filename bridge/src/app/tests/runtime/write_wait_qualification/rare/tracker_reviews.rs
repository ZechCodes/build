mod basic;
mod reviews;

use super::VerifiedStep;

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let mut steps = basic::execute().await;
    steps.extend(reviews::execute().await);
    steps
}
