//! Compiles the production verifier modules while consolidated runtime wiring
//! is owned by #911. The identity fixture matches the persisted key fields only.
pub mod config {
    pub const DEFAULT_API_URL: &str = "https://getbuild.ing";
}
pub mod identity {
    #[derive(serde::Deserialize)]
    pub struct StoredIdentity {
        pub device_id: String,
        pub identity_private_key_b64: String,
        pub identity_public_key_b64: String,
        pub approved: bool,
        pub approved_by: Option<String>,
    }
}
#[path = "../../../src/http/discovery.rs"]
pub mod discovery;

#[path = "../../../src/http/auth.rs"]
pub mod auth;
