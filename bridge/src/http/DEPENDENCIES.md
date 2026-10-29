# HTTP runtime dependency review

The runtime harness is isolated from the daemon until the reviewed shared-file
handoff. Its lock starts from the frozen source lock at
`12b8f869bd94e259666df69f1a8c0cde5c695982` and retains common package versions.
This file records the proposed production additions/promotions; production
Cargo/main/lib integration remains a separate reviewed change.

| Package | Exact version / features | Reason and existing graph |
| --- | --- | --- |
| axum | 0.8.9; no defaults; http1, json, tokio | Maintained routing/body/SSE components. HTTP/2 is deliberately absent on the native listener. MIT; Rust 1.80 minimum. |
| hyper | 1.10.1; server, http1 | Already locked via reqwest; server configuration supplies header deadline/byte bounds. |
| hyper-util | 0.1.20; server, http1, tokio, service | Already locked; adapts the Router to native TLS sockets with Tokio timers. |
| tower-http | 0.6.11; no defaults; cors, limit | Already locked; explicit CORS allowlists. Streaming byte validation remains in the adapter, including chunked bodies. |
| tokio-rustls | 0.26.4; no defaults; aws-lc-rs, tls12 | Promote existing dev dependency; use the existing process provider and certificate validation. |
| bytes | 1.12.1 | Promote existing dev/transitive dependency; Bytes::from_owner retains exact delivery and reservation guards through downstream buffering. |
| percent-encoding | 2.3.2 | Already locked; requested by resource-route owner for path segment decoding. |
| jsonwebtoken | 10.3.0; no defaults; aws_lc_rs | Authorization owner selects standard compact EdDSA JWS; no PEM/ASN.1 convenience stack. Its separate review owns proof validation evidence. |
| aws-lc-rs | 1.17.0 | Existing provider version, also used directly to import the paired Ed25519 seed. See feature expansion below. |

Tokio stays at 1.52.3, rustls at 0.23.41, reqwest at 0.13.4 and
rustls-pki-types at 1.14.1. The standalone harness pins the same Tokio/provider
versions and preserves the baseline transitive lock instead of resolving every
existing dependency to a newer release. The only new runtime framework packages
relative to that source lock are axum 0.8.9, axum-core 0.5.6, matchit 0.8.4,
mime 0.3.17 and serde_path_to_error 0.1.20.

Reusing a provider version does not imply an unchanged feature graph:
jsonwebtoken's aws-lc dependency enables defaults (ring-io/ring-sig-verify),
adding untrusted 0.7.1 alongside the source's untrusted 0.9.0. The authorization
owner has included this expansion in its independent dependency review. There
is no second rustls crypto provider added by this runtime.

PEM parsing uses the existing rustls::pki_types::pem::PemObject API, with bounded
file reads, owner-only key validation and certificate/key consistency checking.
No rustls-pemfile dependency is proposed: its
[RustSec unmaintained advisory](https://rustsec.org/advisories/RUSTSEC-2025-0134.html)
and [maintainer migration guide](https://github.com/rustls/pemfile/releases/tag/v%2F2.2.0)
direct users to the underlying API already present in the source graph.

Relevant maintained interfaces are the
[Axum server interface](https://docs.rs/axum/0.8.9/axum/fn.serve.html),
[tower-http CORS](https://docs.rs/tower-http/0.6.11/tower_http/cors/index.html),
and [PemObject](https://docs.rs/rustls-pki-types/1.14.1/rustls_pki_types/pem/trait.PemObject.html).

Tests establish adapter behavior, not peer receipt or a total-process heap cap.
Application reservations exclude already handed-off TLS/kernel buffers, and
parsed/normalized/base64 copies coexist within the separate large-work limits.
No certificate bypass, reverse-proxy trust profile, public deployment, live
service change or durable sync implementation is part of these dependencies.
