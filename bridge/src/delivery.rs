//! Opening an agent's harness, off the app mutex.
//!
//! A spawn asks the disk three questions before it can build an argv — does the
//! provider still hold the conversation this agent was recorded on, and which
//! transcript will the child about to start be writing — and then writes
//! `.build/` into the checkout. These are filesystem walks over a tree the
//! daemon does not own, and every one
//! of them used to run with the app-wide state lock in hand.
//!
//! [`AgentSpawnPlan`] is that work, lifted out whole. The lock-held half of a
//! spawn reads the plan's inputs out of the registry and hands the plan over;
//! the plan then holds nothing — it cannot even name `AppState` — and answers
//! with the argv, the grid and the watcher its caller needs to open a child.

use std::path::{Path, PathBuf};

use portable_pty::PtySize;

use crate::harness::SessionLocator;
use crate::models::{AgentProvider, ModelChoice};
use crate::orchestrator::{Orchestrator, ResumeIdProbe, SessionLocatorFactory};
use crate::pty::HarnessSpec;

/// What a spawn asks the provider's transcript tree, injectable so no test
/// reads the developer's own.
///
/// Built as a literal at every call site: three same-shaped `Arc` closures in a
/// positional constructor are three ways to build a spawn that probes the wrong
/// thing and still compiles.
#[derive(Clone)]
pub struct SessionProbes {
    pub resume_id: ResumeIdProbe,
    pub locator: SessionLocatorFactory,
}

impl SessionProbes {
    /// What this spawn picks back up, decided in order, and the order is the
    /// rule.
    ///
    /// 1. A recorded name the provider still holds is resumed EXACTLY — the
    ///    conversation Build was speaking to, with no cwd guess beside it.
    ///    Verified first, so a dead name costs zero restarts instead of one,
    ///    and the claude uuid an agent carried onto codex is refused here
    ///    rather than choking the resume.
    /// 2. Otherwise fresh, on every carrier. Missing exact lineage is never an
    ///    invitation to guess from the checkout's newest transcript; the cold
    ///    turn catches up from the canonical conversation history instead.
    ///    A brand-new agent record has no
    ///    conversation to pick up, and the checkout's old one belongs to
    ///    whoever had it — adoption included: Build cannot show a history it
    ///    never heard.
    ///
    /// The provider is asked where the child will stand, and nowhere else.
    /// Which directory a recorded name belongs to is not asked of the disk at
    /// all: the name only reaches here when the session that had it stood
    /// where this child will (`resumable_session_id`), so a project agent's
    /// conversation from before it moved into its project's base is never
    /// offered, whatever copies of it either directory holds.
    fn pickup(
        &self,
        cwd: &Path,
        provider: AgentProvider,
        recorded: Option<String>,
    ) -> SessionPickup {
        match recorded {
            Some(named) if (self.resume_id)(cwd, provider, &named) => SessionPickup {
                resume_session_id: Some(named),
                continue_session: false,
                recorded_name_is_gone: false,
            },
            Some(_gone) => SessionPickup {
                resume_session_id: None,
                continue_session: false,
                recorded_name_is_gone: true,
            },
            None => SessionPickup {
                resume_session_id: None,
                continue_session: false,
                recorded_name_is_gone: false,
            },
        }
    }

    /// The watcher that names the conversation the child is about to have.
    /// Built BEFORE the child exists, so the transcripts it snapshots as "not
    /// mine" cannot include the child's own.
    fn locator(&self, root: &Path, provider: AgentProvider) -> Option<Box<dyn SessionLocator>> {
        (self.locator)(root, provider)
    }
}

/// Which conversation a spawn opens on.
pub struct SessionPickup {
    resume_session_id: Option<String>,
    continue_session: bool,
    /// The recorded name the provider no longer holds. Forgetting it is an
    /// `AppState` write and has no business in a probe, so it travels to the
    /// apply phase as a fact.
    recorded_name_is_gone: bool,
}

/// One agent spawn's disk work, holding nothing.
///
/// Every field is owned: the orchestrator is cloned out of the project, the
/// probes are `Arc`s, and the checkout is a path. There is no borrow of the
/// registry here to keep the app mutex alive, which is the whole point.
///
/// Built as a literal by the lock-held half of a spawn, which is the only place
/// that can read these out of the registry — and the only place that could
/// hand a harness its agent id as its MCP token, which a positional constructor
/// of same-shaped strings would take without complaint. There is no invariant
/// here for a constructor to enforce: this module cannot name `AppState`.
pub struct AgentSpawnPlan {
    pub project: Orchestrator,
    /// Where Build keeps the agent: its `.build/` scaffold, its tab and its
    /// session lineage.
    pub root: PathBuf,
    /// Where the child process stands. The root, for every agent but a
    /// project agent, which stands in its project's base.
    pub cwd: PathBuf,
    pub agent_id: String,
    pub model_choice: ModelChoice,
    pub recorded_resume_id: Option<String>,
    pub probes: SessionProbes,
    pub session_token: String,
}

