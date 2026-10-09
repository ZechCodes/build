//! Pi's credential context: one entry per provider, never one invented "Pi
//! account".
//!
//! Read-only metadata, like Pi's own `ReadOnlyAuthStorage.list()`: the
//! providers `auth.json` (under `PI_CODING_AGENT_DIR`) holds, each with its
//! kind of credential, and the providers whose documented variable is set.
//! Pi is never run to resolve one: its key resolution can run a configured
//! `!command`, so such a key reads as managed on this device, status unknown.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer};

use super::{oauth_status, read_json, summarise, AuthAdapter, ObservationFailed, Present};
use crate::harness::inventory::environment::DeviceEnvironment;
use crate::harness::inventory::model::{
    AuthFacts, AuthMethod, AuthStatus, Evidence, ProviderAuth, Verification,
};

pub struct PiAuth;

pub static PI_AUTH: PiAuth = PiAuth;

/// Pi 0.86.1's documented provider variables (`docs/providers.md`), by the
/// `auth.json` key each stands in for.
const PROVIDER_VARIABLES: [(&str, &str); 36] = [
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("ant-ling", "ANT_LING_API_KEY"),
    ("azure-openai-responses", "AZURE_OPENAI_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
    ("deepseek", "DEEPSEEK_API_KEY"),
    ("nvidia", "NVIDIA_API_KEY"),
    ("google", "GEMINI_API_KEY"),
    ("amazon-bedrock", "AWS_BEARER_TOKEN_BEDROCK"),
    ("mistral", "MISTRAL_API_KEY"),
    ("groq", "GROQ_API_KEY"),
    ("cerebras", "CEREBRAS_API_KEY"),
    ("cloudflare-ai-gateway", "CLOUDFLARE_API_KEY"),
    ("cloudflare-workers-ai", "CLOUDFLARE_API_KEY"),
    ("xai", "XAI_API_KEY"),
    ("openrouter", "OPENROUTER_API_KEY"),
    ("vercel-ai-gateway", "AI_GATEWAY_API_KEY"),
    ("zai", "ZAI_API_KEY"),
    ("zai-coding-cn", "ZAI_CODING_CN_API_KEY"),
    ("opencode", "OPENCODE_API_KEY"),
    ("opencode-go", "OPENCODE_API_KEY"),
    ("radius", "RADIUS_API_KEY"),
    ("huggingface", "HF_TOKEN"),
    ("fireworks", "FIREWORKS_API_KEY"),
    ("together", "TOGETHER_API_KEY"),
    ("baseten", "BASETEN_API_KEY"),
    ("kimi-coding", "KIMI_API_KEY"),
    ("meta", "META_API_KEY"),
    ("minimax", "MINIMAX_API_KEY"),
    ("minimax-cn", "MINIMAX_CN_API_KEY"),
    ("qwen-token-plan", "QWEN_TOKEN_PLAN_API_KEY"),
    ("qwen-token-plan-individual", "QWEN_TOKEN_PLAN_API_KEY"),
    ("qwen-token-plan-cn", "QWEN_TOKEN_PLAN_CN_API_KEY"),
    ("xiaomi", "XIAOMI_API_KEY"),
    ("xiaomi-token-plan-cn", "XIAOMI_TOKEN_PLAN_CN_API_KEY"),
    ("xiaomi-token-plan-ams", "XIAOMI_TOKEN_PLAN_AMS_API_KEY"),
    ("xiaomi-token-plan-sgp", "XIAOMI_TOKEN_PLAN_SGP_API_KEY"),
];

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Credential {
    Oauth {
        #[serde(default)]
        refresh: Present,
        #[serde(default)]
        access: Present,
        #[serde(default)]
        expires: Option<u64>,
    },
    ApiKey {
        #[serde(default)]
        key: KeyKind,
    },
    #[serde(other)]
    Other,
}

/// What an API key value is, decided from its first characters and then
/// forgotten (`docs/providers.md`, Key Resolution).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum KeyKind {
    Literal,
    /// `$VAR` or `${VAR}`, resolved by Pi from its environment.
    Interpolated,
    /// `!command`, run by Pi; never by Build.
    Command,
    #[default]
    Missing,
}

impl KeyKind {
    /// Read in place, without copying the value anywhere.
    fn of(value: &str) -> KeyKind {
        if super::is_blank(value) {
            return KeyKind::Missing;
        }
        if value.starts_with('!') {
            return KeyKind::Command;
        }
        let bytes = value.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            match (bytes[at], bytes.get(at + 1)) {
                (b'$', Some(b'$' | b'!')) => at += 2,
                (b'$', _) => return KeyKind::Interpolated,
                _ => at += 1,
            }
        }
        KeyKind::Literal
    }
}

