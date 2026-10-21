//! Missing quiet-detach boundary, declared for native TESTFIRST controls.
use super::*;

mod tests;

impl RpcHarness {
    pub(in crate::app::tests::runtime::write_wait_qualification) fn detach_client_watch(
        &self,
    ) -> Result<Option<Arc<FinalReconciliation>>, Vec<String>> {
        Err(vec![
            "quiet trial watch detachment is not implemented".into(),
        ])
    }
}
