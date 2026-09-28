use super::*;

const EFFORTS: [&str; 3] = ["low", "high", "max"];

fn option(id: &'static str, label: &'static str, min_cli: Option<&'static str>) -> ModelOption {
    ModelOption {
        id,
        label,
        supports_effort: true,
        efforts: &EFFORTS,
        context_window: Some(1_000_000),
        min_cli,
    }
}

fn catalog() -> Vec<ModelOption> {
    vec![
        option("claude-sonnet-5-5", "Claude Sonnet 5.5", Some("2.1.284")),
        option("claude-opus-5-5", "Claude Opus 5.5", Some("2.1.280")),
        option("claude-opus-5", "Claude Opus 5", Some("2.1.219")),
        option("claude-haiku-4-5", "Claude Haiku 4.5", Some("2.0.17")),
    ]
}

fn at(version: &str) -> CliReading {
    CliReading {
        version: Some(Version::parse(version).unwrap()),
        listed: None,
    }
}

fn ids(offer: &ModelOffer) -> Vec<&str> {
    offer.models.iter().map(|model| model.id.as_str()).collect()
}

#[test]
fn a_cli_older_than_a_model_s_minimum_is_not_offered_it() {
    let offer = ModelOffer::by_version(&catalog(), Some(&at("2.1.280")));

    assert_eq!(
        ids(&offer),
        ["claude-opus-5-5", "claude-opus-5", "claude-haiku-4-5"]
    );
    assert_eq!(
        offer.unavailable,
        [UnavailableModel {
            id: "claude-sonnet-5-5",
            label: "Claude Sonnet 5.5",
            requires_cli: "2.1.284",
        }]
    );
    assert_eq!(offer.cli_version, Some(Version::parse("2.1.280").unwrap()));
}

#[test]
fn a_minimum_is_inclusive() {
    let exactly = ModelOffer::by_version(&catalog(), Some(&at("2.1.284")));
    let one_below = ModelOffer::by_version(&catalog(), Some(&at("2.1.283")));
    let one_above = ModelOffer::by_version(&catalog(), Some(&at("2.1.285")));

    assert!(ids(&exactly).contains(&"claude-sonnet-5-5"));
    assert!(!ids(&one_below).contains(&"claude-sonnet-5-5"));
    assert!(ids(&one_above).contains(&"claude-sonnet-5-5"));
}

#[test]
fn a_prerelease_of_the_minimum_is_older_than_it() {
    let offer = ModelOffer::by_version(&catalog(), Some(&at("2.1.284-beta.1")));

    assert!(!ids(&offer).contains(&"claude-sonnet-5-5"));
}

#[test]
fn an_unread_version_offers_the_whole_catalog() {
    for reading in [None, Some(&CliReading::default())] {
        let offer = ModelOffer::by_version(&catalog(), reading);

        assert_eq!(offer.models.len(), catalog().len());
        assert!(offer.unavailable.is_empty());
        assert_eq!(offer.refusal("claude-sonnet-5-5", "Claude Code"), None);
    }
}

#[test]
fn a_model_with_no_minimum_is_never_gated() {
    let catalog = [option("claude-next", "Claude Next", None)];

    let offer = ModelOffer::by_version(&catalog, Some(&at("0.0.1")));

    assert_eq!(ids(&offer), ["claude-next"]);
}

#[test]
fn a_too_new_model_is_refused_with_why_and_what_to_do() {
    let offer = ModelOffer::by_version(&catalog(), Some(&at("2.1.280")));

    assert_eq!(
        offer.refusal("claude-sonnet-5-5", "Claude Code").as_deref(),
        Some("Build cannot start Claude Sonnet 5.5 here: Claude Code 2.1.280 is installed, and Claude Sonnet 5.5 needs 2.1.284 or newer. An older Claude Code refuses it or runs it with too small a context window. Update Claude Code, or choose another model.")
    );
    assert_eq!(offer.refusal("claude-opus-5-5", "Claude Code"), None);
}

#[test]
fn a_dated_id_is_its_catalog_model() {
    let offer = ModelOffer::by_version(&catalog(), Some(&at("2.1.280")));

    assert!(offer
        .refusal("claude-sonnet-5-5-20260901", "Claude Code")
        .is_some());
    assert_eq!(
        offer.refusal("claude-sonnet-5-5-beta", "Claude Code"),
        None,
        "only an eight-digit date names the same model"
    );
}

