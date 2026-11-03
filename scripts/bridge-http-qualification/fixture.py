"""Real Skrift issuer and disposable account/device for REST qualification."""

import argparse
import asyncio
import base64
from datetime import datetime, timedelta, timezone
import ipaddress
import json
import os
from pathlib import Path
import secrets
import sys
from uuid import uuid4


def private_write(path, content):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", opener=lambda name, flags: os.open(name, flags, 0o600)) as stream:
        stream.write(content)


def create_certificates(run_dir):
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import rsa
    from cryptography.x509.oid import NameOID

    now = datetime.now(timezone.utc)
    ca_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    ca_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Build disposable qualification CA")])
    ca = (x509.CertificateBuilder().subject_name(ca_name).issuer_name(ca_name)
          .public_key(ca_key.public_key()).serial_number(x509.random_serial_number())
          .not_valid_before(now - timedelta(minutes=1)).not_valid_after(now + timedelta(days=2))
          .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
          .add_extension(x509.SubjectKeyIdentifier.from_public_key(ca_key.public_key()), critical=False)
          .add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key(ca_key.public_key()), critical=False)
          .add_extension(x509.KeyUsage(digital_signature=True, content_commitment=False,
              key_encipherment=False, data_encipherment=False, key_agreement=False,
              key_cert_sign=True, crl_sign=True, encipher_only=None, decipher_only=None), critical=True)
          .sign(ca_key, hashes.SHA256()))
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")])
    cert = (x509.CertificateBuilder().subject_name(name).issuer_name(ca_name)
            .public_key(key.public_key()).serial_number(x509.random_serial_number())
            .not_valid_before(now - timedelta(minutes=1)).not_valid_after(now + timedelta(days=2))
            .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
            .add_extension(x509.SubjectKeyIdentifier.from_public_key(key.public_key()), critical=False)
            .add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key(ca_key.public_key()), critical=False)
            .add_extension(x509.SubjectAlternativeName([x509.DNSName("localhost"),
                x509.IPAddress(ipaddress.ip_address("127.0.0.1")),
                x509.IPAddress(ipaddress.ip_address("::1"))]), critical=False)
            .add_extension(x509.ExtendedKeyUsage([x509.oid.ExtendedKeyUsageOID.SERVER_AUTH]), critical=False)
            .sign(ca_key, hashes.SHA256()))
    private_write(run_dir / "ca.pem", ca.public_bytes(serialization.Encoding.PEM).decode())
    private_write(run_dir / "cert.pem", cert.public_bytes(serialization.Encoding.PEM).decode())
    private_write(run_dir / "key.pem", key.private_bytes(serialization.Encoding.PEM,
        serialization.PrivateFormat.PKCS8, serialization.NoEncryption()).decode())


def configure(repo, run_dir, issuer):
    import yaml

    config = yaml.safe_load((repo / "skriftapp/app.dev.yaml").read_text())
    config["db"] = {"url": f"sqlite+aiosqlite:///{run_dir / 'app.db'}"}
    config["auth"]["redirect_base_url"] = issuer
    config["email"]["public_base_url"] = issuer
    config["debug"] = True
    (run_dir / "app.dev.yaml").write_text(yaml.safe_dump(config, sort_keys=False))
    if not (run_dir / ".env").exists():
        private_write(run_dir / ".env", "SECRET_KEY=" + secrets.token_urlsafe(48) + "\n")
    templates = run_dir / "templates"
    if not templates.exists():
        templates.symlink_to(repo / "skriftapp/templates", target_is_directory=True)


def generated_identity(issuer):
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey
    from cryptography.hazmat.primitives.serialization import Encoding, PrivateFormat, PublicFormat, NoEncryption

    def encoded(value):
        return base64.b64encode(value).decode().rstrip("=")

    signing = Ed25519PrivateKey.generate()
    transport = X25519PrivateKey.generate()
    return {
        "device_id": str(uuid4()), "name": "REST qualification", "approved": True, "approved_by": issuer,
        "identity_private_key_b64": encoded(signing.private_bytes(Encoding.Raw, PrivateFormat.Raw, NoEncryption())),
        "identity_public_key_b64": encoded(signing.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)),
        "transport": {
            "private_key_b64": encoded(transport.private_bytes(Encoding.Raw, PrivateFormat.Raw, NoEncryption())),
            "public_key_b64": encoded(transport.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)),
        },
    }


