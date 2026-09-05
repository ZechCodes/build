use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::harness::{Harness, HarnessContext, HarnessError, INHERITED_AGENT_MARKERS};
use crate::models::{AgentProvider, ModelChoice, ModelOption};
use crate::orchestrator::SpawnOptions;
use crate::pty::HarnessSpec;

pub const EFFORT_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
pub const PI_TUI_SETTLE: Duration = Duration::from_millis(500);
pub const PI_TUI_SUBMIT_DELAY: Duration = Duration::from_millis(750);

const EXTENSION_SOURCE: &[u8] = include_bytes!("build-tools.ts");

pub struct PiHarness;

impl Harness for PiHarness {
    fn provider(&self) -> AgentProvider {
        AgentProvider::Pi
    }

    fn label(&self) -> &'static str {
        "Pi"
    }

    fn models(&self) -> Vec<ModelOption> {
        Vec::new()
    }

    fn effort_levels(&self) -> &'static [&'static str] {
        &EFFORT_LEVELS
    }

    fn model_args(&self, choice: &ModelChoice) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(model) = &choice.model {
            args.extend(["--model".to_string(), model.clone()]);
        }
        if let Some(effort) = &choice.effort {
            args.extend(["--thinking".to_string(), effort.clone()]);
        }
        args
    }

    fn spec(
        &self,
        choice: &ModelChoice,
        options: &SpawnOptions,
        context: &HarnessContext,
    ) -> Result<HarnessSpec, HarnessError> {
        ensure_pi_root_outside_checkout(&context.state_root, &options.cwd)?;
        let extension_path = BuildPiExtension::materialize(&context.state_root)?;
        let session_dir = private_session_dir(&context.state_root, &options.owner_id)?;
        let mut spec = HarnessSpec::new("pi")
            .settle(PI_TUI_SETTLE)
            .submit_delay(PI_TUI_SUBMIT_DELAY)
            .unset_all(INHERITED_AGENT_MARKERS)
            .arg("--approve")
            .arg("--no-extensions")
            .arg("--extension")
            .arg(extension_path.to_string_lossy().into_owned())
            .arg("--session-dir")
            .arg(session_dir.to_string_lossy().into_owned())
            .arg("--session-id")
            .arg(&options.owner_id);
        for arg in self.model_args(choice) {
            spec = spec.arg(arg);
        }
        Ok(spec
            .known_session_id(&options.owner_id)
            .env(
                "BRIDGE_MCP_SOCKET",
                context.mcp_socket.to_string_lossy().into_owned(),
            )
            .env("BRIDGE_MCP_TOKEN", &options.mcp_session_token)
            .env(
                "BUILD_PI_MCP_COMMAND",
                context.bridge_exe.to_string_lossy().into_owned(),
            )
            .env("BUILD_PI_MCP_OWNER", &options.owner_id))
    }

    fn routes_captures(&self) -> bool {
        false
    }

    fn has_transcript(&self, _home: &Path, _cwd: &Path) -> bool {
        false
    }

    fn holds_conversation(&self, _home: &Path, _cwd: &Path, _id: &str) -> bool {
        false
    }
}

fn ensure_pi_root_outside_checkout(state_root: &Path, cwd: &Path) -> Result<(), HarnessError> {
    let checkout = std::fs::canonicalize(cwd).map_err(|error| {
        HarnessError::Setup(format!(
            "canonicalize Pi checkout {}: {error}",
            cwd.display()
        ))
    })?;
    let state_root = std::fs::canonicalize(state_root).map_err(|error| {
        HarnessError::Setup(format!(
            "canonicalize Pi state root {}: {error}",
            state_root.display()
        ))
    })?;
    let pi_root = state_root.join("harness/pi");
    if pi_root.starts_with(&checkout) || checkout.starts_with(&pi_root) {
        return Err(HarnessError::Setup(format!(
            "Pi state directory must be outside checkout {}: {}",
            checkout.display(),
            pi_root.display()
        )));
    }
    Ok(())
}

struct BuildPiExtension;

