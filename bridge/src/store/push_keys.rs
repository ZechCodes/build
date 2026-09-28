//! The notification keys push content is sealed to (#200, schema 11).
//!
//! One row per push subscription: its `sid` (`b64u(SHA-256(endpoint))`), the
//! browser's notification public key, and when it was registered. Bounded at
//! [`MAX_PUSH_KEYS`]: registering past the cap evicts the key registered
//! longest ago, so a browser that re-subscribes forever cannot grow the table.
//!
//! A key here is a public key, but a secret one: sealed content cannot be
//! forged only while nobody but this bridge and the browser holds it. So it
//! never reaches a log — [`PushKey`]'s `Debug` leaves it out.

use super::{Store, StoreError};

/// At most this many keys are kept.
pub const MAX_PUSH_KEYS: usize = 32;

/// One registered notification key.
#[derive(Clone, PartialEq, Eq)]
pub struct PushKey {
    pub subscription_id: String,
    /// `b64u(uncompressed P-256 point)`, validated when it was registered.
    pub public_key: String,
}

impl std::fmt::Debug for PushKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushKey")
            .field("subscription_id", &self.subscription_id)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Register (or replace) the key for `subscription_id`, then evict past
    /// the cap, oldest `registered_at` (unix millis) first.
    pub fn upsert_push_key(
        &self,
        subscription_id: &str,
        public_key: &str,
        registered_at: i64,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute(
                "INSERT INTO push_keys (subscription_id, public_key, registered_at) \
                 VALUES (?1, ?2, ?3) \
                 ON CONFLICT(subscription_id) DO UPDATE SET \
                 public_key = excluded.public_key, registered_at = excluded.registered_at",
                rusqlite::params![subscription_id, public_key, registered_at],
            )?;
            tx.execute(
                "DELETE FROM push_keys WHERE subscription_id NOT IN \
                 (SELECT subscription_id FROM push_keys \
                  ORDER BY registered_at DESC, rowid DESC LIMIT ?1)",
                [MAX_PUSH_KEYS as i64],
            )?;
            Ok(())
        })
    }

    /// Forget one key. Forgetting a key that is not here is not an error.
    pub fn delete_push_key(&self, subscription_id: &str) -> Result<(), StoreError> {
        self.delete_push_keys(&[subscription_id.to_string()])
    }

    /// Forget every key named, in one transaction.
    pub fn delete_push_keys(&self, subscription_ids: &[String]) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            let mut statement = tx.prepare("DELETE FROM push_keys WHERE subscription_id = ?1")?;
            for subscription_id in subscription_ids {
                statement.execute([subscription_id])?;
            }
            Ok(())
        })
    }

    /// Every registered key, newest first.
    pub fn list_push_keys(&self) -> Result<Vec<PushKey>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT subscription_id, public_key FROM push_keys \
             ORDER BY registered_at DESC, rowid DESC",
        )?;
        let keys = statement
            .query_map([], |row| {
                Ok(PushKey {
                    subscription_id: row.get(0)?,
                    public_key: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(keys)
    }
}
