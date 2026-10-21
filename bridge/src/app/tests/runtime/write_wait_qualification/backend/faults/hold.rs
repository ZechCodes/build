//! Own a real holder. Cleanup sends release then joins; timeout is not success.
use super::*;
use std::sync::mpsc;
pub(super) enum Target {
    App(Arc<Mutex<AppState>>),
    Exclusion(PathBuf),
}
pub(super) struct HoldResult {
    pub held_for: Duration,
    pub safety_release: bool,
}
pub(super) struct Hold {
    release: Option<mpsc::Sender<()>>,
    worker: Option<std::thread::JoinHandle<Result<HoldResult, String>>>,
}
impl Hold {
    pub async fn start(target: Target) -> (Self, Result<Instant, String>) {
        let (release, receiver) = mpsc::channel();
        let (ready, arrived) = tokio::sync::oneshot::channel();
        let worker = std::thread::spawn(move || match target {
            Target::App(state) => {
                let _guard = state
                    .lock()
                    .map_err(|_| "physical AppState holder poisoned".to_owned())?;
                wait(ready, receiver)
            }
            Target::Exclusion(path) => {
                let Some((_guard, _)) =
                    crate::source_sync::SyncLock::acquire(&path, Duration::ZERO)
                else {
                    let _ = ready.send(Err("actual source exclusion already held".into()));
                    return Err("source holder did not acquire".into());
                };
                wait(ready, receiver)
            }
        });
        let result = match tokio::time::timeout(CLEANUP_BOUND, arrived).await {
            Ok(Ok(value)) => value,
            _ => Err("physical holder did not acknowledge actual acquisition".into()),
        };
        (
            Self {
                release: Some(release),
                worker: Some(worker),
            },
            result,
        )
    }
    pub async fn release_and_join(&mut self) -> Result<HoldResult, String> {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        let until = Instant::now() + CLEANUP_BOUND;
        while self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            if Instant::now() >= until {
                return Err("actual holder join unfinished; handle retained".into());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        self.worker
            .take()
            .ok_or("holder already joined")?
            .join()
            .map_err(|_| "physical holder panicked".to_owned())?
    }
}
fn wait(
    ready: tokio::sync::oneshot::Sender<Result<Instant, String>>,
    receiver: mpsc::Receiver<()>,
) -> Result<HoldResult, String> {
    let held = Instant::now();
    let _ = ready.send(Ok(held));
    let safety_release = receiver.recv_timeout(CLEANUP_BOUND).is_err();
    Ok(HoldResult {
        held_for: held.elapsed(),
        safety_release,
    })
}
impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}
