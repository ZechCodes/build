"""Standalone hosted tunnel worker and operator rotation request."""

import argparse
import asyncio
import logging
import os
import re
from contextlib import asynccontextmanager
from pathlib import Path
from uuid import UUID

from sqlalchemy.engine import make_url
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from buildapp.cloudflare_tunnels import CloudflareTunnelClient
from buildapp.hosted_tunnel_assignments import CHANGE_CHANNEL, HostedModeSource
from buildapp.hosted_tunnel_credentials import load_wrapping_key
from buildapp.hosted_tunnel_provisioner import HostedTunnelProvisioner

log = logging.getLogger(__name__)
SIGNED_MODE_SOURCE = "buildapp.hosted_tunnel_source:hosted_mode_source"


class ModeSourceConfigurationError(ValueError):
    pass


def read_secret_file(setting: str) -> str:
    try:
        with Path(os.environ[setting]).open("rb") as stream:
            value = stream.read(4097)
        if len(value) > 4096 or not re.fullmatch(rb"[!-~]+", value.strip()):
            raise ValueError
        return value.strip().decode("ascii")
    except (KeyError, OSError, ValueError):
        raise ValueError(f"{setting} must name a mounted token file") from None


@asynccontextmanager
async def wait_for_database_changes(engine, nudge):
    """Listen on a dedicated session, then return its disconnect signal."""
    async with engine.connect() as connection:
        driver = (await connection.get_raw_connection()).driver_connection
        disconnected = asyncio.Event()

        def changed(*_):
            nudge()

        def connection_lost(_):
            disconnected.set()

        driver.add_termination_listener(connection_lost)
        try:
            await driver.add_listener(CHANGE_CHANNEL, changed)
            yield disconnected
        finally:
            if not driver.is_closed():
                driver.remove_termination_listener(connection_lost)
                await driver.remove_listener(CHANGE_CHANNEL, changed)


async def _listen(engine, nudge):
    while True:
        try:
            async with wait_for_database_changes(engine, nudge) as disconnected:
                nudge()
                await disconnected.wait()
        except Exception as exc:
            log.warning("Hosted tunnel notifications unavailable (%s)", type(exc).__name__)
        await asyncio.sleep(5)


def _database_engine():
    url = make_url(os.environ["DATABASE_URL"])
    if url.drivername != "postgresql+asyncpg":
        raise ValueError("the hosted tunnel worker requires PostgreSQL through asyncpg")
    return create_async_engine(
        url, isolation_level="READ COMMITTED", hide_parameters=True, pool_pre_ping=True,
        connect_args={"timeout": 10, "command_timeout": 15},
    )


def _mode_source() -> HostedModeSource:
    if os.environ.get("HOSTED_TUNNEL_MODE_SOURCE") != SIGNED_MODE_SOURCE:
        raise ModeSourceConfigurationError(f"HOSTED_TUNNEL_MODE_SOURCE must be {SIGNED_MODE_SOURCE}")
    # Import lazily so the provisioner can ship independently of the issuer adapter.
    try:
        from buildapp.hosted_tunnel_source import hosted_mode_source
    except ModuleNotFoundError as exc:
        if exc.name != "buildapp.hosted_tunnel_source":
            raise
        raise ModeSourceConfigurationError("The signed hosted-mode adapter is unavailable") from None
    if not callable(getattr(hosted_mode_source, "hosted_pairings", None)):
        raise ModeSourceConfigurationError("The signed hosted-mode adapter does not provide hosted_pairings")
    return hosted_mode_source


def _provider():
    return CloudflareTunnelClient(
        base_url=os.environ["HOSTED_TUNNEL_API_BASE_URL"],
        account_id=os.environ["HOSTED_TUNNEL_ACCOUNT_ID"],
        zone_id=os.environ["HOSTED_TUNNEL_ZONE_ID"],
        zone_name=os.environ["HOSTED_TUNNEL_ZONE_NAME"],
        token=read_secret_file("HOSTED_TUNNEL_API_TOKEN_FILE"),
    )


async def _run(args):
    reports = _mode_source()
    engine = _database_engine()
    try:
        async with _provider() as provider:
            worker = HostedTunnelProvisioner(
                async_sessionmaker(engine, expire_on_commit=False),
                reports, provider, load_wrapping_key(),
                int(os.environ["HOSTED_TUNNEL_CAPACITY"]),
            )
            if args.command == "rotate":
                if not await worker.request_rotation(args.device_id, args.generation):
                    log.error("Hosted tunnel rotation refused for the requested generation")
                    return 1
                return 0
            async with asyncio.TaskGroup() as group:
                group.create_task(_listen(engine, worker.nudge))
                await worker.run()
    finally:
        await engine.dispose()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command")
    rotate = commands.add_parser("rotate", help="queue replacement of the current assignment")
    rotate.add_argument("device_id", type=UUID)
    rotate.add_argument("generation", type=int)
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO)
    # HTTP transport debug traces can include provider response headers.
    logging.getLogger("httpcore").setLevel(logging.WARNING)
    logging.getLogger("httpx").setLevel(logging.WARNING)
    try:
        return asyncio.run(_run(args))
    except KeyboardInterrupt:
        return 130
    except ModeSourceConfigurationError as exc:
        log.error("%s", exc)
        return 1
    except Exception as exc:
        log.error("Hosted tunnel worker could not start or complete the request (%s)", type(exc).__name__)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
