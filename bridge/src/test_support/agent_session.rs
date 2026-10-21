//! Fixture-scoped finite providers for actual admitted session-opening tests.
//! Factory selection belongs inside harness::open_session_with_harness,
//! immediately around harness.open_session(request), leaving common capability
//! validation, PolicySession wrapping and all app reservation/publication intact.

use crate::harness::{HarnessError, OpenedSession, SessionOpenRequest};
use crate::models::AgentProvider;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

pub(crate) type SessionFactory = Arc<
    dyn Fn(AgentProvider, SessionOpenRequest) -> Result<OpenedSession, HarnessError> + Send + Sync,
>;

struct Enrollment {
    id: u64,
    factory: Option<SessionFactory>,
}

type Enrollments = BTreeMap<PathBuf, Enrollment>;
static ENROLLMENTS: OnceLock<Mutex<Enrollments>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn enrollments() -> &'static Mutex<Enrollments> {
    ENROLLMENTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(crate) struct FixtureSessions {
    root: PathBuf,
    id: u64,
}

impl FixtureSessions {
    pub(crate) fn install(owned_root: PathBuf, factory: SessionFactory) -> Self {
        assert!(
            owned_root.is_absolute(),
            "fixture session root must be absolute"
        );
        let root = std::fs::canonicalize(&owned_root).expect("existing fixture session root");
        assert!(root.is_dir(), "fixture session root must be a directory");
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut entries = enrollments().lock().unwrap();
        let overlap = entries.iter().any(|(other, entry)| {
            entry.factory.is_some() && (root.starts_with(other) || other.starts_with(&root))
        });
        if overlap {
            // An invalid fixture must not poison the process-wide test map.
            drop(entries);
            panic!("live fixture session roots must not overlap");
        }
        entries.insert(
            root.clone(),
            Enrollment {
                id,
                factory: Some(factory),
            },
        );
        Self { root, id }
    }

    pub(crate) fn is_enrolled(&self) -> bool {
        enrollments()
            .lock()
            .unwrap()
            .get(&self.root)
            .is_some_and(|entry| entry.id == self.id && entry.factory.is_some())
    }
}

impl Drop for FixtureSessions {
    fn drop(&mut self) {
        let mut entries = enrollments().lock().unwrap();
        if let Some(entry) = entries
            .get_mut(&self.root)
            .filter(|entry| entry.id == self.id)
        {
            // Retain a small tombstone, not the factory/resources: an accepted
            // operation that arrives after owned shutdown must never fall
            // through to an installed provider. Explicit re-enrollment wins.
            entry.factory = None;
        }
    }
}

pub(crate) fn factory_for(root: &Path) -> Result<Option<SessionFactory>, HarnessError> {
    let selected = {
        let entries = enrollments().lock().unwrap();
        // A new explicit active owner can reuse a retired subtree; otherwise
        // a retired owner remains fail-closed for late work in its subtree.
        entries
            .iter()
            .filter(|(owned, _)| root.starts_with(owned))
            .max_by_key(|(owned, entry)| (entry.factory.is_some(), owned.components().count()))
            .map(|(owned, entry)| (owned.clone(), entry.factory.clone()))
    };
    let Some((owned, factory)) = selected else {
        return Ok(None);
    };
    let factory = factory
        .ok_or_else(|| HarnessError::Setup("fixture session owner already shut down".into()))?;
    let actual = std::fs::canonicalize(root)
        .map_err(|error| HarnessError::Setup(format!("fixture session root: {error}")))?;
    if !actual.starts_with(&owned) {
        return Err(HarnessError::Setup(
            "fixture session root escaped its owned directory".into(),
        ));
    }
    // No registry guard survives factory invocation: the caller executes it
    // synchronously in the inherited actual-write context.
    Ok(Some(factory))
}
