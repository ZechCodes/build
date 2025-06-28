"""Message encryption for sensitive WebSocket data."""

import base64
import json
import secrets
import time
from typing import Dict, Any, Optional, Union
from cryptography.fernet import Fernet
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC
import structlog

from app.core.config import get_settings

logger = structlog.get_logger(__name__)


class MessageEncryption:
    """Handle encryption/decryption of sensitive WebSocket messages."""
    
    def __init__(self):
        self.settings = get_settings()
        self._encryption_key = None
        self._salt = None
        self._fernet = None
        
        # Message types that should be encrypted
        self.sensitive_message_types = {
            "terminal_data",  # Terminal content may contain sensitive info
            "session_create", # Session creation data
            "session_restore", # Session restore data
            "auth_refresh",   # Authentication data
        }
        
        # Initialize encryption
        self._initialize_encryption()
    
    def _initialize_encryption(self):
        """Initialize encryption system with key derivation."""
        try:
            # Generate or retrieve salt for key derivation
            self._salt = self._get_or_create_salt()
            
            # Derive encryption key from JWT secret + salt
            kdf = PBKDF2HMAC(
                algorithm=hashes.SHA256(),
                length=32,
                salt=self._salt,
                iterations=100000,
            )
            
            # Use JWT secret as base for key derivation
            jwt_secret = self.settings.jwt_secret.encode('utf-8')
            key = base64.urlsafe_b64encode(kdf.derive(jwt_secret))
            
            # Create Fernet instance for encryption
            self._fernet = Fernet(key)
            
            logger.info("Message encryption initialized successfully")
            
        except Exception as e:
            logger.error("Failed to initialize message encryption", error=str(e))
            # Fall back to no encryption if initialization fails
            self._fernet = None
    
    def _get_or_create_salt(self) -> bytes:
        """Get or create encryption salt."""
        # In production, this should be stored securely (e.g., in secrets manager)
        # For now, derive from a combination of JWT secret and fixed string
        salt_source = f"{self.settings.jwt_secret}:websocket_encryption_salt"
        return salt_source.encode('utf-8')[:16]  # Use first 16 bytes as salt
    
    def should_encrypt_message(self, message_type: str) -> bool:
        """Determine if a message type should be encrypted."""
        return message_type in self.sensitive_message_types
    
    def encrypt_message(self, message: Dict[str, Any]) -> Dict[str, Any]:
        """Encrypt sensitive parts of a message."""
        if not self._fernet:
            logger.warning("Encryption not available, sending message unencrypted")
            return message
        
        message_type = message.get("type", "")
        
        # Only encrypt sensitive message types
        if not self.should_encrypt_message(message_type):
            return message
        
        try:
            # Create encrypted payload
            encrypted_data = self._encrypt_payload(message)
            
            # Create new message with encrypted payload
            encrypted_message = {
                "type": "encrypted_message",
                "original_type": message_type,
                "encrypted_data": encrypted_data,
                "timestamp": message.get("timestamp", time.time()),
                "encryption_version": "1.0"
            }
            
            logger.debug("Message encrypted", original_type=message_type)
            return encrypted_message
            
        except Exception as e:
            logger.error("Failed to encrypt message", 
                        message_type=message_type,
                        error=str(e))
            # Return original message if encryption fails
            return message
    
    def decrypt_message(self, message: Dict[str, Any]) -> Dict[str, Any]:
        """Decrypt an encrypted message."""
        if not self._fernet:
            logger.warning("Encryption not available, cannot decrypt message")
            return message
        
        # Check if this is an encrypted message
        if message.get("type") != "encrypted_message":
            return message  # Not encrypted, return as-is
        
        try:
            encrypted_data = message.get("encrypted_data")
            if not encrypted_data:
                raise ValueError("Missing encrypted_data in encrypted message")
            
            # Decrypt the payload
            decrypted_message = self._decrypt_payload(encrypted_data)
            
            # Restore original message type
            original_type = message.get("original_type")
            if original_type:
                decrypted_message["type"] = original_type
            
            logger.debug("Message decrypted", original_type=original_type)
            return decrypted_message
            
        except Exception as e:
            logger.error("Failed to decrypt message", error=str(e))
            # Return error message if decryption fails
            return {
                "type": "error",
                "data": {
                    "code": "DECRYPTION_FAILED",
                    "message": "Failed to decrypt message"
                }
            }
    
    def _encrypt_payload(self, data: Dict[str, Any]) -> str:
        """Encrypt a data payload."""
        # Convert to JSON and encrypt
        json_data = json.dumps(data, sort_keys=True)
        encrypted_bytes = self._fernet.encrypt(json_data.encode('utf-8'))
        
        # Return base64 encoded string
        return base64.b64encode(encrypted_bytes).decode('ascii')
    
    def _decrypt_payload(self, encrypted_data: str) -> Dict[str, Any]:
        """Decrypt a data payload."""
        # Decode from base64 and decrypt
        encrypted_bytes = base64.b64decode(encrypted_data.encode('ascii'))
        decrypted_bytes = self._fernet.decrypt(encrypted_bytes)
        
        # Parse JSON
        json_data = decrypted_bytes.decode('utf-8')
        return json.loads(json_data)
    
    def encrypt_connection_state(self, state_data: Dict[str, Any]) -> str:
        """Encrypt connection state for Redis storage."""
        if not self._fernet:
            # Fall back to JSON without encryption
            return json.dumps(state_data)
        
        try:
            return self._encrypt_payload(state_data)
        except Exception as e:
            logger.error("Failed to encrypt connection state", error=str(e))
            # Fall back to unencrypted JSON
            return json.dumps(state_data)
    
    def decrypt_connection_state(self, encrypted_state: str) -> Dict[str, Any]:
        """Decrypt connection state from Redis storage."""
        if not self._fernet:
            # Fall back to JSON parsing
            try:
                return json.loads(encrypted_state)
            except json.JSONDecodeError:
                return {}
        
        try:
            # Try to decrypt first
            return self._decrypt_payload(encrypted_state)
        except Exception:
            # Fall back to JSON parsing (for backward compatibility)
            try:
                return json.loads(encrypted_state)
            except json.JSONDecodeError as e:
                logger.error("Failed to decrypt and parse connection state", error=str(e))
                return {}
    
    def create_secure_token(self, data: Dict[str, Any], ttl_seconds: int = 3600) -> str:
        """Create a secure, encrypted token with TTL."""
        if not self._fernet:
            raise ValueError("Encryption not available")
        
        # Add expiration timestamp
        token_data = {
            **data,
            "expires_at": time.time() + ttl_seconds,
            "created_at": time.time()
        }
        
        return self._encrypt_payload(token_data)
    
    def verify_secure_token(self, token: str) -> Optional[Dict[str, Any]]:
        """Verify and decrypt a secure token."""
        if not self._fernet:
            return None
        
        try:
            token_data = self._decrypt_payload(token)
            
            # Check expiration
            expires_at = token_data.get("expires_at", 0)
            if time.time() > expires_at:
                logger.warning("Secure token expired")
                return None
            
            return token_data
            
        except Exception as e:
            logger.warning("Failed to verify secure token", error=str(e))
            return None


