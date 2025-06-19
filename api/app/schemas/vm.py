"""VM schemas for request/response models."""

from typing import Optional, Dict, Any
from datetime import datetime
from pydantic import BaseModel, Field, validator


class VMCreate(BaseModel):
    """VM creation schema."""
    name: str = Field(..., min_length=1, max_length=100)
    cpu_count: int = Field(1, ge=1, le=8)
    memory_mb: int = Field(512, ge=512, le=8192)
    disk_gb: int = Field(10, ge=10, le=100)
    template: Optional[str] = Field(None, description="Base template to use")
    config: Optional[Dict[str, Any]] = Field(default_factory=dict)
    
    @validator('name')
    def validate_name(cls, v):
        """Validate VM name format."""
        if not v.replace('-', '').replace('_', '').isalnum():
            raise ValueError('VM name can only contain letters, numbers, hyphens, and underscores')
        return v


class VMUpdate(BaseModel):
    """VM update schema."""
    name: Optional[str] = Field(None, min_length=1, max_length=100)
    cpu_count: Optional[int] = Field(None, ge=1, le=8)
    memory_mb: Optional[int] = Field(None, ge=512, le=8192)
    disk_gb: Optional[int] = Field(None, ge=10, le=100)
    config: Optional[Dict[str, Any]] = None
    
    @validator('name')
    def validate_name(cls, v):
        """Validate VM name format."""
        if v is not None and not v.replace('-', '').replace('_', '').isalnum():
            raise ValueError('VM name can only contain letters, numbers, hyphens, and underscores')
        return v


class VMResponse(BaseModel):
    """VM response schema."""
    id: str
    name: str
    state: str
    cpu_count: int
    memory_mb: int
    disk_gb: int
    firecracker_id: Optional[str]
    created_at: datetime
    updated_at: datetime
    started_at: Optional[datetime]
    stopped_at: Optional[datetime]
    config: Dict[str, Any]
    
    class Config:
        from_attributes = True