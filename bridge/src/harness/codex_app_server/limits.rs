use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AppServerLimits {
    pub inbound_frame_bytes: usize,
    pub outbound_frame_bytes: usize,
    pub stderr_line_bytes: usize,
    pub stderr_total_bytes: usize,
    pub pending_requests: usize,
    pub queued_turns: usize,
    pub queued_turn_bytes: usize,
    pub open_items: usize,
    pub open_item_bytes: usize,
    pub reconciliation: Duration,
}

impl Default for AppServerLimits {
    fn default() -> Self {
        AppServerLimits {
            inbound_frame_bytes: 1024 * 1024,
            outbound_frame_bytes: 1024 * 1024,
            stderr_line_bytes: 16 * 1024,
            stderr_total_bytes: 32 * 1024,
            pending_requests: 64,
            queued_turns: 16,
            queued_turn_bytes: 256 * 1024,
            open_items: 256,
            open_item_bytes: 128 * 1024,
            reconciliation: Duration::from_secs(5),
        }
    }
}
