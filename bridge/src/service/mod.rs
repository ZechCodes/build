//! Service install for the bridge daemon — one shape, two platforms.
//!
//! `build-bridge install-service` is **gated on account pairing**: the device
//! must hold a local identity AND the api must confirm it is approved and owned
//! by a user account before any service file is written. The api is the ground
//! truth — a local `approved` flag can go stale (e.g. the backend was replaced
//! and its device registry reset), so it is never trusted on its own.
//!
//! What a "service" is varies by platform, so it is a trait, not a branch:
//! [`Launchd`] writes a LaunchAgent plist and bootstraps it into the user's gui
//! domain, [`Systemd`] writes a `--user` unit and enables it. [`manager_for`] is
//! the ONE place in the crate that looks at which platform this is; everything
//! above it — install, uninstall, `main.rs` — is platform-blind.
//!
//! Nothing here spawns a process: [`install`] and [`uninstall`] take the command
//! runner as a closure, so the whole orchestration is testable against a
//! recorder and a temp dir.

pub mod launchd;
pub mod systemd;

use std::io;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub use launchd::Launchd;
pub use systemd::Systemd;

use crate::identity::StoredIdentity;
use crate::pairing::{Lapse, PairingError, StatusResponse};

pub const SERVICE_LABEL: &str = "ing.getbuild.bridge";

/// Why the daemon may not be installed yet, with operator instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallGateError {
    /// No identity file — this machine has never started pairing.
    NotPaired,
    /// Registered, but no human has entered its pairing code in the web app yet.
    PendingApproval,
    /// Stored as approved, but the api no longer approves it: revoked, or a
    /// device the api does not know (#317). Pairing again is the only way on.
    NoLongerPaired(Lapse),
    /// An api not known to have approved this device said it is not approved;
    /// `pair` keeps the identity unless told `--retire` (#320). `approver` is
    /// the api the identity records, if it records one.
    ApprovedElsewhere {
        asked: String,
        approver: Option<String>,
    },
    /// The status call failed — the api could not say either way.
    Unreachable { detail: String },
    /// The identity file could not be read, or could not be moved aside.
    IdentityFile { detail: String },
    /// The api confirmed the device but it is approved without an owner —
    /// never expected from the real pairing flow; refuse rather than guess.
    ApprovedWithoutOwner,
}

impl std::fmt::Display for InstallGateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotPaired => write!(
                f,
                "this device is not paired to an account — run `build-bridge pair` and \
                 enter its pairing code in the web app (Settings → Devices → Add a device)"
            ),
            Self::PendingApproval => write!(
                f,
                "this device's pairing is waiting for approval — enter the pairing code \
                 `build-bridge pair` printed in the web app (Settings → Devices → Add a \
                 device), or run `build-bridge pair` again for a new code"
            ),
            Self::NoLongerPaired(lapse) => write!(
                f,
                "this machine's earlier pairing is no longer valid: {lapse} — run \
                 `build-bridge pair` to pair it again"
            ),
            Self::ApprovedElsewhere {
                asked,
                approver: Some(approver),
            } => write!(
                f,
                "asked {asked}, but {approver} approved this device; \
                 pair --retire sets it aside anyway"
            ),
            Self::ApprovedElsewhere {
                asked,
                approver: None,
            } => write!(
                f,
                "asked {asked}, but this identity does not record which api approved \
                 it; pair --retire sets it aside anyway"
            ),
            Self::Unreachable { detail } => write!(
                f,
                "could not confirm this device's pairing with the api ({detail}) — check \
                 the connection and try again"
            ),
            Self::IdentityFile { detail } => write!(
                f,
                "could not read or move this device's identity file ({detail}) — fix its \
                 permissions or contents, then try again"
            ),
            Self::ApprovedWithoutOwner => write!(
                f,
                "the api reports this device approved but owned by no account — re-pair \
                 with `build-bridge pair`"
            ),
        }
    }
}

