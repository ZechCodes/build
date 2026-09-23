//! End-to-end signature fixtures from Sigstore cosign v3.1.3 release assets.
//!
//! Source: https://github.com/sigstore/cosign/releases/tag/v3.1.3
//! `cosign_checksums.txt` SHA-256 aec2a6f68d307b09ae196e388dc691a146fa8bdba7fcce9ca4ca41b918adfa63
//! `cosign_checksums.txt.sigstore.json` SHA-256 976bcb216e45ed0274e464e2e16d81e84cc85a69b3ed6e3488c1e7cda116379a
//! The fixture is signed by Sigstore's documented release identity. Its pinned
//! certificate lets the offline test verify actual ECDSA bytes without fetching
//! Sigstore trust roots. That pin is only a test fixture; production calls the
//! SHA-256-pinned cosign tool with Build's exact workflow identity and GitHub
//! issuer, so the fixture cannot authorize a bridge release.

use super::*;
use crate::update::{UpdateConfig, UpdateService, UpdateState};
use base64::Engine as _;
use ring::signature::{
    Ed25519KeyPair, KeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_ASN1, ED25519,
};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

const SIGNER: &str = "keyless@projectsigstore.iam.gserviceaccount.com";
const ISSUER: &str = "https://accounts.google.com";
const SIGNED_CHECKSUMS: &str = r###"17c9fc9d0cb7f54492dc297ea75f8ea992576071e933a859120a2647abbfa347  cosign-3.1.3-1.aarch64.rpm
bffc26eceae4a715c4306a0a768a142f65d189cdc509c6788c02fadf8f9dd8c2  cosign-3.1.3-1.armv7hl.rpm
48d59cb4b2eba8c15056ca2c3b06b05dda62729ae193eefcf3d0c348e0225e5f  cosign-3.1.3-1.ppc64le.rpm
6a293bbe1ff031ebaac7ff7af0de8f3d08ee50e2e7dbf71d227a55b9f2efb742  cosign-3.1.3-1.riscv64.rpm
6d91467f8dfb505b5d19c9dcdb8d198b947f6fddf4073864648b7e7c654f1ee9  cosign-3.1.3-1.s390x.rpm
2e126115465ba55d03d3aea606cced2a24a1df578c8feb1d9384d584ebda9226  cosign-3.1.3-1.x86_64.rpm
2347488e5d5b25336644024dfeca5601b190e91197a71a917bda44744aff106c  cosign-darwin-amd64
42f7a10576a2f1c9311ee02984931a6b0575a914563c989b8866a17fd2e315d8  cosign-darwin-amd64_3.1.3_darwin_amd64.sbom.json
5cf948c2f4dfe59687bdd0b8523709067383e03982cc543475c8a7dc70e92a76  cosign-darwin-arm64
d0b1806ea2ca1af1ec62d508ad6fa2a686ae8deb5683e11f480dc9f87231d820  cosign-darwin-arm64_3.1.3_darwin_arm64.sbom.json
4629c757b7618056f8ddd7e2625ae9fdd94c0372a65049520bc7d9df9efc7f71  cosign-linux-amd64
d4a7d1a4f3cb5f4f87a01e81e511abb5f6f99c2e2bb7b929bde608a1ccfd14c3  cosign-linux-amd64_3.1.3_linux_amd64.sbom.json
3275e61b43a45aa56a6242b49475d8a01874a07469c08fc32d027ba554996e4c  cosign-linux-arm
c5d324e091826b0d7a78eb16fef316450b4eb9aaec045611c08ba06f5e73220a  cosign-linux-arm64
97149d5bbaaeea1e7f437b140b084da5ea14da4cbe49c3e1ec875ba538390dde  cosign-linux-arm64_3.1.3_linux_arm64.sbom.json
0ae9363bdaa5922ede815d15cf8936dec46267ee8775eb0b553d1eba18fc4259  cosign-linux-arm_3.1.3_linux_arm.sbom.json
549398fbe5a2f930b4eb564c7bbe9588270566ffcc8c9cb45644c066714aa380  cosign-linux-pivkey-pkcs11key-amd64
d22466f2011edc6eac938a27315ab8e81c0c9a901bcda08ae9cce95138b703fc  cosign-linux-pivkey-pkcs11key-amd64_3.1.3_linux_amd64.sbom.json
43266ec58f867517ab60e46972a1700f72f277d4c62a039325a4af66e4a1a1e4  cosign-linux-pivkey-pkcs11key-arm64
a4737f5cca4c54ddaf6fdf5a6eae6b2e1e7301fe5519a0755c5de7e911d18972  cosign-linux-pivkey-pkcs11key-arm64_3.1.3_linux_arm64.sbom.json
15619e924681c97e0fb4f79c0eb8ef5d29665a06fe6a25078500985aed79321b  cosign-linux-ppc64le
c3dd40355bf968fc68417ed5156556d3b4d98f4774dad0a184aead46e8163f37  cosign-linux-ppc64le_3.1.3_linux_ppc64le.sbom.json
9736177c6be33e4493304fc34feb8aa29209d531b3d40142d7e2a40bbe2f542f  cosign-linux-riscv64
6fc867619259fab50d1645bd2dfbcc9b809bbe8e46492dae4d8c80d9e851762c  cosign-linux-riscv64_3.1.3_linux_riscv64.sbom.json
253b571ef9e1aef72ef56d74f464ef9fe5182dc83b1ab430bbffbc4c8e43e7fd  cosign-linux-s390x
df0a6188f56c389f2d40a9fac65bf60f692c683adc4b2d142917bdc750ab97e8  cosign-linux-s390x_3.1.3_linux_s390x.sbom.json
9fe59be0eca1271873ce019061335eb1ac419b7059202e797828467ddabe33be  cosign-windows-amd64.exe
34d5b303eea474d37ffb19393c27ea92f41b7b73fe085a88de58d595f490cef0  cosign-windows-amd64.exe_3.1.3_windows_amd64.sbom.json
971ee36d1c18393d4b38535e353eb9b740c648b267a4593c0fd573e08f3597e9  cosign_3.1.3_aarch64.apk
75357d96161da4d06d37c4b2831fa6978483cdce661999e5951b586f9ee1d710  cosign_3.1.3_amd64.deb
cfa1a4ef37201be3086bb68f7d5f6e51dd497f28cdfa5bd990fbdffa92557cf8  cosign_3.1.3_arm64.deb
e9c627875b177f17097955ca075feb7764de8680ec2ddf95994b59c9c6b578db  cosign_3.1.3_armhf.deb
0afb5e41a4630c4f2aa9849cac98a619dd7956c830a20f29324336a379b5ffae  cosign_3.1.3_armv7.apk
9c40fa156af4f79d75b370e15e295df6130690e993739b26dd1b60d2316d801e  cosign_3.1.3_ppc64el.deb
2d75c251452376a4f3ba0f4c9887463779588ed5b00e5042aa8e6182eb40a78e  cosign_3.1.3_ppc64le.apk
7d81d4035f49f503daa3b65ded58b6a91a9993bef2693d4bb4c2ff80e5370bee  cosign_3.1.3_riscv64.apk
210733f1ae95eca08b8d035669358b9be9ed32b9fcefdcb5a111592ea7315cdf  cosign_3.1.3_riscv64.deb
bb4f115758f9c6bcdbe6ae4d0764875a4630f4fb4f4e29bfd495466fc0425f2e  cosign_3.1.3_s390x.apk
cc2a7046ec6510718384d49e85a96f841e37ce3e03da873a0d296f380850a959  cosign_3.1.3_s390x.deb
c42695b24ea2e7dff02a423a13f58140a156d94c15ea116cc4bd85309ecc527d  cosign_3.1.3_x86_64.apk
"###;
const SIGNATURE_BUNDLE: &str = r###"{"mediaType":"application/vnd.dev.sigstore.bundle.v0.3+json","verificationMaterial":{"certificate":{"rawBytes":"MIIDDzCCApWgAwIBAgIUWzNUlff8FTJG/V8PvS8uvqpo5qowCgYIKoZIzj0EAwMwNzEVMBMGA1UEChMMc2lnc3RvcmUuZGV2MR4wHAYDVQQDExVzaWdzdG9yZS1pbnRlcm1lZGlhdGUwHhcNMjYwODA2MDEwMzQ3WhcNMjYwODA2MDExMzQ3WjAAMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEKvfCMo2sk+Fx6XqfMfqQLJjwzTbnVfJLtwz1PFKyczIAlUM7DjzHrxYEfkbiJct9oboB9cu4Px29P4Ft3S7ixaOCAbQwggGwMA4GA1UdDwEB/wQEAwIHgDATBgNVHSUEDDAKBggrBgEFBQcDAzAdBgNVHQ4EFgQU3vhFSPS2EydigMpKHJQz0lRza0YwHwYDVR0jBBgwFoAU39Ppz1YkEZb5qNjpKFWixi4YZD8wPQYDVR0RAQH/BDMwMYEva2V5bGVzc0Bwcm9qZWN0c2lnc3RvcmUuaWFtLmdzZXJ2aWNlYWNjb3VudC5jb20wKQYKKwYBBAGDvzABAQQbaHR0cHM6Ly9hY2NvdW50cy5nb29nbGUuY29tMCsGCisGAQQBg78wAQgEHQwbaHR0cHM6Ly9hY2NvdW50cy5nb29nbGUuY29tMCUGCisGAQQBg78wARgEFwwVMTA1MDA0MDA1NDM0NTQ5OTYzMzg0MIGKBgorBgEEAdZ5AgQCBHwEegB4AHYA3T0wasbHETJjGR4cmWc3AqJKXrjePK3/h4pygC8p7o4AAAGf1JkJcwAABAMARzBFAiArSpoXzhAOLNXnDsjEK5V1f4VKFjKmxRBV3zXPLj7LNwIhAPPN81G0+oYo6/Q+FX3orD0l8kUMAkqh7C15We/AqGOtMAoGCCqGSM49BAMDA2gAMGUCMQDE7i2n3Go+sXGiPyutv2y92l23jAweqILCznws2tb5uWK/Z1RHPnhq60K5EKQgY+YCMHDn8EGDgXtzBKbosJKcX7ElaXmQmBcDBws1EXexl13Hd85vLNgDOAkr0r9Bb7oRvA=="},"tlogEntries":[{"logIndex":"2352248048","logId":{"keyId":"wNI9atQGlz+VWfO6LRygH4QUfY/8W4RFwiT5i5WRgB0="},"kindVersion":{"kind":"hashedrekord","version":"0.0.1"},"integratedTime":"1785978227","inclusionPromise":{"signedEntryTimestamp":"MEQCIFveqWpiUrAfDZHyuKXr0OLTb8NmSnsT6Yx8+DCMXsWxAiAOdbSjXkxp0/iGdF3FnxM4TsMwD9lJjymIneeNDuG9zw=="},"inclusionProof":{"logIndex":"2230343786","rootHash":"1soUQqxDMXQurTCRr9oIP6Gd2s1u8YL1wxvSvRABCQM=","treeSize":"2230343824","hashes":["cHjnd/TX4cGn3pplyiaqSsLmcFQ2L18L0QLrWIEH/X4=","J0JPKsNATJSNFWszgu280jRlFPBFIgO7EZz4kSF9x7I=","YYzqXkL1BiokWjqrR6/kmXX2tQNy3dsl8GcuNl0YkdI=","/zSRdXWvse6GsMQ0TQMgbqfOTQY1Z4KVbsIsln4lsQ8=","xv38Dgc8XCEk8lbxkjXrsH4SSe7IXR4QONz8RgDfsd0=","TC1iLW5yvQ6bLiwI+FZs2nuFlr+8t40ndXCR//knGz0=","vh+fBdJ49DLBF9pYFN/nhUk7C3fFVoAywZ1b1lATxLM=","329nYSRFAcC2eU4drvMvJbJRC8bJJ53wbxUamh0q0XM=","wpH9uko6yklplw3VaQWEil9xzif1VlLyuVe6mRENjEI=","3O8/HrRGJ+ITq7+KcdcoeAP3X88LLKLZiackw8+Qfo0=","yAmx3QPo4BZGYxvECCoBUmxpA4OCA4+cTB0j+t4esr4=","RGYIu/QtTq8UK9eY/VBV/9KKQqWfmDCK7jGP8AZSAYw=","VoiOIn261t/L0byz070dBNvoVbt1uIIQOHaEINtpgIk=","clYs9AKsQhNipZdpMaUoueKCBAQ1hVrYUWg0xHHCHBg=","ScH++eGfXwQDzE/H9Ae7YYnPqN5zAS9Hiq5LGoXV7Hg=","i5Zl8FZrDwxCDv2e2DNO2M8JvpR/c11ElvCZS53/teA=","xH/DCseLHr9eKoYT8qsORZK7zVdEGYWHuVtsVrD95wY="],"checkpoint":{"envelope":"rekor.sigstore.dev - 1193050959916656506\n2230343824\n1soUQqxDMXQurTCRr9oIP6Gd2s1u8YL1wxvSvRABCQM=\n\n— rekor.sigstore.dev wNI9ajBFAiEAiEodD8Im3tPGs/Osog1UOCJXchCmjVqBxFz/9JjenoQCIG1pH4Mvczgj25EC7aI35gzpxwLpmPuXasoXr7b587Pn\n"}},"canonicalizedBody":"eyJhcGlWZXJzaW9uIjoiMC4wLjEiLCJraW5kIjoiaGFzaGVkcmVrb3JkIiwic3BlYyI6eyJkYXRhIjp7Imhhc2giOnsiYWxnb3JpdGhtIjoic2hhMjU2IiwidmFsdWUiOiJhZWMyYTZmNjhkMzA3YjA5YWUxOTZlMzg4ZGM2OTFhMTQ2ZmE4YmRiYTdmY2NlOWNhNGNhNDFiOTE4YWRmYTYzIn19LCJzaWduYXR1cmUiOnsiY29udGVudCI6Ik1FUUNJQTFaem9qNkVzZGFqb2lnVjl0anVSMXFMZW9sMWVtYlVsWTBKQjQvTG42MUFpQjZld09LNkpLakduVnN0SXVuSFR4Y2pDaUhTS2VrdXlLcjFNZkhydDVzcXc9PSIsInB1YmxpY0tleSI6eyJjb250ZW50IjoiTFMwdExTMUNSVWRKVGlCRFJWSlVTVVpKUTBGVVJTMHRMUzB0Q2sxSlNVUkVla05EUVhCWFowRjNTVUpCWjBsVlYzcE9WV3htWmpoR1ZFcEhMMVk0VUhaVE9IVjJjWEJ2TlhGdmQwTm5XVWxMYjFwSmVtb3dSVUYzVFhjS1RucEZWazFDVFVkQk1WVkZRMmhOVFdNeWJHNWpNMUoyWTIxVmRWcEhWakpOVWpSM1NFRlpSRlpSVVVSRmVGWjZZVmRrZW1SSE9YbGFVekZ3WW01U2JBcGpiVEZzV2tkc2FHUkhWWGRJYUdOT1RXcFpkMDlFUVRKTlJFVjNUWHBSTTFkb1kwNU5hbGwzVDBSQk1rMUVSWGhOZWxFelYycEJRVTFHYTNkRmQxbElDa3R2V2tsNmFqQkRRVkZaU1V0dldrbDZhakJFUVZGalJGRm5RVVZMZG1aRFRXOHljMnNyUm5nMldIRm1UV1p4VVV4S2FuZDZWR0p1Vm1aS1RIUjNlakVLVUVaTGVXTjZTVUZzVlUwM1JHcDZTSEo0V1VWbWEySnBTbU4wT1c5aWIwSTVZM1UwVUhneU9WQTBSblF6VXpkcGVHRlBRMEZpVVhkblowZDNUVUUwUndwQk1WVmtSSGRGUWk5M1VVVkJkMGxJWjBSQlZFSm5UbFpJVTFWRlJFUkJTMEpuWjNKQ1owVkdRbEZqUkVGNlFXUkNaMDVXU0ZFMFJVWm5VVlV6ZG1oR0NsTlFVekpGZVdScFowMXdTMGhLVVhvd2JGSjZZVEJaZDBoM1dVUldVakJxUWtKbmQwWnZRVlV6T1ZCd2VqRlphMFZhWWpWeFRtcHdTMFpYYVhocE5Ga0tXa1E0ZDFCUldVUldVakJTUVZGSUwwSkVUWGROV1VWMllUSldOV0pIVm5wak1FSjNZMjA1Y1ZwWFRqQmpNbXh1WXpOU2RtTnRWWFZoVjBaMFRHMWtlZ3BhV0VveVlWZE9iRmxYVG1waU0xWjFaRU0xYW1JeU1IZExVVmxMUzNkWlFrSkJSMFIyZWtGQ1FWRlJZbUZJVWpCalNFMDJUSGs1YUZreVRuWmtWelV3Q21ONU5XNWlNamx1WWtkVmRWa3lPWFJOUTNOSFEybHpSMEZSVVVKbk56aDNRVkZuUlVoUmQySmhTRkl3WTBoTk5reDVPV2haTWs1MlpGYzFNR041Tlc0S1lqSTVibUpIVlhWWk1qbDBUVU5WUjBOcGMwZEJVVkZDWnpjNGQwRlNaMFZHZDNkV1RWUkJNVTFFUVRCTlJFRXhUa1JOTUU1VVVUVlBWRmw2VFhwbk1BcE5TVWRMUW1kdmNrSm5SVVZCWkZvMVFXZFJRMEpJZDBWbFowSTBRVWhaUVROVU1IZGhjMkpJUlZSS2FrZFNOR050VjJNelFYRktTMWh5YW1WUVN6TXZDbWcwY0hsblF6aHdOMjgwUVVGQlIyWXhTbXRLWTNkQlFVSkJUVUZTZWtKR1FXbEJjbE53YjFoNmFFRlBURTVZYmtSemFrVkxOVll4WmpSV1MwWnFTMjBLZUZKQ1ZqTjZXRkJNYWpkTVRuZEphRUZRVUU0NE1VY3dLMjlaYnpZdlVTdEdXRE52Y2tRd2JEaHJWVTFCYTNGb04wTXhOVmRsTDBGeFIwOTBUVUZ2UndwRFEzRkhVMDAwT1VKQlRVUkJNbWRCVFVkVlEwMVJSRVUzYVRKdU0wZHZLM05ZUjJsUWVYVjBkako1T1RKc01qTnFRWGRsY1VsTVEzcHVkM015ZEdJMUNuVlhTeTlhTVZKSVVHNW9jVFl3U3pWRlMxRm5XU3RaUTAxSVJHNDRSVWRFWjFoMGVrSkxZbTl6U2t0aldEZEZiR0ZZYlZGdFFtTkVRbmR6TVVWWVpYZ0tiREV6U0dRNE5YWk1UbWRFVDBGcmNqQnlPVUppTjI5U2RrRTlQUW90TFMwdExVVk9SQ0JEUlZKVVNVWkpRMEZVUlMwdExTMHRDZz09In19fX0="}],"timestampVerificationData":{"rfc3161Timestamps":[{"signedTimestamp":"MIICyjADAgEAMIICwQYJKoZIhvcNAQcCoIICsjCCAq4CAQMxDTALBglghkgBZQMEAgEwgbgGCyqGSIb3DQEJEAEEoIGoBIGlMIGiAgEBBgkrBgEEAYO/MAIwMTANBglghkgBZQMEAgEFAAQgYt2Wtmw0hMDp333QdTrUJK49BLkiAR2yjDWhiroO6CACFQDiCgv+iID0xDlhzUO9Fjh9qHTf/hgPMjAyNjA4MDYwMTAzNDdaMAMCAQGgMqQwMC4xFTATBgNVBAoTDHNpZ3N0b3JlLmRldjEVMBMGA1UEAxMMc2lnc3RvcmUtdHNhoAAxggHbMIIB1wIBATBRMDkxFTATBgNVBAoTDHNpZ3N0b3JlLmRldjEgMB4GA1UEAxMXc2lnc3RvcmUtdHNhLXNlbGZzaWduZWQCFDoTVC8MkGHuvMFDL8uKjosqI4sMMAsGCWCGSAFlAwQCAaCB/DAaBgkqhkiG9w0BCQMxDQYLKoZIhvcNAQkQAQQwHAYJKoZIhvcNAQkFMQ8XDTI2MDgwNjAxMDM0N1owLwYJKoZIhvcNAQkEMSIEIJwm8Ka0FVPpUdSx0BI/vCriIa8+XLqtSG1NbJdGK1MXMIGOBgsqhkiG9w0BCRACLzF/MH0wezB5BCCF+Se8B6tiysO0Q1bBDvyBssaIP9p6uebYcNnROs0FtzBVMD2kOzA5MRUwEwYDVQQKEwxzaWdzdG9yZS5kZXYxIDAeBgNVBAMTF3NpZ3N0b3JlLXRzYS1zZWxmc2lnbmVkAhQ6E1QvDJBh7rzBQy/Lio6LKiOLDDAKBggqhkjOPQQDAgRnMGUCMQDrfMSQYGyC4Yezy/bkoOfVfm0c6aNN+zpEGPrAfuQYwGVA8bXVzAwavy9FN3HVF5ICMFs4XjmoD33NoNwlxrv/P8VMcWUYWaM/Cgy9LpWHUeJksIVAcEqZMlSqhFSvs52nKQ=="}]}},"messageSignature":{"messageDigest":{"algorithm":"SHA2_256","digest":"rsKm9o0wewmuGW44jcaRoUb6i9un/M6cpMpBuRit+mM="},"signature":"MEQCIA1Zzoj6EsdajoigV9tjuR1qLeol1embUlY0JB4/Ln61AiB6ewOK6JKjGnVstIunHTxcjCiHSKekuyKr1MfHrt5sqw=="}}"###;

