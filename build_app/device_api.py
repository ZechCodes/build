"""Backward-compatibility shim — all code moved to build_app.devices package."""

# Re-export everything so existing imports (tests, etc.) still work.
from build_app.devices import *  # noqa: F401, F403
from build_app.devices import (
    DeviceApiController,
    ConnectedDevice,
    E2ESession,
    HEARTBEAT_TIMEOUT_MULTIPLIER,
    MAX_MISSED_WINDOWS,
    _connected_devices,
    _e2e_sessions,
    _pending_registrations,
    get_connected_devices,
    get_e2e_sessions,
    start_heartbeat_monitor,
    stop_heartbeat_monitor,
)

# Aliases for old underscore-prefixed names.
_load_public_key = load_public_key  # noqa: F405
_verify_device_signature = verify_device_signature  # noqa: F405
_record_heartbeat = record_heartbeat  # noqa: F405
_record_missed_window = record_missed_window  # noqa: F405
_notify_device_event = notify_device_event  # noqa: F405
_set_device_status = set_device_status  # noqa: F405
_cleanup_expired_pending = cleanup_expired_pending  # noqa: F405
