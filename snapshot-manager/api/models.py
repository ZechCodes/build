"""
Pydantic models for snapshot API requests and responses.
"""

from typing import Optional, List, Dict, Any
from pydantic import BaseModel, Field, field_validator, ConfigDict
from datetime import datetime, timezone
from enum import Enum


class SnapshotType(str, Enum):
    """Types of snapshots supported by the system."""
    FULL = "full"
    INCREMENTAL = "incremental"
    DIFFERENTIAL = "differential"


class SnapshotState(str, Enum):
    """Possible states of a snapshot."""
    CREATING = "creating"
    AVAILABLE = "available"
    DELETING = "deleting"
    ERROR = "error"


class CreateSnapshotRequest(BaseModel):
    """Request model for creating a snapshot."""
    vm_id: str = Field(..., description="ID of the VM to snapshot")
    name: str = Field(..., min_length=1, max_length=255, description="Snapshot name")
    description: Optional[str] = Field(None, max_length=1000, description="Snapshot description")
    snapshot_type: SnapshotType = Field(SnapshotType.FULL, description="Type of snapshot")
    tags: Optional[Dict[str, str]] = Field(None, description="User-defined tags")
    
    @field_validator('name')
    @classmethod
    def validate_name(cls, v):
        """Validate snapshot name format."""
        if not v.replace('-', '').replace('_', '').replace(' ', '').isalnum():
            raise ValueError('Name must contain only alphanumeric characters, spaces, hyphens, and underscores')
        return v
    
    @field_validator('tags')
    @classmethod
    def validate_tags(cls, v):
        """Validate tags format."""
        if v is None:
            return v
        
        if len(v) > 10:
            raise ValueError('Maximum 10 tags allowed')
        
        for key, value in v.items():
            if not isinstance(key, str) or not isinstance(value, str):
                raise ValueError('Tags must be string key-value pairs')
            if len(key) > 50 or len(value) > 200:
                raise ValueError('Tag keys max 50 chars, values max 200 chars')
        
        return v


class RestoreSnapshotRequest(BaseModel):
    """Request model for restoring a snapshot."""
    target_vm_id: Optional[str] = Field(None, description="Target VM ID (defaults to source VM)")
    restore_options: Optional[Dict[str, Any]] = Field(None, description="Restore-specific options")


class SnapshotResponse(BaseModel):
    """Response model for snapshot operations."""
    snapshot_id: str
    vm_id: str
    user_id: str
    name: str
    description: Optional[str]
    snapshot_type: SnapshotType
    state: SnapshotState
    size_bytes: Optional[int]
    tags: Optional[Dict[str, str]]
    created_at: datetime
    updated_at: datetime
    expires_at: Optional[datetime]


class SnapshotListResponse(BaseModel):
    """Response model for listing snapshots."""
    snapshots: List[SnapshotResponse]
    total_count: int
    page: int
    page_size: int
    has_next: bool


class SnapshotStatsResponse(BaseModel):
    """Response model for snapshot statistics."""
    total_snapshots: int
    total_size_bytes: int
    snapshots_by_state: Dict[str, int]
    snapshots_by_type: Dict[str, int]
    average_size_bytes: float
    storage_efficiency: Optional[float]  # If deduplication enabled


class ErrorResponse(BaseModel):
    """Error response model."""
    error: str
    details: Optional[str] = None
    error_code: Optional[str] = None
    timestamp: str = Field(default_factory=lambda: datetime.now(timezone.utc).isoformat())


class HealthCheckResponse(BaseModel):
    """Health check response model."""
    status: str
    version: str
    uptime_seconds: float
    storage_backend: str
    storage_status: str
    database_status: str
    components: Dict[str, str]