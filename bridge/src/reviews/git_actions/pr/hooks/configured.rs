//! Preserve configured hook commands under the PR command's hook overlay.
use crate::git_process::{git_failure, run_git_unattended};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

const MAX_COMMANDS: usize = 128;
const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const SHELL_SPECIAL: &str = "|&;<>()$`\\\"' \t\n*?[#~=%";

pub(super) fn overrides(
    checkout: &Path,
    destination: &Path,
    environment: &str,
) -> Result<Vec<(String, String)>, String> {
    if !supports_commands(checkout)? {
        return Ok(Vec::new());
    }
    configured_overrides(checkout, destination, environment)
}

fn configured_overrides(
    checkout: &Path,
    destination: &Path,
    environment: &str,
) -> Result<Vec<(String, String)>, String> {
    let commands = commands(checkout)?;
    let shell = if commands.values().any(|command| needs_shell(command)) {
        Some(shell_path(checkout)?)
    } else {
        None
    };
    let mut overrides = Vec::new();
    for (key, command) in commands {
        let shim = destination.join(format!(
            "build-review-configured-hook-{}",
            uuid::Uuid::new_v4()
        ));
        super::write_hook(&shim, &script(&command, environment, shell.as_deref())?)?;
        overrides.push((key, super::quote_path(&shim)?));
    }
    Ok(overrides)
}

fn commands(checkout: &Path) -> Result<BTreeMap<String, String>, String> {
    let args = ["config", "--null", "--get-regexp", "^hook\\..*\\.command$"];
    let output = git_output(checkout, &args)?;
    if output.status.code() == Some(1) && output.stdout.is_empty() && output.stderr.is_empty() {
        return Ok(BTreeMap::new());
    }
    require_success(&args, &output)?;
    if output.stdout.len() > MAX_CONFIG_BYTES {
        return Err("configured Git hook commands exceed their bounded capacity".into());
    }
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| "configured Git hook commands are not UTF-8")?;
    let text = text
        .strip_suffix('\0')
        .ok_or("configured Git hook command output is not NUL terminated")?;
    let mut commands = BTreeMap::new();
    for record in text.split('\0') {
        let (key, command) = record
            .split_once('\n')
            .ok_or("configured Git hook command output has no key separator")?;
        validate_key(key)?;
        commands.insert(key.into(), command.into());
        if commands.len() > MAX_COMMANDS {
            return Err("too many configured Git hook commands".into());
        }
    }
    Ok(commands)
}

fn validate_key(key: &str) -> Result<(), String> {
    let name = key
        .strip_prefix("hook.")
        .and_then(|key| key.strip_suffix(".command"))
        .filter(|name| !name.is_empty());
    if name.is_none() || key.contains('=') || key.chars().any(char::is_control) {
        return Err("configured Git hook command key cannot be preserved safely".into());
    }
    Ok(())
}

fn git_output(checkout: &Path, args: &[&str]) -> Result<Output, String> {
    let args = args.iter().map(OsStr::new).collect::<Vec<_>>();
    run_git_unattended(checkout, &args, Duration::from_secs(30)).map_err(|error| error.to_string())
}

fn require_success(args: &[&str], output: &Output) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    let args = args.iter().map(OsStr::new).collect::<Vec<_>>();
    Err(git_failure(&args, output).to_string())
}

fn shell_path(checkout: &Path) -> Result<String, String> {
    let args = ["var", "GIT_SHELL_PATH"];
    let output = git_output(checkout, &args)?;
    require_success(&args, &output)?;
    let path = std::str::from_utf8(&output.stdout)
        .map_err(|_| "configured Git hook shell path is not UTF-8")?
        .strip_suffix('\n')
        .ok_or("configured Git hook shell path has no line terminator")?;
    if path.is_empty() || path.contains('\0') {
        return Err("configured Git hook shell path is invalid".into());
    }
    Ok(path.into())
}

fn needs_shell(command: &str) -> bool {
    command
        .chars()
        .any(|character| SHELL_SPECIAL.contains(character))
}

fn supports_commands(checkout: &Path) -> Result<bool, String> {
    let args = ["--version"];
    let output = git_output(checkout, &args)?;
    require_success(&args, &output)?;
    let version = std::str::from_utf8(&output.stdout)
        .map_err(|_| "configured Git hook version is not UTF-8")?;
    version_supports_commands(version)
}

fn version_supports_commands(version: &str) -> Result<bool, String> {
    let version = version
        .strip_prefix("git version ")
        .and_then(|version| version.split_whitespace().next())
        .ok_or("cannot identify native Git configured-hook support")?;
    let mut components = version.split('.');
    let parse = |component: Option<&str>| {
        component
            .and_then(|component| component.parse::<u32>().ok())
            .ok_or_else(|| "cannot identify native Git configured-hook support".to_string())
    };
    let major = parse(components.next())?;
    let minor = parse(components.next())?;
    // Git v2.54 introduced hook.<name>.command and its native dispatch.
    Ok((major, minor) >= (2, 54))
}

fn script(command: &str, environment: &str, shell: Option<&str>) -> Result<String, String> {
    let use_shell = needs_shell(command);
    let command = super::quote(command);
    if !use_shell {
        return Ok(format!("#!/bin/sh\n{environment}exec {command} \"$@\"\n"));
    }
    let shell = super::quote(shell.ok_or("configured Git hook has no shell interpreter")?);
    // Mirror Git's prepare_shell_cmd: only append "$@" when hook arguments
    // exist, and retain the original command as the shell's positional zero.
    Ok(format!(
        "#!/bin/sh\n{environment}build_hook_command={command}\n\
         if [ \"$#\" -eq 0 ]; then\n\
         exec {shell} -c \"$build_hook_command\" \"$build_hook_command\"\nfi\n\
         exec {shell} -c \"$build_hook_command \\\"\\$@\\\"\" \"$build_hook_command\" \"$@\"\n"
    ))
}

#[cfg(test)]
mod tests;
