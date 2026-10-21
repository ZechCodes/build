use super::*;
use crate::timing::write_wait::mutex::assert_blocked_acquisition;

#[test]
fn published_terminal_size_records_the_actual_screen_leaf_wait() {
    let screen = ScreenHandle::new("publication-terminal", 120, 40);
    let mut dimensions = None;
    assert_blocked_acquisition(&screen.screen, "terminal-size-publication", || {
        dimensions = Some(screen.size());
    });
    assert_eq!(dimensions, Some((120, 40)));
}
