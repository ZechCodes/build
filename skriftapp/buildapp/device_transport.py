"""Issuer policy and transport discovery for an approved pairing."""

from sqlalchemy import update
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.bridge_http_auth import valid_endpoint
from buildapp.bridge_http_models import BridgeHttpEndpoint
from buildapp.models import Device


async def mark_hosted_ready_once(db_session: AsyncSession, pairing: Device) -> bool:
    """Latch first readiness for this pairing in the caller's transaction."""
    if not pairing.approved or pairing.owner_user_id is None:
        return False
    # A revoked row can be purged and later recreated with the same device ID.
    # The creation stamp fences that case, even if the new row restarts at epoch 0.
    updated = await db_session.scalar(
        update(Device)
        .where(
            Device.id == pairing.id,
            Device.owner_user_id == pairing.owner_user_id,
            Device.approved.is_(True),
            Device.authorization_epoch == pairing.authorization_epoch,
            Device.created_at == pairing.created_at,
            Device.hosted_ready_once.is_(False),
        )
        .values(hosted_ready_once=True)
        .returning(Device.id)
    )
    return updated is not None


async def rtc_allowed(db_session: AsyncSession, device: Device) -> bool:
    """Shared issuer decision. Entitlement enforcement arrives in slice 3."""
    return True


async def transport_projection(
    db_session: AsyncSession, device: Device, endpoint: BridgeHttpEndpoint | None,
) -> dict:
    origin = endpoint.origin if endpoint is not None and valid_endpoint(endpoint, device) else None
    return {
        "source": "personal" if origin is not None else "webrtc",
        "rtc_allowed": await rtc_allowed(db_session, device),
        "reason": None,
        "origin": origin,
        "assignment_revision": None,
    }
