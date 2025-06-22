"""
Input Sanitization and Validation Module

Provides comprehensive input sanitization and validation to prevent injection attacks,
privilege escalation, and other security vulnerabilities.
"""
import re
import hashlib
from typing import Any, Dict, List, Optional
import structlog

logger = structlog.get_logger()


class SecurityException(Exception):
    """Exception raised when security validation fails"""
    pass


class InputSanitizer:
    """Comprehensive input sanitization and validation"""
    
    def __init__(self):
        # Dangerous patterns that should be blocked
        self.sql_injection_patterns = [
            r"['\";\-\-]",  # SQL injection characters
            r"\b(DROP|DELETE|INSERT|UPDATE|CREATE|ALTER|EXEC|UNION|SELECT)\b",  # SQL keywords
            r"\b(OR|AND)\s+['\"]?\w+['\"]?\s*=\s*['\"]?\w+['\"]?",  # SQL boolean injection
            r"['\"];?\s*(DROP|DELETE|INSERT|UPDATE)",  # Terminator + SQL
        ]
        
        self.xss_patterns = [
            r"<script[^>]*>.*?</script>",  # Script tags
            r"javascript:",  # JavaScript protocol
            r"on\w+\s*=",  # Event handlers
            r"<iframe[^>]*>.*?</iframe>",  # Iframes
        ]
        
        self.command_injection_patterns = [
            r"[;&|`$]",  # Command separators and substitution
            r"\$\([^)]*\)",  # Command substitution
            r"`[^`]*`",  # Backtick execution
            r"\|\s*(rm|del|format|dd|cat|nc|wget|curl)",  # Dangerous commands
        ]
        
        self.path_traversal_patterns = [
            r"\.\./",  # Directory traversal
            r"\\\.\\.",  # Windows directory traversal
            r"/etc/passwd",  # Unix system files
            r"\\windows\\system32",  # Windows system files
            r"\x00",  # Null byte injection
        ]
        
        # Compiled patterns for performance
        self.compiled_patterns = {
            'sql': [re.compile(pattern, re.IGNORECASE) for pattern in self.sql_injection_patterns],
            'xss': [re.compile(pattern, re.IGNORECASE) for pattern in self.xss_patterns],
            'cmd': [re.compile(pattern, re.IGNORECASE) for pattern in self.command_injection_patterns],
            'path': [re.compile(pattern, re.IGNORECASE) for pattern in self.path_traversal_patterns]
        }
    
    def sanitize_session_id(self, session_id: str) -> str:
        """Sanitize and validate session ID"""
        if not session_id:
            raise SecurityException("Session ID cannot be empty")
        
        # Check for injection attacks
        self._check_for_attacks(session_id, "session_id")
        
        # Session IDs should be alphanumeric with limited special chars
        if not re.match(r'^[a-zA-Z0-9_\-]{1,128}$', session_id):
            raise SecurityException(f"Invalid session ID format: {session_id}")
        
        # Additional length check
        if len(session_id) > 128:
            raise SecurityException("Session ID too long")
        
        return session_id
    
    def sanitize_user_id(self, user_id: str) -> str:
        """Sanitize and validate user ID"""
        if not user_id:
            raise SecurityException("User ID cannot be empty")
        
        # Check for injection attacks
        self._check_for_attacks(user_id, "user_id")
        
        # User IDs should be alphanumeric with limited special chars
        if not re.match(r'^[a-zA-Z0-9_\-@\.]{1,64}$', user_id):
            raise SecurityException(f"Invalid user ID format: {user_id}")
        
        return user_id
    
    def sanitize_buffer_data(self, data: bytes, max_size: int = 1024 * 1024) -> bytes:
        """Sanitize buffer data to prevent attacks"""
        if not isinstance(data, bytes):
            raise SecurityException("Buffer data must be bytes")
        
        # Size check
        if len(data) > max_size:
            logger.warning("Buffer data truncated", 
                          original_size=len(data), max_size=max_size)
            data = data[:max_size]
        
        # Check for dangerous binary patterns
        if b'\x00' in data and len(data) < 100:  # Small data with null bytes is suspicious
            raise SecurityException("Suspicious null bytes in buffer data")
        
        # Check for executable patterns (simplified)
        dangerous_patterns = [
            b'\x7fELF',  # ELF executable
            b'MZ',       # Windows executable
            b'\x89PNG',  # Could be steganography
        ]
        
        for pattern in dangerous_patterns:
            if data.startswith(pattern):
                logger.warning("Dangerous binary pattern detected in buffer data")
                # Don't block entirely, but log for monitoring
        
        return data
    
    def _check_for_attacks(self, input_str: str, field_name: str):
        """Check input string for various attack patterns"""
        attack_types = []
        
        # Check each attack type
        for attack_type, patterns in self.compiled_patterns.items():
            for pattern in patterns:
                if pattern.search(input_str):
                    attack_types.append(attack_type)
                    break
        
        if attack_types:
            logger.error("Attack patterns detected",
                        field=field_name,
                        input=input_str[:100],  # Log first 100 chars
                        attack_types=attack_types)
            raise SecurityException(f"Security violation: {', '.join(attack_types)} attack detected in {field_name}")
    
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
        
        # Check for injection in User-Agent
        self._check_for_attacks(user_agent, "user_agent")
        
        # Length check and truncation
        if len(user_agent) > 512:
            logger.warning("User-Agent truncated", original_length=len(user_agent))
            user_agent = user_agent[:512]
        
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