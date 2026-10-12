#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum DeferredDeliveryBoundary {
    Reserved,
    Materialized,
    Agent,
    Frozen,
    Posted,
    Settled,
}

#[cfg(test)]
impl crate::app::AppState {
    pub(in crate::app) fn stop_task_delivery_after_for_test(&self, _boundary: DeferredDeliveryBoundary) {}
    pub(in crate::app) fn pump_task_deliveries_for_test(&mut self) -> Result<bool, String> { Ok(false) }
}
