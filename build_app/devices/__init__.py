"""Device management package — re-exports for backward compatibility."""

from build_app.devices.controller import DeviceApiController
from build_app.devices._state import (
    ConnectedDevice,
    E2ESession,
    HEARTBEAT_TIMEOUT_MULTIPLIER,
    MAX_ENVELOPE_SIZE,
    MAX_MISSED_WINDOWS,
    PENDING_EXPIRY_S,
    SESSION_TTL_S,
    _connected_devices,
    _e2e_sessions,
    _pending_registrations,
    cleanup_expired_pending,
    cleanup_expired_sessions,
    get_connected_devices,
    get_e2e_sessions,
)
from build_app.devices._crypto import (
    load_public_key,
    verify_device_signature,
    normalize_b64_padding,
)
from build_app.devices._helpers import (
    notify_device_event,
    record_heartbeat,
    record_missed_window,
    set_device_status,
    create_device_record,
)
from build_app.devices._heartbeat import (
    start_heartbeat_monitor,
    stop_heartbeat_monitor,
)

__all__ = [
    "DeviceApiController",
    "ConnectedDevice",
    "E2ESession",
    "HEARTBEAT_TIMEOUT_MULTIPLIER",
    "MAX_ENVELOPE_SIZE",
    "MAX_MISSED_WINDOWS",
    "PENDING_EXPIRY_S",
    "SESSION_TTL_S",
    "_connected_devices",
    "_e2e_sessions",
    "_pending_registrations",
    "cleanup_expired_pending",
    "cleanup_expired_sessions",
    "get_connected_devices",
    "get_e2e_sessions",
    "load_public_key",
    "verify_device_signature",
    "normalize_b64_padding",
    "notify_device_event",
    "record_heartbeat",
    "record_missed_window",
    "set_device_status",
    "create_device_record",
    "start_heartbeat_monitor",
    "stop_heartbeat_monitor",
]
