"""Real Skrift issuer and disposable account/device for HTTP qualification."""

import argparse
import asyncio
import base64
from contextlib import ExitStack
from datetime import datetime, timedelta, timezone
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import sys
import tempfile
import time
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


async def seed(run_dir, issuer, bridge_origin, terminal=False):
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
    unrelated = {}
    try:
        async with maker() as session:
            account = await add_account(session, email)
            await add_member(session, account, email)
            session.add(Setting(key="setup_completed_at", value="qualification-fixture"))
            session.add(Device(id=UUID(identity["device_id"]), name=identity["name"],
                owner_user_id=account, approved=True,
                identity_public_key_b64=identity["identity_public_key_b64"],
                transport_public_key_b64=identity["transport"]["public_key_b64"]))
            if terminal:
                unrelated = await seed_unrelated_device(session, issuer)
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
        "logsDir": str(run_dir), **unrelated}
    private_write(run_dir / "runtime.json", json.dumps(metadata, indent=2))
    print("Prepared disposable account and paired device", flush=True)


async def seed_unrelated_device(session, issuer):
    from uuid import UUID
    from buildapp.db_test_support import add_account, add_member
    from buildapp.models import Device

    email = "unrelated-terminal-qualification@example.com"
    account = await add_account(session, email)
    await add_member(session, account, email)
    identity = generated_identity(issuer)
    session.add(Device(id=UUID(identity["device_id"]), name="Unrelated qualification device",
        owner_user_id=account, approved=True,
        identity_public_key_b64=identity["identity_public_key_b64"],
        transport_public_key_b64=identity["transport"]["public_key_b64"]))
    return {"unrelatedAccount": str(account), "unrelatedDevice": identity["device_id"]}


async def serve(app, run_dir, port, terminal=False):
    from hypercorn.asyncio import serve as run_server
    from hypercorn.config import Config

    config = Config()
    config.bind = [f"127.0.0.1:{port}"]
    config.certfile = str(run_dir / "cert.pem")
    config.keyfile = str(run_dir / "key.pem")
    config.accesslog = None
    config.errorlog = "-"
    await run_server(with_electron_navigation(app) if terminal else app, config)


def with_electron_navigation(app):
    async def navigation(scope, receive, send):
        matches = scope["type"] == "http" and scope["path"] == "/app/desktop"
        case = scope.get("query_string")
        if matches and case in (b"qualification_navigation=empty", b"qualification_navigation=download"):
            download = case.endswith(b"download")
            headers = [(b"content-disposition", b"attachment; filename=qualification.txt")] if download else []
            await send({"type": "http.response.start", "status": 200 if download else 204, "headers": headers})
            await send({"type": "http.response.body", "body": b"qualification" if download else b""})
            if not download:
                print("Qualification navigation sent HTTP 204", flush=True)
        else:
            await app(scope, receive, send)

    return navigation


TERMINAL_LISTENER = (
    "http::runtime::terminal::tests::qualification::serve_terminal_qualification_fixture"
)


