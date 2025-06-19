"""Custom exception classes for the Build Platform API."""

from typing import Any, Dict, Optional
from fastapi import HTTPException, status


class BuildPlatformException(Exception):
    """Base exception for Build Platform errors."""
    
    def __init__(
        self,
        message: str,
        error_code: str = "UNKNOWN_ERROR",
        details: Optional[Dict[str, Any]] = None
    ):
        self.message = message
        self.error_code = error_code
        self.details = details or {}
        super().__init__(self.message)


class ValidationError(BuildPlatformException):
    """Validation error exception."""
    
    def __init__(self, message: str, field: Optional[str] = None, **kwargs):
        super().__init__(message, error_code="VALIDATION_ERROR", **kwargs)
        self.field = field


class AuthenticationError(BuildPlatformException):
    """Authentication error exception."""
    
    def __init__(self, message: str = "Authentication failed", **kwargs):
        super().__init__(message, error_code="AUTHENTICATION_ERROR", **kwargs)


class AuthorizationError(BuildPlatformException):
    """Authorization error exception."""
    
    def __init__(self, message: str = "Access denied", **kwargs):
        super().__init__(message, error_code="AUTHORIZATION_ERROR", **kwargs)


class ResourceNotFoundError(BuildPlatformException):
    """Resource not found exception."""
    
    def __init__(self, resource_type: str, resource_id: str, **kwargs):
        message = f"{resource_type} with ID {resource_id} not found"
        super().__init__(message, error_code="RESOURCE_NOT_FOUND", **kwargs)
        self.resource_type = resource_type
        self.resource_id = resource_id


class ResourceConflictError(BuildPlatformException):
    """Resource conflict exception."""
    
    def __init__(self, message: str, resource_type: Optional[str] = None, **kwargs):
        super().__init__(message, error_code="RESOURCE_CONFLICT", **kwargs)
        self.resource_type = resource_type


class RateLimitExceededError(BuildPlatformException):
    """Rate limit exceeded exception."""
    
    def __init__(
        self, 
        message: str = "Rate limit exceeded",
        retry_after: Optional[int] = None,
        **kwargs
    ):
        super().__init__(message, error_code="RATE_LIMIT_EXCEEDED", **kwargs)
        self.retry_after = retry_after


class ServiceUnavailableError(BuildPlatformException):
    """Service unavailable exception."""
    
    def __init__(self, service_name: str, **kwargs):
        message = f"Service {service_name} is currently unavailable"
        super().__init__(message, error_code="SERVICE_UNAVAILABLE", **kwargs)
        self.service_name = service_name


class VMError(BuildPlatformException):
    """VM-related error exception."""
    
    def __init__(self, message: str, vm_id: Optional[str] = None, **kwargs):
        super().__init__(message, error_code="VM_ERROR", **kwargs)
        self.vm_id = vm_id


class QuotaExceededError(BuildPlatformException):
    """Quota exceeded exception."""
    
    def __init__(self, quota_type: str, current: int, limit: int, **kwargs):
        message = f"{quota_type} quota exceeded: {current}/{limit}"
        super().__init__(message, error_code="QUOTA_EXCEEDED", **kwargs)
        self.quota_type = quota_type
        self.current = current
        self.limit = limit


# HTTP Exception converters

def to_http_exception(exc: BuildPlatformException) -> HTTPException:
    """Convert BuildPlatformException to HTTPException."""
    
    status_code_map = {
        "VALIDATION_ERROR": status.HTTP_422_UNPROCESSABLE_ENTITY,
        "AUTHENTICATION_ERROR": status.HTTP_401_UNAUTHORIZED,
        "AUTHORIZATION_ERROR": status.HTTP_403_FORBIDDEN,
        "RESOURCE_NOT_FOUND": status.HTTP_404_NOT_FOUND,
        "RESOURCE_CONFLICT": status.HTTP_409_CONFLICT,
        "RATE_LIMIT_EXCEEDED": status.HTTP_429_TOO_MANY_REQUESTS,
        "SERVICE_UNAVAILABLE": status.HTTP_503_SERVICE_UNAVAILABLE,
        "QUOTA_EXCEEDED": status.HTTP_429_TOO_MANY_REQUESTS,
        "VM_ERROR": status.HTTP_400_BAD_REQUEST,
        "UNKNOWN_ERROR": status.HTTP_500_INTERNAL_SERVER_ERROR,
    }
    
    status_code = status_code_map.get(exc.error_code, status.HTTP_500_INTERNAL_SERVER_ERROR)
    
    detail = {
        "message": exc.message,
        "error_code": exc.error_code,
        "details": exc.details
    }
    
    headers = {}
    if isinstance(exc, RateLimitExceededError) and exc.retry_after:
        headers["Retry-After"] = str(exc.retry_after)
    
    return HTTPException(
        status_code=status_code,
        detail=detail,
        headers=headers if headers else None
    )


class ErrorResponse:
    """Standard error response format."""
    
    @staticmethod
    def format_error(
        message: str,
        error_code: str = "UNKNOWN_ERROR",
        details: Optional[Dict[str, Any]] = None,
        path: Optional[str] = None,
        timestamp: Optional[str] = None
    ) -> Dict[str, Any]:
        """Format error response."""
        from datetime import datetime
        
        return {
            "error": {
                "message": message,
                "code": error_code,
                "details": details or {},
                "path": path,
                "timestamp": timestamp or datetime.utcnow().isoformat()
            }
        }
    
    @staticmethod
    def validation_error(
        message: str,
        field_errors: Optional[Dict[str, str]] = None,
        path: Optional[str] = None
    ) -> Dict[str, Any]:
        """Format validation error response."""
        return ErrorResponse.format_error(
            message=message,
            error_code="VALIDATION_ERROR",
            details={"field_errors": field_errors or {}},
            path=path
        )
    
    @staticmethod
    def authentication_error(
        message: str = "Authentication required",
        path: Optional[str] = None
    ) -> Dict[str, Any]:
        """Format authentication error response."""
        return ErrorResponse.format_error(
            message=message,
            error_code="AUTHENTICATION_ERROR",
            path=path
        )
    
    @staticmethod
    def authorization_error(
        message: str = "Access denied",
        required_permissions: Optional[List[str]] = None,
        path: Optional[str] = None
    ) -> Dict[str, Any]:
        """Format authorization error response."""
        return ErrorResponse.format_error(
            message=message,
            error_code="AUTHORIZATION_ERROR",
            details={"required_permissions": required_permissions or []},
            path=path
        )
    
    @staticmethod
    def rate_limit_error(
        message: str = "Rate limit exceeded",
        limit: Optional[int] = None,
        window: Optional[int] = None,
        retry_after: Optional[int] = None,
        path: Optional[str] = None
    ) -> Dict[str, Any]:
        """Format rate limit error response."""
        return ErrorResponse.format_error(
            message=message,
            error_code="RATE_LIMIT_EXCEEDED",
            details={
                "limit": limit,
                "window_seconds": window,
                "retry_after_seconds": retry_after
            },
            path=path
        )