fn pinned_cosign() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var("BUILD_COSIGN_TEST_BINARY").ok()?);
    let (_, expected) = cosign_asset().unwrap();
    assert_eq!(
        digest(&fs::read(&path).unwrap()),
        expected,
        "test cosign must match the production pin"
    );
    Some(path)
}

fn fixture_files(dir: &Path) -> (PathBuf, PathBuf) {
    let sums = dir.join("SHA256SUMS");
    let bundle = dir.join("SHA256SUMS.sigstore.json");
    fs::write(&sums, SIGNED_CHECKSUMS).unwrap();
    fs::write(&bundle, SIGNATURE_BUNDLE).unwrap();
    (sums, bundle)
}

#[test]
fn real_bundle_has_valid_ecdsa_signature_for_exact_payload() {
    assert_eq!(
        digest(SIGNED_CHECKSUMS.as_bytes()),
        "aec2a6f68d307b09ae196e388dc691a146fa8bdba7fcce9ca4ca41b918adfa63"
    );
    assert_eq!(
        digest(SIGNATURE_BUNDLE.as_bytes()),
        "976bcb216e45ed0274e464e2e16d81e84cc85a69b3ed6e3488c1e7cda116379a"
    );
    let bundle: serde_json::Value = serde_json::from_str(SIGNATURE_BUNDLE).unwrap();
    let decode = |value: &serde_json::Value| {
        base64::engine::general_purpose::STANDARD
            .decode(value.as_str().unwrap())
            .unwrap()
    };
    let cert = decode(&bundle["verificationMaterial"]["certificate"]["rawBytes"]);
    assert_eq!(
        digest(&cert),
        "4794bd0a75ab46dde4f53677b76f894474238284a145c3301f0ae5aa41fefe31",
        "the fixture certificate is pinned"
    );
    assert!(cert
        .windows(SIGNER.len())
        .any(|bytes| bytes == SIGNER.as_bytes()));
    assert!(cert
        .windows(ISSUER.len())
        .any(|bytes| bytes == ISSUER.as_bytes()));
    let public_key_start = cert
        .windows(4)
        .position(|bytes| bytes == [3, 66, 0, 4])
        .unwrap()
        + 3;
    let public_key = &cert[public_key_start..public_key_start + 65];
    let digest_in_bundle = decode(&bundle["messageSignature"]["messageDigest"]["digest"]);
    assert_eq!(
        &digest_in_bundle[..],
        &Sha256::digest(SIGNED_CHECKSUMS.as_bytes())[..]
    );
    let signature = decode(&bundle["messageSignature"]["signature"]);
    let verifier = UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, public_key);
    verifier
        .verify(SIGNED_CHECKSUMS.as_bytes(), &signature)
        .expect("genuine ECDSA signature");
    assert!(verifier.verify(b"tampered payload", &signature).is_err());
    let mut altered_signature = signature;
    altered_signature[5] ^= 1;
    assert!(verifier
        .verify(SIGNED_CHECKSUMS.as_bytes(), &altered_signature)
        .is_err());
}

