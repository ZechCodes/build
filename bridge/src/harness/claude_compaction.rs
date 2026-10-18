//! Claude Code 2.1.284 compaction translation shared by both carriers.
mod managed;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod children;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Translation {
    pub window: u64,
    pub percentage: Option<String>,
    pub trigger: u64,
}

pub fn translate(version: &str, tokens: u64, model_window: u64, max_output: u64) -> Result<Translation, &'static str> {
    let _ = (version, tokens, model_window, max_output);
    Err("not implemented")
}
