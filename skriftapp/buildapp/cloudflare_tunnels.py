"""Bounded Cloudflare API client for locally managed tunnels and proxied CNAMEs."""

import asyncio
import base64
import hashlib
import ipaddress
import json
import re
from dataclasses import dataclass
from urllib.parse import urlsplit
from uuid import UUID

import httpx

MAX_BODY = 2 * 1024 * 1024
MAX_PAGES = 1000
ID = re.compile(r"[A-Za-z0-9_-]{1,64}")
LABEL = r"[0-9a-f]{32}"
DNS_LABEL = re.compile(r"[a-z0-9](?:[-a-z0-9]{0,61}[a-z0-9])?")


class ProviderError(RuntimeError):
    def __init__(self, reason: str, *, retryable: bool = False):
        super().__init__(f"Cloudflare {reason}")
        self.retryable = retryable


@dataclass(frozen=True)
class Tunnel:
    id: UUID
    name: str
    config_src: str | None


@dataclass(frozen=True)
class DNSRecord:
    id: str
    name: str
    type: str
    content: str
    proxied: bool
    comment: str | None

    @property
    def target_tunnel(self) -> UUID | None:
        if self.type == "CNAME" and self.content.endswith(".cfargotunnel.com"):
            try:
                return UUID(self.content.removesuffix(".cfargotunnel.com"))
            except ValueError:
                pass
        return None


@dataclass(frozen=True)
class BridgeZone:
    name: str

    def __post_init__(self):
        name = self.name.lower().rstrip(".")
        if len(name) > 218 or "." not in name or not all(DNS_LABEL.fullmatch(p) for p in name.split(".")):
            raise ValueError("invalid hosted bridge zone")
        object.__setattr__(self, "name", name)

    @property
    def tunnel_prefix(self) -> str:
        # Tunnel inventories are account-wide, while each environment owns a DNS
        # zone. The zone-derived prefix keeps sweeps out of other environments.
        scope = hashlib.sha256(self.name.encode("ascii")).hexdigest()[:24]
        return f"build-{scope}-"

    def hostname(self, label: str) -> str:
        return f"b-{label}.{self.name}"

    def tunnel_name(self, label: str) -> str:
        return self.tunnel_prefix + label

    def dns_comment(self, label: str, tunnel_id: UUID) -> str:
        return f"{self.tunnel_name(label)}:{tunnel_id}"

    def label_from_hostname(self, hostname: str) -> str | None:
        match = re.fullmatch(rf"b-({LABEL})\.{re.escape(self.name)}", hostname.lower().rstrip("."))
        return match[1] if match else None

    def label_from_tunnel(self, name: str) -> str | None:
        label = name.removeprefix(self.tunnel_prefix)
        return label if label != name and re.fullmatch(LABEL, label) else None


@dataclass(frozen=True)
class Inventory:
    tunnels: dict[UUID, Tunnel]
    dns: dict[str, DNSRecord]

    def reserved_labels(self, zone: BridgeZone) -> set[str]:
        return {
            label for label in (
                *(zone.label_from_tunnel(tunnel.name) for tunnel in self.tunnels.values()),
                *(zone.label_from_hostname(record.name) for record in self.dns.values()),
            ) if label is not None
        }


def _safe_base_url(value: str) -> str:
    url = urlsplit(value)
    if not url.hostname or url.username or url.password or url.query or url.fragment:
        raise ValueError("invalid Cloudflare API base URL")
    if url.scheme == "https":
        return value.rstrip("/")
    if url.scheme == "http":
        try:
            if ipaddress.ip_address(url.hostname).is_loopback:
                return value.rstrip("/")
        except ValueError:
            pass
    raise ValueError("Cloudflare API base URL requires HTTPS or a loopback HTTP address")


def _tunnel(data: dict) -> Tunnel:
    return Tunnel(UUID(data["id"]), _str(data["name"]), data.get("config_src"))


def _dns(data: dict) -> DNSRecord:
    record_id = _str(data["id"])
    if not ID.fullmatch(record_id):
        raise ValueError
    return DNSRecord(
        record_id, _str(data["name"]).lower().rstrip("."), _str(data["type"]),
        _str(data["content"]).lower().rstrip("."), data.get("proxied") is True, data.get("comment"),
    )


def _str(value: object) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError
    return value


