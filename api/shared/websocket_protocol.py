"""
WebSocket Protocol Specification for Terminal Communication

This module defines the message protocol for WebSocket communication between
the frontend terminal and the PTY layer for Sessions 5 & 8 integration.
"""

from datetime import datetime
from enum import Enum
from typing import Any, Dict, List, Optional, Union
from pydantic import BaseModel, Field


class MessageType(str, Enum):
    """WebSocket message types for terminal communication."""
    TERMINAL_DATA = "terminal_data"
    RESIZE = "resize"
    SESSION_CONTROL = "session_control"
    HEARTBEAT = "heartbeat"
    AUTH_REFRESH = "auth_refresh"
    ERROR = "error"
    ACK = "ack"


class DataDirection(str, Enum):
    """Direction of terminal data flow."""
    INPUT = "input"
    OUTPUT = "output"


class SessionAction(str, Enum):
    """Actions for session control messages."""
    PAUSE = "pause"
    RESUME = "resume"
    SNAPSHOT = "snapshot"
    RESTORE = "restore"
    TERMINATE = "terminate"


class WSMessage(BaseModel):
    """Base WebSocket message structure."""
    type: MessageType
    session_id: str = Field(..., description="Session identifier")
    timestamp: datetime = Field(default_factory=datetime.utcnow)
    message_id: str = Field(..., description="Unique message identifier")
    data: Optional[Dict[str, Any]] = None


class TerminalDataMessage(WSMessage):
    """Message for terminal input/output data."""
    type: MessageType = MessageType.TERMINAL_DATA
    data: Dict[str, Any] = Field(
        ...,
        description="Terminal data payload",
        example={
            "content": "ls -la\n",
            "direction": "input",
            "encoding": "utf-8"
        }
    )


class ResizeMessage(WSMessage):
    """Message for terminal resize events."""
    type: MessageType = MessageType.RESIZE
    data: Dict[str, Any] = Field(
        ...,
        description="Terminal dimensions",
        example={
            "cols": 80,
            "rows": 24,
            "width": 640,
            "height": 480
        }
    )


class SessionControlMessage(WSMessage):
    """Message for session control operations."""
    type: MessageType = MessageType.SESSION_CONTROL
    data: Dict[str, Any] = Field(
        ...,
        description="Session control payload",
        example={
            "action": "snapshot",
            "metadata": {"name": "before-deploy", "description": "Pre-deployment state"}
        }
    )


class HeartbeatMessage(WSMessage):
    """Heartbeat message to maintain connection."""
    type: MessageType = MessageType.HEARTBEAT
    data: Dict[str, Any] = Field(
        default_factory=lambda: {"ping": True},
        description="Heartbeat payload"
    )


class AuthRefreshMessage(WSMessage):
    """Message for token refresh during long sessions."""
    type: MessageType = MessageType.AUTH_REFRESH
    data: Dict[str, Any] = Field(
        ...,
        description="Authentication refresh payload",
        example={
            "refresh_token": "...",
            "new_access_token": "..."
        }
    )


class ErrorMessage(WSMessage):
    """Error message for WebSocket communication."""
    type: MessageType = MessageType.ERROR
    data: Dict[str, Any] = Field(
        ...,
        description="Error details",
        example={
            "code": "TERMINAL_UNAVAILABLE",
            "message": "Terminal session has expired",
            "retry_after": 5
        }
    )


class AckMessage(WSMessage):
    """Acknowledgment message for reliable delivery."""
    type: MessageType = MessageType.ACK
    data: Dict[str, Any] = Field(
        ...,
        description="Acknowledgment payload",
        example={
            "ack_message_id": "msg-123",
            "status": "received"
        }
    )


class WSProtocolConfig:
    """Configuration for WebSocket protocol behavior."""
    
    # Connection management
    HEARTBEAT_INTERVAL = 30  # seconds
    CONNECTION_TIMEOUT = 300  # seconds
    MAX_RECONNECT_ATTEMPTS = 5
    RECONNECT_BACKOFF_MULTIPLIER = 2.0
    
    # Rate limiting
    MAX_MESSAGES_PER_MINUTE = 1000
    MAX_DATA_SIZE_BYTES = 1024 * 1024  # 1MB
    MAX_TERMINAL_OUTPUT_RATE = 10 * 1024 * 1024  # 10MB/s
    
    # Message reliability
    ENABLE_MESSAGE_ACKNOWLEDGMENTS = True
    MESSAGE_TIMEOUT = 10  # seconds
    MAX_PENDING_MESSAGES = 100
    
    # Security
    VALIDATE_SESSION_OWNERSHIP = True
    REQUIRE_AUTH_REFRESH = True
    TOKEN_REFRESH_THRESHOLD = 120  # seconds before expiry


