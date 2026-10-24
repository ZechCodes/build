//! Offline report. Pass a disposable COPY of the tasks directory, never live state.
use build_bridge::store::{RetentionPolicy, Store};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args()
        .nth(1)
        .ok_or("usage: retention_report <copied-tasks-directory> [timestamp]")?;
    let now = std::env::args()
        .nth(2)
        .unwrap_or_else(build_bridge::store::now_rfc3339);
    // Opening initializes schema/readers. Only the supplied disposable copy is touched.
    let store = Store::new(directory)?;
    let report = store.retention_report(RetentionPolicy::default(), &now)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