impl BuildPiExtension {
    fn materialize(state_root: &Path) -> Result<PathBuf, HarnessError> {
        Self::materialize_with(state_root, &ExtensionMaterializer::new())
    }

    fn materialize_with(
        state_root: &Path,
        materializer: &ExtensionMaterializer,
    ) -> Result<PathBuf, HarnessError> {
        materializer.materialize(state_root)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InjectedFailure {
    CreateRoot,
    CanonicalizeRoot,
    InspectDir,
    CreateDir,
    CanonicalizeDir,
    RestrictDir,
    InspectExtension,
    CanonicalizeExtension,
    OpenExtension,
    ReadExtension,
    Stage,
    Write,
    Restrict,
    SyncFile,
    Rename,
    RemoveStaged,
    SyncParent,
}

struct ExtensionMaterializer {
    #[cfg(test)]
    failures: Vec<InjectedFailure>,
    #[cfg(test)]
    private_at_publish: std::sync::atomic::AtomicBool,
}

struct InspectedExtension {
    bytes: Vec<u8>,
    mode: u32,
}

struct StagedExtension {
    path: PathBuf,
}

impl ExtensionMaterializer {
    fn new() -> Self {
        Self {
            #[cfg(test)]
            failures: Vec::new(),
            #[cfg(test)]
            private_at_publish: std::sync::atomic::AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    fn failing_at(failure: InjectedFailure) -> Self {
        Self::failing_at_all(&[failure])
    }

    #[cfg(test)]
    fn failing_at_all(failures: &[InjectedFailure]) -> Self {
        Self {
            failures: failures.to_vec(),
            private_at_publish: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn materialize(&self, state_root: &Path) -> Result<PathBuf, HarnessError> {
        let (state_root, installed) = self.extension_path(state_root)?;
        if self.extension_matches(&installed, &state_root)? {
            return Ok(installed);
        }
        self.install_extension(&installed, &state_root)?;
        Ok(installed)
    }

    fn extension_path(&self, state_root: &Path) -> Result<(PathBuf, PathBuf), HarnessError> {
        let state_root = self.prepare_root(state_root)?;
        let extensions_root =
            self.private_dir(&state_root.join("harness/pi/extensions"), &state_root)?;
        let digest = format!("{:x}", Sha256::digest(EXTENSION_SOURCE));
        let digest_dir = self.private_dir(&extensions_root.join(digest), &state_root)?;
        Ok((state_root, digest_dir.join("build-tools.ts")))
    }

    fn session_dir(&self, state_root: &Path, owner_id: &str) -> Result<PathBuf, HarnessError> {
        let state_root = self.prepare_root(state_root)?;
        let sessions_root =
            self.private_dir(&state_root.join("harness/pi/sessions"), &state_root)?;
        self.private_dir(&sessions_root.join(owner_id), &state_root)
    }

    fn extension_matches(&self, path: &Path, state_root: &Path) -> Result<bool, HarnessError> {
        Ok(self
            .inspect_extension(path, state_root)?
            .is_some_and(|extension| {
                extension.mode == 0o400 && extension.bytes == EXTENSION_SOURCE
            }))
    }

    fn inspect_extension(
        &self,
        path: &Path,
        state_root: &Path,
    ) -> Result<Option<InspectedExtension>, HarnessError> {
        self.checkpoint(InjectedFailure::InspectExtension)
            .map_err(|error| setup_error("inspect Pi extension", path, error))?;
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => metadata,
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(setup_error("inspect Pi extension", path, error)),
        };
        self.checkpoint(InjectedFailure::CanonicalizeExtension)
            .map_err(|error| setup_error("canonicalize Pi extension", path, error))?;
        let canonical = std::fs::canonicalize(path)
            .map_err(|error| setup_error("canonicalize Pi extension", path, error))?;
        if !canonical.starts_with(state_root) {
            return Err(HarnessError::Setup(format!(
                "Pi extension escapes state root: {}",
                canonical.display()
            )));
        }
        self.checkpoint(InjectedFailure::OpenExtension)
            .map_err(|error| setup_error("read Pi extension", path, error))?;
        let mut file =
            File::open(path).map_err(|error| setup_error("read Pi extension", path, error))?;
        self.checkpoint(InjectedFailure::ReadExtension)
            .map_err(|error| setup_error("read Pi extension", path, error))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| setup_error("read Pi extension", path, error))?;
        Ok(Some(InspectedExtension {
            bytes,
            mode: metadata.permissions().mode() & 0o777,
        }))
    }

    fn install_extension(&self, installed: &Path, state_root: &Path) -> Result<(), HarnessError> {
        let staged = self.stage_extension(installed)?;
        let parent = self
            .extension_parent(installed, state_root)
            .map_err(|error| self.cleanup_staged_after_error(&staged.path, error))?;
        self.checkpoint(InjectedFailure::Rename).map_err(|error| {
            self.cleanup_staged_after_error(
                &staged.path,
                setup_error("install Pi extension", installed, error),
            )
        })?;
        std::fs::rename(&staged.path, installed).map_err(|error| {
            self.cleanup_staged_after_error(
                &staged.path,
                setup_error("install Pi extension", installed, error),
            )
        })?;
        #[cfg(test)]
        self.record_published_mode(installed)?;
        self.checkpoint(InjectedFailure::SyncParent)
            .map_err(|error| setup_error("sync Pi extension directory", &parent, error))?;
        File::open(&parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| setup_error("sync Pi extension directory", &parent, error))?;
        if !self.extension_matches(installed, state_root)? {
            return Err(HarnessError::Setup(format!(
                "Pi extension hash verification failed at {}",
                installed.display()
            )));
        }
        Ok(())
    }

    fn prepare_root(&self, state_root: &Path) -> Result<PathBuf, HarnessError> {
        if !state_root.is_absolute() {
            return Err(HarnessError::Setup(format!(
                "Pi state root must be absolute: {}",
                state_root.display()
            )));
        }
        self.checkpoint(InjectedFailure::CreateRoot)
            .map_err(|error| setup_error("create Pi state root", state_root, error))?;
        std::fs::create_dir_all(state_root)
            .map_err(|error| setup_error("create Pi state root", state_root, error))?;
        self.checkpoint(InjectedFailure::CanonicalizeRoot)
            .map_err(|error| setup_error("canonicalize Pi state root", state_root, error))?;
        std::fs::canonicalize(state_root)
            .map_err(|error| setup_error("canonicalize Pi state root", state_root, error))
    }

    fn private_dir(&self, path: &Path, state_root: &Path) -> Result<PathBuf, HarnessError> {
        let relative = path.strip_prefix(state_root).map_err(|_| {
            HarnessError::Setup(format!(
                "private Pi directory is outside state root: {}",
                path.display()
            ))
        })?;
        let mut current = state_root.to_path_buf();
        for component in relative.components() {
            current.push(component);
            self.ensure_private_component(&current)?;
        }
        self.checkpoint(InjectedFailure::CanonicalizeDir)
            .map_err(|error| setup_error("canonicalize private Pi directory", path, error))?;
        let canonical = std::fs::canonicalize(path)
            .map_err(|error| setup_error("canonicalize private Pi directory", path, error))?;
        if !canonical.starts_with(state_root) {
            return Err(HarnessError::Setup(format!(
                "private Pi directory escapes state root: {}",
                canonical.display()
            )));
        }
        Ok(canonical)
    }

    fn ensure_private_component(&self, path: &Path) -> Result<(), HarnessError> {
        self.checkpoint(InjectedFailure::InspectDir)
            .map_err(|error| setup_error("inspect private Pi directory", path, error))?;
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_dir() => self.restrict_dir(path),
            Ok(_) => Err(HarnessError::Setup(format!(
                "private Pi path is not a directory: {}",
                path.display()
            ))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.create_private_dir(path)
            }
            Err(error) => Err(setup_error("inspect private Pi directory", path, error)),
        }
    }

    fn create_private_dir(&self, path: &Path) -> Result<(), HarnessError> {
        self.checkpoint(InjectedFailure::CreateDir)
            .map_err(|error| setup_error("create private Pi directory", path, error))?;
        let mut builder = std::fs::DirBuilder::new();
        match builder.mode(0o700).create(path) {
            Ok(()) => self.restrict_dir(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                self.ensure_private_component(path)
            }
            Err(error) => Err(setup_error("create private Pi directory", path, error)),
        }
    }

    fn restrict_dir(&self, path: &Path) -> Result<(), HarnessError> {
        self.checkpoint(InjectedFailure::RestrictDir)
            .map_err(|error| setup_error("restrict private Pi directory", path, error))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| setup_error("restrict private Pi directory", path, error))
    }

    fn stage_extension(&self, installed: &Path) -> Result<StagedExtension, HarnessError> {
        let parent = installed.parent().ok_or_else(|| {
            HarnessError::Setup(format!(
                "Pi extension has no parent: {}",
                installed.display()
            ))
        })?;
        let staged = parent.join(format!(".build-tools-{}.tmp", uuid::Uuid::new_v4()));
        self.checkpoint(InjectedFailure::Stage)
            .map_err(|error| setup_error("stage Pi extension", &staged, error))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staged)
            .map_err(|error| setup_error("stage Pi extension", &staged, error))?;
        let preparation = self.prepare_staged_extension(&mut file, &staged);
        drop(file);
        preparation.map_err(|error| self.cleanup_staged_after_error(&staged, error))?;
        Ok(StagedExtension { path: staged })
    }

    fn prepare_staged_extension(&self, file: &mut File, staged: &Path) -> Result<(), HarnessError> {
        self.checkpoint(InjectedFailure::Write)
            .map_err(|error| setup_error("write Pi extension", staged, error))?;
        file.write_all(EXTENSION_SOURCE)
            .map_err(|error| setup_error("write Pi extension", staged, error))?;
        self.checkpoint(InjectedFailure::Restrict)
            .map_err(|error| setup_error("restrict staged Pi extension", staged, error))?;
        file.set_permissions(std::fs::Permissions::from_mode(0o400))
            .map_err(|error| setup_error("restrict staged Pi extension", staged, error))?;
        self.checkpoint(InjectedFailure::SyncFile)
            .map_err(|error| setup_error("sync Pi extension", staged, error))?;
        file.sync_all()
            .map_err(|error| setup_error("sync Pi extension", staged, error))
    }

    fn extension_parent(&self, path: &Path, state_root: &Path) -> Result<PathBuf, HarnessError> {
        let parent = path.parent().ok_or_else(|| {
            HarnessError::Setup(format!("Pi extension has no parent: {}", path.display()))
        })?;
        self.checkpoint(InjectedFailure::CanonicalizeDir)
            .map_err(|error| setup_error("canonicalize Pi extension parent", parent, error))?;
        let canonical = std::fs::canonicalize(parent)
            .map_err(|error| setup_error("canonicalize Pi extension parent", parent, error))?;
        if !canonical.starts_with(state_root) {
            return Err(HarnessError::Setup(format!(
                "Pi extension parent escapes state root: {}",
                canonical.display()
            )));
        }
        Ok(canonical)
    }

    fn cleanup_staged_after_error(&self, staged: &Path, primary: HarnessError) -> HarnessError {
        let cleanup = self
            .checkpoint(InjectedFailure::RemoveStaged)
            .and_then(|()| std::fs::remove_file(staged));
        let Err(cleanup_error) = cleanup else {
            return primary;
        };
        let primary = match primary {
            HarnessError::Setup(message) => message,
            error => error.to_string(),
        };
        HarnessError::Setup(format!(
            "{primary}; remove staged Pi extension {} after setup failure: {cleanup_error}",
            staged.display()
        ))
    }

    fn checkpoint(&self, failure: InjectedFailure) -> std::io::Result<()> {
        #[cfg(test)]
        if self.failures.contains(&failure) {
            return Err(std::io::Error::other("injected materialization failure"));
        }
        #[cfg(not(test))]
        let _ = failure;
        Ok(())
    }

    #[cfg(test)]
    fn record_published_mode(&self, installed: &Path) -> Result<(), HarnessError> {
        let mode = std::fs::metadata(installed)
            .map_err(|error| setup_error("inspect published Pi extension", installed, error))?
            .permissions()
            .mode()
            & 0o777;
        self.private_at_publish
            .store(mode == 0o400, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

fn setup_error(action: &str, path: &Path, error: std::io::Error) -> HarnessError {
    HarnessError::Setup(format!("{action} {}: {error}", path.display()))
}

#[cfg(test)]
fn extension_path(state_root: &Path) -> Result<(PathBuf, PathBuf), HarnessError> {
    ExtensionMaterializer::new().extension_path(state_root)
}

fn private_session_dir(state_root: &Path, owner_id: &str) -> Result<PathBuf, HarnessError> {
    if !crate::harness::is_a_filename(owner_id) {
        return Err(HarnessError::Setup(format!(
            "Pi owner id is not a safe directory name: {owner_id:?}"
        )));
    }
    ExtensionMaterializer::new().session_dir(state_root, owner_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(state_root: &Path) -> HarnessContext {
        HarnessContext {
            bridge_exe: PathBuf::from("/usr/local/bin/build-bridge"),
            mcp_socket: PathBuf::from("/tmp/build-mcp.sock"),
            state_root: state_root.to_path_buf(),
        }
    }

    fn options(owner_id: &str, cwd: &Path) -> SpawnOptions {
        SpawnOptions {
            owner_id: owner_id.to_string(),
            mcp_session_token: "secret-token".to_string(),
            cwd: cwd.to_path_buf(),
            continue_session: true,
            resume_session_id: Some("ignored-session".to_string()),
        }
    }

    #[test]
    fn pi_spec_is_tui_only_and_uses_private_stable_paths() {
        let state = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let choice = ModelChoice {
            provider: AgentProvider::Pi,
            model: Some("anthropic/claude-opus-4".replace('/', "-")),
            effort: Some("high".to_string()),
        };
        let spec = PiHarness
            .spec(
                &choice,
                &options("agent-01J", worktree.path()),
                &context(state.path()),
            )
            .unwrap();
        assert_eq!(spec.binary, "pi");
        assert_eq!(
            spec.args,
            vec![
                "--approve",
                "--no-extensions",
                "--extension",
                spec.args[3].as_str(),
                "--session-dir",
                spec.args[5].as_str(),
                "--session-id",
                "agent-01J",
                "--model",
                "anthropic-claude-opus-4",
                "--thinking",
                "high",
            ]
        );
        let canonical_state = std::fs::canonicalize(state.path()).unwrap();
        assert!(Path::new(&spec.args[3]).starts_with(canonical_state.join("harness/pi/extensions")));
        assert_eq!(
            PathBuf::from(&spec.args[5]),
            canonical_state.join("harness/pi/sessions/agent-01J")
        );
        assert_eq!(spec.known_session_id.as_deref(), Some("agent-01J"));
        assert_eq!(spec.settle, PI_TUI_SETTLE);
        assert_eq!(spec.submit_delay, PI_TUI_SUBMIT_DELAY);
        let args = spec.args.join(" ");
        for forbidden in ["--print", "--mode", "--continue", "ignored-session"] {
            assert!(!spec.args.iter().any(|arg| arg == forbidden), "{args}");
        }
        for expected in [
            ("BRIDGE_MCP_SOCKET", "/tmp/build-mcp.sock"),
            ("BRIDGE_MCP_TOKEN", "secret-token"),
            ("BUILD_PI_MCP_COMMAND", "/usr/local/bin/build-bridge"),
            ("BUILD_PI_MCP_OWNER", "agent-01J"),
        ] {
            assert!(spec
                .env
                .iter()
                .any(|entry| entry.0 == expected.0 && entry.1 == expected.1));
        }
    }

    #[test]
    fn pi_spec_rejects_state_inside_checkout_before_creating_pi_files() {
        let checkout = tempfile::tempdir().unwrap();
        let state_root = checkout.path().join("private-state");
        std::fs::create_dir(&state_root).unwrap();

        let error = PiHarness
            .spec(
                &ModelChoice {
                    provider: AgentProvider::Pi,
                    ..ModelChoice::default()
                },
                &options("agent-contained", checkout.path()),
                &context(&state_root),
            )
            .unwrap_err();

        assert!(matches!(error, HarnessError::Setup(_)), "{error}");
        assert!(!state_root.join("harness/pi").exists());
    }

    #[test]
    fn pi_spec_rejects_checkout_inside_pi_root_before_creating_pi_files() {
        let state = tempfile::tempdir().unwrap();
        let pi_root = state.path().join("harness/pi");
        let checkout = pi_root.join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();

        let error = PiHarness
            .spec(
                &ModelChoice {
                    provider: AgentProvider::Pi,
                    ..ModelChoice::default()
                },
                &options("agent-containing", &checkout),
                &context(state.path()),
            )
            .unwrap_err();

        assert!(matches!(error, HarnessError::Setup(_)), "{error}");
        assert!(!pi_root.join("extensions").exists());
        assert!(!pi_root.join("sessions").exists());
    }

    #[test]
    fn pi_has_no_transcript_locator_or_workspace_mutation() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let before = std::fs::read_dir(cwd.path()).unwrap().count();
        PiHarness.prepare_workspace(cwd.path());
        assert_eq!(std::fs::read_dir(cwd.path()).unwrap().count(), before);
        assert!(!PiHarness.has_transcript(home.path(), cwd.path()));
        assert!(!PiHarness.holds_conversation(home.path(), cwd.path(), "agent-01J"));
        assert!(PiHarness.session_locator(home.path(), cwd.path()).is_none());
    }

    #[test]
    fn extension_install_is_idempotent_private_and_repairs_content() {
        let state = tempfile::tempdir().unwrap();
        let first = BuildPiExtension::materialize(state.path()).unwrap();
        let second = BuildPiExtension::materialize(state.path()).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            std::fs::metadata(&first).unwrap().permissions().mode() & 0o777,
            0o400
        );
        assert_eq!(
            std::fs::metadata(first.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&first, b"corrupt").unwrap();
        assert_eq!(BuildPiExtension::materialize(state.path()).unwrap(), first);
        assert_eq!(std::fs::read(&first).unwrap(), EXTENSION_SOURCE);
        assert_eq!(
            std::fs::metadata(&first).unwrap().permissions().mode() & 0o777,
            0o400
        );
        assert!(std::fs::read_dir(first.parent().unwrap())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
    }

    #[test]
    fn extension_inspection_returns_owned_content() {
        let state = tempfile::tempdir().unwrap();
        let installed = BuildPiExtension::materialize(state.path()).unwrap();
        let state_root = std::fs::canonicalize(state.path()).unwrap();

        let inspected = ExtensionMaterializer::new()
            .inspect_extension(&installed, &state_root)
            .unwrap()
            .unwrap();
        std::fs::remove_file(installed).unwrap();

        assert_eq!(inspected.bytes, EXTENSION_SOURCE);
        assert_eq!(inspected.mode, 0o400);
    }

    #[test]
    fn extension_is_private_when_atomically_published() {
        let state = tempfile::tempdir().unwrap();
        let materializer = ExtensionMaterializer::new();
        let installed = BuildPiExtension::materialize_with(state.path(), &materializer).unwrap();
        assert!(materializer
            .private_at_publish
            .load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            std::fs::metadata(installed).unwrap().permissions().mode() & 0o777,
            0o400
        );
    }

    #[test]
    fn publication_failures_leave_only_complete_private_results() {
        for failure in [
            InjectedFailure::Stage,
            InjectedFailure::Write,
            InjectedFailure::Restrict,
            InjectedFailure::SyncFile,
            InjectedFailure::Rename,
            InjectedFailure::SyncParent,
        ] {
            let state = tempfile::tempdir().unwrap();
            let materializer = ExtensionMaterializer::failing_at(failure);
            let (_, installed) = extension_path(state.path()).unwrap();
            std::fs::write(&installed, b"previous complete artifact").unwrap();
            std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o400)).unwrap();
            assert!(BuildPiExtension::materialize_with(state.path(), &materializer).is_err());
            let expected = if failure == InjectedFailure::SyncParent {
                EXTENSION_SOURCE
            } else {
                b"previous complete artifact"
            };
            assert_eq!(std::fs::read(&installed).unwrap(), expected);
            assert_eq!(
                std::fs::metadata(&installed).unwrap().permissions().mode() & 0o777,
                0o400
            );
            assert!(std::fs::read_dir(installed.parent().unwrap())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")));
        }
    }

    #[test]
    fn publication_and_cleanup_failures_are_both_reported() {
        let state = tempfile::tempdir().unwrap();
        let materializer = ExtensionMaterializer::failing_at_all(&[
            InjectedFailure::Rename,
            InjectedFailure::RemoveStaged,
        ]);

        let error = BuildPiExtension::materialize_with(state.path(), &materializer)
            .unwrap_err()
            .to_string();

        assert!(error.contains("install Pi extension"), "{error}");
        assert!(error.contains("remove staged Pi extension"), "{error}");
    }

    #[test]
    fn root_and_directory_failures_propagate_through_the_filesystem_boundary() {
        for failure in [
            InjectedFailure::CreateRoot,
            InjectedFailure::CanonicalizeRoot,
            InjectedFailure::InspectDir,
            InjectedFailure::CreateDir,
            InjectedFailure::CanonicalizeDir,
            InjectedFailure::RestrictDir,
        ] {
            let parent = tempfile::tempdir().unwrap();
            let state_root = parent.path().join("state");
            let materializer = ExtensionMaterializer::failing_at(failure);
            let result = BuildPiExtension::materialize_with(&state_root, &materializer);
            assert!(result.is_err(), "{failure:?} unexpectedly succeeded");
        }
    }

    #[test]
    fn extension_verification_failures_propagate_through_the_filesystem_boundary() {
        for failure in [
            InjectedFailure::InspectExtension,
            InjectedFailure::CanonicalizeExtension,
            InjectedFailure::OpenExtension,
            InjectedFailure::ReadExtension,
        ] {
            let state = tempfile::tempdir().unwrap();
            BuildPiExtension::materialize(state.path()).unwrap();
            let materializer = ExtensionMaterializer::failing_at(failure);
            let result = BuildPiExtension::materialize_with(state.path(), &materializer);
            assert!(result.is_err(), "{failure:?} unexpectedly succeeded");
        }
    }

    #[test]
    fn pi_owns_its_empty_model_catalog() {
        assert!(PiHarness.models().is_empty());
    }

    #[test]
    fn extension_install_rejects_malformed_roots_and_unsafe_owners() {
        assert!(BuildPiExtension::materialize(Path::new("relative-state")).is_err());
        let state = tempfile::tempdir().unwrap();
        let file = state.path().join("not-a-directory");
        std::fs::write(&file, b"x").unwrap();
        assert!(BuildPiExtension::materialize(&file).is_err());
        assert!(private_session_dir(state.path(), "../escape").is_err());

        let symlinked_state = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), symlinked_state.path().join("harness")).unwrap();
        assert!(BuildPiExtension::materialize(symlinked_state.path()).is_err());
        assert!(!outside.path().join("pi").exists());

        let blocked_state = tempfile::tempdir().unwrap();
        let installed = BuildPiExtension::materialize(blocked_state.path()).unwrap();
        std::fs::remove_file(&installed).unwrap();
        std::fs::create_dir(&installed).unwrap();
        assert!(BuildPiExtension::materialize(blocked_state.path()).is_err());
    }

    #[test]
    fn every_pi_thinking_level_maps_directly() {
        let state = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        for effort in EFFORT_LEVELS {
            let spec = PiHarness
                .spec(
                    &ModelChoice {
                        provider: AgentProvider::Pi,
                        model: None,
                        effort: Some(effort.to_string()),
                    },
                    &options("agent-thinking", worktree.path()),
                    &context(state.path()),
                )
                .unwrap();
            assert!(spec
                .args
                .windows(2)
                .any(|pair| pair == ["--thinking", effort]));
        }
    }

    #[test]
    fn pi_respawns_the_same_agent_exactly_and_separates_another_agent() {
        let state = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let first = PiHarness
            .spec(
                &ModelChoice {
                    provider: AgentProvider::Pi,
                    ..ModelChoice::default()
                },
                &options("agent-one", worktree.path()),
                &context(state.path()),
            )
            .unwrap();
        let mut respawn_options = options("agent-one", worktree.path());
        respawn_options.continue_session = false;
        respawn_options.resume_session_id = None;
        let respawn = PiHarness
            .spec(
                &ModelChoice {
                    provider: AgentProvider::Pi,
                    ..ModelChoice::default()
                },
                &respawn_options,
                &context(state.path()),
            )
            .unwrap();
        let another = PiHarness
            .spec(
                &ModelChoice {
                    provider: AgentProvider::Pi,
                    ..ModelChoice::default()
                },
                &options("agent-two", worktree.path()),
                &context(state.path()),
            )
            .unwrap();
        assert_eq!(first.args, respawn.args);
        assert_eq!(first.known_session_id, respawn.known_session_id);
        assert_ne!(first.args[5], another.args[5]);
        assert_ne!(first.args[7], another.args[7]);
        assert_eq!(another.known_session_id.as_deref(), Some("agent-two"));
    }

    #[tokio::test]
    async fn fake_pi_exercises_readiness_delayed_submit_terminal_and_exit() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::Instant;

        use portable_pty::PtySize;

        use crate::harness::{
            open_terminal_session, AgentStatus, SessionIdentitySource, TerminalOpenOptions, Turn,
        };

        let state = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let capture = state.path().join("fake-pi-stdin");
        let fake_pi = state.path().join("pi");
        std::fs::write(
            &fake_pi,
            "#!/bin/sh\nprintf '\\033[?2004h'\ncat > \"$FAKE_PI_CAPTURE\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_pi, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut spec = PiHarness
            .spec(
                &ModelChoice {
                    provider: AgentProvider::Pi,
                    ..ModelChoice::default()
                },
                &options("agent-fake-pi", worktree.path()),
                &context(state.path()),
            )
            .unwrap();
        spec.binary = fake_pi.to_string_lossy().into_owned();
        spec.env.push((
            "FAKE_PI_CAPTURE".to_string(),
            capture.to_string_lossy().into_owned(),
        ));
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let identity = SessionIdentitySource::Known(spec.known_session_id.clone().unwrap());
        let session = open_terminal_session(
            &spec,
            worktree.path().to_path_buf(),
            TerminalOpenOptions {
                size,
                turn_ready_grace: Some(Duration::from_secs(3)),
                identity: Some(identity),
            },
        )
        .unwrap()
        .session;
        assert_eq!(session.session_id().as_deref(), Some("agent-fake-pi"));
        let sent_at = Instant::now();
        session
            .send_turn(&Turn::new("first line\nsecond line"))
            .unwrap();
        assert!(sent_at.elapsed() < Duration::from_millis(200));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let before_submit = std::fs::read_to_string(&capture).unwrap_or_default();
        assert!(
            !before_submit.contains("\u{1b}[201~\n"),
            "the Pi submit key must not arrive with the paste: {before_submit:?}"
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        let captured = loop {
            let captured = std::fs::read_to_string(&capture).unwrap_or_default();
            if captured.contains("\u{1b}[201~\n") {
                break captured;
            }
            assert!(Instant::now() < deadline, "Pi prompt was not submitted");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(sent_at.elapsed() >= PI_TUI_SUBMIT_DELAY);
        assert_eq!(captured, "\u{1b}[200~first line\nsecond line\u{1b}[201~\n");
        let terminal = session.terminal().unwrap();
        terminal
            .resize(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        terminal.write_input(b"human input\r").unwrap();
        session.end();
        assert!(session.exited_within(Duration::from_secs(1)));
        assert!(matches!(session.status(), AgentStatus::Ended { .. }));
    }
}
