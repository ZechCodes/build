use crate::app::AppState;
use crate::orchestrator::ActiveRun;
use serde_json::Value;

/// Who asked for an implementation: what they hear once the run behind it is
/// open, and what is left written down when the git that would have opened it
/// failed.
///
/// It travels with the job, so one object answers both ways — a verb asking
/// for a run hears the run; a scheduler asking for one carries on to the stage
/// it was cutting the checkout for, and marks the Issue blocked if it cannot.
pub trait ImplementationCaller: Send {
    fn opened(self: Box<Self>, state: &mut AppState, run_id: &str) -> Result<Value, String>;
    /// The message the frame gets, after whatever the decide phase armed on the
    /// strength of this implementation has been settled.
    fn refused(self: Box<Self>, state: &mut AppState, error: String) -> String;

    /// Hand the implementation back: the run it opened, or the error that
    /// stopped it. This is how the two halves above are used — every one of
    /// them, so a fourth implementation mutation cannot pair them a fifth way.
    fn settle(
        self: Box<Self>,
        state: &mut AppState,
        opened: Result<&str, String>,
    ) -> Result<Value, String> {
        match opened {
            Ok(run_id) => self.opened(state, run_id),
            Err(error) => Err(self.refused(state, error)),
        }
    }
}

/// One door to an Issue's planning agent, waiting on the workspace it works
/// in: `plan.create`'s first dispatch, the first message to an inert Issue, a
/// batch of notes, one stage's comments, a freeform message.
///
/// Every door is gated before any disk work and settled here, after it — so
/// the plan event, the prompt and the queued turn are the only things a door
/// writes for itself. The disk is [`OpenPlanWorkspace`]'s, once.
///
/// [`OpenPlanWorkspace`]: crate::lifecycle::OpenPlanWorkspace
pub trait PlanSessionOpening: Send {
    /// The workspace is on disk: apply the event this door was gated on,
    /// render the prompt, queue the turn, and answer whoever asked.
    fn open(
        self: Box<Self>,
        state: &mut AppState,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String>;

    /// The workspace could not be written. A refusal is the caller's error for
    /// every door that asked for a session; a door that had already reached its
    /// destination (a routed capture) says so instead and overrides this.
    fn refused(self: Box<Self>, _state: &mut AppState, error: String) -> Result<Value, String> {
        Err(error)
    }
}

/// What a run whose checkout has just been let go of still owes the records.
/// `run.abandon` writes a verdict onto it; `run.delete` clears its card away.
///
/// The run arrives as an argument because the git phase carried it: it rides
/// through the removal so that no failure can strand it off the board.
pub trait DiscardSettlement: Send {
    /// Ask git whatever this settlement has to know before the checkout is let
    /// go of — the refs an answer depends on are readable only until then. Runs
    /// inside [`DiscardCheckout::perform`], with the app mutex released.
    ///
    /// Most settlements have nothing to ask, and a question nobody asked costs
    /// nothing: it is the abandon that judges a run's stages, and saying so
    /// here is what keeps a delete from paying for the verdict.
    fn judge_before_removal(&mut self) {}

    fn settle(self: Box<Self>, state: &mut AppState, active: ActiveRun) -> Result<Value, String>;
}