class SecureWebSocketProtocol:
    """Enhanced WebSocket protocol with encryption support."""
    
    def __init__(self):
        self.encryption = MessageEncryption()
    
    def prepare_outgoing_message(self, message: Dict[str, Any], 
                                connection_id: str = None) -> Dict[str, Any]:
        """Prepare a message for sending (apply encryption if needed)."""
        # Add timestamp if not present
        if "timestamp" not in message:
            message["timestamp"] = time.time()
        
        # Add message ID for tracking
        if "message_id" not in message:
            message["message_id"] = secrets.token_urlsafe(16)
        
        # Apply encryption if needed
        return self.encryption.encrypt_message(message)
    
    def process_incoming_message(self, message: Dict[str, Any], 
                                connection_id: str = None) -> Dict[str, Any]:
        """Process an incoming message (apply decryption if needed)."""
        # Decrypt if encrypted
        decrypted_message = self.encryption.decrypt_message(message)
        
        # Validate timestamp (replay protection)
        timestamp = decrypted_message.get("timestamp", 0)
        current_time = time.time()
        
        # Allow 5 minute window for clock skew
        if abs(current_time - timestamp) > 300:
            logger.warning("Message timestamp outside acceptable window",
                         message_timestamp=timestamp,
                         current_time=current_time,
                         connection_id=connection_id)
            return {
                "type": "error",
                "data": {
                    "code": "INVALID_TIMESTAMP",
                    "message": "Message timestamp invalid"
                }
            }
        
        return decrypted_message


# Global instances
_message_encryption = None
_secure_protocol = None


def get_message_encryption() -> MessageEncryption:
    """Get global message encryption instance."""
    global _message_encryption
    if _message_encryption is None:
        _message_encryption = MessageEncryption()
    return _message_encryption


def get_secure_protocol() -> SecureWebSocketProtocol:
    """Get global secure WebSocket protocol instance."""
    global _secure_protocol
    if _secure_protocol is None:
        _secure_protocol = SecureWebSocketProtocol()
    return _secure_protocol