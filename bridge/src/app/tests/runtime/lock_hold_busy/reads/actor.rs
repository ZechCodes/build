//! Additional read streams start with the unchanged writer/status cohort.
use super::*;

pub(in super::super) struct Reader {
    pub owner: String,
    pub agent: String,
    pub fixture: Arc<Fixture>,
    pub samples: PhaseSamples,
    pub completed: Arc<AtomicUsize>,
    pub progress: Arc<Progress>,
}

pub(in super::super) async fn run(
    state: Arc<Mutex<AppState>>,
    plane: Arc<ControlPlane>,
    reader: Reader,
    start: Arc<tokio::sync::Barrier>,
) {
    let (client, daemon) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(handle_done_stream(state, daemon, plane));
    let mut client = tokio::io::BufReader::new(client);
    let ids = reader.fixture.ids(&reader.owner, &reader.agent);
    start.wait().await;
    let began = Instant::now();
    reader.progress.read_started(&reader.agent).await;
    for _ in 0..ROUNDS {
        for (_, action) in cases(&ids)
            .into_iter()
            .filter(|(_, action)| action.tool_name() != "get_task")
        {
            let frame = reader.fixture.frame(&reader.agent, &action);
            let requested_at = Instant::now();
            reader.progress.at_send(&action);
            let response = control_reply(&mut client, frame).await;
            reader.samples.record_client_total(
                &format!("mcp.control:{}", action.tool_name()),
                requested_at.elapsed(),
            );
            reader.fixture.assert_reply(&action, &response);
            reader.completed.fetch_add(1, Ordering::Relaxed);
        }
    }
    reader.progress.reader_finished(began.elapsed());
    drop(client);
    server.await.unwrap();
}
