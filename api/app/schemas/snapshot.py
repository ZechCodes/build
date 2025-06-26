"""Snapshot schemas for request/response validation."""

from datetime import datetime
from typing import List, Optional, Dict, Any
from uuid import UUID

from pydantic import BaseModel, Field, ConfigDict


class SnapshotBase(BaseModel):
    """Base snapshot schema."""
    
    name: str = Field(..., min_length=1, max_length=100, description="Snapshot name")
    description: Optional[str] = Field(None, max_length=500, description="Snapshot description")


class SnapshotCreate(SnapshotBase):
    """Schema for creating a new snapshot."""
    
    vm_instance_id: UUID = Field(..., description="VM instance ID to snapshot")
    tags: Optional[List[str]] = Field(default_factory=list, max_items=10, description="Snapshot tags")


class SnapshotUpdate(BaseModel):
    """Schema for updating a snapshot."""
    
    name: Optional[str] = Field(None, min_length=1, max_length=100, description="Snapshot name")
    description: Optional[str] = Field(None, max_length=500, description="Snapshot description")


class SnapshotResponse(SnapshotBase):
    """Schema for snapshot response."""
    
    model_config = ConfigDict(from_attributes=True)
    
    id: UUID = Field(..., description="Snapshot ID")
    vm_instance_id: UUID = Field(..., description="VM instance ID")
    user_id: UUID = Field(..., description="User ID")
    storage_path: str = Field(..., description="Storage path")
    size_bytes: Optional[int] = Field(None, description="Snapshot size in bytes")
    snapshot_metadata: Optional[Dict[str, Any]] = Field(None, description="Snapshot metadata")
    created_at: datetime = Field(..., description="Creation timestamp")
    updated_at: datetime = Field(..., description="Last update timestamp")


class SnapshotList(BaseModel):
    """Schema for snapshot list response."""
    
    snapshots: List[SnapshotResponse] = Field(..., description="List of snapshots")
    total_count: int = Field(..., description="Total snapshot count")
    limit: int = Field(..., description="Pagination limit")
    offset: int = Field(..., description="Pagination offset")


class SnapshotRestore(BaseModel):
    """Schema for snapshot restore request."""
    
    target_vm_id: Optional[UUID] = Field(None, description="Target VM ID (defaults to original VM)")


class SnapshotQuota(BaseModel):
    """Schema for user snapshot quota information."""
    
    user_id: UUID = Field(..., description="User ID")
    max_snapshots: int = Field(..., description="Maximum snapshots allowed")
    current_snapshots: int = Field(..., description="Current snapshot count")
    max_storage_gb: float = Field(..., description="Maximum storage in GB")
    current_storage_gb: float = Field(..., description="Current storage usage in GB")
    remaining_snapshots: int = Field(..., description="Remaining snapshot slots")
    remaining_storage_gb: float = Field(..., description="Remaining storage in GB")


class SnapshotStats(BaseModel):
    """Schema for snapshot statistics."""
    
    total_snapshots: int = Field(..., description="Total number of snapshots")
    total_size_gb: float = Field(..., description="Total storage used in GB")
    compression_ratio: float = Field(..., description="Average compression ratio")
    deduplication_savings: float = Field(..., description="Storage savings from deduplication")
    oldest_snapshot: Optional[datetime] = Field(None, description="Oldest snapshot timestamp")
    newest_snapshot: Optional[datetime] = Field(None, description="Newest snapshot timestamp")