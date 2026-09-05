//! Opening an agent's harness, off the app mutex.
//!
//! A spawn asks the disk three questions before it can build an argv — does the
//! provider still hold the conversation this agent was recorded on, is there a
//! transcript in this checkout to continue, and which transcript will the child
//! about to start be writing — and then writes `.build/` into the checkout. All
//! four are filesystem walks over a tree the daemon does not own, and every one
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
use crate::orchestrator::{Orchestrator, ResumeIdProbe, SessionLocatorFactory, TranscriptProbe};
use crate::pty::HarnessSpec;

/// What a spawn asks the provider's transcript tree, injectable so no test
/// reads the developer's own.
#[derive(Clone)]
pub struct SessionProbes {
    transcript: TranscriptProbe,
    resume_id: ResumeIdProbe,
    locator: SessionLocatorFactory,
}

impl SessionProbes {
    pub fn new(
        transcript: TranscriptProbe,
        resume_id: ResumeIdProbe,
        locator: SessionLocatorFactory,
    ) -> SessionProbes {
        SessionProbes {
            transcript,
            resume_id,
            locator,
        }
    }

    /// What this spawn picks back up, decided in order, and the order is the
    /// rule.
    ///
    /// 1. A recorded name the provider still holds is resumed EXACTLY — the
    ///    conversation Build was speaking to, with no cwd guess beside it.
    ///    Verified first, so a dead name costs zero restarts instead of one,
    ///    and the claude uuid an agent carried onto codex is refused here
    ///    rather than choking the resume.
    /// 2. No name, but this agent's record shows history: the same agent
    ///    continuing its own conversation, which `--continue` guesses at as the
    ///    newest one in the checkout, still gated on the transcript probe.
    /// 3. Otherwise fresh, on every carrier. A brand-new agent record has no
    ///    conversation to pick up, and the checkout's old one belongs to
    ///    whoever had it — adoption included: Build cannot show a history it
    ///    never heard.
    fn pickup(
        &self,
        root: &Path,
        provider: AgentProvider,
        recorded: Option<String>,
        may_pick_up: bool,
    ) -> SessionPickup {
        match recorded {
            Some(named) if (self.resume_id)(root, provider, &named) => SessionPickup {
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
                continue_session: may_pick_up && (self.transcript)(root, provider),
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
pub struct AgentSpawnPlan {
    project: Orchestrator,
    root: PathBuf,
    agent_id: String,
    model_choice: ModelChoice,
    recorded_resume_id: Option<String>,
    may_pick_up_a_conversation: bool,
    probes: SessionProbes,
    session_token: String,
}

impl AgentSpawnPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project: Orchestrator,
        root: PathBuf,
        agent_id: String,
        model_choice: ModelChoice,
        recorded_resume_id: Option<String>,
        may_pick_up_a_conversation: bool,
        probes: SessionProbes,
        session_token: String,
    ) -> AgentSpawnPlan {
        AgentSpawnPlan {
            project,
            root,
            agent_id,
            model_choice,
            recorded_resume_id,
            may_pick_up_a_conversation,
            probes,
            session_token,
        }
    }

    /// Ask the transcript tree what this session continues, write `.build/`
    /// into the checkout, and build the argv.
    ///
    /// The scaffold is unconditional and idempotent: under
    /// `--strict-mcp-config` a missing config kills the harness before it reads
    /// a byte of the prompt, and the config is written per AGENT, so two agents
    /// sharing a checkout report as themselves.
    pub fn probe_and_scaffold(self) -> Result<ReadyToSpawn, String> {
        let provider = self.model_choice.provider;
        let pickup = self.probes.pickup(
            &self.root,
            provider,
            self.recorded_resume_id,
            self.may_pick_up_a_conversation,
        );
        let locator = self.probes.locator(&self.root, provider);
        self.project
            .scaffold_agent_worktree(&self.root, &self.agent_id)
            .map_err(|refusal| refusal.to_string())?;
        let spec = self.project.agent_harness_spec(
            &self.agent_id,
            &self.root,
            &self.model_choice,
            pickup.continue_session,
            pickup.resume_session_id,
            &self.session_token,
        );
        Ok(ReadyToSpawn {
            spec,
            size: self.project.pty_size(),
            locator,
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
    pub recorded_name_is_gone: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn probes(
        holds: impl Fn(&str) -> bool + Send + Sync + 'static,
        transcript: bool,
    ) -> SessionProbes {
        SessionProbes::new(
            Arc::new(move |_, _| transcript),
            Arc::new(move |_, _, id: &str| holds(id)),
            Arc::new(|_, _| None),
        )
    }

    #[test]
    fn a_recorded_name_the_provider_still_holds_is_resumed_exactly() {
        let pickup = probes(|id| id == "sess-live", true).pickup(
            Path::new("/tmp"),
            AgentProvider::default(),
            Some("sess-live".into()),
            true,
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
        let pickup = probes(|_| false, true).pickup(
            Path::new("/tmp"),
            AgentProvider::default(),
            Some("sess-gone".into()),
            true,
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
    fn an_agent_with_history_and_no_name_continues_its_checkouts_transcript() {
        let pickup =
            probes(|_| true, true).pickup(Path::new("/tmp"), AgentProvider::default(), None, true);
        assert!(pickup.continue_session);
        assert!(!pickup.recorded_name_is_gone);
    }

    #[test]
    fn a_brand_new_agent_opens_fresh_however_much_the_checkout_holds() {
        let pickup =
            probes(|_| true, true).pickup(Path::new("/tmp"), AgentProvider::default(), None, false);
        assert!(
            !pickup.continue_session,
            "the checkout's old conversation belongs to whoever had it"
        );
    }
}
