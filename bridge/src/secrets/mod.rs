//! Device-local encrypted secret storage. No RPC or capability exposes this foundation.
mod model;
pub mod vault;
pub use model::*;

pub type Result<T> = std::result::Result<T, SecretError>;

/// Errors deliberately carry no secret values or caller-controlled context.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("secret_io")]
    Io(#[from] std::io::Error),
    #[error("secret_database")]
    Database(#[from] rusqlite::Error),
    #[error("secret_invalid")]
    Invalid,
    #[error("secret_limit")]
    Limit,
    #[error("secret_unavailable")]
    Unavailable,
    #[error("secret_conflict")]
    Conflict,
    #[error("secret_clock_rollback")]
    ClockRollback,
}
mod lifecycle;
pub use lifecycle::*;
#[cfg(test)]
mod backup_tests;
