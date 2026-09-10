"""App-update announcement tests: the startup push that tells every subscribed
client a new frontend deployed, so a live client re-checks version.json now
instead of on its next 10-minute poll.

E2EE invariant under test: the payload names a kind and a url, nothing else —
a deploy announcement carries no task content and not even the version (the
client learns that from version.json, the single source of truth).
"""

from __future__ import annotations

import json
from pathlib import Path

from buildapp import app_update


def test_payload_is_kind_and_url_only() -> None:
    payload = json.loads(app_update.app_update_payload())
    assert payload == {"kind": app_update.APP_UPDATE_KIND, "url": "/app/"}


def test_reads_the_version_the_build_stamped(tmp_path: Path) -> None:
    (tmp_path / "version.json").write_text(json.dumps({"version": "sha-abc"}))
    assert app_update.read_built_version(tmp_path) == "sha-abc"


def test_missing_or_malformed_stamp_reads_as_none(tmp_path: Path) -> None:
    assert app_update.read_built_version(tmp_path) is None
    (tmp_path / "version.json").write_text("not json")
    assert app_update.read_built_version(tmp_path) is None


def test_announces_only_a_real_new_version() -> None:
    # A changed version announces; the same version (pod restart, scale-up) and
    # a dev build (no stamp) stay silent.
    assert app_update.should_announce("sha-b", last_announced="sha-a")
    assert app_update.should_announce("sha-a", last_announced=None)
    assert not app_update.should_announce("sha-a", last_announced="sha-a")
    assert not app_update.should_announce("dev", last_announced="sha-a")
    assert not app_update.should_announce(None, last_announced="sha-a")
    assert not app_update.should_announce("", last_announced="sha-a")
