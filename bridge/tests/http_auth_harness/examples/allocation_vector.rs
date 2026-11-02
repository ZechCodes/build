//! RFC7515 flattened JSON storage of a compact-JWS public test vector.
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
    let proof = signer.registration_challenge_assertion(1800000000).unwrap();
    let parts: Vec<_> = proof.split('.').collect();
    println!("{}",serde_json::to_string_pretty(&serde_json::json!({"description":"Public fixed test seed byte7 repeated32; no deployed credential. RFC7515 flattened JWS.","identity_public_key_b64":public,"now":1800000000_u64,"protected":parts[0],"payload":parts[1],"signature":parts[2]})).unwrap());
}
