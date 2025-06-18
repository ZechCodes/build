"""Snapshot model."""

from datetime import datetime
from enum import Enum
from sqlalchemy import String, Integer, DateTime, ForeignKey, Text, BigInteger, func
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.core.database import Base


class SnapshotStatus(str, Enum):
    """Snapshot status enumeration."""
    CREATING = "creating"
    AVAILABLE = "available"
    RESTORING = "restoring"
    ERROR = "error"
    DELETING = "deleting"


class Snapshot(Base):
    """VM snapshot model."""

    __tablename__ = "snapshots"

    id: Mapped[int] = mapped_column(primary_key=True, index=True)
    name: Mapped[str] = mapped_column(String(255), nullable=False)
    status: Mapped[SnapshotStatus] = mapped_column(String(20), default=SnapshotStatus.CREATING, nullable=False)
    
    # Relationships
    vm_id: Mapped[int] = mapped_column(ForeignKey("vms.id"), nullable=False)
    
    # Snapshot metadata
    description: Mapped[str] = mapped_column(Text, nullable=True)
    snapshot_type: Mapped[str] = mapped_column(String(50), default="manual", nullable=False)  # manual, auto, scheduled
    
    # Storage information
    storage_path: Mapped[str] = mapped_column(String(500), nullable=True)
    memory_snapshot_path: Mapped[str] = mapped_column(String(500), nullable=True)
    vm_state_path: Mapped[str] = mapped_column(String(500), nullable=True)
    
    # Size information
    disk_size_bytes: Mapped[int] = mapped_column(BigInteger, nullable=True)
    memory_size_bytes: Mapped[int] = mapped_column(BigInteger, nullable=True)
    compressed_size_bytes: Mapped[int] = mapped_column(BigInteger, nullable=True)
    
    # Snapshot configuration at time of creation
    vm_config_snapshot: Mapped[str] = mapped_column(Text, nullable=True)  # JSON snapshot of VM config
    
    # Retention
    retain_until: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=True)
    is_protected: Mapped[bool] = mapped_column(default=False, nullable=False)
    
    # Versioning
    parent_snapshot_id: Mapped[int] = mapped_column(ForeignKey("snapshots.id"), nullable=True)
    version: Mapped[int] = mapped_column(Integer, default=1, nullable=False)
    
    # Timestamps
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now(), nullable=False
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now(), onupdate=func.now(), nullable=False
    )
    completed_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=True)

    # Relationships
    vm: Mapped["VM"] = relationship("VM", back_populates="snapshots")
    parent_snapshot: Mapped["Snapshot"] = relationship("Snapshot", remote_side=[id], back_populates="child_snapshots")
    child_snapshots: Mapped[list["Snapshot"]] = relationship("Snapshot", back_populates="parent_snapshot")

    def __repr__(self) -> str:
        return f"<Snapshot(id={self.id}, name='{self.name}', status='{self.status}')>"