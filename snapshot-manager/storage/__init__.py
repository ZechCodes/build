"""
Storage backend for snapshot management.
"""

from .s3_backend import S3StorageBackend
from .deduplication import DeduplicationEngine

__all__ = ["S3StorageBackend", "DeduplicationEngine"]