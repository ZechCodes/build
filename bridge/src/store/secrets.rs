//! Dedicated secret metadata. Values and sealed uploads never enter SQLite.
//! Trusted service callers supply authorized canonical contexts. Message admission
//! must call `attach_in` inside its own main-store operation transaction.
mod attach;
mod audit;
mod release;
pub(crate) use release::{clock_observe_in, expire_in, scope_allows};
mod key_state;
mod records;
mod retirement;
mod schema;
mod writes;

pub use attach::attach_in;
pub(super) use retirement::{
    retire_agent_in, retire_conversation_in, retire_owner_in, retire_project_in,
};
pub(super) use schema::initialize;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod key_tests;

#[cfg(test)]
mod expiry_tests;
