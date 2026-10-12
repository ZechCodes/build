> Historical protocol document imported from the pinned upstream commit in
> [UPSTREAM.md](UPSTREAM.md). Its relay data plane and relay-owned public-key
> description predate Build's current topology. See
> [ARCHITECTURE.md](../ARCHITECTURE.md#system-topology): the API owns
> presence/device keys, the relay handles signaling, and application traffic
> uses WebRTC data channels. This annotation changes no crypto behavior.

# E2E Encrypted Messaging - MVP v2

An end-to-end encryption protocol for authenticated message passing between a client session and a device through a zero-trust relay.

The relay routes opaque bytes between peers. It knows who is talking to whom. It never sees plaintext, private keys, or session keys. Everything beyond routing is the peers' responsibility.

---

## Goals

- Establish a shared session key between the client and device through the relay
- Protect message content from relay-side plaintext access in both modes
- Support two trust modes with clearly different guarantees: low-friction standard mode and higher-assurance verified-device mode

## Non-Goals for MVP

- Full zero-knowledge metadata hiding
- Multi-device group key management
- Forward secrecy across the full lifetime of a device key
- Reliability, ordering, retries, or delivery guarantees (peer responsibility)

---

## Actors

### Device

A long-lived process or machine with a stable public/private keypair.

- Identified by `device_id`
- Holds a long-lived X25519 keypair
- May optionally hold a long-lived `Ed25519` identity key for verified-device mode
- Maintains a connection to the relay when online

### Client Session

A short-lived UI or API session that wants to exchange messages with a device.

- Identified by `session_id`
- Generates a fresh random symmetric session key per session

### Relay

A routing layer. Not a message broker.

- Authenticates connections and authorizes which client session may talk to which device
- Serves device public keys to clients
- Forwards opaque envelopes between peers
- Never receives device private keys, session keys, or plaintext

---

## Crypto Stack

| Purpose | Algorithm | Notes |
|---------|-----------|-------|
| Device identity (optional) | Ed25519 | Long-lived signing key for verified-device mode |
| Device transport key | X25519 | Stable keypair per device |
| Session key wrapping | libsodium sealed box | Encrypt a session key to the device public key |
| Message encryption | XSalsa20-Poly1305 (`secretbox`) | Encrypt inner frames with the session key |
| Randomness | CSPRNG | 32-byte session keys, 24-byte nonces |

Why this shape:

- The device keeps one durable transport key.
- Each client session creates one fresh symmetric session key.
- The relay only ever touches ciphertext it cannot decrypt.
- Verified-device mode adds a signing identity the relay cannot forge after pairing.

---

## Trust Modes

### Standard Mode

The default low-friction mode.

- The client obtains device public keys through the relay
- The user trusts the relay not to substitute device keys
- The relay cannot read payloads without the corresponding private key
- The relay should not persist plaintext or encrypted application blobs beyond in-memory transport

Goal in this mode:

- Keep plaintext and session keys off the relay
- Limit the relay to secure in-memory transport of encrypted session bootstrap and encrypted message frames

This mode protects content from routine server-side exposure and storage. It does not protect against a malicious relay distributing fake device keys, replaying traffic, or selectively dropping messages.

### Verified-Device Mode

The optional higher-assurance mode.

- The device generates a long-lived `Ed25519` identity key locally
- The user copies, pastes, or scans the device public key or fingerprint into the client out-of-band
- The client pins that identity key
- The device signs its X25519 transport key with the pinned identity key
- The client rejects unsigned or incorrectly signed transport keys

Goal in this mode:

- Treat the relay as zero-trust for key authenticity and message contents
- Guarantee that any delivered message accepted by a peer was not forged or modified by the relay

After pairing, the relay cannot substitute the device's transport key, forge valid messages, or tamper with accepted messages without detection. It can still drop, delay, or replay traffic unless higher layers address availability behavior.

---

## Session Bootstrap

The client creates a fresh session key and delivers it to the device through the relay.

```
Client Session                 Relay                      Device
     |                           |                           |
     | request device key        |                           |
     |-------------------------->|                           |
     |                           |                           |
     | device_public_key         |                           |
     |<--------------------------|                           |
     |                           |                           |
     | [verified-device mode:    |                           |
     |  verify Ed25519 signature |                           |
     |  on device_public_key     |                           |
     |  against pinned identity] |                           |
     |                           |                           |
     | generate session_key      |                           |
     | wrap = sealed_box(        |                           |
     |   device_public_key,      |                           |
     |   session_key             |                           |
     | )                         |                           |
     |                           |                           |
     | session_init              |                           |
     | {session_id,              |                           |
     |  device_id,               |                           |
     |  wrapped_session_key}     |                           |
     |-------------------------->|-------------------------->|
     |                           |                           |
     |                           |     session_key =         |
     |                           |     sealed_box.open(...)  |
     |                           |                           |
     |                           |     session_accept =      |
     |                           |     secretbox(            |
     |                           |       session_key,        |
     |                           |       {session_id},       |
     |                           |       nonce               |
     |                           |     )                     |
     |                           |                           |
     |                           |<--------------------------|
     |<--------------------------|   session_accept          |
     |                           |   {nonce, ciphertext}     |
     |                           |                           |
     | decrypt session_accept    |                           |
     | verify session_id matches |                           |
```

After `session_accept`, both sides hold the same `session_key`. The encrypted accept is cryptographic proof the device unwrapped the session key. If the client cannot decrypt the accept or the `session_id` inside does not match, the session must be aborted.

---

## Envelope Format

Every message between peers uses the same two-layer structure.

### Outer Envelope (relay-visible)

- `version`
- `session_id`
- `route_to`
- `nonce`
- `ciphertext`

The relay reads only this layer. It uses `session_id` and `route_to` for routing. Everything else is opaque. The relay must verify that `route_to` matches the authorized peer for the given `session_id` and reject envelopes that do not match.

### Inner Frame (peer-visible, encrypted)

- `session_id`
- `message_id`
- `frame_type` (`data` or `close`)
- `sender` (`client` or `device`)
- `created_at`
- `payload`

A frame is valid only if it decrypts successfully and the inner `session_id` matches the outer `session_id`.

This is the core integrity boundary: if a peer accepts a frame, the relay did not modify its contents.

### Sending a Message

```
Sender
  1. Build inner frame {session_id, message_id, frame_type, sender, created_at, payload}
  2. Generate random 24-byte nonce
  3. Encrypt ciphertext = secretbox(session_key, inner_frame, nonce)
  4. Send outer envelope {version, session_id, route_to, nonce, ciphertext} to relay

Relay
  5. Forward outer envelope to the connected peer for that session

Peer
  6. Decrypt ciphertext using session_key
  7. Verify inner session_id matches outer session_id
  8. Process inner frame
```

### Nonce Requirements

Nonces are 24 bytes from a CSPRNG for every encrypt operation. XSalsa20's nonce space (2^192) makes random collision negligible at any realistic message volume. Implementations must not use sequential counters — random nonces survive retransmission and reordering and avoid nonce reuse on process restart.

---

## Session Close

### Authenticated Close

A peer with a valid session key sends an inner frame with `frame_type: close`. This is encrypted and authenticated like any other frame. The other peer can trust it came from the real session participant.

### Local Crypto Failure

If decryption fails or the inner and outer `session_id` values do not match, the receiving side closes the session locally:

- Drop the invalid envelope
- Discard the session key and session state
- Disconnect the session at the transport level

No peer-authenticated error frame is sent because the session may already be compromised.

Control frames are authenticated when they come from a peer. Transport-level closure is never treated as peer-authored.

---

## Key Lifecycle

### Device Keys

- Long-lived X25519 transport keypair registered with the relay
- In standard mode, clients use the relay-provided public key directly
- In verified-device mode, the transport key must be signed by the pinned `Ed25519` identity key

### Device Identity Keys (Verified-Device Mode)

- Long-lived `Ed25519` keypair generated on the device
- Public key or fingerprint exported to the client via QR or copy/paste
- Regenerating the identity key invalidates all client pins and requires re-pairing

### Session Keys

- Fresh random 32-byte symmetric key per client session
- Created by the client, wrapped to the device via sealed box
- Deleted by both sides when the session closes
- A single session key should not encrypt unbounded traffic — peers should establish new sessions periodically (recommended: after 10,000 messages or 1 hour)

### Key Rotation

- Rotating a device transport key invalidates bootstrap until clients refresh
- In verified-device mode, the new transport key must be re-signed by the same identity key or the user must re-pair
- Device loss or reinstall should trigger transport key replacement and pin invalidation

---

## What the Relay Sees

- Device IDs and connection state
- Client session IDs
- Which session is talking to which device
- Envelope timing and sizes
- Device public keys
- Opaque ciphertext

## What the Relay Does Not See

- Session keys
- Device private keys
- Plaintext message content
- Application semantics inside inner frames

---

## Security Properties

| Property | How |
|----------|-----|
| Payload confidentiality | Inner frames encrypted with `secretbox` under a session key the relay never holds |
| Payload integrity | `secretbox` authentication detects any modification to accepted inner frames |
| Device-only session bootstrap | Session key wrapped to the device public key via sealed box |
| Bootstrap verification | Encrypted `session_accept` proves the device holds the private key |
| Relay tamper resistance | All peer-visible semantics live inside the authenticated inner frame |
| Authenticated session close | `close` frames are encrypted under the session key and cannot be forged by the relay |
| Bounded session exposure | Periodic new sessions limit single-key blast radius |
| Optional relay-independent key authenticity | Verified-device mode pins an `Ed25519` identity outside the relay path |

### Mode-Specific Guarantee Summary

#### Standard Mode

- The relay should never hold plaintext or persist encrypted application payloads beyond in-memory transport
- The relay does not learn session keys or message contents
- The relay is still trusted for initial device key distribution

#### Verified-Device Mode

- The relay is not trusted for device key authenticity
- Accepted messages cannot be forged or modified by the relay without detection
- The relay still retains availability power: it can drop, delay, or replay traffic

---

## Deferred

- Forward secrecy beyond session-key turnover
- Key transparency or signed key directory
- Mutual cryptographic authentication between client sessions and devices
- Multi-device fanout or group sessions
- Reliability, ordering, retry, and delivery guarantees (peer-layer concerns)
- Traffic analysis resistance
- Guarantees against malicious hosted web clients

---

## Summary

For MVP v2:

- devices are long-lived and own stable transport keys
- client sessions are short-lived and create fresh session keys
- the relay is a router for encrypted bootstrap and encrypted message frames
- standard mode trusts the relay for device key distribution and aims to keep plaintext and persistent blobs off the relay
- verified-device mode pins an `Ed25519` identity out-of-band so the relay cannot substitute keys or tamper with accepted messages
- encrypted `session_accept` proves the device unwrapped the session key
- peers own everything beyond routing: reliability, ordering, retries, application semantics