/// A pairing call that failed, as the refusal it is: the identity file's own
/// trouble, or an api that did not answer.
impl From<PairingError> for InstallGateError {
    fn from(error: PairingError) -> Self {
        match error {
            PairingError::Identity(detail) => Self::IdentityFile { detail },
            PairingError::ApprovedElsewhere { asked, approver } => {
                Self::ApprovedElsewhere { asked, approver }
            }
            other => Self::Unreachable {
                detail: other.to_string(),
            },
        }
    }
}

/// The install gate. `stored` is the identity file's contents, if there is
/// one; `api_status` is the live answer from `/api/devices/{id}/status` (`Err`
/// carries why the call failed). A stored approval the api no longer honours
/// is a lapsed pairing, not one waiting for approval.
pub fn check_install_gate(
    stored: Option<&StoredIdentity>,
    api_status: Result<&StatusResponse, &str>,
) -> Result<String, InstallGateError> {
    let stored = stored.ok_or(InstallGateError::NotPaired)?;
    let status = api_status.map_err(|detail| InstallGateError::Unreachable {
        detail: detail.to_string(),
    })?;
    if status.approved {
        return status
            .owner_user_id
            .clone()
            .ok_or(InstallGateError::ApprovedWithoutOwner);
    }
    Err(status.lapse().filter(|_| stored.approved).map_or(
        InstallGateError::PendingApproval,
        InstallGateError::NoLongerPaired,
    ))
}

/// Everything the service unit needs baked into it, whatever its format.
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    pub binary_path: PathBuf,
    pub log_dir: PathBuf,
    /// BRIDGE_* environment carried into the daemon, already resolved.
    pub env: Vec<(String, String)>,
}

/// The installing user, as the platform's activation commands need them.
#[derive(Debug, Clone)]
pub struct ServiceContext {
    pub home: PathBuf,
    /// `id -u` — launchd addresses the user's gui domain by numeric uid.
    pub uid: String,
}

/// One command a manager wants run around the unit file. `tolerate_failure`
/// marks the ones whose failure is normal (booting out a service that was never
/// loaded); the rest must succeed or the install failed. `failure_note` marks a
/// tolerated command whose failure the operator should still hear about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommand {
    pub program: &'static str,
    pub args: Vec<String>,
    pub tolerate_failure: bool,
    /// What a failure leaves the operator with, said after the command.
    pub failure_note: Option<&'static str>,
}

impl ShellCommand {
    fn new(program: &'static str, args: &[&str], tolerate_failure: bool) -> Self {
        Self {
            program,
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            tolerate_failure,
            failure_note: None,
        }
    }

    /// A command whose failure fails the install.
    pub fn required(program: &'static str, args: &[&str]) -> Self {
        Self::new(program, args, false)
    }

    /// A command that is allowed to fail (nothing was loaded to unload).
    pub fn tolerated(program: &'static str, args: &[&str]) -> Self {
        Self::new(program, args, true)
    }

    /// A command worth running whose failure still leaves a working install;
    /// the failure is reported with `note` and the install carries on.
    pub fn best_effort(program: &'static str, args: &[&str], note: &'static str) -> Self {
        Self {
            failure_note: Some(note),
            ..Self::new(program, args, true)
        }
    }
}

/// An install or uninstall that did not complete.
#[derive(Debug)]
pub enum ServiceError {
    /// The unit file could not be written, created or removed.
    Io(io::Error),
    /// A required activation command reported failure.
    Activate {
        program: &'static str,
        args: Vec<String>,
    },
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Activate { program, args } => write!(f, "`{program} {}` failed", args.join(" ")),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<io::Error> for ServiceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// What `uninstall` found, so the caller can say so without re-stat'ing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uninstalled {
    Removed(PathBuf),
    NothingInstalled(PathBuf),
}

/// One platform's idea of "keep this daemon running". Everything above this
/// trait is platform-blind.
pub trait ServiceManager {
    /// What to call it in operator-facing output.
    fn name(&self) -> &'static str;
    /// Where the unit file lives for this user.
    fn unit_path(&self, home: &Path) -> PathBuf;
    /// The unit file's text.
    fn render_unit(&self, config: &ServiceConfig) -> String;
    /// Run in order, after the unit file is written.
    fn activate(&self, ctx: &ServiceContext, unit_path: &Path) -> Vec<ShellCommand>;
    /// Run in order, before the unit file is removed.
    fn deactivate(&self, ctx: &ServiceContext, unit_path: &Path) -> Vec<ShellCommand>;
    /// One sentence the operator still has to act on, if any.
    fn after_install_hint(&self) -> Option<&'static str>;
}

