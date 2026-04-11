"""DB helpers and notification helpers."""

from __future__ import annotations

import logging
from typing import Any
from uuid import UUID

from sqlalchemy import update
from sqlalchemy.ext.asyncio import AsyncSession

from skrift.lib.notifications import notify_user, NotificationMode

from build_app.models import Device

logger = logging.getLogger(__name__)


async def set_device_status(
    db_session: AsyncSession,
    device: Device,
    status: str,
) -> None:
    """Update device status in the DB."""
    await db_session.execute(
        update(Device)
        .where(Device.id == device.id)
        .values(status=status)
    )
    await db_session.commit()


async def notify_device_event(
    owner_user_id: UUID,
    event_type: str,
    device_id: UUID,
    device_name: str,
    **extra: Any,
) -> None:
    """Fire a time-series notification for the dashboard."""
    await notify_user(
        str(owner_user_id),
        f"build:device:{event_type}",
        mode=NotificationMode.TIMESERIES,
        push_notify=False,
        device_id=str(device_id),
        device_name=device_name,
        **extra,
    )


async def create_device_record(
    db_session: AsyncSession,
    name: str,
    public_key_b64: str,
    owner_user_id: UUID,
) -> Device | None:
    """Create a new device record if the name isn't taken.

    Returns the Device on success, None if a duplicate name exists.
    """
    existing = await db_session.execute(
        select(Device).where(
            Device.name == name,
            Device.owner_user_id == owner_user_id,
        )
    )
    if existing.scalar_one_or_none():
        return None

    device = Device(
        name=name,
        public_key=public_key_b64,
        owner_user_id=owner_user_id,
        approved=True,
        status="offline",
    )
    db_session.add(device)
    await db_session.commit()
    await db_session.refresh(device)
    return device
