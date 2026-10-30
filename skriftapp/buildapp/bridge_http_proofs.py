"""Disjoint compact-JWS validators using pinned paired Ed25519 public keys."""

import base64
import ipaddress
import re
from urllib.parse import urlsplit
from uuid import UUID

import jwt
import idna
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
from litestar.exceptions import NotAuthorizedException

TYPES = {
    purpose: f"build-http-{purpose}+jwt"
    for purpose in ("registration", "registration-challenge", "discovery", "introspection")
}


def canonical_origin(value: str) -> str:
    if (
        not isinstance(value, str)
        or not value
        or len(value) > 2048
        or re.search(r"[\s\\%]", value)
    ):
        raise ValueError("invalid HTTPS origin")
    url = urlsplit(value)
    if url.scheme != "https" or url.username is not None or url.password is not None:
        raise ValueError("invalid HTTPS origin")
    if (
        url.path not in ("", "/")
        or url.query
        or url.fragment
        or "?" in value
        or "#" in value
    ):
        raise ValueError("origin must contain only authority")
    if url.netloc.endswith(":"):
        raise ValueError("empty port")
    host = url.hostname or ""
    if not host or host.endswith("."):
        raise ValueError("invalid host")
    host = _canonical_host(host)
    port = url.port
    if port is not None and not 0 < port <= 65535:
        raise ValueError("invalid port")
    return f"https://{host}" + (f":{port}" if port not in (None, 443) else "")


def _canonical_host(host):
    try:
        address = ipaddress.ip_address(host)
        return f"[{address.compressed}]" if address.version == 6 else str(address)
    except ValueError:
        if re.fullmatch(r"[0-9.]+", host) or host.lower().startswith("0x"):
            raise ValueError("ambiguous IP address")
    host = idna.encode(host, uts46=True, std3_rules=True).decode("ascii").lower()
    if re.fullmatch(r"[0-9.]+", host):
        raise ValueError("mapped IP alias")
    if len(host) > 253 or any(
        not re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", label)
        for label in host.split(".")
    ):
        raise ValueError("invalid DNS host")
    return host


def audience(issuer, purpose, device_id=""):
    paths = {
        "registration": f"/api/devices/{device_id}/http-endpoint",
        "registration-challenge": f"/api/devices/{device_id}/http-endpoint/challenge",
        "discovery": "/api/bridge-discovery/complete",
        "introspection": "/api/bridge-http/introspect",
    }
    return canonical_origin(issuer) + paths[purpose]


def proof_device_id(proof):
    try:
        if not isinstance(proof, str) or len(proof) > 16000:
            raise ValueError()
        # Untrusted lookup selector only. verify() must authenticate with the stored
        # paired key before any claim can grant authority (wrong-key tests pin this).
        # nosemgrep: python.jwt.security.unverified-jwt-decode.unverified-jwt-decode
        payload = jwt.decode(proof, options={"verify_signature": False})
        return UUID(payload["iss"])
    except (ValueError, TypeError, KeyError, jwt.PyJWTError, RecursionError):
        raise NotAuthorizedException("Invalid device proof") from None


def verify(proof, device, purpose, issuer, now):
    try:
        if not isinstance(proof, str) or len(proof) > 16000:
            raise ValueError()
        header = jwt.get_unverified_header(proof)
        if header != {"alg": "EdDSA", "typ": TYPES[purpose]}:
            raise ValueError()
        key = Ed25519PublicKey.from_public_bytes(
            base64.b64decode(
                device.identity_public_key_b64
                + "=" * (-len(device.identity_public_key_b64) % 4),
                validate=True,
            )
        )
        claims = jwt.decode(
            proof,
            key,
            algorithms=["EdDSA"],
            audience=audience(issuer, purpose, str(device.id)),
            issuer=str(device.id),
            options={
                "require": ["iss", "sub", "aud", "iat", "exp", "jti"],
                "verify_iat": False,
                "verify_exp": False,
            },
        )
        _validate_claims(claims, device, now)
        return claims
    except (ValueError, TypeError, KeyError, jwt.PyJWTError, RecursionError):
        raise NotAuthorizedException("Invalid device proof") from None


def _validate_claims(claims, device, now):
    timestamp = now.timestamp()
    iat, exp = claims["iat"], claims["exp"]
    if type(iat) is not int or type(exp) is not int:
        raise ValueError()
    if exp <= iat or exp - iat > 30 or iat > timestamp + 5 or exp < timestamp - 5:
        raise ValueError()
    if claims["sub"] != str(device.id) or not isinstance(claims["aud"], str):
        raise ValueError()
    jti = claims["jti"]
    if not isinstance(jti, str) or not 16 <= len(jti) <= 128:
        raise ValueError()