/// The crate's ONLY platform branch. `os` is `std::env::consts::OS` at the one
/// call site in `main.rs`; anything else is a platform Build has no daemon for.
pub fn manager_for(os: &str) -> Option<Box<dyn ServiceManager>> {
    match os {
        "macos" => Some(Box::new(Launchd)),
        "linux" => Some(Box::new(Systemd)),
        _ => None,
    }
}

/// The unit file's mode. It carries the daemon's whole environment, so it is
/// written like a secret: readable and writable by the installing user only,
/// never by the umask's default 0644.
const UNIT_MODE: u32 = 0o600;

/// Write the unit, then activate it. The file is written first so a failing
/// activation leaves something to look at, and is never removed on failure.
pub fn install(
    manager: &dyn ServiceManager,
    ctx: &ServiceContext,
    config: &ServiceConfig,
    run: &mut dyn FnMut(&ShellCommand) -> io::Result<bool>,
) -> Result<PathBuf, ServiceError> {
    let unit_path = manager.unit_path(&ctx.home);
    if let Some(parent) = unit_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_unit(&unit_path, &manager.render_unit(config))?;
    for command in manager.activate(ctx, &unit_path) {
        run_command(&command, run)?;
    }
    Ok(unit_path)
}

/// Write the unit owner-only. `mode` covers the file this call creates; the
/// `set_permissions` covers one an earlier install left world-readable, and
/// runs before a single byte of the new text is on disk.
fn write_unit(unit_path: &Path, unit: &str) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(UNIT_MODE)
        .open(unit_path)?;
    file.set_permissions(std::fs::Permissions::from_mode(UNIT_MODE))?;
    file.write_all(unit.as_bytes())
}

/// The environment variables that carry the device's own key material. A
/// provisioned or seeded shell has them set, and `install` runs in that shell.
pub const DEVICE_KEY_VARS: [&str; 3] = [
    "BRIDGE_IDENTITY_PRIV",
    "BRIDGE_TRANSPORT_PRIV",
    "BRIDGE_TRANSPORT_PUB",
];

/// Drop the device's key material from a daemon environment. The unit file
/// outlives the shell that wrote it and is readable by anything that can read
/// the user's config dir or ask systemd (`systemctl --user show -p Environment`),
/// and an installed daemon has no need for them: it runs from the identity file
/// the install gate just verified.
pub fn without_device_keys(env: Vec<(String, String)>) -> Vec<(String, String)> {
    env.into_iter()
        .filter(|(key, _)| !DEVICE_KEY_VARS.contains(&key.as_str()))
        .collect()
}

