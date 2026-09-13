//! The board family: `board.list`, `archive.list`, `archived.list`,
//! `project.*`, `capture.*`, `settings.*`, `models.list`.
//!
//! Not yet converted: every verb still answers from the legacy route in
//! `app/rpc.rs::route`. Register each here with `v1_method!` as it converts,
//! and drop it from that route and from `LEGACY_METHODS` in
//! `tests/api_contract.rs`.

use super::Handler;

/// The verbs this family serves. Fill it with [`v1_methods!`] and
/// [`v1_method!`], as `git.rs` does.
///
/// [`v1_methods!`]: crate::v1_methods
/// [`v1_method!`]: crate::v1_method
pub fn methods() -> &'static [(&'static str, Handler)] {
    &[]
}
