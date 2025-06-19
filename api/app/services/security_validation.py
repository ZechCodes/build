"""Security validation service for comprehensive security checks."""

import re
import ipaddress
import structlog
from typing import Any, Dict, List, Optional, Union
from datetime import datetime, timedelta
from sqlalchemy.ext.asyncio import AsyncSession

from ..core.config import get_settings
from .audit import AuditService


logger = structlog.get_logger(__name__)
settings = get_settings()


class SecurityValidationService:
    """Service for comprehensive security validation and monitoring."""
    
    def __init__(self, db_session: AsyncSession):
        self.db = db_session
        self.audit = AuditService(db_session)
    
    async def validate_input_data(
        self,
        data: Dict[str, Any],
        validation_rules: Dict[str, Dict[str, Any]]
    ) -> tuple[bool, List[str]]:
        """Validate input data against security rules."""
        errors = []
        
        for field, rules in validation_rules.items():
            if field not in data:
                if rules.get("required", False):
                    errors.append(f"Field '{field}' is required")
                continue
            
            value = data[field]
            
            # Type validation
            expected_type = rules.get("type")
            if expected_type and not isinstance(value, expected_type):
                errors.append(f"Field '{field}' must be of type {expected_type.__name__}")
                continue
            
            # Length validation for strings
            if isinstance(value, str):
                min_length = rules.get("min_length")
                max_length = rules.get("max_length")
                
                if min_length and len(value) < min_length:
                    errors.append(f"Field '{field}' must be at least {min_length} characters")
                
                if max_length and len(value) > max_length:
                    errors.append(f"Field '{field}' must be at most {max_length} characters")
                
                # Pattern validation
                pattern = rules.get("pattern")
                if pattern and not re.match(pattern, value):
                    errors.append(f"Field '{field}' format is invalid")
                
                # SQL injection detection
                if self._detect_sql_injection(value):
                    errors.append(f"Field '{field}' contains suspicious content")
                    await self.audit.log_security_event(
                        event_type="sql_injection_attempt",
                        severity="high",
                        details={"field": field, "value": value[:100]}
                    )
                
                # XSS detection
                if self._detect_xss_attempt(value):
                    errors.append(f"Field '{field}' contains suspicious script content")
                    await self.audit.log_security_event(
                        event_type="xss_attempt",
                        severity="high",
                        details={"field": field, "value": value[:100]}
                    )
            
            # Numeric validation
            if isinstance(value, (int, float)):
                min_value = rules.get("min_value")
                max_value = rules.get("max_value")
                
                if min_value is not None and value < min_value:
                    errors.append(f"Field '{field}' must be at least {min_value}")
                
                if max_value is not None and value > max_value:
                    errors.append(f"Field '{field}' must be at most {max_value}")
        
        return len(errors) == 0, errors
    
    async def validate_password_strength(self, password: str) -> tuple[bool, List[str]]:
        """Validate password against security requirements."""
        errors = []
        
        if len(password) < 8:
            errors.append("Password must be at least 8 characters long")
        
        if len(password) > 128:
            errors.append("Password must be at most 128 characters long")
        
        if not re.search(r"[a-z]", password):
            errors.append("Password must contain at least one lowercase letter")
        
        if not re.search(r"[A-Z]", password):
            errors.append("Password must contain at least one uppercase letter")
        
        if not re.search(r"\d", password):
            errors.append("Password must contain at least one digit")
        
        if not re.search(r"[!@#$%^&*(),.?\":{}|<>]", password):
            errors.append("Password must contain at least one special character")
        
        # Check for common passwords
        if self._is_common_password(password):
            errors.append("Password is too common, please choose a stronger password")
        
        return len(errors) == 0, errors
    
    async def check_ip_reputation(self, ip_address: str) -> Dict[str, Any]:
        """Check IP address reputation and behavior."""
        reputation = {
            "is_valid": self._is_valid_ip(ip_address),
            "is_private": self._is_private_ip(ip_address),
            "is_suspicious": False,
            "risk_score": 0,
            "reasons": []
        }
        
        # Check for recent failed attempts
        recent_failures = await self.audit.get_failed_login_attempts(
            ip_address=ip_address,
            hours_back=1
        )
        
        if len(recent_failures) > 5:
            reputation["is_suspicious"] = True
            reputation["risk_score"] += 30
            reputation["reasons"].append("Multiple failed login attempts")
        
        # Check for rate limiting violations
        security_events = await self.audit.get_security_events(
            severity="high",
            hours_back=24
        )
        
        ip_violations = [
            event for event in security_events
            if event.ip_address == ip_address
        ]
        
        if len(ip_violations) > 3:
            reputation["is_suspicious"] = True
            reputation["risk_score"] += 25
            reputation["reasons"].append("Multiple security violations")
        
        return reputation
    
    async def validate_file_upload(
        self,
        filename: str,
        content_type: str,
        file_size: int,
        allowed_types: List[str],
        max_size: int = 10 * 1024 * 1024  # 10MB default
    ) -> tuple[bool, List[str]]:
        """Validate file uploads for security."""
        errors = []
        
        # File size validation
        if file_size > max_size:
            errors.append(f"File size exceeds limit of {max_size} bytes")
        
        # Content type validation
        if content_type not in allowed_types:
            errors.append(f"File type '{content_type}' is not allowed")
        
        # Filename validation
        if not self._is_safe_filename(filename):
            errors.append("Filename contains unsafe characters")
        
        # Extension validation
        file_extension = filename.lower().split(".")[-1] if "." in filename else ""
        dangerous_extensions = [
            "exe", "bat", "cmd", "com", "pif", "scr", "vbs", "js", "jar",
            "php", "py", "pl", "sh", "ps1", "asp", "aspx", "jsp"
        ]
        
        if file_extension in dangerous_extensions:
            errors.append(f"File extension '.{file_extension}' is not allowed")
        
        return len(errors) == 0, errors
    
    async def check_session_security(
        self,
        session_id: str,
        user_id: str,
        ip_address: str,
        user_agent: str
    ) -> Dict[str, Any]:
        """Check session security for anomalies."""
        security_check = {
            "is_valid": True,
            "warnings": [],
            "risk_score": 0
        }
        
        # Check for IP address changes (basic session hijacking detection)
        # This would be implemented with session storage in Redis
        
        # Check for user agent changes
        # This would be implemented with session storage in Redis
        
        # Check session age
        # This would be implemented when we have session management
        
        return security_check
    
    def _detect_sql_injection(self, value: str) -> bool:
        """Detect potential SQL injection attempts."""
        sql_injection_patterns = [
            r"(\b(union|select|insert|update|delete|drop|create|alter|exec|execute)\b)",
            r"(--|\#|\/\*|\*\/)",
            r"(\b(or|and)\b\s+[\d\w]+\s*=\s*[\d\w]+)",
            r"('|\");\s*\w+",
            r"\b\d+\s*=\s*\d+\b",
            r"(\|\||&&)"
        ]
        
        value_lower = value.lower()
        for pattern in sql_injection_patterns:
            if re.search(pattern, value_lower, re.IGNORECASE):
                return True
        
        return False
    
    def _detect_xss_attempt(self, value: str) -> bool:
        """Detect potential XSS attempts."""
        xss_patterns = [
            r"<script\b[^<]*(?:(?!<\/script>)<[^<]*)*<\/script>",
            r"javascript:",
            r"on\w+\s*=",
            r"<iframe\b",
            r"<object\b",
            r"<embed\b",
            r"<link\b",
            r"<meta\b.*http-equiv",
            r"vbscript:",
            r"data:text\/html"
        ]
        
        for pattern in xss_patterns:
            if re.search(pattern, value, re.IGNORECASE):
                return True
        
        return False
    
    def _is_common_password(self, password: str) -> bool:
        """Check against common password list."""
        common_passwords = [
            "password", "123456", "password123", "admin", "qwerty",
            "letmein", "welcome", "monkey", "1234567890", "abc123",
            "password1", "123456789", "welcome123", "admin123"
        ]
        
        return password.lower() in common_passwords
    
    def _is_valid_ip(self, ip_address: str) -> bool:
        """Validate IP address format."""
        try:
            ipaddress.ip_address(ip_address)
            return True
        except ValueError:
            return False
    
    def _is_private_ip(self, ip_address: str) -> bool:
        """Check if IP is in private range."""
        try:
            ip = ipaddress.ip_address(ip_address)
            return ip.is_private
        except ValueError:
            return False
    
    def _is_safe_filename(self, filename: str) -> bool:
        """Check if filename is safe."""
        # Check for directory traversal attempts
        if ".." in filename or "/" in filename or "\\" in filename:
            return False
        
        # Check for null bytes
        if "\x00" in filename:
            return False
        
        # Check for control characters
        if any(ord(char) < 32 for char in filename):
            return False
        
        # Check for reserved names on Windows
        reserved_names = [
            "CON", "PRN", "AUX", "NUL",
            "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
            "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9"
        ]
        
        filename_upper = filename.upper().split(".")[0]
        if filename_upper in reserved_names:
            return False
        
        return True


# Validation rule constants
USER_VALIDATION_RULES = {
    "email": {
        "required": True,
        "type": str,
        "max_length": 255,
        "pattern": r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
    },
    "username": {
        "required": True,
        "type": str,
        "min_length": 3,
        "max_length": 50,
        "pattern": r"^[a-zA-Z0-9_-]+$"
    },
    "password": {
        "required": True,
        "type": str,
        "min_length": 8,
        "max_length": 128
    },
    "full_name": {
        "required": False,
        "type": str,
        "max_length": 100
    }
}

VM_VALIDATION_RULES = {
    "name": {
        "required": True,
        "type": str,
        "min_length": 1,
        "max_length": 100,
        "pattern": r"^[a-zA-Z0-9_-]+$"
    },
    "cpu_count": {
        "required": False,
        "type": int,
        "min_value": 1,
        "max_value": 8
    },
    "memory_mb": {
        "required": False,
        "type": int,
        "min_value": 256,
        "max_value": 8192
    },
    "disk_gb": {
        "required": False,
        "type": int,
        "min_value": 1,
        "max_value": 100
    }
}