/// Deactivate, then remove the unit. A unit that was never installed is not an
/// error — the operator asked for it gone and it is gone.
pub fn uninstall(
    manager: &dyn ServiceManager,
    ctx: &ServiceContext,
    run: &mut dyn FnMut(&ShellCommand) -> io::Result<bool>,
) -> Result<Uninstalled, ServiceError> {
    let unit_path = manager.unit_path(&ctx.home);
    for command in manager.deactivate(ctx, &unit_path) {
        run_command(&command, run)?;
    }
    match std::fs::remove_file(&unit_path) {
        Ok(()) => Ok(Uninstalled::Removed(unit_path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(Uninstalled::NothingInstalled(unit_path))
        }
        Err(error) => Err(error.into()),
    }
}

fn run_command(
    command: &ShellCommand,
    run: &mut dyn FnMut(&ShellCommand) -> io::Result<bool>,
) -> Result<(), ServiceError> {
    let outcome = run(command);
    if command.tolerate_failure {
        if let Some(report) = failure_report(command, &outcome) {
            eprintln!("{report}");
        }
        return Ok(());
    }
    match outcome {
        Ok(true) => Ok(()),
        Ok(false) => Err(ServiceError::Activate {
            program: command.program,
            args: command.args.clone(),
        }),
        Err(error) => Err(ServiceError::Io(error)),
    }
}

/// The stderr line for a best-effort command that did not take: the command as
/// it ran, then its note. `None` for a success, and for a plain tolerated
/// command, whose failure is routine.
fn failure_report(command: &ShellCommand, outcome: &io::Result<bool>) -> Option<String> {
    let note = command.failure_note?;
    if matches!(outcome, Ok(true)) {
        return None;
    }
    Some(format!(
        "`{} {}` failed; {note}",
        command.program,
        command.args.join(" ")
    ))
}

/// Pin the installing shell's PATH into the daemon environment. Both launchd
/// and systemd start user services with a bare PATH, and a bare PATH means the
/// `claude` harness — which lives wherever the user's toolchain manager put it
/// — cannot be spawned at all. `path` supplies the value (normally
/// `std::env::var("PATH")`); an explicit PATH already in `env` wins.
pub fn with_install_path(
    mut env: Vec<(String, String)>,
    path: impl FnOnce() -> Option<String>,
) -> Vec<(String, String)> {
    if env.iter().any(|(key, _)| key == "PATH") {
        return env;
    }
    if let Some(path) = path().filter(|path| !path.trim().is_empty()) {
        env.push(("PATH".to_string(), path));
    }
    env
}

/// Test fixtures shared by this module and both platform managers: the shape of
/// a `ServiceConfig` and a `ServiceContext` has one home in the tests too, so
/// changing either struct is a one-place edit.
#[cfg(test)]
pub(super) mod fixtures {
    use super::{ServiceConfig, ServiceContext};
    use std::path::Path;

    /// A config as `install-service` builds one: the binary where the installer
    /// put it, logs under the bridge's state dir, and the two URLs the daemon is
    /// pinned to. `home` is whatever names a directory — the platform tests
    /// spell it as a literal, the orchestration tests hand over a temp dir.
    pub fn sample_config(home: impl AsRef<Path>) -> ServiceConfig {
        let home = home.as_ref();
        ServiceConfig {
            binary_path: home.join(".local/bin/build-bridge"),
            log_dir: home.join(".build/log"),
            env: vec![
                ("BRIDGE_RELAY_URL".into(), "wss://relay.getbuild.ing".into()),
                ("BRIDGE_API_URL".into(), "https://getbuild.ing".into()),
            ],
        }
    }

    /// The installing user: their home, and the numeric uid launchd addresses
    /// their gui domain by.
    pub fn context(home: impl AsRef<Path>, uid: &str) -> ServiceContext {
        ServiceContext {
            home: home.as_ref().to_path_buf(),
            uid: uid.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn status(approved: bool, owner: Option<&str>) -> StatusResponse {
        status_in(approved, owner, None)
    }

    fn status_in(approved: bool, owner: Option<&str>, state: Option<&str>) -> StatusResponse {
        serde_json::from_value(serde_json::json!({
            "approved": approved,
            "owner_user_id": owner,
            "state": state,
        }))
        .unwrap()
    }

    fn identity(approved: bool) -> StoredIdentity {
        let mut stored = crate::identity::generate("my-box");
        stored.approved = approved;
        stored
    }

    #[test]
    fn gate_refuses_when_no_local_identity() {
        let api = status(true, Some("user-1"));
        let result = check_install_gate(None, Ok(&api));
        assert_eq!(result, Err(InstallGateError::NotPaired));
    }

    /// A failed status call is not the api saying no; the operator is told to
    /// try again rather than to throw a working pairing away.
    #[test]
    fn gate_refuses_when_the_api_cannot_answer() {
        let result = check_install_gate(Some(&identity(true)), Err("503 Service Unavailable"));
        assert_eq!(
            result,
            Err(InstallGateError::Unreachable {
                detail: "503 Service Unavailable".into()
            })
        );
    }

    #[test]
    fn gate_refuses_pending_approval() {
        let api = status(false, None);
        let result = check_install_gate(Some(&identity(false)), Ok(&api));
        assert_eq!(result, Err(InstallGateError::PendingApproval));
    }

    /// The installer's dead end (#317): an identity stored as approved that the
    /// api no longer honours is a lapsed pairing to redo, never "not approved
    /// yet" — revoked and unknown devices have no code waiting to be approved.
    #[test]
    fn gate_reads_a_stored_approval_the_api_dropped_as_a_lapsed_pairing() {
        for (state, lapse) in [
            (Some("revoked"), Lapse::Revoked),
            (Some("unknown"), Lapse::Unknown),
            (None, Lapse::NotApproved),
        ] {
            let api = status_in(false, None, state);
            let result = check_install_gate(Some(&identity(true)), Ok(&api));
            assert_eq!(
                result,
                Err(InstallGateError::NoLongerPaired(lapse)),
                "{state:?}"
            );
        }
    }

    #[test]
    fn gate_refuses_approved_but_ownerless() {
        let api = status(true, None);
        let result = check_install_gate(Some(&identity(true)), Ok(&api));
        assert_eq!(result, Err(InstallGateError::ApprovedWithoutOwner));
    }

    #[test]
    fn gate_passes_only_for_an_approved_owned_device() {
        let api = status(true, Some("user-42"));
        let result = check_install_gate(Some(&identity(true)), Ok(&api));
        assert_eq!(result, Ok("user-42".to_string()));
    }

    /// Every refusal about pairing names a command the operator can actually run. `serve` is
    /// not one of them: it pairs only as a side effect of starting a daemon
    /// that never returns, which is why `pair` exists. None sends the operator
    /// to approve a device in Settings → Devices, which lists only devices
    /// already approved: a pending one is approved by entering its code.
    #[test]
    fn gate_error_messages_tell_the_operator_what_to_do() {
        for error in [
            InstallGateError::NotPaired,
            InstallGateError::PendingApproval,
            InstallGateError::NoLongerPaired(Lapse::Revoked),
            InstallGateError::ApprovedWithoutOwner,
        ] {
            let message = error.to_string();
            assert!(message.contains("`build-bridge pair`"), "{message}");
            assert!(
                !message.contains("build-bridge serve"),
                "pairing is `build-bridge pair`, not a daemon that never returns: {message}"
            );
            assert!(!message.contains("approve this device"), "{message}");
        }
    }

    /// A status call that failed is a connection problem; an identity file
    /// that could not be read or moved is a file problem, said as one.
    #[test]
    fn pairing_errors_map_to_the_refusal_that_names_their_cause() {
        use crate::pairing::PairingError;
        assert!(matches!(
            InstallGateError::from(PairingError::Http("timed out".into())),
            InstallGateError::Unreachable { .. }
        ));
        assert!(matches!(
            InstallGateError::from(PairingError::Rejected("503".into())),
            InstallGateError::Unreachable { .. }
        ));
        assert_eq!(
            InstallGateError::from(PairingError::Identity("permission denied".into())),
            InstallGateError::IdentityFile {
                detail: "permission denied".into()
            }
        );
    }

    /// `pair` refuses with these too, so neither sends the operator back to
    /// `pair`.
    #[test]
    fn connection_and_file_refusals_do_not_send_the_operator_round_in_a_circle() {
        let unreachable = InstallGateError::Unreachable {
            detail: "timed out".into(),
        }
        .to_string();
        assert!(unreachable.contains("could not confirm"), "{unreachable}");
        assert!(unreachable.contains("try again"), "{unreachable}");
        assert!(!unreachable.contains("build-bridge pair"), "{unreachable}");
        let file = InstallGateError::IdentityFile {
            detail: "~/.build/identity.json: expected value".into(),
        }
        .to_string();
        assert!(file.contains("identity file"), "{file}");
        assert!(file.contains("~/.build/identity.json"), "{file}");
        assert!(!file.contains("connection"), "{file}");
        assert!(!file.contains("build-bridge pair"), "{file}");
    }

    #[test]
    fn a_lapsed_pairing_says_it_is_no_longer_valid_and_why() {
        let message = InstallGateError::NoLongerPaired(Lapse::Revoked).to_string();
        assert!(message.contains("no longer valid"), "{message}");
        assert!(
            message.contains("revoked in Settings → Devices"),
            "{message}"
        );
    }

    #[test]
    fn a_pending_pairing_points_at_the_code_not_the_device_list() {
        let message = InstallGateError::PendingApproval.to_string();
        assert!(message.contains("pairing code"), "{message}");
        assert!(message.contains("Add a device"), "{message}");
    }

    #[test]
    fn manager_for_selects_launchd_on_macos() {
        assert_eq!(
            manager_for("macos").expect("macOS has a manager").name(),
            "launchd LaunchAgent"
        );
    }

    #[test]
    fn manager_for_selects_systemd_on_linux() {
        assert_eq!(
            manager_for("linux").expect("Linux has a manager").name(),
            "systemd user unit"
        );
    }

    #[test]
    fn manager_for_knows_no_other_platform() {
        assert!(manager_for("windows").is_none());
        assert!(manager_for("freebsd").is_none());
    }

    /// A manager with no platform behind it: its unit is a nested file under
    /// HOME and its commands are whatever the test hands it, so the shared
    /// orchestration can be observed on its own.
    struct FakeManager {
        activate: Vec<ShellCommand>,
        deactivate: Vec<ShellCommand>,
    }

    impl ServiceManager for FakeManager {
        fn name(&self) -> &'static str {
            "fake service"
        }
        fn unit_path(&self, home: &Path) -> PathBuf {
            home.join("units/nested/fake.service")
        }
        fn render_unit(&self, config: &ServiceConfig) -> String {
            format!("unit for {}", config.binary_path.display())
        }
        fn activate(&self, _ctx: &ServiceContext, _unit_path: &Path) -> Vec<ShellCommand> {
            self.activate.clone()
        }
        fn deactivate(&self, _ctx: &ServiceContext, _unit_path: &Path) -> Vec<ShellCommand> {
            self.deactivate.clone()
        }
        fn after_install_hint(&self) -> Option<&'static str> {
            None
        }
    }

    #[test]
    fn install_writes_the_unit_then_runs_activation_in_order() {
        let home = tempfile::tempdir().expect("temp home");
        let ctx = fixtures::context(home.path(), "501");
        let manager = FakeManager {
            activate: vec![
                ShellCommand::tolerated("first", &["a"]),
                ShellCommand::required("second", &["b"]),
            ],
            deactivate: vec![],
        };
        let unit = manager.unit_path(home.path());
        let config = fixtures::sample_config("/home/dev");
        let rendered = manager.render_unit(&config);
        let mut seen: Vec<(&'static str, String)> = vec![];

        let written = install(&manager, &ctx, &config, &mut |command| {
            seen.push((
                command.program,
                std::fs::read_to_string(&unit).unwrap_or_default(),
            ));
            Ok(true)
        })
        .expect("install succeeds");

        assert_eq!(written, unit);
        // Every command saw the finished unit file: it is written, in its
        // freshly created directory, before activation begins.
        assert_eq!(
            seen,
            vec![("first", rendered.clone()), ("second", rendered)],
        );
    }

    #[test]
    fn install_fails_when_a_required_command_fails_and_keeps_the_file_for_inspection() {
        let home = tempfile::tempdir().expect("temp home");
        let ctx = fixtures::context(home.path(), "501");
        let manager = FakeManager {
            activate: vec![ShellCommand::required("bootstrap", &["gui/501", "unit"])],
            deactivate: vec![],
        };

        let error = install(
            &manager,
            &ctx,
            &fixtures::sample_config("/home/dev"),
            &mut |_| Ok(false),
        )
        .expect_err("a required command that fails fails the install");

        match error {
            ServiceError::Activate { program, args } => {
                assert_eq!(program, "bootstrap");
                assert_eq!(args, vec!["gui/501".to_string(), "unit".to_string()]);
            }
            other => panic!("expected an activation failure, got {other:?}"),
        }
        assert!(
            manager.unit_path(home.path()).exists(),
            "the unit stays on disk so the operator can read what was written"
        );
    }

    #[test]
    fn install_ignores_a_tolerated_command_failure() {
        let home = tempfile::tempdir().expect("temp home");
        let ctx = fixtures::context(home.path(), "501");
        let manager = FakeManager {
            activate: vec![
                ShellCommand::tolerated("bootout", &["nothing-loaded"]),
                ShellCommand::required("bootstrap", &["gui/501"]),
            ],
            deactivate: vec![],
        };

        let result = install(
            &manager,
            &ctx,
            &fixtures::sample_config("/home/dev"),
            &mut |command| Ok(command.program != "bootout"),
        );

        assert!(result.is_ok(), "a tolerated failure is not a failure");
    }

    #[test]
    fn install_survives_a_best_effort_command_that_fails() {
        let home = tempfile::tempdir().expect("temp home");
        let ctx = fixtures::context(home.path(), "1000");
        let manager = FakeManager {
            activate: vec![
                ShellCommand::best_effort("set-property", &["slice"], "left at the default"),
                ShellCommand::required("enable", &["--now"]),
            ],
            deactivate: vec![],
        };
        let mut ran = vec![];

        let result = install(
            &manager,
            &ctx,
            &fixtures::sample_config("/home/dev"),
            &mut |command| {
                ran.push(command.program);
                Ok(command.program != "set-property")
            },
        );

        assert!(result.is_ok(), "a best-effort failure is not a failure");
        assert_eq!(
            ran,
            vec!["set-property", "enable"],
            "the install carried on"
        );
    }

    /// The operator hears about a best-effort command that did not take — the
    /// command as it ran, then what that leaves them with.
    #[test]
    fn a_failed_best_effort_command_is_reported_with_its_note() {
        let command = ShellCommand::best_effort(
            "systemctl",
            &["--user", "set-property", "x.slice", "CPUWeight=20"],
            "the agents keep the default priority.",
        );

        assert_eq!(
            failure_report(&command, &Ok(false)).as_deref(),
            Some(
                "`systemctl --user set-property x.slice CPUWeight=20` failed; \
                 the agents keep the default priority."
            )
        );
        assert!(failure_report(&command, &Err(io::Error::other("no systemctl"))).is_some());
        assert_eq!(failure_report(&command, &Ok(true)), None);
    }

    /// A plain tolerated command fails as a matter of course (booting out what
    /// was never loaded), so its failure is not news.
    #[test]
    fn a_failed_tolerated_command_is_not_reported() {
        let command = ShellCommand::tolerated("launchctl", &["bootout", "gui/501/x"]);
        assert_eq!(failure_report(&command, &Ok(false)), None);
    }

    /// Both platforms start a user service with a bare PATH, and a bare PATH
    /// cannot find the `claude` harness wherever the user's toolchain manager
    /// put it, so the installing shell's PATH is pinned into the unit.
    #[test]
    fn the_daemon_environment_pins_the_installing_shells_path() {
        let env = with_install_path(vec![("BRIDGE_API_URL".into(), "https://x".into())], || {
            Some("/opt/homebrew/bin:/usr/bin".to_string())
        });

        assert_eq!(
            env.iter().find(|(key, _)| key == "PATH").map(|(_, v)| v),
            Some(&"/opt/homebrew/bin:/usr/bin".to_string())
        );
    }

    /// An operator who set PATH deliberately meant it, so the shell's copy does
    /// not overwrite it.
    #[test]
    fn an_explicit_path_in_the_env_is_left_alone() {
        let env = with_install_path(vec![("PATH".into(), "/pinned".into())], || {
            Some("/opt/homebrew/bin".to_string())
        });

        assert_eq!(env, vec![("PATH".to_string(), "/pinned".to_string())]);
    }

    /// A shell with no usable PATH pins nothing rather than an empty value the
    /// daemon would then trust.
    #[test]
    fn a_blank_shell_path_is_not_pinned() {
        assert_eq!(with_install_path(vec![], || Some("  ".into())), vec![]);
        assert_eq!(with_install_path(vec![], || None), vec![]);
    }

    /// The unit is a copy of the daemon's whole environment, so it is written
    /// like a secret file, not like a config file the umask decides on.
    #[test]
    fn install_writes_a_unit_only_its_owner_can_read() {
        let home = tempfile::tempdir().expect("temp home");
        let ctx = fixtures::context(home.path(), "501");
        let manager = FakeManager {
            activate: vec![],
            deactivate: vec![],
        };

        let unit = install(
            &manager,
            &ctx,
            &fixtures::sample_config("/home/dev"),
            &mut |_| Ok(true),
        )
        .expect("install succeeds");

        assert_eq!(mode_of(&unit), 0o600);
    }

    /// Re-installing over a unit an earlier version left world-readable has to
    /// close it, or the fix only reaches machines that never installed before.
    #[test]
    fn install_tightens_a_unit_an_earlier_install_left_world_readable() {
        let home = tempfile::tempdir().expect("temp home");
        let ctx = fixtures::context(home.path(), "501");
        let manager = FakeManager {
            activate: vec![],
            deactivate: vec![],
        };
        let unit = manager.unit_path(home.path());
        std::fs::create_dir_all(unit.parent().expect("unit has a parent")).expect("unit dir");
        std::fs::write(&unit, "an older, readable unit").expect("pre-existing unit");
        std::fs::set_permissions(&unit, std::fs::Permissions::from_mode(0o644)).expect("chmod");

        install(
            &manager,
            &ctx,
            &fixtures::sample_config("/home/dev"),
            &mut |_| Ok(true),
        )
        .expect("install succeeds");

        assert_eq!(mode_of(&unit), 0o600);
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("the unit is on disk")
            .permissions()
            .mode()
            & 0o777
    }

    /// The unit outlives the shell that installed it, so the device's key
    /// material must never be copied into it: an installed daemon runs from the
    /// identity file the gate just verified.
    #[test]
    fn the_daemon_environment_drops_the_devices_key_material() {
        let env = without_device_keys(vec![
            ("BRIDGE_API_URL".into(), "https://getbuild.ing".into()),
            ("BRIDGE_IDENTITY_PRIV".into(), "ed25519-private".into()),
            ("BRIDGE_TRANSPORT_PRIV".into(), "x25519-private".into()),
            ("BRIDGE_TRANSPORT_PUB".into(), "x25519-public".into()),
        ]);

        assert_eq!(
            env,
            vec![(
                "BRIDGE_API_URL".to_string(),
                "https://getbuild.ing".to_string()
            )]
        );
    }

    #[test]
    fn uninstall_deactivates_then_removes_the_unit() {
        let home = tempfile::tempdir().expect("temp home");
        let ctx = fixtures::context(home.path(), "501");
        let manager = FakeManager {
            activate: vec![],
            deactivate: vec![ShellCommand::tolerated("disable", &["--now"])],
        };
        let unit = install(
            &manager,
            &ctx,
            &fixtures::sample_config("/home/dev"),
            &mut |_| Ok(true),
        )
        .expect("install succeeds");
        let mut seen: Vec<(&'static str, bool)> = vec![];

        let outcome = uninstall(&manager, &ctx, &mut |command| {
            seen.push((command.program, unit.exists()));
            Ok(true)
        })
        .expect("uninstall succeeds");

        assert_eq!(
            seen,
            vec![("disable", true)],
            "deactivation runs while the unit is still on disk"
        );
        assert_eq!(outcome, Uninstalled::Removed(unit.clone()));
        assert!(!unit.exists());
    }

    #[test]
    fn uninstall_reports_nothing_installed_when_the_unit_is_absent() {
        let home = tempfile::tempdir().expect("temp home");
        let ctx = fixtures::context(home.path(), "501");
        let manager = FakeManager {
            activate: vec![],
            deactivate: vec![],
        };

        let outcome =
            uninstall(&manager, &ctx, &mut |_| Ok(true)).expect("a missing unit is not an error");

        assert_eq!(
            outcome,
            Uninstalled::NothingInstalled(manager.unit_path(home.path()))
        );
    }
}