// Run with BUILD_COSIGN_TEST_BINARY set to the exact SHA-256-pinned cosign
// executable to additionally exercise its complete Sigstore trust chain.
#[test]
fn pinned_cosign_accepts_real_bundle_and_rejects_policy_changes() {
    let Some(cosign) = pinned_cosign() else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let (sums, bundle) = fixture_files(temp.path());

    verify_signature_with_identity(&cosign, &sums, &bundle, SIGNER, ISSUER)
        .expect("the genuine Sigstore bundle must verify");
    assert!(
        verify_signature_with_identity(&cosign, &sums, &bundle, "wrong@example.com", ISSUER)
            .is_err()
    );
    assert!(verify_signature_with_identity(
        &cosign,
        &sums,
        &bundle,
        SIGNER,
        "https://wrong.example"
    )
    .is_err());

    fs::write(&sums, format!("{SIGNED_CHECKSUMS}tampered\n")).unwrap();
    assert!(verify_signature_with_identity(&cosign, &sums, &bundle, SIGNER, ISSUER).is_err());
}

#[test]
fn production_identity_rejects_an_authentic_other_project_bundle() {
    let Some(cosign) = pinned_cosign() else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let (sums, bundle) = fixture_files(temp.path());
    assert!(verify_signature(&cosign, &sums, &bundle, "bridge-v0.2.0").is_err());
}

