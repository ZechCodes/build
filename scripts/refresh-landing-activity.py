#!/usr/bin/env python3
"""Refresh the landing activity cache from anonymously visible merged pull requests.

Only pull requests carrying the ``landing-activity`` label and both public fields from
the repository pull-request template are eligible. Any GitHub, parsing, or validation
failure exits without touching the last known cache.
"""

from __future__ import annotations

import json
import re
import sys
from datetime import UTC, datetime
from pathlib import Path
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen

REPOSITORY = "ZechCodes/build-web"
INCLUSION_LABEL = "landing-activity"
API_ROOT = f"https://api.github.com/repos/{REPOSITORY}"
CONTENT_PATH = (
    Path(__file__).resolve().parents[1]
    / "skriftapp"
    / "buildapp"
    / "landing"
    / "content.json"
)
FIELD_PATTERN = re.compile(
    r"^### (?P<field>Landing activity (?:summary|category))\s*$"
    r"(?P<value>.*?)(?=^### |\Z)",
    re.MULTILINE | re.DOTALL,
)
COMMENT_PATTERN = re.compile(r"<!--.*?-->", re.DOTALL)


def api_json(url: str) -> Any:
    if not url.startswith("https://api.github.com/"):
        raise ValueError("refusing a non-GitHub API URL")
    request = Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "User-Agent": "build-landing-activity-refresh",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    with urlopen(request, timeout=15) as response:  # noqa: S310 - URL constrained above
        return json.load(response)


def public_fields(body: str | None) -> dict[str, str]:
    fields = {
        match.group("field").removeprefix("Landing activity "): " ".join(
            COMMENT_PATTERN.sub("", match.group("value")).strip().split()
        )
        for match in FIELD_PATTERN.finditer(body or "")
    }
    if not fields.get("summary") or not fields.get("category"):
        raise ValueError("labeled pull request is missing its public activity fields")
    return fields


def eligible_pull_requests() -> list[dict[str, str]]:
    issues_url = (
        f"{API_ROOT}/issues?state=closed&labels={quote(INCLUSION_LABEL)}"
        "&sort=updated&direction=desc&per_page=20"
    )
    entries: list[dict[str, str]] = []
    for issue in api_json(issues_url):
        pull_url = issue.get("pull_request", {}).get("url")
        if not pull_url:
            continue
        pull = api_json(pull_url)
        if not pull.get("merged_at"):
            continue
        fields = public_fields(pull.get("body"))
        entries.append(
            {
                "summary": fields["summary"][:180],
                "category": fields["category"][:60],
                "merged_at": pull["merged_at"],
                "source_url": pull["html_url"],
                "release_status": "merged_not_released",
            }
        )
        if len(entries) == 5:
            break
    return entries


def refresh() -> int:
    try:
        repository = api_json(API_ROOT)
        if repository.get("private") is not False:
            raise ValueError("repository is not anonymously public")
        entries = eligible_pull_requests()
        content = json.loads(CONTENT_PATH.read_text())
        content["source"]["repository_public"] = True
        content["source"]["checked_at"] = datetime.now(UTC).isoformat().replace(
            "+00:00", "Z"
        )
        content["activity"] = {
            "state": "available" if entries else "empty",
            "reason": "Public development updates will appear here when available.",
            "entries": entries,
        }
    except (HTTPError, URLError, TimeoutError, json.JSONDecodeError, KeyError, TypeError, ValueError) as error:
        print(f"activity cache retained: {error}", file=sys.stderr)
        return 1

    temporary_path = CONTENT_PATH.with_suffix(".json.tmp")
    temporary_path.write_text(json.dumps(content, indent=2) + "\n")
    temporary_path.replace(CONTENT_PATH)
    print(f"cached {len(entries)} public merged pull request(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(refresh())
