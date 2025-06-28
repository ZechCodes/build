"""Message protocols for WebSocket terminal communication."""

import json
import time
import base64
import hashlib
import hmac
import secrets
from typing import Dict, Any, Union, Optional, Tuple
from enum import Enum
import structlog
from dataclasses import dataclass
from .compression import get_message_compressor, CompressionAlgorithm

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
    
    # Protocol negotiation
    PROTOCOL_NEGOTIATE = "protocol_negotiate"
    PROTOCOL_ACCEPTED = "protocol_accepted"
    PROTOCOL_REJECTED = "protocol_rejected"
    
    # System
    HEARTBEAT = "heartbeat"
    HEARTBEAT_RESPONSE = "heartbeat_response"
    ERROR = "error"
    ACK = "ack"


@dataclass
class ProtocolVersion:
    """Protocol version information."""
    major: int
    minor: int
    patch: int
    
    def __str__(self) -> str:
        return f"{self.major}.{self.minor}.{self.patch}"
    
    def to_dict(self) -> Dict[str, int]:
        return {"major": self.major, "minor": self.minor, "patch": self.patch}
    
    @classmethod
    def from_dict(cls, data: Dict[str, int]) -> 'ProtocolVersion':
        return cls(data["major"], data["minor"], data["patch"])
    
    def is_compatible(self, other: 'ProtocolVersion') -> bool:
        """Check if this version is compatible with another."""
        # Compatible if major version matches and this minor >= other minor
        return (self.major == other.major and 
                self.minor >= other.minor)
    
    def __lt__(self, other: 'ProtocolVersion') -> bool:
        return (self.major, self.minor, self.patch) < (other.major, other.minor, other.patch)
    
    def __le__(self, other: 'ProtocolVersion') -> bool:
        return (self.major, self.minor, self.patch) <= (other.major, other.minor, other.patch)
    
    def __gt__(self, other: 'ProtocolVersion') -> bool:
        return (self.major, self.minor, self.patch) > (other.major, other.minor, other.patch)
    
    def __ge__(self, other: 'ProtocolVersion') -> bool:
        return (self.major, self.minor, self.patch) >= (other.major, other.minor, other.patch)
    
    def __eq__(self, other: 'ProtocolVersion') -> bool:
        return (self.major, self.minor, self.patch) == (other.major, other.minor, other.patch)


class ProtocolState(Enum):
    """Protocol negotiation states."""
    INIT = "init"
    NEGOTIATING = "negotiating"
    ESTABLISHED = "established"
    REJECTED = "rejected"
    ERROR = "error"


