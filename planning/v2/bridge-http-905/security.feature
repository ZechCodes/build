# Normative acceptance specifications for implementation TDD; no step runner or
# production HTTP adapter exists in this design-only task. Not passing evidence.
@http_contract @pending_implementation
Feature: Account-owned optional HTTPS interface
  Background:
    Given isolated hosted accounts Alice and Bob
    And Alice owns approved paired bridge A at "https://a.example:8443"
    And Bob owns approved paired bridge B at "https://b.example:8443"
    And all clocks, certificates, network authorities and stores are test fixtures

  Scenario: Disabled listener preserves WebRTC
    Given HTTP enablement is absent
    When the daemon starts its shared workers
    Then no HTTP port is bound or endpoint registered
    And the existing authenticated WebRTC path remains available

  Scenario Outline: Invalid enabled configuration fails before binding
    Given HTTP is explicitly enabled with <defect>
    When configuration is validated
    Then startup fails without a listening socket or private-key disclosure
    Examples:
      | defect |
      | a plaintext origin |
      | no explicit bind address |
      | an unreadable or mismatched TLS key |
      | a wildcard CORS origin |
      | dev state or issuer resolving to production |

  Scenario: Owner discovery succeeds without hosted network access
    Given A registered its configured origin using a fresh paired-key proof
    And Alice has a pending discovery challenge for A's exact origin
    When Alice directly fetches A's proof and submits it to hosted completion
    Then hosted verifies the stored paired key and current ownership
    And atomically consumes the challenge and issues an endpoint-scoped credential
    And a hosted outbound-request spy records no lookup or fetch of the entered URL
    And no hosted cookie or OAuth token reaches A

  Scenario Outline: Entered URL normalization rejects ambiguous authority
    When Alice enters <url>
    Then no candidate CSP origin or credential is created
    Examples:
      | url |
      | http://a.example |
      | https://alice:secret@a.example |
      | https://a.example/path |
      | https://a.example/?token=x |
      | https://a.example/#fragment |
      | https://a.example. |
      | https://[fe80::1%25eth0] |

  Scenario: Foreign account cannot add a reachable bridge
    Given Alice discovers a genuine current proof from B
    When Alice submits it to hosted completion
    Then no credential, registration change or protected B data is returned

  Scenario: Forwarded challenge cannot substitute endpoint identity
    Given Alice's pending origin is "https://evil.example"
    And evil forwards Alice's nonce to A
    When evil returns A's genuine signed proof to Alice
    Then hosted rejects because A's configured signed origin differs from pending origin
    And changing the signed origin or device makes signature verification fail
    And A never signs an origin obtained from Host or Forwarded headers

  Scenario Outline: Invalid or replayed device proof is rejected
    Given a proof with <defect>
    When hosted validates it
    Then no credential or endpoint registration is committed
    Examples:
      | defect |
      | wrong device public key |
      | a different account's pending challenge |
      | stale endpoint epoch |
      | wrong issuer or exact audience |
      | unsupported JWT type, algorithm, crit or remote key URL |
      | expired or future issued-at time beyond allowed skew |
      | consumed nonce or repeated jti |
      | reused registration proof presented as a discovery proof |

  Scenario: Concurrent challenge completion has one winner
    When two transactions complete the same discovery challenge concurrently
    Then exactly one credential family is created
    And a lost response requires a new guarded challenge rather than secret recovery

  Scenario: Token cannot cross device, purpose or endpoint
    Given Alice has an HTTP access token for A's current origin and epoch
    When it is presented to B, a replaced A endpoint, the relay or a WebRTC opening
    Then no request is admitted
    And an account-wide gateway or desktop OAuth token is also refused by A

  Scenario: Bridge introspection authenticates the target device freshly
    Given a token for A
    When A uses a correctly signed client assertion for the fixed introspection URL
    Then hosted returns the current immutable token binding
    But a repeated jti, expired assertion, wrong audience or B's assertion is refused
    And filling replay storage fails closed rather than forgetting live entries

  Scenario: Caller fields cannot create verified authority
    Given Alice's valid request contains Bob's account ID or B's session handle
    When HTTP normalizes the request
    Then only verifier-created authority can enter ClientSessions or execution
    And the request cannot access another family, device, session or subscription

  Scenario: Cached authorization has a hard bounded lifetime
    Given introspection starts at monotonic time 100 and responds at time 102
    And the access token expires after time 130
    Then the grant deadline is no later than time 130
    And a response arriving after that deadline cannot create a usable grant
    And every admission and reply checks the current grant and generation

  Scenario: Hosted outage closes HTTP authority without affecting existing WebRTC
    Given an HTTP stream holds a grant expiring at time 130
    And proactive revalidation times out
    When time reaches 130
    Then no further HTTP request or queued event is admitted or emitted
    And its opening and stream bindings close without waiting for network I/O
    And an uncached HTTP token receives auth_unavailable without execution
    And WebRTC retains the configured-relay authorization availability baseline

  Scenario Outline: Revocation reaches all related HTTP targets
    Given an HTTP request and stream have a currently cached grant
    When <change> commits on the authority
    Then online revalidation rejects within the existing 30-second hard deadline
    And queued and delayed replies cannot reach a replacement opening or cache
    And accepted durable commands may settle without implying rollback
    Examples:
      | change |
      | device approval is revoked |
      | owner changes |
      | account membership is suspended |
      | credential family is revoked |
      | issuing account session signs out |
      | endpoint security epoch changes |

  Scenario: Token rotation preserves principal but not cross-family sessions
    Given a live HTTP session and an admitted durable operation
    When Alice mints a new token from the same family and binding
    Then verified refresh preserves the session and stable store principal
    And the old token ID is not used as operation identity
    But another family or epoch requires a new opening
    And WebRTC status/retry of that operation uses the same authorized principal

  Scenario: Redirects cannot leak credentials
    Given a discovery, application, desktop or introspection endpoint returns a redirect
    When the client receives it
    Then no follow-up request is sent to the redirect target
    And no bearer, hosted cookie or OAuth token appears in a URL or log

  Scenario: Browser candidate permission is narrow and has no authority
    Given production connect-src initially blocks A
    When Alice records a normalized candidate and reloads the hosted document
    Then only that candidate and bounded selected owned endpoints are added
    And another account's document and the expired candidate are excluded
    And no broad https scheme or raw header interpolation is used
    And network reachability alone cannot issue a credential

  Scenario: Revoked origins leave only residual stale-document reachability
    Given Alice loaded a document permitting A
    When A is revoked
    Then subsequent documents exclude A and active handles close
    And suspended stale documents cannot obtain a new credential
    And the test does not claim to retract CSP already delivered to a browser

  Scenario: Browser transport obeys certificate and local-network permissions
    Given a self-signed untrusted certificate or denied local-network permission
    When Alice tries Add by URL
    Then connection fails without bypassing certificate or browser permission checks
    And cached views and the separate WebRTC option remain usable

  Scenario: CORS is exact and separate from authentication
    Given an unknown, null or suffix-matching Origin
    When it requests discovery or a protected route
    Then it receives no permissive CORS headers or protected data
    And a successful OPTIONS request never creates authority
    And a native caller with no Origin still needs a valid target-specific bearer

  Scenario: Desktop uses a separate constrained device credential path
    Given existing renderer hooks strip all supplied Authorization
    When the trusted app invokes a typed operation on a verified native endpoint handle
    Then main attaches only that endpoint's device token
    And an arbitrary URL, header override, foreign frame or stale webContents is refused
    And hosted OAuth injection remains limited to the hosted origin
    And webSecurity, certificate checks and default permission policy stay enabled

  Scenario: Route adapters preserve shared execution and mutation admission
    When equivalent typed calls arrive over HTTP and WebRTC
    Then shared normalization, authorization, lanes and results agree
    And HTTP returns 202 only after the same journaled command admission commits
    And Idempotency-Key names the existing operation ID with no second job journal
    And loss after dispatch yields uncertain delivery rather than a blind retry

  Scenario: SSE replacement affects its output binding only
    Given an opening has a live SSE subscription and an accepted durable command
    When a new GET replaces the subscription's active event binding
    Then old queued events fail the generation fence
    And HTTP per-request reply leases remain independent
    And the durable command continues and can be polled by operation ID

  Scenario: Slow SSE reader cannot exhaust memory or stall unrelated calls
    Given a client stops consuming SSE bytes
    When producers exceed 128 queued events or 1 MiB or a write waits 10 seconds
    Then the binding coalesces safe hints or closes with best-effort reset indication
    And producer and socket-buffer memory remain within measured bounds
    And another client's published and SQL reads meet their existing latency bound

  Scenario: Transient stream never promises durable replay
    When a current live stream sends changes and keepalives
    Then it emits no durable id field and declares durable false
    And Last-Event-ID receives cursor_unsupported
    And reconnect uses existing epoch-fenced current-state refresh
    And no #900 task-page cursor is interpreted as a journal cursor

  @future_887_only
  Scenario: Durable resume advances only committed cache state
    Given a future negotiated #892 capability and a #893 cache consumer
    And an SSE batch was received but its cache transaction failed
    When the client reconnects
    Then it resumes from the previously atomically committed scope cursor
    And snapshots, replay, tombstones, filter exits and retention reset obey the future frozen #887 contract without an HTTP-specific journal
    And current deployment advertises no invented durable capability or RPC names

  Scenario: Build connection capability requires advertised view coverage
    Given only lifecycle and task-read automation resource routes are implemented
    When the SPA evaluates the HTTP Build connection capability
    Then it withholds the user-visible connection option
    And release acceptance requires all advertised nonterminal views and actions
    And terminal explicitly uses authenticated WebRTC or reports unavailable
    And no POST rpc or arbitrary method tunnel fills missing resource routes
