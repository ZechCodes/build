"""Session model."""

from datetime import datetime
from enum import Enum
from sqlalchemy import String, Integer, DateTime, ForeignKey, Text, func
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.core.database import Base


class SessionStatus(str, Enum):
    """Session status enumeration."""
    ACTIVE = "active"
    INACTIVE = "inactive"
    EXPIRED = "expired"
    TERMINATED = "terminated"


class Session(Base):
    """Terminal session model."""

    __tablename__ = "sessions"

    id: Mapped[int] = mapped_column(primary_key=True, index=True)
    session_id: Mapped[str] = mapped_column(String(255), unique=True, index=True, nullable=False)
    status: Mapped[SessionStatus] = mapped_column(String(20), default=SessionStatus.ACTIVE, nullable=False)
    
    # Relationships
    user_id: Mapped[int] = mapped_column(ForeignKey("users.id"), nullable=False)
    vm_id: Mapped[int] = mapped_column(ForeignKey("vms.id"), nullable=False)
    
    # Session configuration
    terminal_cols: Mapped[int] = mapped_column(Integer, default=80, nullable=False)
    terminal_rows: Mapped[int] = mapped_column(Integer, default=24, nullable=False)
    shell: Mapped[str] = mapped_column(String(100), default="/bin/bash", nullable=False)
    working_directory: Mapped[str] = mapped_column(String(500), default="/home/user", nullable=False)
    
    # PTY information
    pty_pid: Mapped[int] = mapped_column(Integer, nullable=True)
    pty_fd: Mapped[int] = mapped_column(Integer, nullable=True)
    
    # WebSocket connection
    websocket_id: Mapped[str] = mapped_column(String(255), nullable=True)
    client_ip: Mapped[str] = mapped_column(String(45), nullable=True)
    user_agent: Mapped[str] = mapped_column(String(500), nullable=True)
    
    # Session metadata
    title: Mapped[str] = mapped_column(String(255), nullable=True)
    environment_vars: Mapped[str] = mapped_column(Text, nullable=True)  # JSON object as string
    
    # Recording
    is_recorded: Mapped[bool] = mapped_column(default=False, nullable=False)
    recording_path: Mapped[str] = mapped_column(String(500), nullable=True)
    
    # Timestamps
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now(), nullable=False
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now(), onupdate=func.now(), nullable=False
    )
    last_activity: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), server_default=func.now(), nullable=False
    )
    expires_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=True)

    # Relationships
    user: Mapped["User"] = relationship("User", back_populates="sessions")
    vm: Mapped["VM"] = relationship("VM", back_populates="sessions")

    def __repr__(self) -> str:
        return f"<Session(id={self.id}, session_id='{self.session_id}', status='{self.status}')>"