fn rejecting_verifier(dir: &Path) -> PathBuf {
    let script = dir.join("rejecting-verifier");
    fs::write(
        &script,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/args\"\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    script
}

#[test]
fn production_command_pins_exact_workflow_identity_and_issuer() {
    let temp = tempfile::tempdir().unwrap();
    let (sums, bundle) = fixture_files(temp.path());
    let verifier = rejecting_verifier(temp.path());
    assert!(verify_signature(&verifier, &sums, &bundle, "bridge-v0.2.0").is_err());
    let args = fs::read_to_string(temp.path().join("args")).unwrap();
    assert_eq!(args.lines().collect::<Vec<_>>(), vec![
        "verify-blob", "--bundle", bundle.to_str().unwrap(),
        "--certificate-identity",
        "https://github.com/ZechCodes/build-web/.github/workflows/release.yml@refs/tags/bridge-v0.2.0",
        "--certificate-oidc-issuer", "https://token.actions.githubusercontent.com",
        sums.to_str().unwrap(),
    ]);
    assert!(verify_signature(&verifier, &sums, &bundle, "../other").is_err());
}

#[tokio::test]
async fn local_signed_release_stages_and_rejects_bad_signature_or_archive_digest() {
    let rng = ring::rand::SystemRandom::new();
    let key_material = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
    let signer = Ed25519KeyPair::from_pkcs8(key_material.as_ref()).unwrap();
    let public_key = signer.public_key().as_ref().to_vec();
    let archive_name = format!("build-bridge-{}.tar.gz", platform_key().unwrap());
    let archive = compressed_tar(&[(
        "build-bridge",
        b"verified test binary",
        tar::EntryType::Regular,
    )]);
    let manifest = format!("{}  {archive_name}\n", digest(&archive));
    let bundle = serde_json::json!({
        "tag": "bridge-v0.2.0",
        "signature": base64::engine::general_purpose::STANDARD.encode(signer.sign(manifest.as_bytes()).as_ref()),
    }).to_string();

    run_local_release_case(&archive, &manifest, &bundle, &public_key, true).await;

    let mut bad_bundle: serde_json::Value = serde_json::from_str(&bundle).unwrap();
    bad_bundle["signature"] =
        serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode([0u8; 64]));
    run_local_release_case(
        &archive,
        &manifest,
        &bad_bundle.to_string(),
        &public_key,
        false,
    )
    .await;

    let mut bad_archive = archive.clone();
    let final_byte = bad_archive.len() - 1;
    bad_archive[final_byte] ^= 1;
    run_local_release_case(&bad_archive, &manifest, &bundle, &public_key, false).await;
}

