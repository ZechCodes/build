from __future__ import annotations

import json
import sys

from build_secure_transport import (
    build_session_accept,
    decrypt_envelope,
    encrypt_frame,
    generate_identity_keypair,
    generate_transport_keypair,
    open_session_init,
    sign_transport_key,
)


def main() -> int:
    request = json.load(sys.stdin)
    action = request["action"]

    if action == "generate_transport_bundle":
        identity = generate_identity_keypair()
        transport = generate_transport_keypair()
        response = {
            "identity_public_key_b64": identity["public_key_b64"],
            "transport_public_key_b64": transport["public_key_b64"],
            "transport_private_key_b64": transport["private_key_b64"],
            "signature_b64": sign_transport_key(identity["private_key_b64"], transport["public_key_b64"]),
        }
    elif action == "open_session_init":
        response = open_session_init(request["transport_private_key_b64"], request["session_init"])
    elif action == "build_session_accept":
        response = build_session_accept(
            request["session_key_b64"],
            request["session_id"],
            request["route_to"],
        )
    elif action == "decrypt_envelope":
        response = decrypt_envelope(request["session_key_b64"], request["envelope"])
    elif action == "encrypt_frame":
        response = encrypt_frame(
            request["session_key_b64"],
            request["outer_fields"],
            request["frame_fields"],
        )
    elif action == "fingerprint_identity_key":
        from build_secure_transport import fingerprint_identity_key

        response = {"fingerprint": fingerprint_identity_key(request["identity_public_key_b64"])}
    else:
        raise ValueError(f"unsupported action: {action}")

    json.dump(response, sys.stdout, sort_keys=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
