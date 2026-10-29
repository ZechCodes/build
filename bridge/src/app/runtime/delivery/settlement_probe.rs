//! Test-only barrier around actual background delivery ticket settlement.
use crate::app::AppState;
use std::cell::Cell;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc, Mutex, OnceLock, Weak,
};
use std::time::Duration;
thread_local! { static BACKGROUND: Cell<bool> = const { Cell::new(false) }; }
struct Registration {
    id: u64,
    state: Weak<Mutex<AppState>>,
    operation: String,
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}
fn registrations() -> &'static Mutex<Vec<Registration>> {
    static REGISTRATIONS: OnceLock<Mutex<Vec<Registration>>> = OnceLock::new();
    REGISTRATIONS.get_or_init(|| Mutex::new(Vec::new()))
}
pub(in crate::app) struct Background;
pub(in crate::app) fn background() -> Background {
    BACKGROUND.with(|value| assert!(!value.replace(true)));
    Background
}
impl Drop for Background {
    fn drop(&mut self) {
        BACKGROUND.with(|value| value.set(false));
    }
}
pub(in crate::app) struct Gate {
    id: u64,
    entered: mpsc::Receiver<()>,
    release: Option<mpsc::Sender<()>>,
}
impl Gate {
    pub(in crate::app) fn install(state: &Arc<Mutex<AppState>>, operation: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let (entered, ready) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        registrations().lock().unwrap().push(Registration {
            id,
            state: Arc::downgrade(state),
            operation: operation.into(),
            entered,
            release: wait,
        });
        Self {
            id,
            entered: ready,
            release: Some(release),
        }
    }
    pub(in crate::app) fn entered(&self) {
        self.entered
            .recv_timeout(Duration::from_secs(5))
            .expect("actual background runner reached pre-settlement gate");
    }
    pub(in crate::app) fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        self.release();
        registrations()
            .lock()
            .unwrap()
            .retain(|entry| entry.id != self.id);
    }
}
pub(super) fn before_settlement(state: &Arc<Mutex<AppState>>, operation: Option<&str>) {
    if !BACKGROUND.with(Cell::get) {
        return;
    }
    let registration = {
        let mut entries = registrations().lock().unwrap();
        entries
            .iter()
            .position(|entry| {
                Weak::ptr_eq(&entry.state, &Arc::downgrade(state))
                    && Some(entry.operation.as_str()) == operation
            })
            .map(|index| entries.remove(index))
    };
    if let Some(registration) = registration {
        let _ = registration.entered.send(());
        let _ = registration.release.recv();
    }
}
