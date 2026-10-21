//! Opt-in finite service controls for isolated qualification fixtures.
//!
//! Work still executes at its real service boundary. The fixture selects a
//! target by original request identity, and actual elapsed time is never
//! replaced by that target in the retained evidence.
use crate::timing::write_wait::{current_context, IntervalCategory, WaitGuard};
use arc_swap::ArcSwap;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ServiceKind {
    /// The fenced current-file read and atomic replacement inside AppState.
    /// Scope resolution, publication and final read remain in the RPC envelope.
    Filesystem,
    /// Real scoped Git work after source exclusion and scope validation.
    Git,
}

impl ServiceKind {
    pub(crate) fn ceiling(self) -> Duration {
        Duration::from_millis(match self {
            Self::Filesystem => 250,
            Self::Git => 500,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PlannedService {
    pub original_id: String,
    pub kind: ServiceKind,
    pub target: Duration,
}

#[derive(Clone, Debug)]
pub(crate) struct ServiceSample {
    pub original_id: String,
    pub kind: ServiceKind,
    pub target: Duration,
    /// Observed wall time, including target padding; never a configured value.
    pub actual: Duration,
    pub succeeded: bool,
    pub abandoned: bool,
}

impl ServiceSample {
    pub(crate) fn exceeded_ceiling(&self) -> bool {
        self.actual > self.kind.ceiling()
    }
}

pub(crate) type EntryObserver = Arc<dyn Fn(&str, ServiceKind) + Send + Sync>;

struct Control {
    plans: BTreeMap<String, PlannedService>,
    samples: Mutex<Vec<ServiceSample>>,
    observer: Option<EntryObserver>,
}

struct Registrations {
    // Registration changes happen only during fixture setup and cleanup.
    writer: Mutex<()>,
    roots: ArcSwap<BTreeMap<PathBuf, Arc<Control>>>,
}

fn registrations() -> &'static Registrations {
    static REGISTRATIONS: OnceLock<Registrations> = OnceLock::new();
    REGISTRATIONS.get_or_init(|| Registrations {
        writer: Mutex::new(()),
        roots: ArcSwap::from_pointee(BTreeMap::new()),
    })
}

/// Owns exact lexical roots until every request and producer has drained.
pub(crate) struct FixtureServices {
    roots: Vec<PathBuf>,
    control: Arc<Control>,
}

impl FixtureServices {
    pub(crate) fn install(
        roots: Vec<PathBuf>,
        plans: Vec<PlannedService>,
        observer: Option<EntryObserver>,
    ) -> Self {
        let control = Arc::new(Control {
            plans: validated_plans(plans),
            samples: Mutex::new(Vec::new()),
            observer,
        });
        assert!(
            roots.iter().all(|root| root.is_absolute()),
            "qualification roots must be absolute"
        );
        let unique: std::collections::BTreeSet<_> = roots.iter().collect();
        assert_eq!(unique.len(), roots.len(), "duplicate qualification root");
        let installed = registrations();
        let writer = installed.writer.lock().unwrap();
        let mut next = (**installed.roots.load()).clone();
        if let Some(root) = roots.iter().find(|root| next.contains_key(*root)) {
            let duplicate = root.display().to_string();
            drop(writer);
            panic!("qualification root already belongs to a live fixture: {duplicate}");
        }
        for root in &roots {
            next.insert(root.clone(), Arc::clone(&control));
        }
        installed.roots.store(Arc::new(next));
        Self { roots, control }
    }

    pub(crate) fn samples(&self) -> Vec<ServiceSample> {
        self.control.samples.lock().unwrap().clone()
    }
}

impl Drop for FixtureServices {
    fn drop(&mut self) {
        let installed = registrations();
        let _writer = installed.writer.lock().unwrap();
        let mut next = (**installed.roots.load()).clone();
        for root in &self.roots {
            if next
                .get(root)
                .is_some_and(|held| Arc::ptr_eq(held, &self.control))
            {
                next.remove(root);
            }
        }
        installed.roots.store(Arc::new(next));
    }
}

fn validated_plans(plans: Vec<PlannedService>) -> BTreeMap<String, PlannedService> {
    let mut indexed = BTreeMap::new();
    for plan in plans {
        assert!(
            !plan.original_id.is_empty(),
            "a planned service needs its original ID"
        );
        assert!(
            plan.target <= plan.kind.ceiling(),
            "service target exceeds its ceiling"
        );
        assert!(
            indexed.insert(plan.original_id.clone(), plan).is_none(),
            "an original write has more than one service plan"
        );
    }
    indexed
}

struct ServiceRun {
    control: Arc<Control>,
    plans: Vec<PlannedService>,
    started: Instant,
    interval: Option<WaitGuard>,
    completed: bool,
}

impl ServiceRun {
    fn begin(root: &Path, kind: ServiceKind) -> Option<Self> {
        // Exact-key lookup: no canonicalization, traversal, or filesystem read.
        let control = registrations().roots.load().get(root).cloned()?;
        let context = current_context().expect("qualified service lost its original write context");
        let origins = context.original_ids();
        let plans = selected_plans(&control, &origins, kind);
        let first = plans.first()?;
        let target = first.target;
        assert!(
            plans.iter().all(|plan| plan.target == target),
            "coalesced services need one target"
        );
        let started = Instant::now();
        let interval = Some(context.interval(IntervalCategory::OwnExternal));
        let service = Self {
            control,
            plans,
            started,
            interval,
            completed: false,
        };
        if let Some(observer) = &service.control.observer {
            for plan in &service.plans {
                observer(&plan.original_id, kind);
            }
        }
        Some(service)
    }

    fn complete(mut self, succeeded: bool) {
        let remaining = self.plans[0].target.saturating_sub(self.started.elapsed());
        if !remaining.is_zero() {
            std::thread::sleep(remaining);
        }
        self.completed = true;
        self.record(succeeded, false);
    }

    fn record(&mut self, succeeded: bool, abandoned: bool) {
        let actual = self.started.elapsed();
        // Collector time is not claimed as external service time.
        self.interval.take();
        self.control
            .samples
            .lock()
            .unwrap()
            .extend(self.plans.iter().map(|plan| ServiceSample {
                original_id: plan.original_id.clone(),
                kind: plan.kind,
                target: plan.target,
                actual,
                succeeded,
                abandoned,
            }));
    }
}

impl Drop for ServiceRun {
    fn drop(&mut self) {
        if !self.completed {
            self.completed = true;
            self.record(false, true);
        }
    }
}

fn selected_plans(control: &Control, origins: &[String], kind: ServiceKind) -> Vec<PlannedService> {
    let selected: Vec<_> = origins
        .iter()
        .filter_map(|id| control.plans.get(id).cloned())
        .collect();
    if !selected.is_empty() {
        assert_eq!(
            selected.len(),
            origins.len(),
            "coalesced service has unplanned origins"
        );
        assert!(
            selected.iter().all(|plan| plan.kind == kind),
            "service kind differs from original plan"
        );
    }
    selected
}

/// Executes the real operation and preserves its exact success/error result.
pub(crate) fn run<T, E>(
    root: &Path,
    kind: ServiceKind,
    work: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let service = ServiceRun::begin(root, kind);
    let result = work();
    if let Some(service) = service {
        service.complete(result.is_ok());
    }
    result
}