async fn run_local_release_case(
    archive: &[u8],
    manifest: &str,
    bundle: &str,
    public_key: &[u8],
    should_stage: bool,
) {
    let server = MockServer::start().await;
    let archive_name = format!("build-bridge-{}.tar.gz", platform_key().unwrap());
    for (asset, body) in [
        (archive_name.as_str(), archive),
        ("SHA256SUMS", manifest.as_bytes()),
        ("SHA256SUMS.sigstore.json", bundle.as_bytes()),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/bridge-v0.2.0/{asset}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
            .mount(&server)
            .await;
    }
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("staged-bridge");
    let release = Release {
        version: "0.2.0".into(),
        tag: "bridge-v0.2.0".into(),
        published_at: None,
    };
    let result = stage_from_base_with_verifier(
        &Client::new(),
        &release,
        &destination,
        &format!("{}/bridge-v0.2.0", server.uri()),
        |sums, bundle, tag| {
            let material: serde_json::Value =
                serde_json::from_slice(&fs::read(bundle).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if material["tag"] != tag {
                return Err("test signer tag mismatch".into());
            }
            let signature = base64::engine::general_purpose::STANDARD
                .decode(
                    material["signature"]
                        .as_str()
                        .ok_or("missing test signature")?,
                )
                .map_err(|e| e.to_string())?;
            UnparsedPublicKey::new(&ED25519, public_key)
                .verify(&fs::read(sums).map_err(|e| e.to_string())?, &signature)
                .map_err(|_| "test signature rejected".to_string())
        },
    )
    .await;
    if should_stage {
        result.unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"verified test binary");
    } else {
        assert!(result.is_err());
        assert!(!destination.exists());
    }
}

