//! State-independent support for the scripted QA agent.

/// Write a file under `dir`, creating parent dirs — the QA agent's file writer.
pub(super) fn write_in_dir(dir: &std::path::Path, rel: &str, contents: &str) -> Result<(), String> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, contents).map_err(|e| e.to_string())
}
