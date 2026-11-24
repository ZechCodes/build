"""Cloudflare ownership rules for adoption and deletion."""

from dataclasses import dataclass
from uuid import UUID

from buildapp.cloudflare_tunnels import BridgeZone, DNSRecord, Inventory, Tunnel
from buildapp.hosted_tunnel_models import HostedTunnelAssignment


class OwnershipCollision(ValueError):
    pass


@dataclass(frozen=True)
class OwnedResources:
    tunnel: Tunnel | None
    dns: DNSRecord | None


def _exact_match(resources, name, stored_id):
    matching = [resource for resource in resources.values() if resource.name == name]
    if len(matching) > 1 or (stored_id in resources and resources[stored_id].name != name):
        raise OwnershipCollision
    found = matching[0] if matching else None
    if found is not None and stored_id is not None and found.id != stored_id:
        raise OwnershipCollision
    return found


def check_tunnel(zone: BridgeZone, label: str, tunnel: Tunnel) -> None:
    if tunnel.name != zone.tunnel_name(label) or tunnel.config_src != "local":
        raise OwnershipCollision


def check_dns(zone: BridgeZone, label: str, record: DNSRecord, tunnel_id: UUID | None) -> UUID:
    target = record.target_tunnel
    if (
        target is None
        or (tunnel_id is not None and target != tunnel_id)
        or record.content != f"{target}.cfargotunnel.com"
        or record.name != zone.hostname(label)
        or not record.proxied
        or record.comment != zone.dns_comment(label, target)
    ):
        raise OwnershipCollision
    return target


def owned_resources(
    inventory: Inventory, zone: BridgeZone, label: str, row: HostedTunnelAssignment | None,
) -> OwnedResources:
    tunnel_id, dns_id = (row.tunnel_id, row.dns_record_id) if row is not None else (None, None)
    tunnel = _exact_match(inventory.tunnels, zone.tunnel_name(label), tunnel_id)
    if tunnel is not None:
        check_tunnel(zone, label, tunnel)
        if row is not None and not row.tunnel_create_started:
            raise OwnershipCollision
        tunnel_id = tunnel.id
    record = _exact_match(inventory.dns, zone.hostname(label), dns_id)
    if record is not None:
        target = check_dns(zone, label, record, tunnel_id)
        if target in inventory.tunnels and inventory.tunnels[target] != tunnel:
            raise OwnershipCollision
    return OwnedResources(tunnel, record)
