# Account HTTP security checklist — #910

Corrected author witness: **100/100** for the ten owned checks below. The
original record was superseded by #921's blocking
logout finding and allocation-availability concern. The corrections follow
#922/#923; their final author evidence is recorded on #910. Independent #921
acceptance, imports and release remain held. The rows below are author checks, not
security acceptance of the full dual-transport release. Native TLS/listener,
execution admission/output wiring, desktop handle lifetime and browser network
qualification remain the #911/#913/#914 integration gates. No HTTP capability is
enabled by these independent modules.

Each row is worth ten points. All named tests run through production modules;
network tests use real isolated reqwest/ASGI entry points. The harness identity
fixture only supplies the existing persisted key fields before consolidated
Cargo/lib integration.

| Check | Witness | Status |
| --- | --- | --- |
| Exact HTTPS origin, paired issuer and key provenance | Rust `origins_are_https_and_canonical_without_url_aliases`, `unapproved_and_other_issuer_cannot_sign`, `stored_unpadded_transport_identity_keys_can_sign_standard_jws`; Python origin/key vectors | 10/10 |
| Standard JWS algorithm, explicit purpose, pinned paired key, audience and time | `test_rust_signed_standard_eddsa_vectors_verify`, `test_signed_wrong_claims_refused`, `test_remote_keys_critical_headers_and_type_substitution_refused` | 10/10 |
| Registration authority, predecessor epoch, owner/alpha state and paired-key fingerprint | `test_routes_discover_renew_revoke_and_never_accept_client_authority`, signed allocation capacity/replay/rollback cases, PostgreSQL allocation races, endpoint CAS and paired-key change cases | 10/10 |
| Account/client/origin-bound discovery; one-use challenge and replay guards | `test_forwarded_proof_cross_account_and_replays_are_refused`, `test_competing_connections_cannot_mint_two_families`, `test_replay_budget_refuses_instead_of_evicting_live_guards` | 10/10 |
| Target-device and scope-bound grants; stable operation owner | `test_other_owned_device_cannot_introspect_target_token`, Rust `every_target_binding_and_unknown_scope_is_rejected`, `rotation_preserves_principal_and_binding_but_closing_never_revives` | 10/10 |
| Revocation, logout and account suspension | `test_logout_endpoint_change_and_membership_revoke_invalidate_tokens`, `test_logged_out_binding_cannot_start_fresh_discovery`, real earliest-cookie regression, #923 new/existing passkey transaction and pre-cookie failure tests, account suspension tests | 10/10 |
| Exact desktop parent issuance and hard expiry caps | `test_two_same_account_desktop_issuances_isolate_logout_and_rotation`, `test_desktop_near_expiry_parent_has_no_authorization_grace`, `test_desktop_parent_expiry_and_revocation_cap_the_family`; Rust delayed-expiry/freshness tests | 10/10 |
| No credential redirects; bounded network/cache/queue and irreversible closure | Rust redirect-trap, response-size, eight-network-call, cancelled-caller queue, local-disable-after-cache-eviction and delayed-mint tests; cache inspection retains only digests | 10/10 |
| Private bounded per-account document policy with one enforced CSP | `test_real_skrift_assembly_has_one_account_csp_keeps_nonce_and_nonapp_defaults`, desktop verified-token policy and stale/reassigned origin tests in `test_bridge_http_policy.py` | 10/10 |
| Schema constraints and isolated migration; review/scan evidence | Executed upgrade/downgrade over legacy schema in `test_bridge_http_migration.py`; Ruff complexity cap, Rust clippy/fmt, Semgrep and immutable task commits | 10/10 |

No hosted code fetches a submitted bridge URL. Hosted account cookies/OAuth stay
at the configured issuer. Direct discovery signs captured configuration; only a
successful hosted verification mints a bridge token. Bodies, tokens, proofs,
private keys and upstream error details do not enter application error messages.
A single narrow Semgrep suppression documents that an untrusted `iss` selects a
stored paired key before the separate mandatory signature/claims verification;
it never confers authority by itself.

Known limits: offline/crash logout cannot guarantee immediate remote revocation.
A bridge token lasts at most five minutes and no later than parent OAuth expiry;
old parent possession can mint only until that immutable expiry unless revoked.
Cached authority never outlives its known expiry or verification-start+30 seconds.
The final task report supplies actual gate exit codes and skipped baseline tests.

Gitleaks initially identified the three public fixed-key JWS vectors as JWTs.
The coordinator reviewed the fixture and authorized only their exact historical
commit/file/rule/line fingerprints in `.gitleaksignore`; the JWT rule remains
enabled. The final committed-range scan records its exit in the task report.

Correction qualifications: #922 allocation proofs spend JTI and allocate nonce
under one device lock/transaction, retain replay guards through exp+5, and have
no unsigned compatibility path. All old allocator workers must drain before
protection is claimed; legacy recovery is at most 60 seconds after the last unsigned
allocation. #923 forbids committing borrowed auth sessions: caller commit owns
new passkey account/enrollment/client atomicity. Post-commit failures may leave
an account/unused identity row but issue no authenticated cookie. Transaction
fixtures represent post-verification passkey output, not external credential
verification. Raw PostgreSQL author logs preserve distinct physical connection
PIDs and actual lock relationships. They do not resolve independent acceptance.

Final correction author gates: 1,169 hosted tests passed, ten baseline skips;
all twelve PostgreSQL cases executed. The 37 focused login ownership/guard
cases and 37 Rust harness cases passed. Ruff, clippy/fmt and Semgrep passed.
The final task report binds these exits, the committed-range Gitleaks result
and the immutable candidate head. None of these author results is independent
#921 acceptance.
