"""
REST API for VM Snapshot Management.
"""

from .snapshot_api import SnapshotAPI
from .auth import AuthenticationMiddleware

__all__ = ["SnapshotAPI", "AuthenticationMiddleware"]