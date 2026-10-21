//! OFFSOURCE cleanup helper for the six focused TESTFIRST controls.
use super::fixture::SavedFixture;
use std::any::Any;

pub(super) async fn finish_caught<T>(
    fixture: SavedFixture,
    exercise: Result<T, Box<dyn Any + Send>>,
) -> T {
    let cleanup = fixture.finish().await;
    match exercise {
        Ok(value) => {
            assert!(cleanup.is_ok(), "saved-effect control cleanup: {cleanup:?}");
            value
        }
        Err(original) => {
            if let Err(error) = cleanup {
                eprintln!("saved-effect cleanup also failed: {error}");
            }
            std::panic::resume_unwind(original)
        }
    }
}
