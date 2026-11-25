//! The hosted endpoint occupies the same predecessor-CAS slot as personal HTTP.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::claim::Assignment;
use super::connector::{self, ChildStatus, PrivateRuntime, RunningConnector};
use super::{DeploymentMode, Services};
use crate::backoff::Backoff;
use crate::http::auth::HttpAuthVerifier;
use crate::http::discovery::{CanonicalOrigin, DeviceProofSigner, EndpointRegistrar};
use crate::http::endpoint_state::{EndpointPolicy, EndpointRecord};
use crate::http::policy::HttpPolicy;
use crate::http::runtime::{self, HttpRuntime};
use crate::http::supervisor::{
    acknowledge_record, endpoint_signer, prepare_listener, prepare_or_shutdown,
    prepare_record_enabled, register_generation, wait_for_shutdown,
};
use crate::http::tls::LoadedTls;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(45);
const WITHDRAW_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct ActiveAssignment {
    assignment: Arc<Assignment>,
    shutdown: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl ActiveAssignment {
    pub fn start(services: &Services, assignment: Assignment) -> Self {
        let (shutdown, stop) = watch::channel(false);
        let assignment = Arc::new(assignment);
        let session = Session::new(services, assignment.clone());
        let task = tokio::spawn(session.run(stop));
        Self {
            assignment,
            shutdown,
            task: Some(task),
        }
    }

    pub fn matches(&self, assignment: &Assignment) -> bool {
        self.assignment.same_as(assignment)
    }

    pub async fn stop(mut self) {
        self.shutdown.send_replace(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for ActiveAssignment {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
    }
}

struct Session {
    services: Services,
    assignment: Arc<Assignment>,
    #[cfg(test)]
    fixture: Option<tests::SessionFixture>,
}

impl Session {
    fn new(services: &Services, assignment: Arc<Assignment>) -> Self {
        Self {
            services: services.clone(),
            assignment,
            #[cfg(test)]
            fixture: None,
        }
    }

    async fn run(self, stop: watch::Receiver<bool>) {
        let runtime = self.services.state_root.join("hosted-runtime");
        let result = async {
            let private = PrivateRuntime::open(runtime)?;
            if private.quarantined(self.assignment.generation)? {
                return self.withdraw_quarantined(stop).await;
            }
            self.supervise(&private, stop).await
        }
        .await;
        if let Err(error) = result {
            crate::logline::say(format!("hosted HTTPS unavailable: {error}"));
        }
        self.services.hosted_reachability.lost();
        self.services
            .transport
            .set_mode(DeploymentMode::HostedPending);
    }

    async fn supervise(
        &self,
        private: &PrivateRuntime,
        mut stop: watch::Receiver<bool>,
    ) -> Result<(), String> {
        let mut backoff = Backoff::new(Duration::from_secs(2), Duration::from_secs(60));
        while !*stop.borrow_and_update() {
            let outcome = self.attempt(private, stop.clone()).await;
            self.services.hosted_reachability.lost();
            self.services
                .transport
                .set_mode(DeploymentMode::HostedPending);
            private.clean_inputs()?;
            match outcome {
                Ok(Attempt::Stop) => break,
                Ok(Attempt::Fatal) => {
                    crate::logline::say(
                        "hosted HTTPS stopped after unsafe connector configuration or output",
                    );
                    return self.withdraw_quarantined(stop).await;
                }
                Ok(Attempt::Retry) => {
                    crate::logline::say("hosted HTTPS connector disconnected");
                }
                Err(error) => crate::logline::say(format!("hosted HTTPS attempt failed: {error}")),
            }
            let () =
                match prepare_or_shutdown(tokio::time::sleep(backoff.current()), &mut stop).await {
                    Some(()) => (),
                    None => break,
                };
            backoff.increase();
        }
        Ok(())
    }

    async fn attempt(
        &self,
        private: &PrivateRuntime,
        mut stop: watch::Receiver<bool>,
    ) -> Result<Attempt, String> {
        let install = self.install_connector();
        let Some(binary) = prepare_or_shutdown(install, &mut stop).await else {
            return Ok(Attempt::Stop);
        };
        let binary = binary.map_err(|_| "connector release verification or install failed")?;
        let Some(prepared) = prepare_or_shutdown(self.prepare_origin(), &mut stop).await else {
            return Ok(Attempt::Stop);
        };
        let prepared = prepared?;
        let address = prepared
            .listener
            .local_addr()
            .map_err(|_| "cannot inspect hosted listener")?;
        private.prepare(&self.assignment, address, &prepared.ca_pem)?;
        let child = RunningConnector::start(private.command(&binary, &self.assignment))?;
        let result = self.with_child(prepared, &child, stop).await;
        let terminal = child.stop().await;
        self.services.hosted_reachability.lost();
        self.services
            .transport
            .set_mode(DeploymentMode::HostedPending);
        if matches!(
            terminal,
            ChildStatus::ConfigChanged | ChildStatus::OutputFailed
        ) {
            // Persist the fence before doing network I/O. A daemon restart
            // cannot relaunch an assignment whose connector applied a push.
            private.quarantine(self.assignment.generation)?;
        }
        let _ = tokio::time::timeout(WITHDRAW_TIMEOUT, self.withdraw()).await;
        if matches!(
            terminal,
            ChildStatus::ConfigChanged | ChildStatus::OutputFailed
        ) {
            return Ok(Attempt::Fatal);
        }
        result
    }

    async fn install_connector(&self) -> Result<std::path::PathBuf, String> {
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            return Ok(fixture.executable.clone());
        }
        let executable = self.services.executable.clone();
        let install = self
            .services
            .state_root
            .join("connectors")
            .join(env!("CARGO_PKG_VERSION"));
        crate::update::release::connector::install(install, executable).await
    }

    fn registrar(&self, signer: Arc<DeviceProofSigner>) -> Result<EndpointRegistrar, String> {
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            return Ok(EndpointRegistrar::with_test_endpoint(
                signer,
                fixture.issuer.clone(),
            ));
        }
        EndpointRegistrar::new(signer).map_err(|error| error.to_string())
    }

    async fn prepare_origin(&self) -> Result<PreparedOrigin, String> {
        let origin = self.origin()?;
        let issuer =
            CanonicalOrigin::parse(&self.services.issuer).map_err(|error| error.to_string())?;
        let web =
            CanonicalOrigin::parse(&self.services.web_origin).map_err(|error| error.to_string())?;
        let allowed_origins = vec![web.as_str().to_owned()];
        let policy = HttpPolicy::new(origin.as_str(), &allowed_origins)?;
        let (tls, ca_pem) = connector::origin_tls(&self.assignment.hostname)?;
        let desired = EndpointPolicy {
            device_id: self.services.identity.device_id.clone(),
            issuer: issuer.as_str().to_owned(),
            origin: origin.as_str().to_owned(),
            allowed_origins,
            certificate_fingerprint: tls.certificate_fingerprint.clone(),
            assignment_generation: Some(self.assignment.generation),
        };
        let listener = prepare_listener(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), |_| async {
            self.register(origin, desired.clone(), false)
                .await
                .map(|_| ())
        })
        .await?;
        Ok(PreparedOrigin {
            listener,
            tls,
            policy,
            ca_pem,
            desired,
        })
    }

    async fn with_child(
        &self,
        prepared: PreparedOrigin,
        child: &RunningConnector,
        mut stop: watch::Receiver<bool>,
    ) -> Result<Attempt, String> {
        let mut status = child.status.clone();
        let connected = async {
            status
                .wait_for(|status| *status != ChildStatus::Starting)
                .await
                .map(|status| *status)
        };
        let connected = tokio::time::timeout(CONNECT_TIMEOUT, connected);
        let Some(connected) = prepare_or_shutdown(connected, &mut stop).await else {
            return Ok(Attempt::Stop);
        };
        if !matches!(connected, Ok(Ok(value)) if value == ChildStatus::Connected) {
            return Ok(Attempt::Retry);
        }
        self.enable_and_serve(prepared, child.status.clone(), stop)
            .await
    }

    async fn enable_and_serve(
        &self,
        prepared: PreparedOrigin,
        mut status: watch::Receiver<ChildStatus>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<Attempt, String> {
        let origin = self.origin()?;
        let enable = self.register(origin, prepared.desired, true);
        let signer = tokio::select! {
            biased;
            _ = disconnected(&mut status) => return Ok(Attempt::Retry),
            () = wait_for_shutdown(&mut stop) => return Ok(Attempt::Stop),
            result = enable => result?,
        };
        let verifier = HttpAuthVerifier::new(signer.clone()).map_err(|error| error.to_string())?;
        let runtime =
            runtime::mount(self.services.execution.clone(), verifier).with_discovery(signer);
        self.services.transport.hosted_endpoint_acknowledged();
        self.services.hosted_reachability.reached();
        self.services
            .transport
            .set_mode(DeploymentMode::HostedReady);
        serve(
            prepared.listener,
            prepared.tls,
            prepared.policy,
            runtime,
            status,
            stop,
        )
        .await
    }

    async fn register(
        &self,
        origin: CanonicalOrigin,
        policy: EndpointPolicy,
        enabled: bool,
    ) -> Result<Arc<DeviceProofSigner>, String> {
        let record = prepare_record_enabled(&self.services.state_path, Some(policy), enabled)
            .await?
            .ok_or("hosted endpoint intent was not persisted")?;
        let signer = self.signer(origin, &record)?;
        let registrar = self.registrar(signer.clone())?;
        register_generation(&registrar, &record, enabled).await;
        acknowledge_record(&self.services.state_path, record).await?;
        Ok(signer)
    }

    async fn withdraw_quarantined(&self, mut stop: watch::Receiver<bool>) -> Result<(), String> {
        // Retry the issuer acknowledgement even after the child has been
        // permanently stopped. A fresh claim for another assignment can cancel
        // this retirement before it reserves the new endpoint generation.
        prepare_or_shutdown(self.withdraw(), &mut stop)
            .await
            .unwrap_or(Ok(()))
    }

    async fn withdraw(&self) -> Result<(), String> {
        let state = EndpointRecord::current(&self.services.state_path)?;
        if !state.is_some_and(|record| {
            record.policy.assignment_generation == Some(self.assignment.generation)
        }) {
            return Ok(());
        }
        let record = prepare_record_enabled(&self.services.state_path, None, false)
            .await?
            .ok_or("hosted endpoint intent was not persisted")?;
        let signer = self.signer(self.origin()?, &record)?;
        let registrar = self.registrar(signer)?;
        register_generation(&registrar, &record, false).await;
        acknowledge_record(&self.services.state_path, record).await
    }

    fn origin(&self) -> Result<CanonicalOrigin, String> {
        CanonicalOrigin::parse(&self.assignment.origin()).map_err(|error| error.to_string())
    }

    fn signer(
        &self,
        origin: CanonicalOrigin,
        record: &EndpointRecord,
    ) -> Result<Arc<DeviceProofSigner>, String> {
        endpoint_signer(&self.services.identity, origin, record)
    }
}

struct PreparedOrigin {
    listener: TcpListener,
    tls: LoadedTls,
    policy: HttpPolicy,
    ca_pem: String,
    desired: EndpointPolicy,
}

enum Attempt {
    Stop,
    Retry,
    Fatal,
}

async fn disconnected(status: &mut watch::Receiver<ChildStatus>) {
    let _ = status
        .wait_for(|status| *status != ChildStatus::Connected)
        .await;
}

async fn serve(
    listener: TcpListener,
    tls: LoadedTls,
    policy: HttpPolicy,
    runtime: HttpRuntime,
    mut status: watch::Receiver<ChildStatus>,
    mut stop: watch::Receiver<bool>,
) -> Result<Attempt, String> {
    let (stop_listener, listener_stop) = watch::channel(false);
    let listener = runtime::serve_mounted(listener, tls.config, policy, runtime, listener_stop);
    tokio::pin!(listener);
    let result = tokio::select! {
        biased;
        () = disconnected(&mut status) => Attempt::Retry,
        () = wait_for_shutdown(&mut stop) => Attempt::Stop,
        result = &mut listener => return result.map(|()| Attempt::Retry).map_err(|_| "hosted origin listener failed".into()),
    };
    stop_listener.send_replace(true);
    listener
        .await
        .map_err(|_| "hosted origin listener failed")?;
    Ok(result)
}

#[cfg(test)]
mod tests;
