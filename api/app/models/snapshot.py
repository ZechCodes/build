"""Snapshot model with UUID primary key."""

from sqlalchemy import Column, String, Text, BigInteger, ForeignKey, UUID, JSON
from sqlalchemy.orm import relationship

from .base import Base, TimestampMixin, UUIDMixin


class Snapshot(Base, UUIDMixin, TimestampMixin):
    """Snapshot model for VM snapshots."""
    
    __tablename__ = "snapshots"
    
    vm_instance_id = Column(UUID(as_uuid=True), ForeignKey("vm_instances.id", ondelete="CASCADE"), nullable=False)
    user_id = Column(UUID(as_uuid=True), ForeignKey("users.id", ondelete="CASCADE"), nullable=False)
    name = Column(String(100), nullable=False)
    description = Column(Text, nullable=True)
    storage_path = Column(String(500), nullable=False)
    size_bytes = Column(BigInteger, nullable=True)
    snapshot_metadata = Column(JSON, nullable=True)
    
    # Relationships
    vm_instance = relationship("VMInstance", back_populates="snapshots")
    user = relationship("User", back_populates="snapshots")

    def __repr__(self) -> str:
        return f"<Snapshot(id={self.id}, name='{self.name}', vm_id={self.vm_instance_id})>"