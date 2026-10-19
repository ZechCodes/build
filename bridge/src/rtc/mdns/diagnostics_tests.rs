#[tokio::test(start_paused = true)]
async fn diagnostic_round_reports_applied_timeout_retry_sends_and_deadline() {
    let interfaces = vec![interface("eth0", "192.168.1.10")];
    let transport = FakeTransport::new(HashMap::new());
    let (result, report) =
        resolve_observed_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, None).await;
    assert_eq!(result, Err(ResolveFailure::Unresolved));
    let summary = report.summary(&result);
    for expected in [
        "timeout_ms=5000",
        "elapsed_ms=5000",
        "stop=deadline",
        "datagrams=0",
        "responses=0",
        "queries=0",
        "retries=4",
        "sent=5",
        "failed_sends=0",
        "if1(index=1,joined=1,sent=5,failed=0",
    ] {
        assert!(summary.contains(expected), "missing {expected}: {summary}");
    }
}

#[tokio::test(start_paused = true)]
async fn diagnostic_round_accounts_for_failed_memberships_and_each_send_attempt() {
    let interfaces = vec![
        interface("eth0", "192.168.1.10"),
        interface("wlan0", "192.168.68.2"),
        interface("eth2", "192.168.2.10"),
    ];
    let mut transport = FakeTransport::new(HashMap::new());
    transport.failed_join = Some(interfaces[0].address);
    transport.failed_send = Some(interfaces[1].address);
    let (result, report) =
        resolve_observed_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, None).await;
    let summary = report.summary(&result);
    for expected in [
        "eligible=3",
        "joined=2",
        "failed_joins=1",
        "sent=5",
        "failed_sends=5",
        "if1(index=1,joined=0,sent=0,failed=0",
        "if2(index=2,joined=1,sent=0,failed=5",
        "if3(index=1,joined=1,sent=5,failed=0",
    ] {
        assert!(summary.contains(expected), "missing {expected}: {summary}");
    }
}

#[tokio::test(start_paused = true)]
async fn diagnostic_round_distinguishes_queries_and_reasons_responses_are_not_accepted() {
    let interfaces = vec![interface("eth0", "192.168.1.10")];
    let transport = FakeTransport::new(HashMap::new());
    let packets = [
        (query_bytes(NAME), Some(1)),
        (vec![0, 0, 0x80], Some(1)),
        (response("other.local", "192.168.1.50"), Some(1)),
        (response(NAME, "fd00::123"), Some(1)),
        (response(NAME, "192.168.1.50"), Some(99)),
        (response(NAME, "192.168.68.50"), Some(1)),
        (response(NAME, "8.8.8.8"), Some(1)),
    ];
    for (packet, arrival) in packets {
        transport
            .sender
            .send(Datagram {
                packet,
                source: "192.168.1.50:5353".parse().unwrap(),
                arrival,
            })
            .unwrap();
    }
    let (result, report) =
        resolve_observed_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, None).await;
    assert_eq!(result, Err(ResolveFailure::AnswerRejected));
    let summary = report.summary(&result);
    for expected in [
        "datagrams=7",
        "responses=6",
        "queries=1",
        "malformed=1",
        "wrong_name=1",
        "no_matching_a=1",
        "unqueried-interface=1",
        "off-subnet=1",
        "nonlocal-address=1",
        "unknown_arrival=1",
    ] {
        assert!(summary.contains(expected), "missing {expected}: {summary}");
    }
}

#[tokio::test(start_paused = true)]
async fn diagnostic_round_counts_receive_errors_and_distinguishes_gone_socket() {
    let interfaces = vec![interface("eth0", "192.168.1.10")];
    let transport = FakeTransport::new(HashMap::new());
    transport.receive_errors.lock().unwrap().extend([
        io::Error::from(io::ErrorKind::ConnectionReset),
        socket_gone_error(),
    ]);
    let (result, report) =
        resolve_observed_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, None).await;
    assert_eq!(result, Err(ResolveFailure::SocketUnavailable));
    let summary = report.summary(&result);
    assert!(summary.contains("receive_errors=2"), "{summary}");
    assert!(summary.contains("stop=receive-failed"), "{summary}");
}

