//! Reproducible fixtures only. The fixed seed is public test material.
use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use base64::{engine::general_purpose::STANDARD, Engine};
use bridge_http_auth_harness::{
    discovery::{CanonicalOrigin, DeviceProofSigner},
    identity::StoredIdentity,
};
fn main() {
    let pair = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
    let public = STANDARD.encode(pair.public_key().as_ref());
    let id:StoredIdentity=serde_json::from_value(serde_json::json!({"device_id":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","name":"public fixture","transport":{"private_key_b64":"","public_key_b64":""},"identity_private_key_b64":STANDARD.encode([7;32]),"identity_public_key_b64":public,"approved":true,"approved_by":"https://getbuild.ing"})).unwrap();
    let signer = DeviceProofSigner::new(
        &id,
        "https://getbuild.ing",
        CanonicalOrigin::parse("https://bridge.example").unwrap(),
        "epoch-1".into(),
    )
    .unwrap();
    println!("{}",serde_json::to_string_pretty(&serde_json::json!({
        "description":"Public test key, seed byte 7 repeated 32 times. Never a deployed identity.",
        "identity_public_key_b64":public,
        "now":1800000000_u64,
        "registration":signer.registration_proof("fixture-challenge","fixture-nonce",None,true,1800000000).unwrap(),
        "discovery":signer.discovery_proof("fixture-challenge","fixture-nonce",1800000000).unwrap(),
        "introspection":signer.introspection_assertion(&"a".repeat(64),1800000000).unwrap()
    })).unwrap());
}
