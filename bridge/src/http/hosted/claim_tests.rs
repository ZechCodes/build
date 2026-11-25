use super::*;
use base64::Engine;
use serde_json::Value;

const FIXTURE: &str = include_str!("../../../../planning/v2/fixtures/hosted-tunnel-claim.json");

#[test]
fn hosted_claim_proofs_match_issuer_fixture() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let mut identity = crate::identity::generate("hosted test");
    identity.device_id = fixture["device_id"].as_str().unwrap().into();
    identity.identity_private_key_b64 =
        base64::engine::general_purpose::STANDARD.encode((0u8..32).collect::<Vec<_>>());
    identity.identity_public_key_b64 = fixture["public_key_b64"].as_str().unwrap().into();
    let issuer = fixture["issuer"].as_str().unwrap();
    identity.approved_by = Some(issuer.into());
    identity.approved = true;
    let signer = PairedDeviceProof::new(&identity, issuer).unwrap();
    let key = jsonwebtoken::DecodingKey::from_ed_der(
        &base64::engine::general_purpose::STANDARD
            .decode(&identity.identity_public_key_b64)
            .unwrap(),
    );

    for case in fixture["cases"].as_array().unwrap() {
        let expected = &case["proof_claims"];
        let report = Report {
            mode: serde_json::from_value(expected["mode"].clone()).unwrap(),
            hosted_capable: expected["hosted_capable"].as_bool().unwrap(),
        };
        let challenge: Challenge =
            serde_json::from_value(case["challenge_response"]["body"].clone()).unwrap();
        let first = sign_challenge(&signer, report, expected["iat"].as_u64().unwrap() - 1).unwrap();
        let claim = sign_claim(
            &signer,
            report,
            &challenge,
            expected["iat"].as_u64().unwrap(),
        )
        .unwrap();
        let now = expected["iat"].as_u64().unwrap();
        assert_ne!(first, sign_challenge(&signer, report, now - 1).unwrap());
        assert_ne!(claim, sign_claim(&signer, report, &challenge, now).unwrap());
        assert_proof(
            &key,
            &first,
            CHALLENGE_TYPE,
            &case["challenge_proof_claims"],
        );
        assert_proof(&key, &claim, CLAIM_TYPE, expected);
        let reply: Reply = serde_json::from_value(case["response"]["body"].clone()).unwrap();
        reply
            .validate(
                report,
                challenge
                    .validate(expected["iat"].as_u64().unwrap())
                    .unwrap(),
            )
            .unwrap();
    }
}

fn assert_proof(key: &jsonwebtoken::DecodingKey, proof: &str, typ: &str, expected: &Value) {
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::EdDSA);
    validation.validate_exp = false;
    validation.set_audience(&[expected["aud"].as_str().unwrap()]);
    let result = jsonwebtoken::decode::<Value>(proof, key, &validation).unwrap();
    assert_eq!(result.header.typ.as_deref(), Some(typ));
    let mut claims = result.claims;
    let key = claims.as_object_mut().unwrap().remove("jti").unwrap();
    assert_ne!(key, expected["jti"]);
    let mut expected = expected.clone();
    expected.as_object_mut().unwrap().remove("jti");
    assert_eq!(claims, expected);
}

#[test]
fn hosted_claim_rejects_cross_generation_and_unexpected_assignments() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let case = &fixture["cases"][2];
    let read = || serde_json::from_value::<Reply>(case["response"]["body"].clone()).unwrap();
    let report = Report {
        mode: OriginMode::Hosted,
        hosted_capable: true,
    };
    assert!(read().validate(report, Some(1)).is_err());
    assert!(read()
        .validate(
            Report {
                mode: OriginMode::Personal,
                ..report
            },
            Some(2251799813685265)
        )
        .is_err());
}
