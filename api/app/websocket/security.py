"""WebSocket security utilities and validators."""

import time
from typing import List, Optional, Dict, Any
from urllib.parse import urlparse
from fastapi import WebSocket, HTTPException, status
import structlog

from app.core.config import get_settings
from .certificate_validator import get_certificate_validator

logger = structlog.get_logger(__name__)


class WebSocketSecurityValidator:
    """Security validator for WebSocket connections."""
    
    def __init__(self):
        self.settings = get_settings()
        self.allowed_origins = self.settings.websocket_allowed_origins
        self.require_origin = self.settings.websocket_require_origin
        self.max_message_size = self.settings.websocket_max_message_size
        
    def validate_origin(self, websocket: WebSocket) -> bool:
        """Validate WebSocket origin against allowed origins (CORS protection)."""
        try:
            # Get origin from headers
            origin = websocket.headers.get("origin")
            
            if not origin:
                logger.warning("WebSocket connection missing origin header")
                # Check if origin is required
                if self.require_origin and self.settings.environment == "production":
                    return False
                # In development, allow connections without origin (for testing)
                if not self.require_origin:
                    return True
                return self.settings.environment != "production"
            
            # Parse and normalize origin
            parsed_origin = urlparse(origin)
            normalized_origin = f"{parsed_origin.scheme}://{parsed_origin.netloc}"
            
            # Check against allowed origins
            if normalized_origin in self.allowed_origins:
                logger.info("WebSocket origin validated successfully", origin=normalized_origin)
                return True
            
            # Check for wildcard origins (for development)
            if "*" in self.allowed_origins:
                logger.info("WebSocket origin allowed via wildcard", origin=normalized_origin)
                return True
            
            logger.warning("WebSocket origin not allowed", 
                         origin=normalized_origin, 
                         allowed_origins=self.allowed_origins)
            return False
            
        except Exception as e:
            logger.error("Error validating WebSocket origin", error=str(e))
            return False
    
    def validate_connection_security(self, websocket: WebSocket) -> Dict[str, Any]:
        """Comprehensive connection security validation."""
        validation_result = {
            "valid": True,
            "errors": [],
            "warnings": [],
            "security_info": {}
        }
        
        # 1. Origin validation
        if not self.validate_origin(websocket):
            validation_result["valid"] = False
            validation_result["errors"].append("Invalid or missing origin")
        
        # 2. Check for secure connection in production
        if self.settings.environment == "production":
            # Check if connection is secure (WSS)
            scheme = websocket.url.scheme if hasattr(websocket, 'url') else None
            if scheme != "wss":
                validation_result["valid"] = False
                validation_result["errors"].append("Secure WebSocket (WSS) required in production")
        
        # 3. Extract connection security info
        validation_result["security_info"] = {
            "origin": websocket.headers.get("origin"),
            "user_agent": websocket.headers.get("user-agent"),
            "host": websocket.headers.get("host"),
            "x_forwarded_for": websocket.headers.get("x-forwarded-for"),
            "x_real_ip": websocket.headers.get("x-real-ip")
        }
        
        return validation_result
    
    def validate_websocket_certificate(self, websocket_url: str) -> Dict[str, Any]:
        """Validate WebSocket certificate and pinning."""
        validation_result = {
            "valid": True,
            "certificate_validated": False,
            "pinning_validated": False,
            "errors": [],
            "warnings": []
        }
        
        try:
            # Skip certificate validation for non-secure connections
            if not websocket_url.startswith('wss://'):
                validation_result["warnings"].append("Certificate validation skipped for non-WSS connection")
                return validation_result
            
            # Get certificate validator
            cert_validator = get_certificate_validator()
            
            # Validate certificate
            cert_result = cert_validator.validate_websocket_certificate(websocket_url)
            
            validation_result["certificate_validated"] = cert_result.valid
            validation_result["pinning_validated"] = cert_result.pinning_validated
            validation_result["errors"].extend(cert_result.errors)
            validation_result["warnings"].extend(cert_result.warnings)
            
            if not cert_result.valid:
                validation_result["valid"] = False
                logger.warning("WebSocket certificate validation failed",
                             url=websocket_url,
                             errors=cert_result.errors)
            else:
                logger.info("WebSocket certificate validated",
                           url=websocket_url,
                           pinning_validated=cert_result.pinning_validated)
            
        except Exception as e:
            validation_result["valid"] = False
            validation_result["errors"].append(f"Certificate validation error: {e}")
            logger.error("Certificate validation exception", 
                        url=websocket_url, error=str(e))
        
        return validation_result
    
    def validate_message_security(self, message: Dict[str, Any], connection_info: Dict[str, Any]) -> bool:
        """Validate message security before processing."""
        try:
            # 1. Check message size
            message_size = len(str(message))
            if message_size > self.max_message_size:
                logger.warning("Message too large", size=message_size, max_size=self.max_message_size)
                return False
            
            # 2. Check for required fields
            if "type" not in message:
                logger.warning("Message missing required 'type' field")
                return False
            
            # 3. Validate message timestamp for replay prevention
            if "timestamp" in message:
                message_time = message["timestamp"]
                current_time = time.time()
                # Allow 60 second window for clock skew
                if abs(current_time - message_time) > 60:
                    logger.warning("Message timestamp too old or in future", 
                                 message_time=message_time, 
                                 current_time=current_time)
                    return False
            
            return True
            
        except Exception as e:
            logger.error("Error validating message security", error=str(e))
            return False


