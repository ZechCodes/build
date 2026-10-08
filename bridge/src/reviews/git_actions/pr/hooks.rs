//! Preserve the target checkout's hooks around the command's ref fence.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

mod configured;

pub(super) fn install(
    checkout: &Path,
    destination: &Path,
    target: &str,
    expected: &str,
) -> Result<Vec<(String, String)>, String> {
    let hooks = effective_path(checkout)?;
    let environment = original_config_environment()?;
    let original_reference = copy_entries(Path::new(&hooks), destination, &environment)?;
    let delegate = original_reference
        .as_deref()
        .map(quote_path)
        .transpose()?
        .map(|hook| format!("printf '%s' \"$transaction\" | {hook} \"$@\"\n"))
        .unwrap_or_else(|| "exit 0\n".into());
    write_hook(
        &destination.join("reference-transaction"),
        &format!(
            "#!/bin/sh\ntransaction=$(cat && printf x) || exit 1\n\
             transaction=${{transaction%x}}\n\
             if [ \"$1\" = prepared ]; then\n\
             printf '%s' \"$transaction\" | (\n{}\n) || exit 1\nfi\n\
             {environment}{delegate}",
            super::fence_check(target, expected),
        ),
    )?;
    configured::overrides(checkout, destination, &environment)
}

fn effective_path(checkout: &Path) -> Result<String, String> {
    use std::ffi::OsStr;
    let arguments = ["rev-parse", "--path-format=absolute", "--git-path", "hooks"].map(OsStr::new);
    let output = crate::git_process::run_git_unattended(
        checkout,
        &arguments,
        std::time::Duration::from_secs(30),
    )
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(crate::git_process::git_failure(&arguments, &output).to_string());
    }
    let path =
        String::from_utf8(output.stdout).map_err(|_| "PR hook path is not UTF-8".to_string())?;
    Ok(path.strip_suffix('\n').unwrap_or(&path).to_owned())
}

fn copy_entries(
    hooks: &Path,
    destination: &Path,
    environment: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    let entries = match fs::read_dir(hooks) {
        Ok(entries) => entries,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None)
        }
        Err(error) => return Err(error.to_string()),
    };
    let mut original_reference = None;
    for (index, entry) in entries.enumerate() {
        if index >= 128 {
            return Err("PR hook directory has too many entries".into());
        }
        let entry = entry.map_err(|error| error.to_string())?;
        let source = entry.path();
        if entry.file_name() == "reference-transaction" {
            original_reference = executable(&source).then_some(source);
            continue;
        }
        copy_entry(&source, &destination.join(entry.file_name()), environment)?;
    }
    Ok(original_reference)
}

fn copy_entry(source: &Path, destination: &Path, environment: &str) -> Result<(), String> {
    if executable(source) {
        write_hook(
            destination,
            &format!(
                "#!/bin/sh\n{environment}exec {} \"$@\"\n",
                quote_path(source)?
            ),
        )
    } else {
        link_resource(source, destination)
    }
}

fn original_config_environment() -> Result<String, String> {
    match std::env::var("GIT_CONFIG_PARAMETERS") {
        Ok(parameters) => Ok(format!(
            "GIT_CONFIG_PARAMETERS={}\nexport GIT_CONFIG_PARAMETERS\n",
            quote(&parameters)
        )),
        Err(std::env::VarError::NotPresent) => Ok("unset GIT_CONFIG_PARAMETERS\n".into()),
        Err(error) => Err(format!(
            "cannot preserve Git hook config environment: {error}"
        )),
    }
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn quote_path(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(quote)
        .ok_or_else(|| "PR hook path is not UTF-8".into())
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        metadata.is_file() && unsafe { libc::access(path.as_ptr(), libc::X_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

fn link_resource(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, destination).map_err(|error| error.to_string())
    }
    #[cfg(not(unix))]
    {
        if source.is_dir() {
            std::os::windows::fs::symlink_dir(source, destination)
                .map_err(|error| error.to_string())
        } else {
            std::os::windows::fs::symlink_file(source, destination)
                .map_err(|error| error.to_string())
        }
    }
}

fn write_hook(path: &Path, script: &str) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o700);
    }
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    file.write_all(script.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())
}
