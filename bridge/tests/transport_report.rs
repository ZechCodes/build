//! The transport reporter against a mock api: every event the ledger is handed
//! arrives as one signed request, in order, and a refusal drops that one
//! report without stopping the ones behind it.
use std::time::Duration;

use build_bridge::transport;
use build_bridge::transport_ledger::{TransportEvent, TransportLedger, TransportPath};
use build_bridge::transport_report::{report_challenge, TransportReport, TransportReporter};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

async fn received(server: &MockServer, at_least: usize) -> Vec<TransportReport> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let reports: Vec<TransportReport> = server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r: &&Request| r.url.path() == "/api/transport/report")
            .map(|r| serde_json::from_slice(&r.body).expect("a report body"))
            .collect();
        if reports.len() >= at_least || tokio::time::Instant::now() > deadline {
            return reports;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn every_event_arrives_signed_and_in_order() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/transport/report"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&api)
        .await;
    let identity = transport::generate_identity_keypair();
    let reporter = TransportReporter::start(&api.uri(), "dev-1", &identity.private_key_b64);

    reporter.record("sess-1", TransportEvent::Minted);
    reporter.record(
        "sess-1",
        TransportEvent::Carrying {
            path: TransportPath::Direct,
            detail: "host/host candidates".to_string(),
        },
    );
    reporter.record("sess-1", TransportEvent::FellBack);
    reporter.record("sess-1", TransportEvent::Ended);

    let reports = received(&api, 4).await;
    assert_eq!(
        reports
            .iter()
            .map(|r| format!("{}:{}", r.event, r.path))
            .collect::<Vec<_>>(),
        vec!["minted:-", "carrying:direct", "fell_back:-", "ended:-"]
    );
    for report in &reports {
        assert_eq!(report.device_id, "dev-1");
        assert_eq!(report.session_id, "sess-1");
        let challenge = report_challenge(
            &report.device_id,
            &report.session_id,
            &report.event,
            &report.path,
            report.timestamp,
        );
        assert!(
            transport::verify_message_b64(
                &identity.public_key_b64,
                challenge.as_bytes(),
                &report.signature_b64
            )
            .is_ok(),
            "the api can verify {report:?} against the device identity key"
        );
    }
}

#[tokio::test]
async fn a_refused_report_is_dropped_and_the_next_still_goes() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/transport/report"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/transport/report"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&api)
        .await;
    let identity = transport::generate_identity_keypair();
    let reporter = TransportReporter::start(&api.uri(), "dev-1", &identity.private_key_b64);

    reporter.record("sess-1", TransportEvent::Minted);
    reporter.record("sess-1", TransportEvent::Ended);

    let reports = received(&api, 2).await;
    assert_eq!(
        reports.iter().map(|r| r.event.as_str()).collect::<Vec<_>>(),
        vec!["minted", "ended"],
        "the refused first report is not retried and the second is not held back"
    );
}