impl AgentSpawnPlan {
    /// Ask the transcript tree what this session continues, write `.build/`
    /// into the checkout, and build the argv.
    ///
    /// The scaffold is unconditional and idempotent: under
    /// `--strict-mcp-config` a missing config kills the harness before it reads
    /// a byte of the prompt, and the config is written per AGENT, so two agents
    /// sharing a checkout report as themselves.
    pub fn probe_and_scaffold(self) -> Result<ReadyToSpawn, String> {
        let provider = self.model_choice.provider;
        let pickup = self
            .probes
            .pickup(&self.cwd, provider, self.recorded_resume_id);
        // A fresh transcript is filed under the cwd the child starts in.
        let locator = self.probes.locator(&self.cwd, provider);
        let resume_session_id = pickup.resume_session_id.clone();
        let prepared = self
            .project
            .agent_launch()
            .prepare(
                &self.agent_id,
                crate::orchestrator::LaunchDirs {
                    scaffold: &self.root,
                    cwd: &self.cwd,
                },
                &self.model_choice,
                pickup.continue_session,
                pickup.resume_session_id,
                &self.session_token,
            )
            .map_err(|refusal| refusal.to_string())?;
        Ok(ReadyToSpawn {
            spec: prepared.spec,
            size: prepared.pty_size,
            locator,
            resume_session_id,
            recorded_name_is_gone: pickup.recorded_name_is_gone,
        })
    }
}

/// What the disk answered: everything a child needs, and the one thing the
/// apply phase has to write down.
pub struct ReadyToSpawn {
    pub spec: HarnessSpec,
    pub size: PtySize,
    pub locator: Option<Box<dyn SessionLocator>>,
    pub resume_session_id: Option<String>,
    pub recorded_name_is_gone: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn probes(holds: impl Fn(&str) -> bool + Send + Sync + 'static) -> SessionProbes {
        SessionProbes {
            resume_id: Arc::new(move |_, _, id: &str| holds(id)),
            locator: Arc::new(|_, _| None),
        }
    }

    #[test]
    fn a_recorded_name_the_provider_still_holds_is_resumed_exactly() {
        let pickup = probes(|id| id == "sess-live").pickup(
            Path::new("/tmp"),
            AgentProvider::default(),
            Some("sess-live".into()),
        );
        assert_eq!(pickup.resume_session_id.as_deref(), Some("sess-live"));
        assert!(
            !pickup.continue_session,
            "an exact resume never also guesses at the newest transcript"
        );
        assert!(!pickup.recorded_name_is_gone);
    }

    #[test]
    fn a_recorded_name_the_provider_has_lost_starts_fresh_and_says_so() {
        let pickup = probes(|_| false).pickup(
            Path::new("/tmp"),
            AgentProvider::default(),
            Some("sess-gone".into()),
        );
        assert_eq!(pickup.resume_session_id, None);
        assert!(
            !pickup.continue_session,
            "a dead name is not an invitation to pick up somebody else's conversation"
        );
        assert!(
            pickup.recorded_name_is_gone,
            "the apply phase has to forget it"
        );
    }

    #[test]
    fn history_without_exact_lineage_starts_fresh_for_canonical_catch_up() {
        let pickup = probes(|_| true).pickup(Path::new("/tmp"), AgentProvider::default(), None);
        assert!(
            !pickup.continue_session,
            "shared history must never activate a cwd-most-recent resume"
        );
        assert!(!pickup.recorded_name_is_gone);
    }

    #[test]
    fn a_brand_new_agent_opens_fresh_however_much_the_checkout_holds() {
        let pickup = probes(|_| true).pickup(Path::new("/tmp"), AgentProvider::default(), None);
        assert!(
            !pickup.continue_session,
            "the checkout's old conversation belongs to whoever had it"
        );
    }

    /// A project agent moved into its project's base does not resume the
    /// conversation it had in Build's scratch directory: claude filed that
    /// under the scratch directory's name, and only what the provider holds
    /// where the child stands is picked up. The name is forgotten, and the
    /// child starts fresh for the canonical catch-up.
    #[test]
    fn a_name_filed_only_where_the_child_no_longer_stands_starts_it_fresh() {
        let filed_under_scratch = SessionProbes {
            resume_id: Arc::new(move |dir: &Path, _, id: &str| {
                dir == Path::new("/state/project-scratch/build-0123") && id == "sess-old"
            }),
            locator: Arc::new(|_, _| None),
        };

        let moved = filed_under_scratch.pickup(
            Path::new("/code/build"),
            AgentProvider::default(),
            Some("sess-old".into()),
        );
        assert_eq!(moved.resume_session_id, None);
        assert!(!moved.continue_session);
        assert!(moved.recorded_name_is_gone);

        let stayed = filed_under_scratch.pickup(
            Path::new("/state/project-scratch/build-0123"),
            AgentProvider::default(),
            Some("sess-old".into()),
        );
        assert_eq!(stayed.resume_session_id.as_deref(), Some("sess-old"));
    }
}