#[tokio::test(start_paused = true)]
async fn diagnostic_summary_is_bounded_and_redacts_names_addresses_and_wire_values() {
    let mut interfaces = (1..=100)
        .map(|ordinal| {
            interface(
                &format!("secret-{NAME}-192.168.1.10-{ordinal}\nforged"),
                &format!("192.168.1.{ordinal}"),
            )
        })
        .collect::<Vec<_>>();
    for (ordinal, interface) in interfaces.iter_mut().enumerate() {
        interface.index = ordinal as u32 + 1;
    }
    let transport = FakeTransport::new(HashMap::new());
    for _ in 0..10_000 {
        transport
            .sender
            .send(Datagram {
                packet: vec![0, 0, 0x80],
                source: "192.168.1.50:5353".parse().unwrap(),
                arrival: Some(999),
            })
            .unwrap();
    }
    let (result, report) = resolve_observed_with(
        NAME,
        &interfaces,
        &transport,
        Duration::from_millis(500),
        None,
    )
    .await;
    let summary = report.summary(&result);
    assert!(summary.contains("omitted_interfaces=92"), "{summary}");
    assert!(summary.contains("datagrams=10000"), "{summary}");
    assert!(summary.len() < 2500, "summary grew to {}", summary.len());
    for private in [NAME, "secret", "192.168", "5353", "forged", "\n"] {
        assert!(!summary.contains(private), "leaked {private}: {summary}");
    }
}

#[test]
fn diagnostic_packet_classification_keeps_response_failure_reasons_distinct() {
    assert_eq!(
        inspect_answer(NAME, &query_bytes(NAME)),
        PacketAnswer::Query
    );
    assert_eq!(inspect_answer(NAME, &[0, 0, 0x80]), PacketAnswer::Malformed);
    let mut unsupported = response(NAME, "192.168.1.50");
    unsupported[3] |= 3;
    assert_eq!(
        inspect_answer(NAME, &unsupported),
        PacketAnswer::Unsupported
    );
    assert_eq!(
        inspect_answer(NAME, &response("other.local", "192.168.1.50")),
        PacketAnswer::WrongName
    );
    assert_eq!(
        inspect_answer(NAME, &response(NAME, "fd00::123")),
        PacketAnswer::NoMatchingA
    );
    let mut malformed_a = response(NAME, "192.168.1.50");
    malformed_a.pop();
    assert_eq!(inspect_answer(NAME, &malformed_a), PacketAnswer::Malformed);
}

#[tokio::test(start_paused = true)]
async fn diagnostic_remembered_query_records_the_selected_interface_without_the_peer() {
    let interfaces = vec![
        interface("eth0", "192.168.1.10"),
        interface("wlan0", "192.168.68.2"),
    ];
    for failed in [false, true] {
        let mut transport = FakeTransport::new(HashMap::new());
        transport.failed_direct = failed;
        let (result, report) = resolve_observed_with(
            NAME,
            &interfaces,
            &transport,
            Duration::from_millis(500),
            Some("192.168.68.50".parse().unwrap()),
        )
        .await;
        let summary = report.summary(&result);
        assert!(
            summary.contains(if failed {
                "direct_sent=0 direct_failed=1"
            } else {
                "direct_sent=1 direct_failed=0"
            }),
            "{summary}"
        );
        assert!(
            summary.contains(
                "if1(index=1,joined=1,sent=1,failed=0,arrivals=0,direct_sent=0,direct_failed=0)"
            ),
            "{summary}"
        );
        assert!(
            summary.contains(if failed {
                "if2(index=2,joined=1,sent=1,failed=0,arrivals=0,direct_sent=0,direct_failed=1)"
            } else {
                "if2(index=2,joined=1,sent=1,failed=0,arrivals=0,direct_sent=1,direct_failed=0)"
            }),
            "{summary}"
        );
        assert!(!summary.contains("192.168.68.50"), "{summary}");
    }
}

#[test]
fn diagnostic_counters_saturate_instead_of_wrapping_under_flood() {
    let mut count = u64::MAX;
    diagnostics::add(&mut count, 1);
    assert_eq!(count, u64::MAX);
    let atomic = AtomicU64::new(u64::MAX);
    diagnostics::add_atomic(&atomic, 1);
    assert_eq!(atomic.load(Ordering::Relaxed), u64::MAX);
}

