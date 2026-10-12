"""Fixed public test vectors; also runnable against the pinned pre-move source."""
import hashlib
import json
import os
from pathlib import Path
import unittest

from build_secure_transport import (
    build_session_accept, decrypt_envelope, encrypt_frame, fingerprint_identity_key,
    open_session_init, sign_transport_key, verify_transport_key_signature,
)
from build_secure_transport.exceptions import (
    DecryptionError, InvalidInputError, ProtocolError, VerificationError,
)

FIXTURES = Path(__file__).resolve().parents[1] / 'fixtures'


class CompatibilityVectors(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.vector = json.loads(Path(os.environ.get('BST_FIXTURES', FIXTURES / 'v1.json')).read_text())

    def test_fixture_digest(self):
        expected = (FIXTURES / 'SHA256SUMS').read_text().split()[0]
        self.assertEqual(hashlib.sha256((FIXTURES / 'v1.json').read_bytes()).hexdigest(), expected)

    def test_identity_and_captured_bootstrap(self):
        v = self.vector
        self.assertEqual(sign_transport_key(v['identity_seed'], v['transport_public']), v['signature'])
        verify_transport_key_signature(v['identity_public'], v['transport_public'], v['signature'])
        self.assertEqual(fingerprint_identity_key(v['identity_public']), v['fingerprint'])
        opened = open_session_init(v['transport_private'], v['session_init'])
        self.assertEqual(opened['session_key_b64'], v['session_key'])
        with self.assertRaises(VerificationError):
            verify_transport_key_signature(v['identity_public'], v['transport_public'], v['bad_signature'])
        with self.assertRaises(DecryptionError):
            open_session_init(v['wrong_transport_private'], v['session_init'])

    def test_accept_and_deterministic_frames_both_producers(self):
        v = self.vector
        self.assertEqual(build_session_accept(v['session_key'], 'vector-session', 'client:fixture',
                                             v['accept']['nonce']), v['accept'])
        for frame in v['frames']:
            with self.subTest(frame=frame['name']):
                self.assertEqual(encrypt_frame(v['session_key'], frame['outer'], frame['fields'],
                                               frame['nonce']), frame['envelopes']['python'])
                for producer, envelope in frame['envelopes'].items():
                    with self.subTest(producer=producer):
                        self.assertEqual(decrypt_envelope(v['session_key'], envelope), frame['plaintext'])

    def test_rejections_and_parser_baseline(self):
        v = self.vector
        for case in v['envelope_cases']:
            with self.subTest(case=case['name']):
                if case['accepts']['python']:
                    decrypt_envelope(v['session_key'], case['envelope'])
                else:
                    with self.assertRaises((DecryptionError, InvalidInputError, ProtocolError)):
                        decrypt_envelope(v['session_key'], case['envelope'])
