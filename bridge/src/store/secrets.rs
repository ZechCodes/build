//! Dedicated secret metadata. Values and sealed uploads never enter SQLite.
//! Trusted service callers supply authorized canonical contexts. Message admission
//! must call `attach_in` inside its own main-store operation transaction.
mod attach;
mod post;
pub use post::attach_post_in;
mod audit;
mod audit_metadata;
pub(crate) use audit_metadata::append_metadata_in;
mod release;
pub(crate) use release::{clock_observe_in, expire_in, scope_allows};
mod key_state;
mod metadata;
mod records;
mod retirement;
mod revoke;
mod schema;
mod writes;

pub use attach::attach_in;
pub(super) use retirement::{
    retire_agent_in, retire_conversation_in, retire_owner_in, retire_project_in,
};
pub(super) use schema::initialize;

pub(super) fn store_error(error: crate::secrets::SecretError) -> super::StoreError {
    match error {
        crate::secrets::SecretError::Database(error) => super::StoreError::Db(error),
        crate::secrets::SecretError::Io(error) => super::StoreError::Io(error),
        error => super::StoreError::Io(std::io::Error::other(error.to_string())),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod key_tests;

#[cfg(test)]
mod expiry_tests;

#[cfg(test)]
mod post_tests;
