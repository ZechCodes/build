//! A harness's catalog, seen through what its installed CLI said.

use semver::Version;
use serde::Serialize;

use super::{CliReading, ListedModel};
use crate::models::ModelOption;

/// One model the picker offers.
///
/// The wire shape of [`ModelOption`], owned: a CLI that lists its own models
/// can name one Build's catalog has never heard of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OfferedModel {
    pub id: String,
    pub label: String,
    pub supports_effort: bool,
    pub efforts: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

impl From<&ModelOption> for OfferedModel {
    fn from(option: &ModelOption) -> Self {
        OfferedModel {
            id: option.id.to_string(),
            label: option.label.to_string(),
            supports_effort: option.supports_effort,
            efforts: option.efforts.to_vec(),
            context_window: option.context_window,
        }
    }
}

/// A catalogued model the installed CLI is too old to run correctly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnavailableModel {
    pub id: &'static str,
    pub label: &'static str,
    /// The oldest CLI version that runs it.
    pub requires_cli: &'static str,
}

/// What a harness offers on this machine, and what it will start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOffer {
    /// The installed CLI's version, where it said one.
    pub cli_version: Option<Version>,
    /// The picker's models, most capable first.
    pub models: Vec<OfferedModel>,
    /// Catalogued models the installed CLI is too old for.
    pub unavailable: Vec<UnavailableModel>,
    starts: Starts,
}

/// Which models a session may be started on, beyond the picker's.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Starts {
    /// Anything not [`ModelOffer::unavailable`]: an id Build does not know is
    /// the CLI's to judge.
    AllButUnavailable,
    /// Only what the CLI listed, hidden models included.
    Listed(Vec<String>),
}

impl ModelOffer {
    /// The whole catalog: what a harness offers when its CLI is not asked, or
    /// said nothing Build can read.
    pub fn whole(catalog: &[ModelOption], reading: Option<&CliReading>) -> Self {
        ModelOffer {
            cli_version: reading.and_then(|reading| reading.version.clone()),
            models: catalog.iter().map(OfferedModel::from).collect(),
            unavailable: Vec::new(),
            starts: Starts::AllButUnavailable,
        }
    }

    /// The catalog less every model whose `min_cli` is newer than the CLI.
    /// A CLI whose version is unknown is offered the whole catalog.
    pub fn by_version(catalog: &[ModelOption], reading: Option<&CliReading>) -> Self {
        let Some(installed) = reading.and_then(|reading| reading.version.as_ref()) else {
            return Self::whole(catalog, reading);
        };
        let (runs, too_new): (Vec<&ModelOption>, Vec<&ModelOption>) = catalog
            .iter()
            .partition(|option| !needs_newer(option, installed));
        ModelOffer {
            cli_version: Some(installed.clone()),
            models: runs.into_iter().map(OfferedModel::from).collect(),
            unavailable: too_new
                .into_iter()
                .filter_map(|option| {
                    Some(UnavailableModel {
                        id: option.id,
                        label: option.label,
                        requires_cli: option.min_cli?,
                    })
                })
                .collect(),
            starts: Starts::AllButUnavailable,
        }
    }

    /// What the CLI listed, in its order, labelled as the catalog labels a
    /// model it knows. A CLI that listed nothing, or an empty list, is offered
    /// the whole catalog: an empty list would refuse every session.
    pub fn listed(
        catalog: &[ModelOption],
        reading: Option<&CliReading>,
        efforts: &'static [&'static str],
    ) -> Self {
        let Some(listed) = reading
            .and_then(|reading| reading.listed.as_ref())
            .filter(|listed| !listed.is_empty())
        else {
            return Self::whole(catalog, reading);
        };
        ModelOffer {
            cli_version: reading.and_then(|reading| reading.version.clone()),
            models: listed
                .iter()
                .filter(|model| !model.hidden)
                .map(|model| offered_from_list(model, catalog, efforts))
                .collect(),
            unavailable: Vec::new(),
            starts: Starts::Listed(listed.iter().map(|model| model.id.clone()).collect()),
        }
    }

    /// Why a session on `model` must not be started, in a sentence for the
    /// person who chose it; `None` when it may. `cli` is what they call the
    /// program: "Claude Code", "Codex".
    pub fn refusal(&self, model: &str, cli: &str) -> Option<String> {
        if let Some(too_new) = self.unavailable.iter().find(|entry| names(entry.id, model)) {
            let installed = self
                .cli_version
                .as_ref()
                .map(Version::to_string)
                .unwrap_or_default();
            return Some(format!(
                "Build cannot start {label} here: {cli} {installed} is installed, and {label} needs {requires} or newer. An older {cli} refuses it or runs it with too small a context window. Update {cli}, or choose another model.",
                label = too_new.label,
                requires = too_new.requires_cli,
            ));
        }
        match &self.starts {
            Starts::Listed(ids) if !ids.iter().any(|id| id == model) => {
                let installed = match &self.cli_version {
                    Some(version) => format!("{cli} {version}"),
                    None => format!("The {cli} on this machine"),
                };
                Some(format!(
                    "Build cannot start {model} here: {installed} does not offer it. Choose another model."
                ))
            }
            _ => None,
        }
    }
}

/// Whether `option` needs a newer CLI than `installed`. A catalog entry with
/// no minimum, or one that does not parse, gates nothing.
fn needs_newer(option: &ModelOption, installed: &Version) -> bool {
    option
        .min_cli
        .and_then(|minimum| Version::parse(minimum).ok())
        .is_some_and(|minimum| *installed < minimum)
}

fn offered_from_list(
    model: &ListedModel,
    catalog: &[ModelOption],
    harness_efforts: &'static [&'static str],
) -> OfferedModel {
    let known = catalog.iter().find(|option| option.id == model.id);
    // Only efforts the harness itself can pass: an effort a CLI lists that
    // Build has no flag for is one no selection could carry.
    let efforts: Vec<&'static str> = harness_efforts
        .iter()
        .copied()
        .filter(|effort| model.efforts.iter().any(|listed| listed == effort))
        .collect();
    OfferedModel {
        id: model.id.clone(),
        label: known.map_or_else(|| model.label.clone(), |option| option.label.to_string()),
        supports_effort: !efforts.is_empty(),
        efforts,
        context_window: known.and_then(|option| option.context_window),
    }
}

/// Whether `model` is the catalog's `id`, or `id` with a release date after
/// it (`claude-haiku-4-5-20251001`).
fn names(id: &str, model: &str) -> bool {
    match model.strip_prefix(id) {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix('-')
            .is_some_and(|date| date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit())),
        None => false,
    }
}

#[cfg(test)]
mod tests;
