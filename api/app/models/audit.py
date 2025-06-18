"""Audit log model with UUID primary key."""

from sqlalchemy import Column, String, Text, ForeignKey, UUID, JSON
from sqlalchemy.orm import relationship

from .base import Base, TimestampMixin, UUIDMixin


class AuditLog(Base, UUIDMixin, TimestampMixin):
    """Audit log model for tracking user actions."""
    
    __tablename__ = "audit_logs"
    
    user_id = Column(UUID(as_uuid=True), ForeignKey("users.id"), nullable=True)
    action = Column(String(100), nullable=False)
    resource_type = Column(String(50), nullable=True)
    resource_id = Column(UUID(as_uuid=True), nullable=True)
    ip_address = Column(String(45), nullable=True)  # IPv4/IPv6 addresses
    user_agent = Column(Text, nullable=True)
    details = Column(JSON, nullable=True)
    
    # Relationships
    user = relationship("User", back_populates="audit_logs")

    def __repr__(self) -> str:
        return f"<AuditLog(id={self.id}, action='{self.action}', user_id={self.user_id})>"