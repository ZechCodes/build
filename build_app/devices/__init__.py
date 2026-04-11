"""Device management package."""

from build_app.devices.controller import DeviceApiController
from build_app.devices._state import (
    E2ESession,
    PENDING_EXPIRY_S,
    SESSION_TTL_S,
    _e2e_sessions,
    _pending_registrations,
    cleanup_expired_pending,
    cleanup_expired_sessions,
    get_e2e_sessions,
)
from build_app.devices._crypto import (
    load_public_key,
    verify_device_signature,
    normalize_b64_padding,
)
from build_app.devices._helpers import (
    notify_device_event,
    set_device_status,
    create_device_record,
)
from build_app.devices._relay import (
    send_to_device,
    disconnect_device,
    is_device_connected,
)

__all__ = [
    "DeviceApiController",
    "E2ESession",
    "PENDING_EXPIRY_S",
    "SESSION_TTL_S",
    "_e2e_sessions",
    "_pending_registrations",
    "cleanup_expired_pending",
    "cleanup_expired_sessions",
    "get_e2e_sessions",
    "load_public_key",
    "verify_device_signature",
    "normalize_b64_padding",
    "notify_device_event",
    "set_device_status",
    "create_device_record",
    "send_to_device",
    "disconnect_device",
    "is_device_connected",
]