#[test]
fn an_id_the_catalog_does_not_know_is_the_cli_s_to_judge() {
    let offer = ModelOffer::by_version(&catalog(), Some(&at("2.1.280")));

    assert_eq!(offer.refusal("claude-opus-6", "Claude Code"), None);
}

fn listed(id: &str, label: &str, hidden: bool, efforts: &[&str]) -> ListedModel {
    ListedModel {
        id: id.into(),
        label: label.into(),
        hidden,
        efforts: efforts.iter().map(|effort| effort.to_string()).collect(),
    }
}

fn codex_catalog() -> Vec<ModelOption> {
    vec![
        ModelOption {
            label: "GPT-6-Sol (Build's label)",
            context_window: Some(272_000),
            ..option("gpt-6-sol", "", None)
        },
        option("gpt-5.2", "GPT-5.2", None),
    ]
}

fn codex_reading() -> CliReading {
    CliReading {
        version: Some(Version::parse("0.155.1").unwrap()),
        listed: Some(vec![
            listed("gpt-7", "GPT-7", false, &["low", "ultra", "max"]),
            listed("gpt-6-sol", "GPT-6-Sol", false, &["low", "high"]),
            listed("gpt-reserve", "GPT-Reserve", true, &["low"]),
            listed("gpt-mini", "GPT-Mini", false, &[]),
        ]),
    }
}

#[test]
fn a_listing_cli_is_offered_what_it_lists_and_nothing_else() {
    let offer = ModelOffer::listed(&codex_catalog(), Some(&codex_reading()), &EFFORTS);

    assert_eq!(ids(&offer), ["gpt-7", "gpt-6-sol", "gpt-mini"]);
    assert!(offer.unavailable.is_empty());
}

#[test]
fn a_listed_model_keeps_the_catalog_s_words_where_it_has_them() {
    let offer = ModelOffer::listed(&codex_catalog(), Some(&codex_reading()), &EFFORTS);

    assert_eq!(
        offer.models[1],
        OfferedModel {
            id: "gpt-6-sol".into(),
            label: "GPT-6-Sol (Build's label)".into(),
            supports_effort: true,
            efforts: vec!["low", "high"],
            context_window: Some(272_000),
        }
    );
    assert_eq!(
        offer.models[0],
        OfferedModel {
            id: "gpt-7".into(),
            label: "GPT-7".into(),
            supports_effort: true,
            efforts: vec!["low", "max"],
            context_window: None,
        },
        "a new model arrives with the CLI's label, and only efforts Build can pass"
    );
    assert!(!offer.models[2].supports_effort);
}

#[test]
fn a_listing_cli_starts_hidden_models_and_refuses_unlisted_ones() {
    let offer = ModelOffer::listed(&codex_catalog(), Some(&codex_reading()), &EFFORTS);

    assert_eq!(offer.refusal("gpt-reserve", "Codex"), None);
    assert_eq!(offer.refusal("gpt-7", "Codex"), None);
    assert_eq!(
        offer.refusal("gpt-5.2", "Codex").as_deref(),
        Some("Build cannot start gpt-5.2 here: Codex 0.155.1 does not offer it. Choose another model.")
    );
}

#[test]
fn a_cli_that_listed_nothing_is_offered_the_whole_catalog() {
    let version_only = CliReading {
        version: Some(Version::parse("0.150.0").unwrap()),
        listed: None,
    };

    let offer = ModelOffer::listed(&codex_catalog(), Some(&version_only), &EFFORTS);

    assert_eq!(ids(&offer), ["gpt-6-sol", "gpt-5.2"]);
    assert_eq!(offer.refusal("gpt-5.2", "Codex"), None);
    assert_eq!(offer.cli_version, version_only.version);
}

#[test]
fn the_whole_catalog_says_the_version_it_read() {
    let offer = ModelOffer::whole(&catalog(), Some(&at("0.86.1")));

    assert_eq!(offer.cli_version, Some(Version::parse("0.86.1").unwrap()));
    assert_eq!(offer.models.len(), 4);
}