class CloudflareTunnelClient:
    def __init__(self, *, base_url: str, account_id: str, zone_id: str, zone_name: str, token: str):
        if not (ID.fullmatch(account_id) and ID.fullmatch(zone_id) and token):
            raise ValueError("invalid Cloudflare account, DNS zone or API token")
        self.zone = BridgeZone(zone_name)
        self._tunnels = f"accounts/{account_id}/cfd_tunnel"
        self._dns = f"zones/{zone_id}/dns_records"
        self._http = httpx.AsyncClient(
            base_url=_safe_base_url(base_url) + "/", headers={"Authorization": f"Bearer {token}"},
            timeout=10.0, follow_redirects=False, trust_env=False,
        )

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_):
        await self._http.aclose()

    async def _request(self, method: str, path: str, **kwargs) -> dict:
        try:
            async with asyncio.timeout(15), self._http.stream(method, path, **kwargs) as response:
                if not 200 <= response.status_code < 300:
                    code = response.status_code
                    raise ProviderError(f"HTTP {code}", retryable=code == 429 or code >= 500)
                body = bytearray()
                async for chunk in response.aiter_bytes(chunk_size=65536):
                    body.extend(chunk)
                    if len(body) > MAX_BODY:
                        raise ProviderError("response exceeds size limit")
            data = json.loads(body)
        except (httpx.HTTPError, TimeoutError):
            raise ProviderError("transport failure", retryable=True) from None
        except (ValueError, UnicodeError):
            raise ProviderError("invalid response") from None
        if not isinstance(data, dict) or data.get("success") is not True or "result" not in data:
            raise ProviderError("unsuccessful response")
        return data

    async def _list(self, path, parse, **filters):
        items = {}
        pagination = None
        for page in range(1, MAX_PAGES + 1):
            data = await self._request("GET", path, params={"page": page, "per_page": 100, **filters})
            records, total, page_size = _inventory_page(data, page, parse)
            if pagination is not None and pagination != (total, page_size):
                raise ProviderError("inventory changed during pagination")
            pagination = (total, page_size)
            for record in records:
                if record.id in items:
                    raise ProviderError("duplicate ID in inventory")
                items[record.id] = record
            if len(items) == total:
                return items
        raise ProviderError("inventory page limit exceeded")

    async def inventory(self) -> Inventory:
        tunnels = await self._list(self._tunnels, _tunnel, is_deleted="false")
        dns = await self._list(self._dns, _dns)
        return Inventory(tunnels, dns)

    async def create_tunnel(self, label: str, secret: bytes) -> Tunnel:
        data = await self._request("POST", self._tunnels, json={
            "name": self.zone.tunnel_name(label),
            "config_src": "local",
            "tunnel_secret": base64.b64encode(secret).decode("ascii"),
        })
        return _parse_resource(_tunnel, data["result"])

    async def create_dns(self, label: str, tunnel_id: UUID) -> DNSRecord:
        data = await self._request("POST", self._dns, json={
            "name": self.zone.hostname(label),
            "type": "CNAME",
            "content": f"{tunnel_id}.cfargotunnel.com",
            "proxied": True,
            "comment": self.zone.dns_comment(label, tunnel_id),
        })
        return _parse_resource(_dns, data["result"])

    async def delete_dns(self, record_id: str) -> None:
        await self._request("DELETE", f"{self._dns}/{record_id}")

    async def delete_tunnel(self, tunnel_id: UUID) -> None:
        await self._request("DELETE", f"{self._tunnels}/{tunnel_id}", params={"cascade": "true"})


def _parse_resource(parse, data):
    try:
        return parse(data)
    except (AttributeError, KeyError, TypeError, ValueError):
        raise ProviderError("invalid resource response") from None


def _inventory_page(data, page, parse):
    try:
        values = data["result"]
        info = data["result_info"]
        count, per_page, total = (info[key] for key in ("count", "per_page", "total_count"))
        if not all(type(n) is int for n in (count, per_page, total, info["page"])):
            raise ValueError
        if info["page"] != page or not isinstance(values, list) or per_page <= 0 or total < 0:
            raise ValueError
        if count != len(values) or count != min(per_page, total - (page - 1) * per_page):
            raise ValueError
        return [parse(value) for value in values], total, per_page
    except (AttributeError, KeyError, TypeError, ValueError):
        raise ProviderError("incomplete inventory") from None
