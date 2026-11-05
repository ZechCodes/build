use super::SecretService;
use std::time::Duration;

pub struct SweeperHandle {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl SecretService {
    /// Captures only the service's own mutex and writer, never AppState.
    pub fn spawn_sweeper(&self, period: Duration) -> SweeperHandle {
        let service = self.clone();
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let mut ticks = tokio::time::interval(period.max(Duration::from_millis(1)));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    _ = ticks.tick() => {
                        let service = service.clone();
                        // Failures stay closed and retry next tick. No values,
                        // names or context are formatted into diagnostics.
                        let _ = tokio::task::spawn_blocking(move ||service.sweep()).await;
                    }
                }
            }
        });
        SweeperHandle {
            stop: Some(stop),
            task: Some(task),
        }
    }
}

impl SweeperHandle {
    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(mut task) = self.task.take() {
            if tokio::time::timeout(Duration::from_secs(1), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
    }
}

impl Drop for SweeperHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
