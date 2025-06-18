"""VM Instance model with UUID primary key."""

from sqlalchemy import Column, String, DateTime, ForeignKey, UUID, JSON
from sqlalchemy.orm import relationship

from .base import Base, TimestampMixin, UUIDMixin


class VMInstance(Base, UUIDMixin, TimestampMixin):
    """VM Instance model for managing virtual machines."""
    
    __tablename__ = "vm_instances"
    
    user_id = Column(UUID(as_uuid=True), ForeignKey("users.id", ondelete="CASCADE"), nullable=False)
    name = Column(String(100), nullable=False)
    state = Column(String(20), nullable=False, default="stopped")
    firecracker_id = Column(String(50), unique=True, nullable=True)
    config = Column(JSON, nullable=False)
    started_at = Column(DateTime(timezone=True), nullable=True)
    stopped_at = Column(DateTime(timezone=True), nullable=True)
    
    # Relationships
    user = relationship("User", back_populates="vm_instances")
    sessions = relationship("Session", back_populates="vm_instance", cascade="all, delete-orphan")
    snapshots = relationship("Snapshot", back_populates="vm_instance", cascade="all, delete-orphan")

    def __repr__(self) -> str:
        return f"<VMInstance(id={self.id}, name='{self.name}', state='{self.state}')>"