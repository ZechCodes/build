"""
Input Sanitization and Validation Module

Provides format validation and sanitization for session management.
Focuses on real security threats: format validation, length limits, and proper data types.
"""
import re
from typing import Any, Dict, List, Optional
import structlog

logger = structlog.get_logger()


class SecurityException(Exception):
    """Exception raised when security validation fails"""
    pass


class InputSanitizer:
    """Input format validation and sanitization for Redis-based session management"""
    
    def __init__(self):
        # Patterns for data that should never appear in session/buffer data
        self.dangerous_binary_patterns = [
            b'\x7fELF',  # ELF executable
            b'MZ',       # Windows executable
        ]
    
    def sanitize_session_id(self, session_id: str) -> str:
        """Validate session ID format"""
        if not session_id:
            raise SecurityException("Session ID cannot be empty")
        
        # Session IDs follow format: sess_uuid_timestamp
        # Allow alphanumeric, underscores, hyphens (for UUIDs)
        if not re.match(r'^sess_[a-f0-9\-]+_\d+$', session_id):
            raise SecurityException(f"Invalid session ID format: {session_id}")
        
        # Length check
        if len(session_id) > 128:
            raise SecurityException("Session ID too long")
        
        return session_id
    
    def sanitize_user_id(self, user_id: str) -> str:
        """Validate user ID format"""
        if not user_id:
            raise SecurityException("User ID cannot be empty")
        
        # User IDs from authentication: alphanumeric, underscore, dash, dot, @ symbol
        if not re.match(r'^[a-zA-Z0-9_\-@\.]{1,64}$', user_id):
            raise SecurityException(f"Invalid user ID format: {user_id}")
        
        # Length check
        if len(user_id) > 64:
            raise SecurityException("User ID too long")
        
        return user_id
    
    def sanitize_buffer_data(self, data: bytes, max_size: int = 1024 * 1024) -> bytes:
        """Validate and sanitize buffer data"""
        if not isinstance(data, bytes):
            raise SecurityException("Buffer data must be bytes")
        
        # Size limit enforcement
        if len(data) > max_size:
            logger.warning("Buffer data truncated", 
                          original_size=len(data), max_size=max_size)
            data = data[:max_size]
        
        # Check for suspicious null bytes in small payloads
        if b'\x00' in data and len(data) < 100:
            raise SecurityException("Suspicious null bytes in buffer data")
        
        # Check for executable patterns
        for pattern in self.dangerous_binary_patterns:
            if data.startswith(pattern):
                logger.warning("Dangerous binary pattern detected in buffer data")
                # Log but don't block - could be legitimate terminal output
        
        return data
    
    def validate_request_context(self, user_id: str, session_id: str, 
                                client_ip: str = None, user_agent: str = None) -> Dict[str, str]:
        """Validate and sanitize complete request context"""
        try:
            sanitized = {
                'user_id': self.sanitize_user_id(user_id),
                'session_id': self.sanitize_session_id(session_id)
            }
            
            # Sanitize optional fields
            if client_ip:
                sanitized['client_ip'] = self._sanitize_ip_address(client_ip)
            
            if user_agent:
                sanitized['user_agent'] = self._sanitize_user_agent(user_agent)
            
            return sanitized
            
        except SecurityException as e:
            logger.error("Request validation failed",
                        user_id=user_id[:20] if user_id else None,
                        session_id=session_id[:20] if session_id else None,
                        error=str(e))
            raise
    
    def _sanitize_ip_address(self, ip: str) -> str:
        """Sanitize IP address"""
        if not ip or ip == "unknown":
            return ip
        
        # Basic IP format validation
        if not re.match(r'^[\d\.:a-fA-F]+$', ip):
            raise SecurityException("Invalid IP address format")
        
        # Length check
        if len(ip) > 45:  # IPv6 max length
            raise SecurityException("IP address too long")
        
        return ip
    
    def _sanitize_user_agent(self, user_agent: str) -> str:
        """Sanitize User-Agent string"""
        if not user_agent or user_agent == "unknown":
            return user_agent
        
        # Length check and truncation
        if len(user_agent) > 512:
            logger.warning("User-Agent truncated", original_length=len(user_agent))
            user_agent = user_agent[:512]
        
        # Basic format check - should contain printable ASCII
        if not all(32 <= ord(c) <= 126 for c in user_agent):
            raise SecurityException("User-Agent contains non-printable characters")
        
        return user_agent


class PrivilegeValidator:
    """Validates user privileges and prevents escalation"""
    
    def __init__(self):
        self.admin_users = set()  # Should be loaded from configuration
        self.user_sessions = {}   # Track user -> session mappings
    
    def validate_session_access(self, user_id: str, session_id: str, 
                               session_owner_id: str) -> bool:
        """Validate that user can access the specified session"""
        
        # Exact user match required
        if user_id != session_owner_id:
            logger.warning("Cross-user session access attempt blocked",
                          requesting_user=user_id,
                          session_owner=session_owner_id,
                          session_id=session_id)
            return False
        
        # Additional checks could be added here:
        # - Role-based access control
        # - Session sharing permissions
        # - Temporary access grants
        
        return True
    
    def validate_admin_access(self, user_id: str, requested_resource: str) -> bool:
        """Validate admin-level access attempts"""
        
        # Check if user is authorized admin
        if user_id not in self.admin_users:
            logger.warning("Unauthorized admin access attempt",
                          user_id=user_id,
                          resource=requested_resource)
            return False
        
        # Log admin access for auditing
        logger.info("Admin access granted",
                   user_id=user_id,
                   resource=requested_resource)
        
        return True
    
    def check_privilege_escalation(self, user_id: str, target_user_id: str) -> bool:
        """Check for privilege escalation attempts"""
        
        # Users cannot access other users' data
        if user_id != target_user_id:
            # Exception: admins can access any user's data
            if user_id in self.admin_users:
                logger.info("Admin accessing user data",
                           admin_user=user_id,
                           target_user=target_user_id)
                return True
            else:
                logger.error("Privilege escalation attempt detected",
                            user_id=user_id,
                            target_user=target_user_id)
                return False
        
        return True


# Global instances
input_sanitizer = InputSanitizer()
privilege_validator = PrivilegeValidator()