class WSConnectionState(str, Enum):
    """WebSocket connection states."""
    CONNECTING = "connecting"
    CONNECTED = "connected"
    AUTHENTICATED = "authenticated"
    TERMINAL_ATTACHED = "terminal_attached"
    DISCONNECTING = "disconnecting"
    DISCONNECTED = "disconnected"
    ERROR = "error"


class WSConnection:
    """WebSocket connection management."""
    
    def __init__(self, websocket, user_id: str, session_id: str):
        self.websocket = websocket
        self.user_id = user_id
        self.session_id = session_id
        self.state = WSConnectionState.CONNECTING
        self.last_heartbeat = datetime.utcnow()
        self.message_count = 0
        self.pending_acks: Dict[str, datetime] = {}
        
    async def send_message(self, message: WSMessage) -> bool:
        """Send message with optional acknowledgment tracking."""
        try:
            message_data = message.model_dump_json()
            await self.websocket.send_text(message_data)
            
            if WSProtocolConfig.ENABLE_MESSAGE_ACKNOWLEDGMENTS:
                self.pending_acks[message.message_id] = datetime.utcnow()
                
            self.message_count += 1
            return True
            
        except Exception as e:
            self.state = WSConnectionState.ERROR
            return False
    
    async def handle_ack(self, ack_message: AckMessage):
        """Handle acknowledgment message."""
        ack_id = ack_message.data.get("ack_message_id")
        if ack_id in self.pending_acks:
            del self.pending_acks[ack_id]
    
    def is_healthy(self) -> bool:
        """Check connection health."""
        now = datetime.utcnow()
        heartbeat_age = (now - self.last_heartbeat).total_seconds()
        
        return (
            self.state in [WSConnectionState.CONNECTED, WSConnectionState.AUTHENTICATED, WSConnectionState.TERMINAL_ATTACHED]
            and heartbeat_age < WSProtocolConfig.CONNECTION_TIMEOUT
            and len(self.pending_acks) < WSProtocolConfig.MAX_PENDING_MESSAGES
        )


# Message factory functions
def create_terminal_data(session_id: str, content: Union[str, bytes], direction: DataDirection, encoding: str = "utf-8") -> TerminalDataMessage:
    """Create a terminal data message."""
    import uuid
    
    if isinstance(content, bytes):
        content = content.decode(encoding, errors='replace')
    
    return TerminalDataMessage(
        session_id=session_id,
        message_id=str(uuid.uuid4()),
        data={
            "content": content,
            "direction": direction.value,
            "encoding": encoding
        }
    )


def create_resize_message(session_id: str, cols: int, rows: int, width: int = None, height: int = None) -> ResizeMessage:
    """Create a terminal resize message."""
    import uuid
    
    return ResizeMessage(
        session_id=session_id,
        message_id=str(uuid.uuid4()),
        data={
            "cols": cols,
            "rows": rows,
            "width": width,
            "height": height
        }
    )


def create_session_control(session_id: str, action: SessionAction, metadata: Dict[str, Any] = None) -> SessionControlMessage:
    """Create a session control message."""
    import uuid
    
    return SessionControlMessage(
        session_id=session_id,
        message_id=str(uuid.uuid4()),
        data={
            "action": action.value,
            "metadata": metadata or {}
        }
    )


def create_heartbeat(session_id: str) -> HeartbeatMessage:
    """Create a heartbeat message."""
    import uuid
    
    return HeartbeatMessage(
        session_id=session_id,
        message_id=str(uuid.uuid4())
    )


def create_error_message(session_id: str, code: str, message: str, retry_after: int = None) -> ErrorMessage:
    """Create an error message."""
    import uuid
    
    return ErrorMessage(
        session_id=session_id,
        message_id=str(uuid.uuid4()),
        data={
            "code": code,
            "message": message,
            "retry_after": retry_after
        }
    )