//! Durable expected-OID proof for repairing a partially published upstream.

use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TrackingExpectation {
    pub(crate) token: String,
    pub(crate) claim_fingerprint: String,
    pub(crate) expected_tracking_head: Option<String>,
    pub(crate) target_head: String,
}

impl Store {
    pub(crate) fn load_review_tracking_expectation(
        &self,
        task_id: &str,
        expected_version: u64,
        directory_id: &str,
    ) -> Result<Option<TrackingExpectation>, StoreError> {
        self.in_transaction(|tx| {
            require_bound_publication(tx, task_id, expected_version, directory_id)?;
            load_expectation(tx, &tracking_key(task_id, directory_id))
        })
    }

    /// Commit before mutating the receiver, while its exact Git lease is held.
    /// A later received snapshot can advance the binding without losing this
    /// original tracking CAS expectation. The version remains unchanged.
    pub(crate) fn prepare_review_tracking_expectation(
        &self,
        task_id: &str,
        expected_version: u64,
        directory_id: &str,
        claim_fingerprint: &str,
        expected_tracking_head: Option<&str>,
        target_head: &str,
    ) -> Result<TrackingExpectation, StoreError> {
        self.in_transaction(|tx| {
            require_bound_publication(tx, task_id, expected_version, directory_id)?;
            let key = tracking_key(task_id, directory_id);
            let existing = load_expectation(tx, &key)?;
            let expectation = next_expectation(existing, claim_fingerprint, expected_tracking_head, target_head)?;
            tx.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, serde_json::to_string(&expectation).expect("tracking expectation serializes")],
            )?;
            Ok(expectation)
        })
    }

    /// Release only the proof settled under the same Git/version fences.
    pub(crate) fn clear_review_tracking_expectation(
        &self,
        task_id: &str,
        expected_version: u64,
        directory_id: &str,
        token: &str,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            require_bound_publication(tx, task_id, expected_version, directory_id)?;
            let key = tracking_key(task_id, directory_id);
            if load_expectation(tx, &key)?.is_some_and(|saved| saved.token != token) {
                return Err(invalid("tracking recovery proof changed before cleanup"));
            }
            tx.execute("DELETE FROM meta WHERE key = ?1", [key])?;
            Ok(())
        })
    }

    /// Carry only a validated original tip, confirmed prior target or exact
    /// matching held receiver tip into the next proof. Replace the exact
    /// token atomically, preserving authorization through every crash window.
    pub(crate) fn replace_review_tracking_expectation(
        &self,
        task_id: &str,
        expected_version: u64,
        directory_id: &str,
        token: &str,
        tracking_heads: (Option<&str>, Option<&str>),
        target_head: &str,
    ) -> Result<TrackingExpectation, StoreError> {
        let (expected_tracking_head, current_received_head) = tracking_heads;
        self.in_transaction(|tx| {
            require_bound_publication(tx, task_id, expected_version, directory_id)?;
            let key = tracking_key(task_id, directory_id);
            let previous = load_expectation(tx, &key)?
                .ok_or_else(|| invalid("tracking recovery proof disappeared before replacement"))?;
            if previous.token != token {
                return Err(invalid(
                    "tracking recovery proof changed before replacement",
                ));
            }
            if expected_tracking_head != previous.expected_tracking_head.as_deref()
                && expected_tracking_head != Some(previous.target_head.as_str())
                && expected_tracking_head != Some(target_head)
                && expected_tracking_head != current_received_head
            {
                return Err(invalid("tracking replacement tip is not authorized"));
            }
            let replacement = TrackingExpectation {
                token: uuid::Uuid::new_v4().to_string(),
                claim_fingerprint: previous.claim_fingerprint,
                expected_tracking_head: expected_tracking_head.map(str::to_owned),
                target_head: target_head.into(),
            };
            validate_expectation(&replacement)?;
            tx.execute(
                "UPDATE meta SET value = ?2 WHERE key = ?1",
                params![
                    key,
                    serde_json::to_string(&replacement).expect("tracking expectation serializes")
                ],
            )?;
            Ok(replacement)
        })
    }
}

fn require_bound_publication(
    tx: &Transaction,
    task_id: &str,
    version: u64,
    directory_id: &str,
) -> Result<(), StoreError> {
    super::sync::require_publication(tx, task_id, version)?;
    let bound: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM review_branch_bindings WHERE task_id = ?1 AND directory_id = ?2)",
        params![task_id, directory_id], |row| row.get(0),
    )?;
    if !bound {
        return Err(invalid("tracking recovery directory is not bound"));
    }
    Ok(())
}

fn next_expectation(
    existing: Option<TrackingExpectation>,
    fingerprint: &str,
    expected: Option<&str>,
    target: &str,
) -> Result<TrackingExpectation, StoreError> {
    let expectation = match existing {
        Some(saved) => {
            if saved.claim_fingerprint != fingerprint {
                return Err(invalid("tracking recovery ownership claim changed"));
            }
            saved
        }
        None => TrackingExpectation {
            token: uuid::Uuid::new_v4().to_string(),
            claim_fingerprint: fingerprint.into(),
            expected_tracking_head: expected.map(str::to_owned),
            target_head: target.into(),
        },
    };
    if expectation.target_head != target {
        return Err(invalid(
            "tracking recovery target must settle before replacement",
        ));
    }
    validate_expectation(&expectation)?;
    Ok(expectation)
}

fn load_expectation(
    conn: &Connection,
    key: &str,
) -> Result<Option<TrackingExpectation>, StoreError> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?;
    raw.map(|raw| {
        let saved = decode(&raw, "meta", key)?;
        validate_expectation(&saved)?;
        Ok(saved)
    })
    .transpose()
}

fn validate_expectation(expectation: &TrackingExpectation) -> Result<(), StoreError> {
    let canonical_token = uuid::Uuid::parse_str(&expectation.token)
        .map_err(|_| invalid("invalid tracking recovery token"))?
        .to_string();
    if canonical_token != expectation.token || !canonical_hex(&expectation.claim_fingerprint, 64) {
        return Err(invalid("invalid tracking recovery identity"));
    }
    if !canonical_hex(&expectation.target_head, 40)
        || expectation
            .expected_tracking_head
            .as_deref()
            .is_some_and(|head| !canonical_hex(head, 40))
    {
        return Err(invalid("invalid tracking recovery commit"));
    }
    Ok(())
}

fn canonical_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn task_prefix(task_id: &str) -> String {
    format!("review-tracking:{:x}:", Sha256::digest(task_id.as_bytes()))
}

fn tracking_key(task_id: &str, directory_id: &str) -> String {
    format!(
        "{}{:x}",
        task_prefix(task_id),
        Sha256::digest(directory_id.as_bytes())
    )
}

pub(in crate::store) fn clear_tracking_expectations(
    tx: &Transaction,
    task_id: &str,
) -> Result<(), StoreError> {
    let prefix = task_prefix(task_id);
    tx.execute(
        "DELETE FROM meta WHERE substr(key, 1, length(?1)) = ?1",
        [prefix],
    )?;
    Ok(())
}
