//! Root-scoped provider substitute for full app delivery tests.
use super::{HarnessError, OpenedSession, SessionOpenRequest};
use crate::models::AgentProvider;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

type Factory = Arc<
    dyn Fn(AgentProvider, SessionOpenRequest) -> Result<OpenedSession, HarnessError> + Send + Sync,
>;

fn factories() -> &'static Mutex<HashMap<PathBuf, Factory>> {
    static FACTORIES: OnceLock<Mutex<HashMap<PathBuf, Factory>>> = OnceLock::new();
    FACTORIES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) struct Installed(PathBuf);

pub(crate) fn install(root: PathBuf, factory: Factory) -> Installed {
    assert!(
        factories()
            .lock()
            .unwrap()
            .insert(root.clone(), factory)
            .is_none(),
        "a test factory already owns this root"
    );
    Installed(root)
}

pub(super) fn for_root(root: &Path) -> Option<Factory> {
    factories()
        .lock()
        .unwrap()
        .iter()
        .find_map(|(scope, factory)| root.starts_with(scope).then(|| Arc::clone(factory)))
}

impl Drop for Installed {
    fn drop(&mut self) {
        factories().lock().unwrap().remove(&self.0);
    }
}
