"""Message protocols for WebSocket terminal communication."""

import json
import time
import base64
from typing import Dict, Any, Union, Optional
from enum import Enum
import structlog

logger = structlog.get_logger(__name__)


class MessageType(Enum):
    # Terminal data
    TERMINAL_DATA = "terminal_data"
    TERMINAL_RESIZE = "terminal_resize"
    
    # Session management
    SESSION_CREATE = "session_create"
    SESSION_JOIN = "session_join"
    SESSION_RESTORE = "session_restore"
    SESSION_STATUS = "session_status"
    SESSION_CREATED = "session_created"
    SESSION_RECOVERED = "session_recovered"
    SESSION_ENDED = "session_ended"
    
    # Authentication
    AUTH_REQUEST = "auth_request"
    AUTH_RESPONSE = "auth_response"
    
    # System
    HEARTBEAT = "heartbeat"
    HEARTBEAT_RESPONSE = "heartbeat_response"
    ERROR = "error"
    ACK = "ack"


class MessageProtocol:
    """Protocol handler for WebSocket terminal messages."""
    
    def __init__(self):
        self.max_message_size = 1024 * 1024  # 1MB
        self.max_terminal_data_size = 64 * 1024  # 64KB for terminal data chunks
    
    def encode_message(self, message_type: MessageType, data: Any = None, 
                      session_id: Optional[str] = None) -> str:
        """Encode a message for transmission."""
        message = {
            "type": message_type.value,
            "timestamp": time.time()
        }
        
        if data is not None:
            # Handle binary data encoding
            if isinstance(data, bytes):
                message["data"] = base64.b64encode(data).decode('ascii')
                message["binary"] = True
            else:
                message["data"] = data
        
        if session_id:
            message["session_id"] = session_id
        
        encoded = json.dumps(message)
        
        if len(encoded.encode()) > self.max_message_size:
            raise ValueError(f"Message too large: {len(encoded)} bytes")
        
        return encoded
    
    def decode_message(self, raw_message: str) -> Dict[str, Any]:
        """Decode a received message."""
        try:
            message = json.loads(raw_message)
            
            # Validate message structure
            if not isinstance(message, dict) or "type" not in message:
                raise ValueError("Invalid message structure")
            
            # Decode binary data if present
            if message.get("binary") and "data" in message:
                message["data"] = base64.b64decode(message["data"])
            
            # Validate message type
            try:
                MessageType(message["type"])
            except ValueError:
                raise ValueError(f"Unknown message type: {message['type']}")
            
            return message
            
        except json.JSONDecodeError as e:
            raise ValueError(f"Invalid JSON: {e}")
        except Exception as e:
            logger.error("Failed to decode message", error=str(e))
            raise ValueError(f"Message decode error: {e}")
    
    def create_terminal_data_message(self, data: bytes, session_id: str) -> str:
        """Create a terminal data message."""
        if len(data) > self.max_terminal_data_size:
            logger.warning("Terminal data chunk too large", size=len(data))
            # Truncate data if too large
            data = data[:self.max_terminal_data_size]
        
        return self.encode_message(
            MessageType.TERMINAL_DATA, 
            data, 
            session_id=session_id
        )
    
    def create_terminal_resize_message(self, rows: int, cols: int, session_id: str) -> str:
        """Create a terminal resize message."""
        return self.encode_message(
            MessageType.TERMINAL_RESIZE,
            {"rows": rows, "cols": cols},
            session_id=session_id
        )
    
    def create_error_message(self, error_code: str, error_message: str) -> str:
        """Create an error message."""
        return self.encode_message(
            MessageType.ERROR,
            {"code": error_code, "message": error_message}
        )
    
    def create_session_created_message(self, session_id: str) -> str:
        """Create a session created message."""
        return self.encode_message(
            MessageType.SESSION_CREATED,
            {"session_id": session_id}
        )
    
    def create_session_status_message(self, session_id: str, status: str) -> str:
        """Create a session status message."""
        return self.encode_message(
            MessageType.SESSION_STATUS,
            {"session_id": session_id, "status": status}
        )
    
    def create_session_recovered_message(self, session_id: str, history: list) -> str:
        """Create a session recovery message."""
        return self.encode_message(
            MessageType.SESSION_RECOVERED,
            {
                "session_id": session_id,
                "recovery_data": {
                    "history": history,
                    "recovery_timestamp": time.time()
                }
            }
        )
    
    def create_heartbeat_message(self) -> str:
        """Create a heartbeat message."""
        return self.encode_message(MessageType.HEARTBEAT)
    
    def create_ack_message(self, original_type: str, success: bool = True) -> str:
        """Create an acknowledgment message."""
        return self.encode_message(
            MessageType.ACK,
            {"original_type": original_type, "success": success}
        )
    
    def validate_terminal_data(self, data: Any) -> bool:
        """Validate terminal data for security."""
        if isinstance(data, bytes):
            # Check for reasonable size
            if len(data) > self.max_terminal_data_size:
                return False
            return True
        elif isinstance(data, str):
            # Check for reasonable size
            if len(data.encode()) > self.max_terminal_data_size:
                return False
            return True
        return False
    
    def validate_resize_data(self, data: Any) -> bool:
        """Validate terminal resize data."""
        if not isinstance(data, dict):
            return False
        
        rows = data.get("rows")
        cols = data.get("cols")
        
        if not isinstance(rows, int) or not isinstance(cols, int):
            return False
        
        # Reasonable terminal size limits
        if rows < 1 or rows > 1000 or cols < 1 or cols > 1000:
            return False
        
        return True