async def seed(run_dir, issuer, bridge_origin):
    from uuid import UUID
    from buildapp.db_test_support import create_skrift_schema_in, file_session_maker, add_account, add_member
    from buildapp.models import Device
    from skrift.auth.tokens import create_signed_token
    from skrift.config import get_settings
    from skrift.db.models.setting import Setting

    database = run_dir / "app.db"
    await create_skrift_schema_in(database)
    maker = file_session_maker(database)
    identity = generated_identity(issuer)
    email = "qualification976@example.com"
    try:
        async with maker() as session:
            account = await add_account(session, email)
            await add_member(session, account, email)
            session.add(Setting(key="setup_completed_at", value="qualification-fixture"))
            session.add(Device(id=UUID(identity["device_id"]), name=identity["name"],
                owner_user_id=account, approved=True,
                identity_public_key_b64=identity["identity_public_key_b64"],
                transport_public_key_b64=identity["transport"]["public_key_b64"]))
            await session.commit()
    finally:
        await maker.kw["bind"].dispose()
    identity_file = run_dir / "home/identity.json"
    private_write(identity_file, json.dumps(identity, indent=2))
    token = create_signed_token({"type": "access", "user_id": str(account), "email": email,
        "name": "REST qualification", "picture_url": None, "client_id": "build-desktop",
        "scope": "openid profile email"}, get_settings().secret_key, 900)
    metadata = {"hostedOrigin": issuer, "bridgeOrigin": bridge_origin,
        "accountId": str(account), "deviceId": identity["device_id"], "email": email,
        "accessToken": token, "identityFile": str(identity_file),
        "projectsDir": str(run_dir / "home/projects"), "runDir": str(run_dir),
        "caFile": str(run_dir / "ca.pem"), "screenshotDir": str(run_dir / "screenshots"),
        "logsDir": str(run_dir)}
    private_write(run_dir / "runtime.json", json.dumps(metadata, indent=2))
    print("Prepared disposable account and paired device", flush=True)


async def serve(app, run_dir, port):
    from hypercorn.asyncio import serve as run_server
    from hypercorn.config import Config

    config = Config()
    config.bind = [f"127.0.0.1:{port}"]
    config.certfile = str(run_dir / "cert.pem")
    config.keyfile = str(run_dir / "key.pem")
    config.accesslog = None
    config.errorlog = "-"
    await run_server(app, config)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["prepare", "serve"])
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--run-dir", type=Path, required=True)
    parser.add_argument("--issuer-port", type=int, default=18476)
    parser.add_argument("--bridge-port", type=int, default=19476)
    parser.add_argument("--scratch-task", type=int, choices=[976, 977], default=976)
    args = parser.parse_args()
    run_dir = args.run_dir.resolve()
    scratch = (Path("/home/zech/Projects/.agent-scratch") / str(args.scratch_task)).resolve()
    if not run_dir.is_relative_to(scratch) or run_dir == scratch:
        parser.error("--run-dir must be a disposable child of the selected task scratch")
    if not all(1024 <= port <= 65535 for port in [args.issuer_port, args.bridge_port]):
        parser.error("ports must be unprivileged loopback ports")
    run_dir.mkdir(parents=True, exist_ok=True)
    if args.command == "prepare" and (run_dir / "app.db").exists():
        parser.error("Qualification requires a fresh disposable database")
    for name in ["tmp", "home", "home/projects", "screenshots"]:
        (run_dir / name).mkdir(parents=True, exist_ok=True)
    os.environ.update(SKRIFT_ENV="dev", HOME=str(run_dir / "home"), TMPDIR=str(run_dir / "tmp"))
    sys.path.insert(0, str(args.repo.resolve() / "skriftapp"))
    issuer = f"https://localhost:{args.issuer_port}"
    configure(args.repo.resolve(), run_dir, issuer)
    os.chdir(run_dir)
    from skrift.asgi import create_app
    app = create_app()
    if args.command == "prepare":
        create_certificates(run_dir)
        asyncio.run(seed(run_dir, issuer, f"https://localhost:{args.bridge_port}"))
    else:
        asyncio.run(serve(app, run_dir, args.issuer_port))


if __name__ == "__main__":
    main()