class ConnectionTokenManager:
    """Manage connection-specific tokens for hijacking prevention."""
    
    def __init__(self):
        self.connection_tokens: Dict[str, Dict[str, Any]] = {}
    
    def generate_connection_token(self, connection_id: str, security_info: Dict[str, Any]) -> str:
        """Generate a connection-specific token."""
        import secrets
        import hashlib
        
        # Create token based on connection security info
        token_data = f"{connection_id}:{security_info.get('origin', '')}:{security_info.get('user_agent', '')}:{time.time()}"
        connection_token = hashlib.sha256(token_data.encode()).hexdigest()
        
        # Store token with security info
        self.connection_tokens[connection_id] = {
            "token": connection_token,
            "created_at": time.time(),
            "security_info": security_info
        }
        
        return connection_token
    
    def validate_connection_token(self, connection_id: str, token: str, current_security_info: Dict[str, Any]) -> bool:
        """Validate connection token and detect hijacking attempts."""
        if connection_id not in self.connection_tokens:
            logger.warning("Connection token not found", connection_id=connection_id)
            return False
        
        stored_info = self.connection_tokens[connection_id]
        
        # Check token match
        if stored_info["token"] != token:
            logger.warning("Connection token mismatch", connection_id=connection_id)
            return False
        
        # Check for security info changes (potential hijacking)
        stored_security = stored_info["security_info"]
        
        # Check critical security indicators
        if (stored_security.get("origin") != current_security_info.get("origin") or
            stored_security.get("user_agent") != current_security_info.get("user_agent")):
            
            logger.warning("Potential connection hijacking detected",
                         connection_id=connection_id,
                         stored_origin=stored_security.get("origin"),
                         current_origin=current_security_info.get("origin"),
                         stored_user_agent=stored_security.get("user_agent"),
                         current_user_agent=current_security_info.get("user_agent"))
            return False
        
        return True
    
    def cleanup_expired_tokens(self, max_age_seconds: int = 3600):
        """Clean up expired connection tokens."""
        current_time = time.time()
        expired_connections = []
        
        for connection_id, token_info in self.connection_tokens.items():
            if current_time - token_info["created_at"] > max_age_seconds:
                expired_connections.append(connection_id)
        
        for connection_id in expired_connections:
            del self.connection_tokens[connection_id]
            
        if expired_connections:
            logger.info("Cleaned up expired connection tokens", count=len(expired_connections))


class MessageReplayDetector:
    """Detect and prevent message replay attacks."""
    
    def __init__(self, window_size: int = 300):  # 5 minute window
        self.message_hashes: Dict[str, float] = {}  # hash -> timestamp
        self.window_size = window_size
    
    def is_replay(self, message: Dict[str, Any]) -> bool:
        """Check if message is a replay of a recent message."""
        try:
            import hashlib
            import json
            
            # Create message hash (excluding timestamp to allow for slight variations)
            message_copy = message.copy()
            message_copy.pop("timestamp", None)
            message_hash = hashlib.sha256(json.dumps(message_copy, sort_keys=True).encode()).hexdigest()
            
            current_time = time.time()
            
            # Clean up old hashes
            self._cleanup_old_hashes(current_time)
            
            # Check if we've seen this message recently
            if message_hash in self.message_hashes:
                last_seen = self.message_hashes[message_hash]
                if current_time - last_seen < self.window_size:
                    logger.warning("Replay attack detected", 
                                 message_hash=message_hash[:16], 
                                 last_seen=last_seen,
                                 current_time=current_time)
                    return True
            
            # Store message hash
            self.message_hashes[message_hash] = current_time
            return False
            
        except Exception as e:
            logger.error("Error checking message replay", error=str(e))
            # Err on the side of caution - allow message but log error
            return False
    
    def _cleanup_old_hashes(self, current_time: float):
        """Remove old message hashes outside the window."""
        expired_hashes = []
        for msg_hash, timestamp in self.message_hashes.items():
            if current_time - timestamp > self.window_size:
                expired_hashes.append(msg_hash)
        
        for msg_hash in expired_hashes:
            del self.message_hashes[msg_hash]


# Global instances
_security_validator = None
_connection_token_manager = None
_replay_detector = None


def get_security_validator() -> WebSocketSecurityValidator:
    """Get global security validator instance."""
    global _security_validator
    if _security_validator is None:
        _security_validator = WebSocketSecurityValidator()
    return _security_validator


def get_connection_token_manager() -> ConnectionTokenManager:
    """Get global connection token manager instance."""
    global _connection_token_manager
    if _connection_token_manager is None:
        _connection_token_manager = ConnectionTokenManager()
    return _connection_token_manager


def get_replay_detector() -> MessageReplayDetector:
    """Get global message replay detector instance."""
    global _replay_detector
    if _replay_detector is None:
        _replay_detector = MessageReplayDetector()
    return _replay_detector