//! Branch-specific remote configuration preserved under owned config locks.

use super::super::model::ReviewBranchBinding;
use super::super::receivers::{git, validate_binding_receiver, with_local_config_locked};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub fn choose_remote_name(binding: &ReviewBranchBinding) -> Result<String, String> {
    choose_remote_name_avoiding(binding, &BTreeSet::new())
}

/// Reserve aliases while planning multiple directories that share Git config.
pub fn choose_remote_name_avoiding(
    binding: &ReviewBranchBinding,
    reserved: &BTreeSet<String>,
) -> Result<String, String> {
    let repository =
        git2::Repository::open(&binding.working_repository).map_err(|error| error.to_string())?;
    let config = repository.config().map_err(|error| error.to_string())?;
    let suffix = format!(
        "{:x}",
        Sha256::digest(binding.dedicated_branch_ref.as_bytes())
    );
    for index in 0..1000 {
        let alias = match index {
            0 => "build-review".into(),
            1 => format!("build-review-{}", &suffix[..12]),
            _ => format!("build-review-{}-{index}", &suffix[..12]),
        };
        if !reserved.contains(&alias)
            && (remote_keys(&config, &alias)?.is_empty()
                || remote_matches(&config, binding, &alias)?)
        {
            return Ok(alias);
        }
    }
    Err("could not allocate a local review remote alias".into())
}

pub fn configure_remote(binding: &ReviewBranchBinding) -> Result<(), String> {
    validate_binding_receiver(binding)?;
    let expected = configuration(binding)?;
    // Recheck and replace all owned entries while Git's local config is locked.
    with_local_config_locked(&binding.working_repository, |config, local| {
        validate_configuration(config, binding, &expected)?;
        for (key, value) in &expected {
            local
                .set_str(key, value)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    })?;
    validate_push_destination(binding)
}

pub fn cleanup_remote(binding: &ReviewBranchBinding) -> Result<(), String> {
    let expected = configuration(binding)?;
    with_local_config_locked(&binding.working_repository, |config, local| {
        validate_configuration(config, binding, &expected)?;
        for (key, value) in expected.iter().rev() {
            if config_values(local, key)? == [value.as_str()] {
                local.remove(key).map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    })
}

pub(super) fn short_branch(binding: &ReviewBranchBinding) -> Result<&str, String> {
    binding
        .dedicated_branch_ref
        .strip_prefix("refs/heads/")
        .filter(|_| git2::Reference::is_valid_name(&binding.dedicated_branch_ref))
        .ok_or_else(|| "invalid dedicated review branch".into())
}

pub(super) fn configuration(
    binding: &ReviewBranchBinding,
) -> Result<Vec<(String, String)>, String> {
    let branch = short_branch(binding)?;
    if !git2::Reference::is_valid_name(&binding.receiving_ref)
        || !binding.receiving_ref.starts_with("refs/heads/")
    {
        return Err("invalid review receiving branch".into());
    }
    if binding.remote_name.is_empty()
        || !binding
            .remote_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("invalid local review remote alias".into());
    }
    let prefix = format!("remote.{}", binding.remote_name);
    let url = binding
        .receiving_repository
        .to_str()
        .ok_or("review receiver path is not UTF-8")?;
    Ok(vec![
        (
            format!("{prefix}.buildreviewrepository"),
            binding.repository_id.clone(),
        ),
        (
            format!("{prefix}.buildreviewbranch"),
            binding.dedicated_branch_ref.clone(),
        ),
        (format!("{prefix}.url"), url.into()),
        (format!("{prefix}.pushurl"), url.into()),
        (
            format!("{prefix}.fetch"),
            format!(
                "+{}:refs/remotes/{}/{}",
                binding.receiving_ref, binding.remote_name, branch
            ),
        ),
        (
            format!("{prefix}.push"),
            format!("{}:{}", binding.dedicated_branch_ref, binding.receiving_ref),
        ),
        (
            format!("branch.{branch}.remote"),
            binding.remote_name.clone(),
        ),
        (
            format!("branch.{branch}.merge"),
            binding.receiving_ref.clone(),
        ),
        (
            format!("branch.{branch}.pushremote"),
            binding.remote_name.clone(),
        ),
    ])
}

pub(super) fn config_values(config: &git2::Config, key: &str) -> Result<Vec<String>, String> {
    let mut entries = match config.multivar(key, None) {
        Ok(entries) => entries,
        Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut values = Vec::new();
    while let Some(entry) = entries.next() {
        values.push(
            entry
                .map_err(|error| error.to_string())?
                .value()
                .ok_or("review Git configuration is not UTF-8")?
                .into(),
        );
    }
    Ok(values)
}

fn remote_keys(config: &git2::Config, alias: &str) -> Result<Vec<String>, String> {
    let prefix = format!("remote.{alias}.");
    let mut entries = config.entries(None).map_err(|error| error.to_string())?;
    let mut keys = Vec::new();
    while let Some(entry) = entries.next() {
        if let Some(name) = entry
            .map_err(|error| error.to_string())?
            .name()
            .filter(|name| name.starts_with(&prefix))
        {
            keys.push(name.to_owned());
        }
    }
    Ok(keys)
}

fn remote_matches(
    config: &git2::Config,
    binding: &ReviewBranchBinding,
    alias: &str,
) -> Result<bool, String> {
    let url = binding
        .receiving_repository
        .to_str()
        .ok_or("review receiver path is not UTF-8")?;
    let prefix = format!("remote.{alias}");
    Ok(config_values(config, &format!("{prefix}.url"))? == [url]
        && config_values(config, &format!("{prefix}.pushurl"))? == [url]
        && config_values(config, &format!("{prefix}.buildreviewrepository"))?
            == [binding.repository_id.as_str()]
        && config_values(config, &format!("{prefix}.buildreviewbranch"))?
            == [binding.dedicated_branch_ref.as_str()])
}

fn validate_configuration(
    config: &git2::Config,
    binding: &ReviewBranchBinding,
    expected: &[(String, String)],
) -> Result<(), String> {
    let keys = remote_keys(config, &binding.remote_name)?;
    if !keys.is_empty()
        && config_values(
            config,
            &format!("remote.{}.buildreviewrepository", binding.remote_name),
        )? != [binding.repository_id.as_str()]
    {
        return Err("local review remote ownership changed".into());
    }
    if keys
        .iter()
        .any(|key| !expected.iter().any(|(owned, _)| owned == key))
    {
        return Err("local review remote configuration changed".into());
    }
    for (key, expected) in expected {
        let values = config_values(config, key)?;
        if !values.is_empty() && values != [expected.as_str()] {
            return Err(format!("local review Git configuration changed: {key}"));
        }
    }
    Ok(())
}

pub(super) fn validate_push_destination(binding: &ReviewBranchBinding) -> Result<(), String> {
    let destinations = git(
        &binding.working_repository,
        &["remote", "get-url", "--push", "--all", &binding.remote_name],
    )?;
    let expected = binding
        .receiving_repository
        .to_str()
        .ok_or("review receiver path is not UTF-8")?;
    if destinations.lines().collect::<Vec<_>>() != [expected] {
        return Err("local review remote push destination changed".into());
    }
    Ok(())
}
