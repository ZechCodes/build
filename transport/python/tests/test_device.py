import unittest
from base64 import b64decode, b64encode

from build_secure_transport import (
    DecryptionError,
    ProtocolError,
    VerificationError,
    build_close_frame,
    build_session_accept,
    decrypt_envelope,
    encrypt_frame,
    fingerprint_identity_key,
    generate_identity_keypair,
    generate_session_key,
    generate_transport_keypair,
    open_session_init,
    sign_transport_key,
    verify_transport_key_signature,
)


class SecureTransportTests(unittest.TestCase):
    def test_verified_device_signature_round_trip(self) -> None:
        identity = generate_identity_keypair()
        transport = generate_transport_keypair()

        signature = sign_transport_key(identity["private_key_b64"], transport["public_key_b64"])
        verify_transport_key_signature(identity["public_key_b64"], transport["public_key_b64"], signature)
        self.assertEqual(len(fingerprint_identity_key(identity["public_key_b64"])), 64)

    def test_verified_device_signature_rejects_tamper(self) -> None:
        identity = generate_identity_keypair()
        transport = generate_transport_keypair()
        signature = sign_transport_key(identity["private_key_b64"], transport["public_key_b64"])
        other_transport = generate_transport_keypair()

        with self.assertRaises(VerificationError):
            verify_transport_key_signature(identity["public_key_b64"], other_transport["public_key_b64"], signature)

    def test_device_bootstrap_and_message_round_trip(self) -> None:
        from nacl import public

        session_key_b64 = generate_session_key()
        transport = generate_transport_keypair()
        session_init = {
            "session_id": "sess-123",
            "device_id": "dev-123",
            "wrapped_session_key": "",
        }

        recipient_key = public.PublicKey(b64decode(transport["public_key_b64"] + "=="))
        wrapped = public.SealedBox(recipient_key).encrypt(b64decode(session_key_b64 + "=="))
        session_init["wrapped_session_key"] = b64encode(wrapped).decode().rstrip("=")

        opened = open_session_init(transport["private_key_b64"], session_init)
        self.assertEqual(opened["session_key_b64"], session_key_b64)

        accept = build_session_accept(session_key_b64, "sess-123", "client-123")
        self.assertEqual(accept["session_id"], "sess-123")

        envelope = encrypt_frame(
            session_key_b64,
            outer_fields={"session_id": "sess-123", "route_to": "client-123"},
            frame_fields={"frame_type": "data", "sender": "device", "payload": {"hello": "world"}},
        )
        frame = decrypt_envelope(session_key_b64, envelope)
        self.assertEqual(frame["payload"], {"hello": "world"})
        self.assertEqual(frame["frame_type"], "data")

        close_envelope = build_close_frame(session_key_b64, "sess-123", "client-123", "device")
        close_frame = decrypt_envelope(session_key_b64, close_envelope)
        self.assertEqual(close_frame["frame_type"], "close")

    def test_decrypt_rejects_session_mismatch(self) -> None:
        session_key_b64 = generate_session_key()
        envelope = encrypt_frame(
            session_key_b64,
            outer_fields={"session_id": "sess-123", "route_to": "device-123"},
            frame_fields={"frame_type": "data", "sender": "client", "payload": {"x": 1}},
        )
        envelope["session_id"] = "other-session"

        with self.assertRaises(ProtocolError):
            decrypt_envelope(session_key_b64, envelope)

    def test_decrypt_rejects_tamper(self) -> None:
        session_key_b64 = generate_session_key()
        envelope = encrypt_frame(
            session_key_b64,
            outer_fields={"session_id": "sess-123", "route_to": "device-123"},
            frame_fields={"frame_type": "data", "sender": "client", "payload": {"x": 1}},
        )
        envelope["ciphertext"] = envelope["ciphertext"][:-1] + ("A" if envelope["ciphertext"][-1] != "A" else "B")

        with self.assertRaises(DecryptionError):
            decrypt_envelope(session_key_b64, envelope)


if __name__ == "__main__":
    unittest.main()
