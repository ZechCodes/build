"""Two disposable API stores, real controller/ownership/session guards, fresh keys.

The relay's internal lookups use each stack's own environment, just as separate
app processes do. No daemon, production account or external service is used.
"""
import base64
from contextlib import ExitStack
from types import SimpleNamespace
from uuid import uuid4

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey
from litestar.middleware.session.client_side import CookieBackendConfig
from litestar.testing import TestClient

from buildapp import internal_auth, pairing_crypto
from buildapp.db_test_support import asgi_app, in_memory_session_maker, sign_in_member
from buildapp.devices_controller import DevicesController
from buildapp.models import Device


@pytest.fixture()
def stacks(monkeypatch):
    monkeypatch.setattr(internal_auth, "get_settings", lambda: SimpleNamespace(
        internal_api=SimpleNamespace(allow_localhost=False)
    ))
    with ExitStack() as resources:
        clients = []
        for name in ("prod", "dev"):
            session = CookieBackendConfig(secret=(name.ljust(32, "-")).encode(), secure=True, httponly=True)
            app = asgi_app([DevicesController], session_maker=in_memory_session_maker(), session_config=session)
            client = resources.enter_context(TestClient(app, base_url=f"https://{name}.test", session_config=session))
            sign_in_member(client, f"{name}@example.com")
            clients.append(client)
        yield clients


def pending_device(client, code):
    device = Device(id=uuid4(), name="throwaway", approved=False,
                    pairing_code_hash=pairing_crypto.hash_code(code),
                    identity_public_key_b64=base64.b64encode(Ed25519PrivateKey.generate().public_key().public_bytes_raw()).decode(),
                    transport_public_key_b64=base64.b64encode(X25519PrivateKey.generate().public_key().public_bytes_raw()).decode())

    async def store():
        async with client.app.state.make_session() as session:
            session.add(device)
            await session.commit()

    with client.portal() as portal:
        portal.call(store)
    return str(device.id)


def test_separate_device_lists_codes_tokens_and_internal_secrets(stacks, monkeypatch):
    prod, dev = stacks
    for own, other, name in ((prod, dev, "prod"), (dev, prod, "dev")):
        code = f"{name}-disposable-code"
        device_id = pending_device(own, code)
        assert own.post("/api/devices/lookup", json={"code": code}).status_code == 201
        assert other.post("/api/devices/lookup", json={"code": code}).status_code == 404
        assert other.post("/api/devices/approve", json={"code": code}).status_code == 404
        assert own.post("/api/devices/approve", json={"code": code}).is_success
        own_devices = own.get("/api/devices").json()["devices"]
        assert device_id in [d["id"] for d in own_devices]
        assert device_id not in [d["id"] for d in other.get("/api/devices").json()["devices"]]
        token = own.post("/api/gateway-token").json()["token"]
        for target, secret, expected in ((own, name, 200), (other, "dev" if name == "prod" else "prod", 404)):
            monkeypatch.setenv("INTERNAL_API_SECRET", f"{secret}-test-secret")
            headers = {"x-internal-secret": f"{secret}-test-secret"}
            assert target.get(f"/internal/gateway-token/{token}", headers=headers).status_code == expected
            assert target.get(f"/internal/devices/{device_id}", headers=headers).status_code == expected
        monkeypatch.setenv("INTERNAL_API_SECRET", f"{name}-test-secret")
        assert own.get(f"/internal/devices/{device_id}").status_code == 404
        assert own.get(f"/internal/devices/{device_id}", headers={"x-internal-secret": "wrong-stack-secret"}).status_code == 404


def test_copying_a_session_cookie_does_not_authenticate_to_other_stack(stacks):
    prod, dev = stacks
    for own, other in ((prod, dev), (dev, prod)):
        original = dict(other.cookies)
        other.cookies.clear()
        # Explicitly transplant the value onto the other host, bypassing the
        # browser's host-only cookie boundary to exercise signing-key isolation.
        for key, value in own.cookies.items():
            other.cookies.set(key, value)
        response = other.get("/api/devices", follow_redirects=False)
        assert response.status_code in (401, 403)
        other.cookies.clear()
        other.cookies.update(original)
