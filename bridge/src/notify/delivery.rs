//! One notify, sealed and sent, off the app lock (#200).
//!
//! [`Notifier::deliver`] runs inside the task `spawn_notify` spawns: it reads
//! the registered keys from the store, seals the content to each, POSTs the
//! notify, and forgets the keys the api reports unknown. Every part of that is
//! best effort except the POST itself — a key that cannot be sealed to is
//! skipped, a store that cannot be read (or is slow to) seals to nobody, and
//! the notify goes out generic either way.

use std::collections::HashSet;
use std::time::Duration;

use super::content::PushContent;
use super::seal::{self, Binding};
use super::{unix_seconds, Notifier, SealedEntry};
use crate::store::push_keys::PushKey;
use crate::store::Store;

/// How long reading keys and sealing may take before the notify goes without
/// them: content must never delay the push itself for long.
const SEAL_BUDGET: Duration = Duration::from_secs(1);

/// One notify's worth of work, owned so it can move into a spawned task.
pub struct Delivery {
    pub entity_id: String,
    pub kind: &'static str,
    /// The words to seal; `None` sends the #191 generic notify.
    pub content: Option<PushContent>,
}

impl Notifier {
    /// Seal `delivery`'s content to every key in `keys`, send the notify, and
    /// forget the keys the api reports unknown. Fails only when the notify
    /// itself does.
    pub async fn deliver(&self, delivery: Delivery, keys: Option<Store>) -> Result<(), String> {
        let sealed = sealed_within_budget(keys.clone(), &delivery).await;
        let sealed_to: HashSet<String> = sealed
            .iter()
            .map(|entry| entry.subscription_id.clone())
            .collect();
        let reply = self
            .notify(&delivery.entity_id, delivery.kind, sealed)
            .await?;
        let unknown: Vec<String> = reply
            .unknown_subscriptions
            .into_iter()
            .filter(|sid| sealed_to.contains(sid))
            .collect();
        forget_keys(keys, unknown).await;
        Ok(())
    }
}

/// The sealed entries for `delivery`, or none when there is nothing to seal,
/// nobody to seal to, or the work overruns [`SEAL_BUDGET`]. Runs on the
/// blocking pool: the store read waits on the store's own mutex.
async fn sealed_within_budget(keys: Option<Store>, delivery: &Delivery) -> Vec<SealedEntry> {
    let (Some(store), Some(content)) = (keys, delivery.content.clone()) else {
        return Vec::new();
    };
    let entity_id = delivery.entity_id.clone();
    let kind = delivery.kind;
    let work = tokio::task::spawn_blocking(move || {
        let keys = store.list_push_keys().unwrap_or_else(|error| {
            eprintln!("push notify: cannot read notification keys: {error}");
            Vec::new()
        });
        seal_to_keys(&keys, &entity_id, kind, &content, unix_seconds())
    });
    match tokio::time::timeout(SEAL_BUDGET, work).await {
        Ok(Ok(sealed)) => sealed,
        Ok(Err(_)) => {
            eprintln!("push notify: sealing failed; sending generic");
            Vec::new()
        }
        Err(_) => {
            eprintln!("push notify: sealing overran its budget; sending generic");
            Vec::new()
        }
    }
}

/// One entry per distinct subscription that could be sealed to, at `iat`.
pub(crate) fn seal_to_keys(
    keys: &[PushKey],
    entity_id: &str,
    kind: &str,
    content: &PushContent,
    iat: i64,
) -> Vec<SealedEntry> {
    let mut seen = HashSet::new();
    keys.iter()
        .filter(|key| seen.insert(key.subscription_id.as_str()))
        .filter_map(|key| seal_to_key(key, entity_id, kind, content, iat))
        .collect()
}

/// The error names only the kind of failure: never the key, never the words.
fn seal_to_key(
    key: &PushKey,
    entity_id: &str,
    kind: &str,
    content: &PushContent,
    iat: i64,
) -> Option<SealedEntry> {
    let binding = Binding {
        subscription_id: &key.subscription_id,
        kind,
        entity_id,
    };
    match seal::seal(&key.public_key, binding, content, iat) {
        Ok(blob) => Some(SealedEntry {
            subscription_id: key.subscription_id.clone(),
            blob,
        }),
        Err(error) => {
            eprintln!("push notify: skipped a notification key: {error}");
            None
        }
    }
}

async fn forget_keys(keys: Option<Store>, unknown: Vec<String>) {
    let Some(store) = keys else {
        return;
    };
    if unknown.is_empty() {
        return;
    }
    let forgotten = tokio::task::spawn_blocking(move || store.delete_push_keys(&unknown)).await;
    if !matches!(forgotten, Ok(Ok(()))) {
        eprintln!("push notify: cannot forget the keys the api reported unknown");
    }
}

#[cfg(test)]
mod tests;
