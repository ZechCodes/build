# Invite security checklist

**Status:** verified — 100/100 (10/10 controls)

**Scope:** email-bound and open-link invites, passkey signup, and optional product
email consent (#338). Production still configures passkey authentication alone.
This covers the application, schema, and shipped server; it does not certify an
external proxy's logging configuration or a future product-mail campaign sender.

## Threat model

An invite is a bearer credential that grants one alpha membership. An
`email_bound` invite additionally restricts the account address; an `open_link`
lets its first redeemer choose an address. Attackers may guess tokens, replay
links, submit a different address or invite during the passkey ceremony, race
redemption against another visitor or revocation, forge consent, or obtain a
credential from a log, cache, or Referer. Operators can revoke either kind.

Product-email consent is separate from a waitlist entry and from alpha access.
No checkbox, missing preference data, or an old account implies no consent.
Unsubscribe must work for a member even when they never joined the waitlist.

## Controls

| # | Control | Status | Required evidence |
| --- | --- | --- | --- |
| 1 | Tokens have 256 random bits, only their SHA-256 hashes are stored, and guesses are rate limited. | [x] | `mint_invite_token` uses `secrets.token_urlsafe(32)`; `test_a_minted_token_is_prefixed_and_stored_only_as_its_hash`, `test_an_open_link_has_no_address_and_keeps_only_the_token_hash`; production `invite_open` limits GET requests to 30/minute/IP. |
| 2 | Creation requires administrator permission; forms require CSRF; both admin creation routes and the API are limited to 20/minute and 300/day per IP. Open-link issuance never sends email. | [x] | `test_invites_admin`, `test_invites_http`, and `test_production_config` cover guards, missing/wrong CSRF, creation policy resolution and mail capture; malformed kinds or open-link requests carrying an address return 400. |
| 3 | The raw open link appears only in the immediate creation response, never in database columns, flash messages, the creation session or subsequent admin GETs. Link responses and invite/auth pages prevent storage and Referer propagation; framework/server logging redacts credentials including exception text. | [x] | Admin one-time-response and HTTP header tests; `test_framework_and_server_logs_redact_invite_paths_and_exception_text`; `invite_log_privacy`; headless admin create→GET proof. Shipped Hypercorn access logging is disabled. External proxy logging must preserve the same restriction (README). |
| 4 | Schema explicitly distinguishes `email_bound` from `open_link`; existing rows remain bound; only unredeemed open links can lack an address. Downgrade revokes unclaimed open links before dropping their kind. New schema grants no implicit email consent. | [x] | `test_open_invites_migration` executes upgrade/downgrade with a legacy SQLite row, tests constraints, preference defaults, and downgrade revocation without changing existing revocations or redeemed/bound links; `test_invites_migration` pins the frozen original revision. |
| 5 | Bound invitations still refuse a different canonical account address. Open-link signup validates newly entered addresses before prompting for a passkey. Existing account redemption compares identity without redefining deliverability. | [x] | Unchanged `signup_invite.admits`; wrong-address HTTP/signup/domain tests; invalid, oversized and localhost open-signup inputs refused by `test_open_link_refuses_addresses_the_waitlist_cannot_accept`; existing dev identity tests remain green. |
| 6 | Signup requires the invite ID carried in the encrypted session. Options bind the exact invite, email and consent; completion rechecks them and current invite state. A changed link, revoked/expired/spent invite, missing CSRF, or forged completion consent cannot bypass this. | [x] | `test_invite_signup`: options without invite, other-address refusal, changed bound/open invite, revoke/expiry, CSRF/provider precedence, and `test_signup_ignores_consent_forged_only_on_completion`. The pinned auth route inventory prevents an unnoticed registration route. |
| 7 | Redemption conditionally updates a still-unused, unrevoked, unexpired row and records one user/address; another stale request cannot overwrite it. An existing invite member cannot spend an additional link. Revocation remains the membership removal mechanism for both kinds. | [x] | `claim_invite` locks the PostgreSQL user row before checking membership and includes a membership predicate in the update for SQLite writers; `test_only_one_stale_request_can_claim_an_invite` for both kinds; `test_two_concurrent_links_cannot_grant_one_account_two_memberships` exercises SQLite contenders; `test_a_member_cannot_spend_another_link_and_escape_revocation` checks both kinds, preserves the extra link and permits rejoining after revocation. |
| 8 | Both signup forms show an unchecked, optional product-email checkbox. Only successful CSRF-checked registration options can record it; successful claim stores the boolean and UTC timestamp on a unique per-user preference. | [x] | Both-kind template and parameterized registration-consent tests in `test_invite_signup`; `passkey-signin.test.mjs` pins checkbox submission; headless Chromium checks default state, registration and returning passkey sign-in for both kinds. |
| 9 | Product-mail recipients require an active user, explicit true consent and its timestamp. Missing rows or unchecked boxes exclude the account. Transactional waitlist/invite mail does not create consent. | [x] | `email_consent.consenting_product_email_addresses` and `test_email_consent`; both-kind false/absent/forged consent tests. There is no campaign sender today; this is the required recipient query for a future sender. |
| 10 | Existing signed unsubscribe tokens clear product consent and the waitlist row by canonical email, including accounts without a waitlist entry. GET is read-only; invalid tokens change nothing; repeated POSTs are safe. | [x] | `test_product_unsubscribe_http` exercises the real app and database with and without a waitlist row, confirmation GET, invalid tokens and repeated POSTs; route unit tests pin response copy, and `test_unsubscribe_revokes_member_consent_by_canonical_address` covers casing. |

## Accepted

- By design, an open link proves nothing about ownership of the typed email
  address. It authorizes whoever first redeems it. Share it privately and revoke
  an exposed link.
- A signup that requires a second factor redeems at the invite link after that
  factor succeeds. It records no product-email consent, even if the person
  checked the signup box; a missing preference remains opted out.
- The invite route and Skrift's return URL retain the raw token in the visitor's
  own browser history/encrypted session until the flow ends. They emit no
  Referer. There is no API to recover the raw token from its stored hash.
- Skrift commits account creation before Build's conditional claim. A competing
  redemption or late revoke can leave a signed-in account without membership or
  product consent. It cannot enter the app. The browser is sent to the matching
  invite outcome, regardless of the saved return URL; the already-used page
  explains how to request another link. The existing
  `test_an_invite_revoked_mid_registration_leaves_an_account_outside_the_app`
  and `test_a_lost_claim_after_account_creation_opens_the_already_used_invite_page`
  pin this boundary; the invite itself can be spent only once.
- Product-mail eligibility is checked when querying recipients. A future sender
  must recheck consent before delivery and include the existing signed
  unsubscribe URL. This change creates no campaign scheduler or outbound send.
- Optional Logfire observability is disabled in the shipped configuration. Its
  direct exception capture bypasses Python logging filters; enabling it requires
  equivalent invite URL and exception redaction first.
- Manual browser evidence uses an isolated SQLite app and real WebAuthn via a
  headless Chromium virtual authenticator. Its authenticated app shell is a
  local fixture; no bridge is paired and no production endpoint is touched.