struct LocalMetadataBackend {
    client: Client,
    url: String,
}

#[async_trait::async_trait]
impl UpdateBackend for LocalMetadataBackend {
    async fn latest(&self) -> Result<Release, String> {
        latest_from_url(&self.client, &self.url).await
    }

    async fn install(&self, _release: &Release, _attempt_id: &str) -> Result<(), String> {
        Err("metadata test does not install".into())
    }
}

#[tokio::test]
async fn local_release_metadata_makes_a_newer_version_available() {
    let server = MockServer::start().await;
    for (route, tag) in [
        ("/latest", "bridge-v0.3.0"),
        ("/wrong-prefix", "desktop-v0.3.0"),
        ("/invalid-semver", "bridge-vbanana"),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tag_name": tag,
                "published_at": "2026-09-23T20:00:00Z"
            })))
            .mount(&server)
            .await;
    }

    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(LocalMetadataBackend {
        client: Client::new(),
        url: format!("{}/latest", server.uri()),
    });
    let service = UpdateService::new(
        UpdateConfig {
            status_path: temp.path().join("status.json"),
            result_path: temp.path().join("result.json"),
            running_version: "0.2.0".into(),
            platform: platform_key().unwrap().into(),
            development_build: false,
            check_interval: Duration::from_secs(86_400),
        },
        backend,
    )
    .unwrap();
    let status = service.check().await.unwrap();
    assert_eq!(status.state, UpdateState::Available);
    assert!(status.update_available);
    assert!(status.can_install);
    assert_eq!(status.latest_release.as_ref().unwrap().version, "0.3.0");
    assert_eq!(status.latest_release.as_ref().unwrap().tag, "bridge-v0.3.0");
    assert_eq!(
        status
            .latest_release
            .as_ref()
            .unwrap()
            .published_at
            .as_deref(),
        Some("2026-09-23T20:00:00Z")
    );

    let client = Client::new();
    assert!(
        latest_from_url(&client, &format!("{}/wrong-prefix", server.uri()))
            .await
            .unwrap_err()
            .contains("not a bridge tag")
    );
    assert!(
        latest_from_url(&client, &format!("{}/invalid-semver", server.uri()))
            .await
            .unwrap_err()
            .contains("invalid bridge tag")
    );
}

