//! The actual coordinator reducer with owned pending execution tasks.
//! These are lifecycle/accounting controls, never fabricated native proofs.
use super::*;

#[tokio::test]
async fn coordinator_decoded_event_returns_credits_without_completing_owned_execution() {
    for caller in [0, 4] {
        let slot = slot(caller, "held-native-proof-tail");
        let observe = Observation::for_native_pump_control(&[slot.clone()]);
        observe
            .stage(&slot.original_id, Stage::DispatchStarted, Instant::now())
            .unwrap();
        let mut active = [0; 3 * CALLERS];
        let mut sockets = [0; CALLERS];
        active[caller] = WRITE_CREDITS;
        let socket = (caller >= 4).then_some(caller);
        if socket.is_some() {
            sockets[caller] = MCP_SOCKET_CREDITS;
        }
        let (finish, pending) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move { pending.await.unwrap() });
        let mut running = Running::from([(
            slot.original_id.clone(),
            RunningExecution {
                credits: credits::CallerCredits::acquired(caller, socket),
                task,
            },
        )]);
        let parsed = Instant::now();
        let event = ExecutionEvent::DecodedReply {
            original_id: slot.original_id.clone(),
            actual_at: parsed,
        };
        let result = match event {
            ExecutionEvent::DecodedReply {
                original_id,
                actual_at,
            } => apply_decoded(
                &mut running,
                &mut active,
                &mut sockets,
                &observe,
                &original_id,
                actual_at,
            ),
            ExecutionEvent::Finished(_) => unreachable!(),
        };
        let pending_while_credit_returned =
            running.len() == 1 && !running[&slot.original_id].task.is_finished();
        let next_write_eligible = active[caller] < WRITE_CREDITS
            && socket.is_none_or(|at| sockets[at] < MCP_SOCKET_CREDITS);
        let before_finish = observe.ledger.lock().unwrap().rows[&slot.original_id].clone();
        finish.send(()).unwrap();
        take_finished(&mut running, &mut active, &mut sockets, &slot.original_id)
            .unwrap()
            .await
            .unwrap();
        result
            .expect("actual coordinator decoded event must release credits with its task pending");
        assert!(pending_while_credit_returned && next_write_eligible);
        assert_eq!(
            before_finish.milestones_us[&Stage::DecodedReplyObserved],
            observe.at_us(parsed)
        );
        assert!(
            before_finish.milestones_us[&Stage::CallerCreditReleased]
                >= before_finish.milestones_us[&Stage::DecodedReplyObserved]
        );
        assert!(
            !before_finish.milestones_us.contains_key(&Stage::Completed),
            "decoded reply must not satisfy predecessor eligibility"
        );
        assert!(before_finish.proof.is_none() && before_finish.waits.is_none());
        assert_eq!(
            before_finish.slot.due_us, slot.due_us,
            "original deadline never moves"
        );
        assert!(running.is_empty());
        assert_eq!(
            active[caller], 1,
            "proof-tail finish must not release caller credit twice"
        );
        assert_eq!(sockets[caller], usize::from(socket.is_some()));
    }
}

#[tokio::test]
async fn coordinator_rejects_unknown_and_duplicate_decode_without_losing_tail_ownership() {
    let slot = slot(4, "identity-owned-tail");
    let observe = Observation::for_native_pump_control(&[slot.clone()]);
    observe
        .stage(&slot.original_id, Stage::DispatchStarted, Instant::now())
        .unwrap();
    let mut active = [0; 3 * CALLERS];
    let mut sockets = [0; CALLERS];
    active[4] = 1;
    sockets[4] = 1;
    let (finish, pending) = tokio::sync::oneshot::channel::<()>();
    let mut running = Running::from([(
        slot.original_id.clone(),
        RunningExecution {
            credits: credits::CallerCredits::acquired(4, Some(4)),
            task: tokio::spawn(async move { pending.await.unwrap() }),
        },
    )]);
    let unknown = apply_decoded(
        &mut running,
        &mut active,
        &mut sockets,
        &observe,
        "different-original",
        Instant::now(),
    );
    let after_unknown = (active[4], sockets[4], running.len());
    let first = apply_decoded(
        &mut running,
        &mut active,
        &mut sockets,
        &observe,
        &slot.original_id,
        Instant::now(),
    );
    let duplicate = apply_decoded(
        &mut running,
        &mut active,
        &mut sockets,
        &observe,
        &slot.original_id,
        Instant::now(),
    );
    let after_duplicate = (active[4], sockets[4], running.len());
    finish.send(()).unwrap();
    take_finished(&mut running, &mut active, &mut sockets, &slot.original_id)
        .unwrap()
        .await
        .unwrap();
    assert!(unknown.unwrap_err().contains("no running original"));
    assert_eq!(after_unknown, (1, 1, 1));
    first.unwrap();
    assert!(duplicate.unwrap_err().contains("duplicate"));
    assert_eq!(after_duplicate, (0, 0, 1));
    assert_eq!((active[4], sockets[4], running.len()), (0, 0, 0));
}

fn slot(caller: usize, name: &str) -> Slot {
    Slot {
        original_id: format!("qual/coordinator-control/{name}"),
        sequence: 0,
        traffic: Traffic::Write,
        route: if caller < 4 {
            Route::SpaRpc
        } else {
            Route::AgentMcp
        },
        caller,
        owner: 0,
        due_us: 0,
        class: Some(WriteClass::State),
        finite_target_us: None,
        finite_ceiling_us: None,
        method: Some("accounting-only-owned-tail".into()),
    }
}