struct HealthTransport {
    inner: FakeTransport,
    health: Mutex<ReceiveHealth>,
}

#[async_trait]
impl Transport for HealthTransport {
    fn join(&self, interface: &LanInterface) -> io::Result<()> {
        self.inner.join(interface)
    }
    async fn send(&self, interface: &LanInterface, query: &[u8]) -> io::Result<()> {
        self.inner.send(interface, query).await
    }
    async fn receive(&self) -> io::Result<Datagram> {
        let received = self.inner.receive().await;
        let mut health = self.health.lock().unwrap();
        if received.is_err() {
            health.errors += 1;
        }
        // Model messages discarded by a shared receiver during this lookup.
        health.lagged = 23;
        received
    }
    fn receive_health(&self) -> Option<ReceiveHealth> {
        Some(*self.health.lock().unwrap())
    }
}

#[tokio::test(start_paused = true)]
async fn diagnostic_round_reports_shared_health_deltas_without_double_counting_errors() {
    let interfaces = vec![interface("eth0", "192.168.1.10")];
    let inner = FakeTransport::new(HashMap::new());
    inner.receive_errors.lock().unwrap().extend([
        io::Error::from(io::ErrorKind::ConnectionReset),
        socket_gone_error(),
    ]);
    let transport = HealthTransport {
        inner,
        health: Mutex::new(ReceiveHealth {
            errors: 50,
            lagged: 20,
        }),
    };
    let (result, report) =
        resolve_observed_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, None).await;
    let summary = report.summary(&result);
    for expected in [
        "receive_errors=2",
        "receive_lagged=3",
        "shared_receiver=true",
        "stop=receive-failed",
    ] {
        assert!(summary.contains(expected), "missing {expected}: {summary}");
    }
}

#[tokio::test(start_paused = true)]
async fn diagnostic_success_reports_accepted_answers_and_collection_grace() {
    let interfaces = vec![interface("eth0", "192.168.1.10")];
    let transport = FakeTransport::new(HashMap::from([(
        interfaces[0].address,
        response(NAME, "192.168.1.50"),
    )]));
    let (result, report) =
        resolve_observed_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, None).await;
    assert!(result.is_ok());
    let summary = report.summary(&result);
    for expected in [
        "outcome=resolved",
        "stop=answer-grace",
        "responses=1",
        "accepted=1",
        "arrivals=1",
    ] {
        assert!(summary.contains(expected), "missing {expected}: {summary}");
    }
}

#[tokio::test]
async fn diagnostic_shared_receiver_counts_messages_lost_to_fanout_lag() {
    // No membership or outbound traffic: only an ephemeral receiver and its
    // bounded broadcast queue are exercised.
    let shared = SharedSocket::new(UdpTransport::bound(0).unwrap());
    let subscriber = shared.subscribe();
    let before = subscriber.receive_health().unwrap();
    for _ in 0..140 {
        shared
            .replies
            .send(Ok(Datagram {
                packet: vec![0, 0, 0x80],
                source: "127.0.0.1:5353".parse().unwrap(),
                arrival: Some(1),
            }))
            .unwrap();
    }
    assert_eq!(subscriber.receive().await.unwrap().packet, [0, 0, 0x80]);
    let after = subscriber.receive_health().unwrap();
    assert_eq!(after.lagged - before.lagged, 12);
    assert_eq!(after.errors - before.errors, 0);
}

#[cfg(target_os = "linux")]
#[tokio::test(start_paused = true)]
async fn diagnostic_round_reports_missing_kernel_arrival_metadata() {
    let interfaces = vec![interface("eth0", "192.168.1.10")];
    let transport = FakeTransport::new(HashMap::new());
    transport
        .sender
        .send(Datagram {
            packet: response(NAME, "192.168.1.50"),
            source: "192.168.1.50:5353".parse().unwrap(),
            arrival: None,
        })
        .unwrap();
    let (result, report) =
        resolve_observed_with(NAME, &interfaces, &transport, RESOLVE_TIMEOUT, None).await;
    assert_eq!(result, Err(ResolveFailure::AnswerRejected));
    let summary = report.summary(&result);
    assert!(
        summary.contains("arrival-interface-unavailable=1"),
        "{summary}"
    );
    assert!(summary.contains("missing_arrival=1"), "{summary}");
}
