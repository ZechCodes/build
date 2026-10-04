"""Shared preflight for fixture entry points that can change firewall rules."""

import os
from pathlib import Path


def require_private_namespace():
    mapping = Path("/proc/self/uid_map").read_text().split()
    if os.geteuid() != 0 or len(mapping) != 3 or mapping[0] != "0" or mapping[2] != "1":
        # Keep this guard active under Python's optimized mode too.
        raise AssertionError("firewall fixture requires its disposable single-user namespace")