def terminal_qualification(args):
    from run import (base_environment, ensure_free_ports, install_shutdown_handlers,
                     launch, run_logged, wait_ready)

    scratch = args.scratch_root
    binary = args.bridge_test_binary.resolve(strict=True)
    target = (scratch / "target/debug/deps").resolve()
    if binary.parent != target or not re.fullmatch(r"build_bridge-[0-9a-f]{16}", binary.name):
        raise ValueError("provide the cargo lib test executable from the task scratch target")
    install_shutdown_handlers()
    ensure_free_ports([args.issuer_port, args.bridge_port])
    run_dir = Path(tempfile.mkdtemp(prefix="run-", dir=scratch))
    for name in ("home", "tmp", "empty-ca-dir"):
        (run_dir / name).mkdir()
    env = base_environment(run_dir)
    env.update(CARGO_TARGET_DIR=str(scratch / "target"), BUILD_HTTP_QUALIFICATION_SCRATCH=str(scratch))
    repo = args.repo.resolve()
    command = [
        sys.executable, Path(__file__).resolve(), "--repo", repo, "--run-dir", run_dir,
        "--scratch-root", scratch, "--bridge-host", args.bridge_host,
        "--issuer-terminal-policy", args.issuer_terminal_policy,
        "--issuer-port", args.issuer_port, "--bridge-port", args.bridge_port,
    ]
    run_logged([*command, "prepare"], env=env, cwd=repo, log=run_dir / "prepare.log")
    fixture_path = run_dir / "runtime.json"
    fixture = json.loads(fixture_path.read_text())
    env.update(
        BUILD_HTTP_QUALIFICATION_FIXTURE=str(fixture_path),
        BUILD_HTTP_QUALIFICATION_TERMINAL=args.gate,
        NODE_EXTRA_CA_CERTS=fixture["caFile"],
        SSL_CERT_FILE=fixture["caFile"],
        SSL_CERT_DIR=str(run_dir / "empty-ca-dir"),
    )
    if args.client == "native-grid":
        env["BUILD_HTTP_QUALIFICATION_GRID"] = "1"
    if args.client == "rebind":
        env["BUILD_HTTP_QUALIFICATION_REBIND"] = "1"
    if args.client == "electron-sign-out":
        env["BUILD_HTTP_QUALIFICATION_ELECTRON_SIGN_OUT"] = "1"
    if args.webkit:
        webkit = args.webkit.resolve(strict=True)
        if not webkit.is_relative_to(scratch):
            raise ValueError("provide the WebKit launcher from the task scratch")
        env["BUILD_HTTP_QUALIFICATION_WEBKIT"] = str(webkit)
    if args.electron:
        electron = args.electron.resolve(strict=True)
        if electron.parent != (scratch / "electron").resolve() or electron.name != "electron":
            raise ValueError("provide the locked Electron executable from the task scratch")
        env["BUILD_HTTP_QUALIFICATION_ELECTRON"] = str(electron)
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "XAUTHORITY"):
            if name in os.environ:
                env[name] = os.environ[name]
    print(f"Gate {args.gate}, evidence directory {run_dir}", flush=True)
    with ExitStack() as stack:
        ca = run_dir / "ca.pem"
        issuer_log = run_dir / "issuer.log"
        issuer = launch(stack, [*command, "serve"], env=env, cwd=repo, log=issuer_log)
        wait_ready(issuer, fixture["hostedOrigin"] + "/auth/dummy/login", ca, issuer_log)
        def start_bridge(log_name):
            log = run_dir / log_name
            bridge = launch(stack, [binary, "--ignored", "--exact", TERMINAL_LISTENER, "--nocapture"],
                            env=env, cwd=run_dir, log=log)
            wait_ready(bridge, fixture["bridgeOrigin"] + "/.well-known/build-bridge", ca, log)
            return bridge

        bridge = start_bridge("bridge.log")
        desktop = ["node", "--test"]
        test_file = repo / "desktop/test/bridge-http-qualification.test.mjs"
        if args.client == "browser":
            run_logged(["node", repo / "scripts/bridge-http-qualification/browser.mjs"],
                       env=env, cwd=repo, log=run_dir / "browser.log")
        elif args.client == "rebind":
            log = run_dir / "desktop.log"
            client = launch(stack, [*desktop, "--test-name-pattern=terminal qualification disables", test_file],
                            env=env, cwd=repo, log=log)
            wait_for_client_marker(client, run_dir / "disable-ready", log)
            stop_terminal_bridge(bridge, run_dir / "bridge.log")
            wait_for_client_marker(client, run_dir / "rebind-ready", log)
            bridge = start_bridge("bridge-rebound.log")
            (run_dir / "rebound").touch()
            status = client.wait(timeout=30)
            print(f"{log.name}: exit={status}", flush=True)
            if status:
                raise RuntimeError(f"Qualification command failed; see {log}")
        else:
            pattern = {
                "electron": "terminal qualification recovers actual Electron",
                "electron-sign-out": "terminal qualification fences delayed Electron",
                "native-grid": "terminal qualification restores the maximum grid",
                "desktop": "terminal qualification (?!recovers actual Electron|fences delayed Electron|restores the maximum grid|disables)",
            }[args.client]
            run_logged([*desktop, f"--test-name-pattern={pattern}", test_file],
                       env=env, cwd=repo, log=run_dir / "desktop.log")
        stop_terminal_bridge(bridge, run_dir / ("bridge-rebound.log" if args.client == "rebind" else "bridge.log"))


