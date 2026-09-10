"""Shared-secret guard for the relay-facing ``/internal/*`` endpoints.

The relay authenticates its calls with ``X-Internal-Secret``, constant-time
compared against the ``INTERNAL_API_SECRET`` environment variable. Dev
configs may additionally allow unauthenticated localhost callers via the
``internal_api.allow_localhost`` app.yaml setting (off by default; the
production config must not enable it).

Failed checks raise ``NotFoundException`` so the internal routes stay
invisible to unauthorized callers.
"""

from __future__ import annotations

import hmac
import os

from litestar.connection import ASGIConnection
from litestar.exceptions import NotFoundException
from litestar.handlers import BaseRouteHandler
from pydantic import BaseModel

from skrift.config import get_settings, register_config_section
from skrift.lib.client_ip import get_client_ip

INTERNAL_SECRET_HEADER = "x-internal-secret"
INTERNAL_SECRET_ENV = "INTERNAL_API_SECRET"
_LOCALHOST_IPS = ("127.0.0.1", "::1")


class InternalApiConfig(BaseModel):
    """``internal_api:`` app.yaml section.

    ``allow_localhost`` lets localhost callers skip the shared secret — a dev
    convenience only; production keeps the default ``False``.
    """

    allow_localhost: bool = False


register_config_section("internal_api", InternalApiConfig)


def internal_request_allowed(
    presented_secret: str | None,
    configured_secret: str | None,
    client_ip: str | None,
    allow_localhost: bool,
) -> bool:
    """Decide whether an internal-endpoint request is authorized.

    Pure decision logic: a non-empty configured secret matched in constant
    time, or — only when explicitly enabled — a localhost caller.
    """
    if configured_secret and presented_secret is not None:
        if hmac.compare_digest(
            configured_secret.encode("utf-8"), presented_secret.encode("utf-8")
        ):
            return True
    return allow_localhost and client_ip in _LOCALHOST_IPS


async def internal_auth_guard(
    connection: ASGIConnection, _route_handler: BaseRouteHandler
) -> None:
    """Litestar guard for ``/internal/*`` routes (relay-facing)."""
    allowed = internal_request_allowed(
        presented_secret=connection.headers.get(INTERNAL_SECRET_HEADER),
        configured_secret=os.environ.get(INTERNAL_SECRET_ENV),
        client_ip=get_client_ip(connection.scope),
        allow_localhost=get_settings().internal_api.allow_localhost,
    )
    if not allowed:
        raise NotFoundException()  # don't reveal the internal route exists