class MessageProtocol:
    """Enhanced protocol handler for WebSocket terminal messages with version validation."""
    
    # Current protocol version
    CURRENT_VERSION = ProtocolVersion(1, 2, 0)
    
    # Supported protocol versions (in order of preference)
    SUPPORTED_VERSIONS = [
        ProtocolVersion(1, 2, 0),  # Current
        ProtocolVersion(1, 1, 0),  # Previous compatible
        ProtocolVersion(1, 0, 0),  # Legacy compatible
    ]
    
    # Minimum supported version (older versions rejected)
    MIN_VERSION = ProtocolVersion(1, 0, 0)
    
    def __init__(self):
        self.max_message_size = 1024 * 1024  # 1MB
        self.max_terminal_data_size = 64 * 1024  # 64KB for terminal data chunks
        
        # Protocol validation state
        self.negotiated_version: Optional[ProtocolVersion] = None
        self.protocol_state = ProtocolState.INIT
        self.client_capabilities: Dict[str, Any] = {}
        
        # Message integrity verification
        self.enable_integrity_checks = True
        self.sequence_number = 0
        self.integrity_key = secrets.token_bytes(32)  # 256-bit key for HMAC
        
        # Compression settings
        self.enable_compression = True
        self.compression_threshold = 200  # Compress messages > 200 bytes
        self.preferred_algorithm = CompressionAlgorithm.GZIP
    
    def encode_message(self, message_type: MessageType, data: Any = None, 
                      session_id: Optional[str] = None, 
                      require_protocol: bool = True) -> str:
        """Encode a message for transmission with protocol validation."""
        # Validate protocol state
        if require_protocol and self.protocol_state != ProtocolState.ESTABLISHED:
            if message_type not in [MessageType.PROTOCOL_NEGOTIATE, MessageType.PROTOCOL_ACCEPTED, 
                                  MessageType.PROTOCOL_REJECTED, MessageType.ERROR]:
                raise ValueError("Protocol not established")
        
        message = {
            "type": message_type.value,
            "timestamp": time.time(),
            "protocol_version": str(self.negotiated_version or self.CURRENT_VERSION),
            "sequence": self.sequence_number
        }
        
        self.sequence_number += 1
        
        if data is not None:
            # Handle binary data encoding
            if isinstance(data, bytes):
                message["data"] = base64.b64encode(data).decode('ascii')
                message["binary"] = True
                message["data_hash"] = hashlib.sha256(data).hexdigest()
            else:
                message["data"] = data
        
        if session_id:
            message["session_id"] = session_id
        
        # Add message integrity check
        if self.enable_integrity_checks:
            message["integrity"] = self._calculate_message_integrity(message)
        
        # Check if compression should be applied
        encoded = json.dumps(message, sort_keys=True)
        
        if (self.enable_compression and 
            len(encoded.encode()) > self.compression_threshold and
            self.negotiated_version and 
            self.negotiated_version >= ProtocolVersion(1, 1, 0)):
            
            try:
                # Use MessagePack compression
                compressor = get_message_compressor()
                compressed_envelope = compressor.create_compressed_message_envelope(
                    message, self.preferred_algorithm
                )
                
                # Encode the compressed envelope
                encoded = json.dumps(compressed_envelope, sort_keys=True)
                
                logger.debug("Message compressed", 
                           original_size=len(json.dumps(message)),
                           compressed_size=len(encoded),
                           algorithm=self.preferred_algorithm.value)
                
            except Exception as e:
                logger.warning("Compression failed, using uncompressed message", error=str(e))
                # Fall back to uncompressed
                encoded = json.dumps(message, sort_keys=True)
        
        if len(encoded.encode()) > self.max_message_size:
            raise ValueError(f"Message too large: {len(encoded)} bytes")
        
        return encoded
    
    def decode_message(self, raw_message: str) -> Dict[str, Any]:
        """Decode a received message with comprehensive validation."""
        try:
            message = json.loads(raw_message)
            
            # Validate message structure
            if not isinstance(message, dict) or "type" not in message:
                raise ValueError("Invalid message structure")
            
            # Validate protocol version
            if "protocol_version" in message:
                client_version_str = message["protocol_version"]
                try:
                    # Parse version string (e.g., "1.2.0")
                    version_parts = [int(x) for x in client_version_str.split(".")]
                    if len(version_parts) != 3:
                        raise ValueError("Invalid version format")
                    
                    client_version = ProtocolVersion(*version_parts)
                    
                    # Check if version is supported
                    if client_version < self.MIN_VERSION:
                        raise ValueError(f"Protocol version {client_version} not supported")
                    
                    # Check for downgrade attacks
                    if (self.negotiated_version and 
                        client_version < self.negotiated_version):
                        raise ValueError("Protocol downgrade attempt detected")
                        
                except (ValueError, TypeError) as e:
                    raise ValueError(f"Invalid protocol version: {e}")
            
            # Validate message integrity
            if self.enable_integrity_checks and "integrity" in message:
                expected_integrity = self._calculate_message_integrity(
                    {k: v for k, v in message.items() if k != "integrity"}
                )
                if not hmac.compare_digest(message["integrity"], expected_integrity):
                    raise ValueError("Message integrity check failed")
            
            # Validate sequence number (prevent replay attacks)
            if "sequence" in message:
                seq_num = message["sequence"]
                if not isinstance(seq_num, int) or seq_num < 0:
                    raise ValueError("Invalid sequence number")
            
            # Handle compressed messages
            if message.get("type") == "compressed_message":
                try:
                    compressor = get_message_compressor()
                    message = compressor.extract_compressed_message(message)
                    logger.debug("Message decompressed", 
                               algorithm=message.get("compression", {}).get("algorithm", "unknown"))
                except Exception as e:
                    raise ValueError(f"Decompression failed: {e}")
            
            # Decode binary data if present
            if message.get("binary") and "data" in message:
                decoded_data = base64.b64decode(message["data"])
                
                # Verify data hash if present
                if "data_hash" in message:
                    calculated_hash = hashlib.sha256(decoded_data).hexdigest()
                    if not hmac.compare_digest(message["data_hash"], calculated_hash):
                        raise ValueError("Binary data integrity check failed")
                
                message["data"] = decoded_data
            
            # Validate message type
            try:
                msg_type = MessageType(message["type"])
                
                # Validate protocol state transitions
                if not self._validate_message_for_state(msg_type):
                    raise ValueError(f"Message type {msg_type.value} not allowed in current protocol state")
                    
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
    
    def _calculate_message_integrity(self, message: Dict[str, Any]) -> str:
        """Calculate HMAC integrity hash for a message."""
        # Create canonical representation for hashing
        message_str = json.dumps(message, sort_keys=True, separators=(',', ':'))
        integrity_hash = hmac.new(
            self.integrity_key,
            message_str.encode('utf-8'),
            hashlib.sha256
        ).hexdigest()
        return integrity_hash
    
    def _validate_message_for_state(self, message_type: MessageType) -> bool:
        """Validate if message type is allowed in current protocol state."""
        # Always allow protocol negotiation and error messages
        always_allowed = {
            MessageType.PROTOCOL_NEGOTIATE,
            MessageType.PROTOCOL_ACCEPTED,
            MessageType.PROTOCOL_REJECTED,
            MessageType.ERROR
        }
        
        if message_type in always_allowed:
            return True
        
        # Other messages require established protocol
        if self.protocol_state == ProtocolState.ESTABLISHED:
            return True
        
        return False
    
    def negotiate_protocol(self, client_versions: list, client_capabilities: Dict[str, Any]) -> Tuple[bool, ProtocolVersion, str]:
        """
        Negotiate protocol version with client.
        
        Returns:
            (success: bool, negotiated_version: ProtocolVersion, reason: str)
        """
        try:
            # Parse client versions
            parsed_versions = []
            for version_str in client_versions:
                try:
                    parts = [int(x) for x in str(version_str).split(".")]
                    if len(parts) == 3:
                        parsed_versions.append(ProtocolVersion(*parts))
                except (ValueError, TypeError):
                    continue
            
            if not parsed_versions:
                return False, self.MIN_VERSION, "No valid client versions provided"
            
            # Find best compatible version
            for server_version in self.SUPPORTED_VERSIONS:
                for client_version in sorted(parsed_versions, reverse=True):
                    if server_version.is_compatible(client_version):
                        self.negotiated_version = server_version
                        self.protocol_state = ProtocolState.ESTABLISHED
                        self.client_capabilities = client_capabilities
                        
                        logger.info("Protocol negotiated", 
                                  server_version=str(server_version),
                                  client_version=str(client_version),
                                  capabilities=client_capabilities)
                        
                        return True, server_version, f"Negotiated version {server_version}"
            
            # No compatible version found
            self.protocol_state = ProtocolState.REJECTED
            return False, self.MIN_VERSION, f"No compatible version found. Server supports {[str(v) for v in self.SUPPORTED_VERSIONS]}"
            
        except Exception as e:
            self.protocol_state = ProtocolState.ERROR
            logger.error("Protocol negotiation error", error=str(e))
            return False, self.MIN_VERSION, f"Protocol negotiation error: {e}"
    
    def create_protocol_negotiate_message(self, client_versions: list, capabilities: Dict[str, Any]) -> str:
        """Create a protocol negotiation message."""
        self.protocol_state = ProtocolState.NEGOTIATING
        
        return self.encode_message(
            MessageType.PROTOCOL_NEGOTIATE,
            {
                "supported_versions": [str(v) for v in client_versions],
                "capabilities": capabilities,
                "client_id": secrets.token_hex(16)
            },
            require_protocol=False
        )
    
    def create_protocol_accepted_message(self, version: ProtocolVersion, server_capabilities: Dict[str, Any]) -> str:
        """Create a protocol accepted message."""
        return self.encode_message(
            MessageType.PROTOCOL_ACCEPTED,
            {
                "negotiated_version": str(version),
                "server_capabilities": server_capabilities,
                "protocol_features": self._get_protocol_features(version)
            },
            require_protocol=False
        )
    
    def create_protocol_rejected_message(self, reason: str, supported_versions: list) -> str:
        """Create a protocol rejected message."""
        self.protocol_state = ProtocolState.REJECTED
        
        return self.encode_message(
            MessageType.PROTOCOL_REJECTED,
            {
                "reason": reason,
                "server_supported_versions": [str(v) for v in supported_versions],
                "min_version": str(self.MIN_VERSION)
            },
            require_protocol=False
        )
    
    def _get_protocol_features(self, version: ProtocolVersion) -> Dict[str, Any]:
        """Get available features for a protocol version."""
        features = {
            "message_integrity": True,
            "binary_data_validation": True,
            "sequence_numbers": True,
            "compression": False,
            "acknowledgments": False
        }
        
        # Add features based on version
        if version >= ProtocolVersion(1, 1, 0):
            features["compression"] = True
            features["compression_algorithms"] = ["gzip", "deflate", "brotli"]
            features["messagepack_support"] = True
        
        if version >= ProtocolVersion(1, 2, 0):
            features["acknowledgments"] = True
            features["enhanced_security"] = True
            features["adaptive_compression"] = True
        
        return features
    
    def validate_protocol_downgrade(self, new_version: ProtocolVersion) -> bool:
        """Validate that protocol version change is not a downgrade attack."""
        if not self.negotiated_version:
            return True  # Initial negotiation
        
        # Allow same version or upgrades, but not downgrades
        return new_version >= self.negotiated_version
    
    def reset_protocol_state(self):
        """Reset protocol state for new connection."""
        self.negotiated_version = None
        self.protocol_state = ProtocolState.INIT
        self.client_capabilities = {}
        self.sequence_number = 0
        self.integrity_key = secrets.token_bytes(32)
    
    def get_protocol_status(self) -> Dict[str, Any]:
        """Get current protocol status for monitoring."""
        return {
            "state": self.protocol_state.value,
            "negotiated_version": str(self.negotiated_version) if self.negotiated_version else None,
            "supported_versions": [str(v) for v in self.SUPPORTED_VERSIONS],
            "min_version": str(self.MIN_VERSION),
            "client_capabilities": self.client_capabilities,
            "sequence_number": self.sequence_number,
            "integrity_enabled": self.enable_integrity_checks
        }