//! Accounting controls only: held tasks are owned proof tails, not fake writes.
use super::*;

#[tokio::test]
async fn decoded_write_releases_caller_and_coordinator_socket_while_tail_stays_owned() {
    for caller in [0, 4] {
        let socket = (caller >= 4).then_some(caller);
        let mut active = [0; 3 * CALLERS];
        let mut sockets = [0; CALLERS];
        active[caller] = WRITE_CREDITS;
        if socket.is_some() {
            sockets[caller] = MCP_SOCKET_CREDITS;
        }
        let mut credit = CallerCredits::acquired(caller, socket);
        let (finish, pending) = tokio::sync::oneshot::channel::<()>();
        let owned_tail = tokio::spawn(async move { pending.await.unwrap() });
        let result = credit.decoded(&mut active, &mut sockets);
        let while_pending = (active[caller], sockets[caller], owned_tail.is_finished());
        // Always join the real owned task before the expected RED assertion.
        finish.send(()).unwrap();
        owned_tail.await.unwrap();
        credit.finished(&mut active, &mut sockets);
        result.expect("decoded final must free caller credit before its owned proof tail finishes");
        assert_eq!(
            while_pending.0, 1,
            "a third same-caller write can now dispatch"
        );
        assert_eq!(while_pending.1, usize::from(socket.is_some()));
        assert!(
            !while_pending.2,
            "credit release must not settle the owned tail"
        );
        assert_eq!(
            active[caller], 1,
            "tail join must not decrement a released credit"
        );
        assert_eq!(sockets[caller], usize::from(socket.is_some()));
    }
}

#[test]
fn decoded_mcp_read_releases_only_coordinator_socket_before_oracle_tail() {
    let caller = 4;
    let queue = CALLERS + caller;
    let mut active = [0; 3 * CALLERS];
    let mut sockets = [0; CALLERS];
    active[queue] = 1;
    sockets[caller] = 1;
    let mut credit = CallerCredits::acquired(queue, Some(caller));
    let result = credit.decoded(&mut active, &mut sockets);
    let after_decode = (active[queue], sockets[caller]);
    credit.finished(&mut active, &mut sockets);
    result.expect("decoded MCP read must free coordinator socket before its oracle tail");
    assert_eq!(
        after_decode,
        (1, 0),
        "read active-credit boundary is unchanged"
    );
    assert_eq!((active[queue], sockets[caller]), (0, 0));
}

#[test]
fn decoded_reply_is_exactly_once_and_finish_never_releases_twice() {
    let caller = 4;
    let mut active = [0; 3 * CALLERS];
    let mut sockets = [0; CALLERS];
    active[caller] = 2;
    sockets[caller] = 2;
    let mut credit = CallerCredits::acquired(caller, Some(caller));
    let first = credit.decoded(&mut active, &mut sockets);
    let duplicate = credit.decoded(&mut active, &mut sockets);
    let after_duplicate = (active[caller], sockets[caller]);
    credit.finished(&mut active, &mut sockets);
    first.expect("the exact original's first decoded final releases its credits");
    assert!(duplicate.unwrap_err().contains("duplicate decoded reply"));
    assert_eq!(after_duplicate, (1, 1));
    assert_eq!((active[caller], sockets[caller]), (1, 1));
}

#[test]
fn no_reply_failure_releases_held_credits_without_decoded_success() {
    for (queue, socket) in [(0, None), (4, Some(4)), (CALLERS + 4, Some(4))] {
        let mut active = [0; 3 * CALLERS];
        let mut sockets = [0; CALLERS];
        active[queue] = 1;
        if let Some(caller) = socket {
            sockets[caller] = 1;
        }
        let credit = CallerCredits::acquired(queue, socket);
        assert!(!credit.decoded, "transport failure is not a decoded reply");
        credit.finished(&mut active, &mut sockets);
        assert_eq!(active[queue], 0);
        assert!(sockets.iter().all(|count| *count == 0));
    }
}
