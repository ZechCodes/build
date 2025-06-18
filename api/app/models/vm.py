"""VM model."""

from datetime import datetime
from enum import Enum
from sqlalchemy import String, Integer, DateTime, Boolean, ForeignKey, Text, func
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.core.database import Base


class VMStatus(str, Enum):
    """VM status enumeration."""
    CREATING = "creating"
    RUNNING = "running"
    STOPPED = "stopped"
    SUSPENDED = "suspended"
    ERROR = "error"
    DELETING = "deleting"


class VM(Base):
    """VM model."""

    __tablename__ = "vms"

    id: Mapped[int] = mapped_column(primary_key=True, index=True)
    name: Mapped[str] = mapped_column(String(255), nullable=False)
    status: Mapped[VMStatus] = mapped_column(String(20), default=VMStatus.CREATING, nullable=False)
    
    # Owner
    owner_id: Mapped[int] = mapped_column(ForeignKey("users.id"), nullable=False)
    
    # VM Configuration
    cpu_count: Mapped[int] = mapped_column(Integer, default=1, nullable=False)
    memory_mb: Mapped[int] = mapped_column(Integer, default=512, nullable=False)
    disk_gb: Mapped[int] = mapped_column(Integer, default=2, nullable=False)
    
    # Firecracker specific
    firecracker_id: Mapped[str] = mapped_column(String(255), unique=True, nullable=True)
    kernel_image_path: Mapped[str] = mapped_column(String(500), nullable=True)
    rootfs_image_path: Mapped[str] = mapped_column(String(500), nullable=True)
    
    # Network configuration
    tap_device: Mapped[str] = mapped_column(String(50), nullable=True)
    ip_address: Mapped[str] = mapped_column(String(45), nullable=True)  # IPv4/IPv6
    
    # Metadata
    description: Mapped[str] = mapped_column(Text, nullable=True)
    tags: Mapped[str] = mapped_column(Text, nullable=True)  # JSON array as string
    
    # State
    is_persistent: Mapped[bool] = mapped_column(Boolean, default=True, nullable=False)
    auto_suspend_minutes: Mapped[int] = mapped_column(Integer, default=60, nullable=True)
    
    # Timestamps
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now(), nullable=False
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now(), onupdate=func.now(), nullable=False
    )
    last_accessed: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=True)

    # Relationships
    owner: Mapped["User"] = relationship("User", back_populates="vms")
    sessions: Mapped[list["Session"]] = relationship("Session", back_populates="vm", cascade="all, delete-orphan")
    snapshots: Mapped[list["Snapshot"]] = relationship("Snapshot", back_populates="vm", cascade="all, delete-orphan")

    def __repr__(self) -> str:
        return f"<VM(id={self.id}, name='{self.name}', status='{self.status}')>"