def wait_for_client_marker(client, path, log):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        if client.poll() is not None:
            raise RuntimeError(f"Qualification command exited {client.returncode}; see {log}")
        if path.exists():
            return
        time.sleep(0.05)
    raise TimeoutError(f"Client did not create {path.name}; see {log}")


def stop_terminal_bridge(bridge, log):
    bridge.terminate()
    status = bridge.wait(timeout=10)
    print(f"Mounted bridge shutdown: exit={status}", flush=True)
    if status:
        raise RuntimeError(f"Qualification listener failed to drain; see {log}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["prepare", "serve", "qualify"])
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--run-dir", type=Path)
    parser.add_argument("--issuer-port", type=int, default=18476)
    parser.add_argument("--bridge-port", type=int, default=19476)
    parser.add_argument("--scratch-root", type=Path, required=True, help="existing disposable fixture directory")
    parser.add_argument("--bridge-host", choices=["localhost", "127.0.0.1"], required=True)
    parser.add_argument("--issuer-terminal-policy", choices=["off", "on"], required=True,
                        help="enable the terminal-specific issuer fixture and navigation routes")
    parser.add_argument("--bridge-test-binary", type=Path)
    parser.add_argument("--electron", type=Path)
    parser.add_argument("--webkit", type=Path)
    parser.add_argument("--gate", choices=["off", "on"], default="on")
    parser.add_argument("--client", choices=[
        "browser", "desktop", "electron", "electron-sign-out", "native-grid", "rebind",
    ], default="desktop")
    args = parser.parse_args()
    if not all(1024 <= port <= 65535 for port in [args.issuer_port, args.bridge_port]):
        parser.error("ports must be unprivileged loopback ports")
    args.scratch_root = args.scratch_root.resolve()
    if not args.scratch_root.is_dir():
        parser.error("--scratch-root must be an existing directory")
    if args.command == "qualify":
        if args.bridge_host != "127.0.0.1" or args.issuer_terminal_policy != "on":
            parser.error("terminal qualification requires --bridge-host 127.0.0.1 --issuer-terminal-policy on")
        if not args.bridge_test_binary or not args.bridge_test_binary.is_absolute():
            parser.error("--bridge-test-binary must be an absolute cargo test executable path")
        if args.client in ("electron", "electron-sign-out") and (not args.electron or args.gate != "on"):
            parser.error("Electron qualification requires --electron and --gate on")
        if args.client in ("native-grid", "rebind") and args.gate != "on":
            parser.error(f"{args.client} qualification requires --gate on")
        terminal_qualification(args)
        return
    if args.run_dir is None:
        parser.error("--run-dir is required for prepare and serve")
    run_dir = args.run_dir.resolve()
    if not run_dir.is_relative_to(args.scratch_root) or run_dir == args.scratch_root:
        parser.error("--run-dir must be a disposable child of --scratch-root")
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
    terminal = args.issuer_terminal_policy == "on"
    if args.command == "prepare":
        create_certificates(run_dir)
        asyncio.run(seed(run_dir, issuer, f"https://{args.bridge_host}:{args.bridge_port}", terminal=terminal))
    else:
        asyncio.run(serve(app, run_dir, args.issuer_port, terminal=terminal))


if __name__ == "__main__":
    main()
