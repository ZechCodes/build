use super::*;
use crate::timing::write_wait::mutex::assert_blocked_acquisition;

#[test]
fn change_note_records_the_actual_subscription_mutex_wait() {
    let bus = ChangeBus::new(Duration::ZERO);
    assert_blocked_acquisition(&bus.subscriptions, "change-note", || {
        bus.note_kind("owner", Kind::State);
    });
}

#[test]
fn change_watch_state_records_the_actual_polled_mutex_wait() {
    let bus = ChangeBus::new(Duration::ZERO);
    assert_blocked_acquisition(&bus.polled, "change-polled", || {
        bus.mark_polled("owner");
    });
    assert!(bus.polled.lock().unwrap().contains("owner"));
}
