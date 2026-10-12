"""Print test vectors using the imported binding on PYTHONPATH (no source writes).

Run against a Git-blob export of the pinned source for provenance. Keys here are
public test material. Never use these keys/nonces for any live session.
"""
import copy
import json

from nacl import public, signing
from build_secure_transport import (
    build_session_accept, decrypt_envelope, encrypt_frame, fingerprint_identity_key,
    sign_transport_key,
)
from build_secure_transport._protocol import b64decode, b64encode

PIN = '4bce3aacebd66a9674aaa96c6ec0d97ab4bcd793'


def main():
    identity = signing.SigningKey(bytes(range(32)))
    transport = public.PrivateKey(bytes(range(32, 64)))
    key = bytes(range(64, 96))
    signature = sign_transport_key(b64encode(bytes(identity)), b64encode(bytes(transport.public_key)))
    bad_signature = bytearray(b64decode(signature))
    bad_signature[0] ^= 1
    vector = {
        'upstream_commit': PIN,
        'identity_seed': b64encode(bytes(identity)),
        'identity_public': b64encode(bytes(identity.verify_key)),
        'transport_private': b64encode(bytes(transport)),
        'transport_public': b64encode(bytes(transport.public_key)),
        'wrong_transport_private': b64encode(bytes(range(96, 128))),
        'session_key': b64encode(key),
        'signature': signature,
        'bad_signature': b64encode(bad_signature),
        'fingerprint': fingerprint_identity_key(b64encode(bytes(identity.verify_key))),
        # Sealed boxes are randomized. Capture once and assert decryption only.
        'session_init': {'session_id': 'vector-session', 'device_id': 'fixture-device',
                         'wrapped_session_key': b64encode(public.SealedBox(transport.public_key).encrypt(key))},
        'accept': build_session_accept(b64encode(key), 'vector-session', 'client:fixture',
                                       b64encode(bytes(range(24)))),
        'frames': [],
    }
    examples = [
        ('unicode-nested', 'data', {'text': 'héllo 世界 🔐', 'nested': [True, None, {'z': 2, 'a': 1}]}),
        ('number-key-order', 'data', {'10': 'ten', '2': 'two', 'value': 1.0}),
        ('close', 'close', None),
    ]
    for index, (name, kind, payload) in enumerate(examples):
        outer = {'session_id': 'vector-session', 'route_to': 'device:fixture'}
        fields = {'frame_type': kind, 'sender': 'client', 'message_id': f'fixture-{index}',
                  'created_at': '2026-01-01T00:00:00Z', 'payload': payload}
        # Distinct nonces for every distinct plaintext, including producer variants.
        nonce = b64encode(bytes([index + 1]) * 24)
        envelope = encrypt_frame(vector['session_key'], outer, fields, nonce)
        vector['frames'].append({'name': name, 'outer': outer, 'fields': fields,
                                 'nonce': nonce, 'js_nonce': b64encode(bytes([index + 17]) * 24),
                                 'plaintext': decrypt_envelope(vector['session_key'], envelope),
                                 'envelopes': {'python': envelope}})
    base = vector['frames'][0]['envelopes']['python']
    cases = []
    for name, version in [('missing-version', 'missing'), ('null-version', None),
                          ('boolean-version', True), ('float-version', 1.0), ('bad-version', 2)]:
        env = copy.deepcopy(base)
        if version == 'missing':
            env.pop('version')
        else:
            env['version'] = version
        cases.append((name, env))
    for name, field, value in [
        ('session-mismatch', 'session_id', 'wrong-session'),
        ('invalid-base64', 'ciphertext', '!not-base64'),
        ('padded-base64', 'ciphertext', base['ciphertext'] + '=' * (-len(base['ciphertext']) % 4)),
        ('unknown-field', 'extra', 'ignored'),
    ]:
        cases.append((name, {**base, field: value}))
    tampered = bytearray(b64decode(base['ciphertext']))
    tampered[0] ^= 1
    cases.append(('tampered-ciphertext', {**base, 'ciphertext': b64encode(tampered)}))
    vector['envelope_cases'] = []
    for name, envelope in cases:
        try:
            decrypt_envelope(vector['session_key'], envelope)
            accepts = True
        except Exception:
            accepts = False
        vector['envelope_cases'].append({'name': name, 'envelope': envelope,
                                         'accepts': {'python': accepts}})
    print(json.dumps(vector, ensure_ascii=False, sort_keys=True, indent=2))


if __name__ == '__main__':
    main()