impl<'de> Deserialize<'de> for KeyKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Kind;
        impl<'de> Visitor<'de> for Kind {
            type Value = KeyKind;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an API key")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<KeyKind, E> {
                Ok(KeyKind::of(value))
            }
            fn visit_unit<E: de::Error>(self) -> Result<KeyKind, E> {
                Ok(KeyKind::Missing)
            }
            // Only a string is a key; any other value holds none.
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<KeyKind, E> {
                Ok(KeyKind::Missing)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<KeyKind, E> {
                Ok(KeyKind::Missing)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<KeyKind, E> {
                Ok(KeyKind::Missing)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<KeyKind, E> {
                Ok(KeyKind::Missing)
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<KeyKind, A::Error> {
                while seq.next_element::<de::IgnoredAny>()?.is_some() {}
                Ok(KeyKind::Missing)
            }
            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<KeyKind, A::Error> {
                while map
                    .next_entry::<de::IgnoredAny, de::IgnoredAny>()?
                    .is_some()
                {}
                Ok(KeyKind::Missing)
            }
        }
        deserializer.deserialize_any(Kind)
    }
}

/// A provider id Build repeats on the wire: short, lowercase, and plain.
fn is_provider_id(id: &str) -> bool {
    id.len() <= 64
        && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
}

fn provider_from(id: &str, credential: &Credential, now: SystemTime) -> Option<ProviderAuth> {
    let (method, status) = match credential {
        Credential::Oauth {
            refresh,
            access,
            expires,
        } if access.0 || refresh.0 => (AuthMethod::Oauth, oauth_status(*expires, refresh.0, now)),
        // OAuth with neither token holds nothing.
        Credential::Oauth { .. } => return None,
        Credential::ApiKey { key } => match key {
            KeyKind::Literal => (AuthMethod::ApiKey, AuthStatus::SignedIn),
            KeyKind::Interpolated => (AuthMethod::ApiKey, AuthStatus::Unknown),
            KeyKind::Command => (AuthMethod::External, AuthStatus::Unknown),
            KeyKind::Missing => return None,
        },
        Credential::Other => (AuthMethod::Unknown, AuthStatus::Unknown),
    };
    Some(ProviderAuth {
        id: id.to_string(),
        method,
        status,
        evidence: vec![Evidence::CredentialsFile],
    })
}

fn agent_dir(environment: &DeviceEnvironment) -> PathBuf {
    environment.dir_or_home("PI_CODING_AGENT_DIR", ".pi/agent")
}

impl AuthAdapter for PiAuth {
    fn context(&self) -> &'static str {
        "pi"
    }

    fn watched(&self, environment: &DeviceEnvironment) -> Vec<PathBuf> {
        vec![agent_dir(environment).join("auth.json")]
    }

    fn observe(
        &self,
        environment: &DeviceEnvironment,
        now: SystemTime,
    ) -> Result<AuthFacts, ObservationFailed> {
        let saved: BTreeMap<String, Credential> =
            read_json(&agent_dir(environment).join("auth.json"))?.unwrap_or_default();
        let mut providers: BTreeMap<&str, ProviderAuth> = saved
            .iter()
            .filter(|(id, _)| is_provider_id(id))
            .filter_map(|(id, credential)| Some((id.as_str(), provider_from(id, credential, now)?)))
            .collect();
        // The auth file wins over the environment, provider by provider.
        for (id, variable) in PROVIDER_VARIABLES {
            if environment.has(variable) && !providers.contains_key(id) {
                providers.insert(
                    id,
                    ProviderAuth {
                        id: id.to_string(),
                        method: AuthMethod::ApiKey,
                        status: AuthStatus::SignedIn,
                        evidence: vec![Evidence::Environment],
                    },
                );
            }
        }
        let providers: Vec<ProviderAuth> = providers.into_values().collect();
        let parts: Vec<_> = providers.iter().map(|p| (p.method, p.status)).collect();
        let (method, status) = summarise(&parts);
        let mut evidence: Vec<Evidence> = providers
            .iter()
            .flat_map(|provider| provider.evidence.iter().copied())
            .collect();
        evidence.sort();
        evidence.dedup();
        if evidence.is_empty() {
            evidence.push(Evidence::CredentialsFile);
        }
        Ok(AuthFacts {
            method,
            status,
            verification: Verification::SavedConfiguration,
            evidence,
            providers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_classified_by_its_first_characters_only() {
        assert_eq!(KeyKind::of("sk-ant-x"), KeyKind::Literal);
        assert_eq!(KeyKind::of("$MY_KEY"), KeyKind::Interpolated);
        assert_eq!(KeyKind::of("${A}_${B}"), KeyKind::Interpolated);
        assert_eq!(KeyKind::of("!op read x"), KeyKind::Command);
        assert_eq!(KeyKind::of("$$literal"), KeyKind::Literal);
        assert_eq!(KeyKind::of("$!literal"), KeyKind::Literal);
        assert_eq!(KeyKind::of(""), KeyKind::Missing);
        assert_eq!(KeyKind::of("  "), KeyKind::Missing);
    }

    #[test]
    fn provider_ids_are_plain() {
        assert!(is_provider_id("qwen-token-plan-cn"));
        assert!(!is_provider_id("Evil<script>"));
        assert!(!is_provider_id(&"a".repeat(65)));
    }
}