fn compressed_tar(entries: &[(&str, &[u8], tar::EntryType)]) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    for &(name, bytes, entry_type) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_entry_type(entry_type);
        // Set the raw tar name so the traversal case reaches extraction;
        // Builder::append_data refuses to construct such an archive.
        header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        header.set_cksum();
        tar.append(&header, bytes).unwrap();
    }
    let tar = tar.into_inner().unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&tar).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn archive_check_requires_one_exact_filename_and_matching_digest() {
    let name = format!("build-bridge-{}.tar.gz", platform_key().unwrap());
    let archive = compressed_tar(&[("build-bridge", b"safe", tar::EntryType::Regular)]);
    let checksum = format!("{}  {name}\n", digest(&archive));
    assert!(verify_archive(checksum.as_bytes(), &archive, &name).is_ok());
    assert!(verify_archive(checksum.as_bytes(), b"different", &name).is_err());
    assert!(verify_archive(format!("{checksum}{checksum}").as_bytes(), &archive, &name).is_err());
    assert!(verify_archive(
        format!("{}  other.tar.gz\n", digest(&archive)).as_bytes(),
        &archive,
        &name
    )
    .is_err());
}

#[test]
fn archive_extraction_accepts_one_regular_binary_and_rejects_other_shapes() {
    let temp = tempfile::tempdir().unwrap();
    let good = compressed_tar(&[("build-bridge", b"safe", tar::EntryType::Regular)]);
    let good_path = temp.path().join("good");
    extract_binary(&good, &good_path).unwrap();
    assert_eq!(fs::read(good_path).unwrap(), b"safe");

    let cases = [
        compressed_tar(&[("../build-bridge", b"evil", tar::EntryType::Regular)]),
        compressed_tar(&[("build-bridge", b"target", tar::EntryType::Symlink)]),
        compressed_tar(&[
            ("build-bridge", b"safe", tar::EntryType::Regular),
            ("extra", b"evil", tar::EntryType::Regular),
        ]),
        compressed_tar(&[("nested/build-bridge", b"evil", tar::EntryType::Regular)]),
    ];
    for (index, archive) in cases.into_iter().enumerate() {
        let destination = temp.path().join(format!("rejected-{index}"));
        assert!(
            extract_binary(&archive, &destination).is_err(),
            "case {index}"
        );
        assert!(!destination.exists(), "case {index} wrote a binary");
    }
}
