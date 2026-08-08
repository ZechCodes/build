"""Deploy announcements over web push — run once at container start.

The frontend version system is client-driven (the bundle embeds its version;
clients compare against ``static/version.json`` on an interval and on PWA
resume). This module is the accelerant for clients that are open RIGHT NOW:
container start is the first moment the new ``version.json`` is being served,
so a push here tells every subscription to re-check immediately instead of on
its next poll. Suspended PWAs cannot receive it usefully — for them the wake
is a visible notification, and the resume check does the real work.

E2EE invariant: the payload is ``{"kind": "app_update", "url": "/app/"}`` and
nothing else. Not even the version travels — version.json is the single source
of truth, and a payload that repeats it could disagree with it.

Announcements are deduplicated through the ``announced_app_versions`` table:
a pod restart or scale-up serving the SAME build must not re-notify anyone.

Pure helpers up top; the DB/engine glue below is the ``python -m`` entry the
container entrypoint runs between migrations and serve.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
from pathlib import Path

from sqlalchemy import select
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from buildapp import web_push
from buildapp.models import AnnouncedAppVersion
from skrift.db.models.push_subscription import PushSubscription

__all__ = [
    "APP_UPDATE_KIND",
    "app_update_payload",
    "read_built_version",
    "should_announce",
]

logger = logging.getLogger(__name__)

APP_UPDATE_KIND = "app_update"

# The build stamps the version beside the bundle; serving and announcing must
# read the SAME file or they could disagree.
STATIC_DIR = Path(__file__).parent / "static"


def app_update_payload() -> str:
    """The push payload: a kind and the app root, content-free."""
    return json.dumps({"kind": APP_UPDATE_KIND, "url": "/app/"})


def read_built_version(static_dir: Path) -> str | None:
    """The version this image was built as, from the build's own stamp.
    ``None`` for a missing or unreadable stamp (a dev tree, a broken build)."""
    try:
        stamp = json.loads((static_dir / "version.json").read_text())
    except (OSError, ValueError):
        return None
    version = stamp.get("version")
    return version if isinstance(version, str) and version else None


def should_announce(built: str | None, last_announced: str | None) -> bool:
    """Announce only a REAL new version: a dev build has no deploys to announce,
    and re-serving the version everyone was already told about (pod restart,
    scale-up) must stay silent."""
    if not built or built == "dev":
        return False
    return built != last_announced


async def _announce_if_new() -> None:
    """Read the built version, compare against the last announced one, and push
    ``app_update`` to every subscription exactly once per new version."""
    built = read_built_version(STATIC_DIR)
    engine = create_async_engine(os.environ["DATABASE_URL"])
    try:
        async with async_sessionmaker(engine)() as session:
            last = (
                (
                    await session.execute(
                        select(AnnouncedAppVersion.version).order_by(
                            AnnouncedAppVersion.created_at.desc()
                        )
                    )
                )
                .scalars()
                .first()
            )
            if not should_announce(built, last):
                logger.info("app-update: nothing to announce (built=%s last=%s)", built, last)
                return
            subscriptions = (
                (await session.execute(select(PushSubscription))).scalars().all()
            )
            infos = [
                {"endpoint": s.endpoint, "keys": {"p256dh": s.key_p256dh, "auth": s.key_auth}}
                for s in subscriptions
            ]
            if infos:
                delivered, _gone = web_push.send_to_subscriptions(
                    infos,
                    app_update_payload(),
                    os.environ[web_push.VAPID_PRIVATE_KEY_ENV],
                    os.environ.get(web_push.VAPID_SUBJECT_ENV, "mailto:ops@getbuild.ing"),
                )
                logger.info(
                    "app-update: announced %s to %d/%d subscriptions",
                    built,
                    delivered,
                    len(infos),
                )
            # Recorded even with zero subscriptions: the version was served and
            # nobody is owed a late announcement for it.
            session.add(AnnouncedAppVersion(version=built))
            await session.commit()
    finally:
        await engine.dispose()


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    asyncio.run(_announce_if_new())
