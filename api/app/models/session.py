"""Session model with UUID primary key."""

from sqlalchemy import Column, String, DateTime, ForeignKey, UUID
from sqlalchemy.orm import relationship

from .base import Base, TimestampMixin, UUIDMixin


class Session(Base, UUIDMixin, TimestampMixin):
    """Session model for terminal sessions."""
    
    __tablename__ = "sessions"
    
    vm_instance_id = Column(UUID(as_uuid=True), ForeignKey("vm_instances.id", ondelete="CASCADE"), nullable=False)
    user_id = Column(UUID(as_uuid=True), ForeignKey("users.id", ondelete="CASCADE"), nullable=False)
    session_token = Column(String(255), unique=True, nullable=False)
    state = Column(String(20), nullable=False, default="active")
    last_activity = Column(DateTime(timezone=True), nullable=False)
    expires_at = Column(DateTime(timezone=True), nullable=False)
    
    # Relationships
    vm_instance = relationship("VMInstance", back_populates="sessions")
    user = relationship("User", back_populates="sessions")

    def __repr__(self) -> str:
        return f"<Session(id={self.id}, token='{self.session_token[:8]}...', state='{self.state}')>"