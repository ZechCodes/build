"""
Standardized Error Handling Framework

Provides consistent error handling across all sessions with:
- Structured error responses
- Error code standardization
- Client-side error recovery strategies
- Automatic retry mechanisms
- Circuit breaker pattern
- Error logging with correlation IDs
"""

import asyncio
import functools
import time
import traceback
from datetime import datetime, timedelta
from enum import Enum
from typing import Any, Dict, List, Optional, Callable, Type, Union
from uuid import uuid4

from pydantic import BaseModel, Field
import structlog


class ErrorCategory(str, Enum):
    """High-level error categories."""
    AUTHENTICATION = "authentication"
    AUTHORIZATION = "authorization"
    VALIDATION = "validation"
    RESOURCE_NOT_FOUND = "resource_not_found"
    RATE_LIMIT = "rate_limit"
    QUOTA_EXCEEDED = "quota_exceeded"
    EXTERNAL_SERVICE = "external_service"
    INTERNAL_SERVER = "internal_server"
    NETWORK = "network"
    TIMEOUT = "timeout"
    CONFLICT = "conflict"
    VM_MANAGEMENT = "vm_management"
    SESSION_MANAGEMENT = "session_management"
    STORAGE = "storage"
    WEBSOCKET = "websocket"


class ErrorSeverity(str, Enum):
    """Error severity levels."""
    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"
    CRITICAL = "critical"


class RetryStrategy(str, Enum):
    """Retry strategies for error recovery."""
    NONE = "none"
    IMMEDIATE = "immediate"
    EXPONENTIAL_BACKOFF = "exponential_backoff"
    LINEAR_BACKOFF = "linear_backoff"
    CUSTOM = "custom"


class ErrorCode(str, Enum):
    """Standardized error codes across all sessions."""
    
    # Authentication & Authorization (Session 2)
    AUTH_TOKEN_INVALID = "AUTH_TOKEN_INVALID"
    AUTH_TOKEN_EXPIRED = "AUTH_TOKEN_EXPIRED"
    AUTH_REFRESH_FAILED = "AUTH_REFRESH_FAILED"
    AUTH_INSUFFICIENT_PERMISSIONS = "AUTH_INSUFFICIENT_PERMISSIONS"
    AUTH_USER_NOT_FOUND = "AUTH_USER_NOT_FOUND"
    
    # VM Management (Session 3)
    VM_NOT_FOUND = "VM_NOT_FOUND"
    VM_CREATION_FAILED = "VM_CREATION_FAILED"
    VM_START_FAILED = "VM_START_FAILED"
    VM_STOP_FAILED = "VM_STOP_FAILED"
    VM_DELETE_FAILED = "VM_DELETE_FAILED"
    VM_RESOURCE_EXHAUSTED = "VM_RESOURCE_EXHAUSTED"
    VM_QUOTA_EXCEEDED = "VM_QUOTA_EXCEEDED"
    VM_INVALID_CONFIG = "VM_INVALID_CONFIG"
    
    # PTY Layer (Session 4)
    PTY_CONNECTION_FAILED = "PTY_CONNECTION_FAILED"
    PTY_COMMAND_FAILED = "PTY_COMMAND_FAILED"
    PTY_SESSION_TERMINATED = "PTY_SESSION_TERMINATED"
    PTY_PERMISSION_DENIED = "PTY_PERMISSION_DENIED"
    
    # WebSocket Layer (Session 5)
    WS_CONNECTION_FAILED = "WS_CONNECTION_FAILED"
    WS_AUTHENTICATION_FAILED = "WS_AUTHENTICATION_FAILED"
    WS_MESSAGE_INVALID = "WS_MESSAGE_INVALID"
    WS_RATE_LIMIT_EXCEEDED = "WS_RATE_LIMIT_EXCEEDED"
    WS_CONNECTION_LOST = "WS_CONNECTION_LOST"
    
    # Session Management (Session 6)
    SESSION_NOT_FOUND = "SESSION_NOT_FOUND"
    SESSION_CREATION_FAILED = "SESSION_CREATION_FAILED"
    SESSION_RECOVERY_FAILED = "SESSION_RECOVERY_FAILED"
    SESSION_EXPIRED = "SESSION_EXPIRED"
    SESSION_CONFLICT = "SESSION_CONFLICT"
    
    # Snapshot System (Session 7)
    SNAPSHOT_NOT_FOUND = "SNAPSHOT_NOT_FOUND"
    SNAPSHOT_CREATION_FAILED = "SNAPSHOT_CREATION_FAILED"
    SNAPSHOT_RESTORE_FAILED = "SNAPSHOT_RESTORE_FAILED"
    SNAPSHOT_DELETE_FAILED = "SNAPSHOT_DELETE_FAILED"
    SNAPSHOT_CORRUPTION = "SNAPSHOT_CORRUPTION"
    
    # Frontend Terminal (Session 8)
    TERMINAL_UNAVAILABLE = "TERMINAL_UNAVAILABLE"
    TERMINAL_RESIZE_FAILED = "TERMINAL_RESIZE_FAILED"
    TERMINAL_INPUT_REJECTED = "TERMINAL_INPUT_REJECTED"
    
    # Git Integration (Session 9)
    GIT_REPOSITORY_NOT_FOUND = "GIT_REPOSITORY_NOT_FOUND"
    GIT_OPERATION_FAILED = "GIT_OPERATION_FAILED"
    GIT_AUTHENTICATION_FAILED = "GIT_AUTHENTICATION_FAILED"
    GIT_MERGE_CONFLICT = "GIT_MERGE_CONFLICT"
    
    # Recording System (Session 10)
    RECORDING_NOT_FOUND = "RECORDING_NOT_FOUND"
    RECORDING_START_FAILED = "RECORDING_START_FAILED"
    RECORDING_STOP_FAILED = "RECORDING_STOP_FAILED"
    RECORDING_PLAYBACK_FAILED = "RECORDING_PLAYBACK_FAILED"
    
    # Storage
    STORAGE_UNAVAILABLE = "STORAGE_UNAVAILABLE"
    STORAGE_QUOTA_EXCEEDED = "STORAGE_QUOTA_EXCEEDED"
    STORAGE_UPLOAD_FAILED = "STORAGE_UPLOAD_FAILED"
    STORAGE_DOWNLOAD_FAILED = "STORAGE_DOWNLOAD_FAILED"
    
    # General
    INTERNAL_SERVER_ERROR = "INTERNAL_SERVER_ERROR"
    SERVICE_UNAVAILABLE = "SERVICE_UNAVAILABLE"
    TIMEOUT = "TIMEOUT"
    RATE_LIMIT_EXCEEDED = "RATE_LIMIT_EXCEEDED"
    VALIDATION_ERROR = "VALIDATION_ERROR"
    RESOURCE_NOT_FOUND = "RESOURCE_NOT_FOUND"
    NETWORK_ERROR = "NETWORK_ERROR"


class ErrorResponse(BaseModel):
    """Standardized error response structure."""
    error: bool = True
    code: ErrorCode = Field(..., description="Standardized error code")
    message: str = Field(..., description="Human-readable error message")
    category: ErrorCategory = Field(..., description="Error category")
    severity: ErrorSeverity = Field(..., description="Error severity")
    correlation_id: str = Field(..., description="Unique correlation ID")
    timestamp: datetime = Field(default_factory=datetime.utcnow)
    details: Optional[Dict[str, Any]] = Field(None, description="Additional error details")
    retry_strategy: RetryStrategy = Field(default=RetryStrategy.NONE)
    retry_after: Optional[int] = Field(None, description="Retry after seconds")
    help_url: Optional[str] = Field(None, description="Link to documentation")


class ErrorContext(BaseModel):
    """Context information for error handling."""
    user_id: Optional[str] = None
    session_id: Optional[str] = None
    vm_id: Optional[str] = None
    request_id: Optional[str] = None
    operation: Optional[str] = None
    component: Optional[str] = None
    additional_data: Dict[str, Any] = Field(default_factory=dict)


class BuildPlatformError(Exception):
    """Base exception for Build Platform errors."""
    
    def __init__(
        self,
        code: ErrorCode,
        message: str,
        category: ErrorCategory = ErrorCategory.INTERNAL_SERVER,
        severity: ErrorSeverity = ErrorSeverity.MEDIUM,
        details: Dict[str, Any] = None,
        retry_strategy: RetryStrategy = RetryStrategy.NONE,
        retry_after: int = None,
        context: ErrorContext = None,
        cause: Exception = None
    ):
        self.code = code
        self.message = message
        self.category = category
        self.severity = severity
        self.details = details or {}
        self.retry_strategy = retry_strategy
        self.retry_after = retry_after
        self.context = context or ErrorContext()
        self.cause = cause
        self.correlation_id = str(uuid4())
        self.timestamp = datetime.utcnow()
        
        super().__init__(message)
    
    def to_response(self) -> ErrorResponse:
        """Convert to standardized error response."""
        return ErrorResponse(
            code=self.code,
            message=self.message,
            category=self.category,
            severity=self.severity,
            correlation_id=self.correlation_id,
            timestamp=self.timestamp,
            details=self.details,
            retry_strategy=self.retry_strategy,
            retry_after=self.retry_after
        )


# Specific error classes for different components
class AuthenticationError(BuildPlatformError):
    """Authentication-related errors."""
    def __init__(self, code: ErrorCode, message: str, **kwargs):
        super().__init__(
            code=code,
            message=message,
            category=ErrorCategory.AUTHENTICATION,
            severity=ErrorSeverity.HIGH,
            **kwargs
        )


class VMError(BuildPlatformError):
    """VM management errors."""
    def __init__(self, code: ErrorCode, message: str, **kwargs):
        super().__init__(
            code=code,
            message=message,
            category=ErrorCategory.VM_MANAGEMENT,
            **kwargs
        )


class SessionError(BuildPlatformError):
    """Session management errors."""
    def __init__(self, code: ErrorCode, message: str, **kwargs):
        super().__init__(
            code=code,
            message=message,
            category=ErrorCategory.SESSION_MANAGEMENT,
            **kwargs
        )


class WebSocketError(BuildPlatformError):
    """WebSocket communication errors."""
    def __init__(self, code: ErrorCode, message: str, **kwargs):
        super().__init__(
            code=code,
            message=message,
            category=ErrorCategory.WEBSOCKET,
            **kwargs
        )


class StorageError(BuildPlatformError):
    """Storage operation errors."""
    def __init__(self, code: ErrorCode, message: str, **kwargs):
        super().__init__(
            code=code,
            message=message,
            category=ErrorCategory.STORAGE,
            **kwargs
        )


class CircuitBreakerState(str, Enum):
    """Circuit breaker states."""
    CLOSED = "closed"
    OPEN = "open"
    HALF_OPEN = "half_open"


class CircuitBreaker:
    """Circuit breaker for external service calls."""
    
    def __init__(
        self,
        name: str,
        failure_threshold: int = 5,
        recovery_timeout: int = 60,
        expected_exception: Type[Exception] = Exception
    ):
        self.name = name
        self.failure_threshold = failure_threshold
        self.recovery_timeout = recovery_timeout
        self.expected_exception = expected_exception
        
        self.failure_count = 0
        self.last_failure_time = None
        self.state = CircuitBreakerState.CLOSED
        
        self.logger = structlog.get_logger().bind(circuit_breaker=name)
    
    def __call__(self, func: Callable) -> Callable:
        """Decorator to wrap function with circuit breaker."""
        @functools.wraps(func)
        async def wrapper(*args, **kwargs):
            if self.state == CircuitBreakerState.OPEN:
                if self._should_attempt_reset():
                    self.state = CircuitBreakerState.HALF_OPEN
                else:
                    raise BuildPlatformError(
                        code=ErrorCode.SERVICE_UNAVAILABLE,
                        message=f"Circuit breaker {self.name} is open",
                        category=ErrorCategory.EXTERNAL_SERVICE,
                        retry_strategy=RetryStrategy.EXPONENTIAL_BACKOFF,
                        retry_after=self.recovery_timeout
                    )
            
            try:
                result = await func(*args, **kwargs)
                self._on_success()
                return result
                
            except self.expected_exception as e:
                self._on_failure()
                raise
        
        return wrapper
    
    def _should_attempt_reset(self) -> bool:
        """Check if enough time has passed to attempt reset."""
        if self.last_failure_time is None:
            return True
        
        return (time.time() - self.last_failure_time) >= self.recovery_timeout
    
    def _on_success(self):
        """Handle successful call."""
        self.failure_count = 0
        self.state = CircuitBreakerState.CLOSED
        self.logger.info("Circuit breaker reset", state=self.state)
    
    def _on_failure(self):
        """Handle failed call."""
        self.failure_count += 1
        self.last_failure_time = time.time()
        
        if self.failure_count >= self.failure_threshold:
            self.state = CircuitBreakerState.OPEN
            self.logger.warning(
                "Circuit breaker opened",
                failure_count=self.failure_count,
                state=self.state
            )


class RetryManager:
    """Manages retry logic with different strategies."""
    
    @staticmethod
    async def retry_with_backoff(
        func: Callable,
        max_attempts: int = 3,
        strategy: RetryStrategy = RetryStrategy.EXPONENTIAL_BACKOFF,
        base_delay: float = 1.0,
        max_delay: float = 60.0,
        backoff_multiplier: float = 2.0,
        exceptions: tuple = (Exception,)
    ) -> Any:
        """Retry function with configurable backoff strategy."""
        
        for attempt in range(max_attempts):
            try:
                if asyncio.iscoroutinefunction(func):
                    return await func()
                else:
                    return func()
                    
            except exceptions as e:
                if attempt == max_attempts - 1:
                    # Last attempt failed, re-raise
                    raise
                
                # Calculate delay based on strategy
                if strategy == RetryStrategy.IMMEDIATE:
                    delay = 0
                elif strategy == RetryStrategy.LINEAR_BACKOFF:
                    delay = min(base_delay * (attempt + 1), max_delay)
                elif strategy == RetryStrategy.EXPONENTIAL_BACKOFF:
                    delay = min(base_delay * (backoff_multiplier ** attempt), max_delay)
                else:
                    delay = base_delay
                
                structlog.get_logger().warning(
                    "Retrying operation",
                    attempt=attempt + 1,
                    max_attempts=max_attempts,
                    delay=delay,
                    error=str(e)
                )
                
                if delay > 0:
                    await asyncio.sleep(delay)


class ErrorLogger:
    """Structured error logging with correlation IDs."""
    
    def __init__(self):
        self.logger = structlog.get_logger()
    
    def log_error(
        self,
        error: BuildPlatformError,
        context: ErrorContext = None,
        stack_trace: bool = True
    ):
        """Log error with structured data."""
        log_data = {
            "correlation_id": error.correlation_id,
            "error_code": error.code.value,
            "error_message": error.message,
            "error_category": error.category.value,
            "error_severity": error.severity.value,
            "timestamp": error.timestamp.isoformat()
        }
        
        # Add context information
        if context or error.context:
            ctx = context or error.context
            log_data.update({
                "user_id": ctx.user_id,
                "session_id": ctx.session_id,
                "vm_id": ctx.vm_id,
                "request_id": ctx.request_id,
                "operation": ctx.operation,
                "component": ctx.component
            })
        
        # Add error details
        if error.details:
            log_data["error_details"] = error.details
        
        # Add stack trace if requested
        if stack_trace and error.cause:
            log_data["stack_trace"] = traceback.format_exception(
                type(error.cause),
                error.cause,
                error.cause.__traceback__
            )
        
        # Log at appropriate level based on severity
        if error.severity in [ErrorSeverity.CRITICAL, ErrorSeverity.HIGH]:
            self.logger.error("Application error", **log_data)
        elif error.severity == ErrorSeverity.MEDIUM:
            self.logger.warning("Application warning", **log_data)
        else:
            self.logger.info("Application info", **log_data)


class ErrorHandler:
    """Central error handler with recovery strategies."""
    
    def __init__(self):
        self.error_logger = ErrorLogger()
        self.circuit_breakers: Dict[str, CircuitBreaker] = {}
    
    def get_circuit_breaker(self, name: str, **kwargs) -> CircuitBreaker:
        """Get or create circuit breaker for service."""
        if name not in self.circuit_breakers:
            self.circuit_breakers[name] = CircuitBreaker(name, **kwargs)
        return self.circuit_breakers[name]
    
    async def handle_error(
        self,
        error: Union[Exception, BuildPlatformError],
        context: ErrorContext = None,
        auto_retry: bool = False
    ) -> ErrorResponse:
        """Handle and log error, return standardized response."""
        
        # Convert to BuildPlatformError if needed
        if not isinstance(error, BuildPlatformError):
            build_error = BuildPlatformError(
                code=ErrorCode.INTERNAL_SERVER_ERROR,
                message=str(error),
                category=ErrorCategory.INTERNAL_SERVER,
                severity=ErrorSeverity.HIGH,
                context=context,
                cause=error
            )
        else:
            build_error = error
            if context:
                build_error.context = context
        
        # Log the error
        self.error_logger.log_error(build_error, context)
        
        # Auto-retry if configured
        if auto_retry and build_error.retry_strategy != RetryStrategy.NONE:
            # This would be implemented based on the specific retry strategy
            pass
        
        return build_error.to_response()


# Global error handler instance
error_handler = ErrorHandler()


# Decorator for automatic error handling
def handle_errors(
    context_func: Callable = None,
    auto_retry: bool = False,
    circuit_breaker: str = None
):
    """Decorator for automatic error handling."""
    def decorator(func: Callable) -> Callable:
        @functools.wraps(func)
        async def wrapper(*args, **kwargs):
            try:
                # Apply circuit breaker if specified
                if circuit_breaker:
                    cb = error_handler.get_circuit_breaker(circuit_breaker)
                    func = cb(func)
                
                # Execute function
                if asyncio.iscoroutinefunction(func):
                    return await func(*args, **kwargs)
                else:
                    return func(*args, **kwargs)
                    
            except Exception as e:
                # Get context if function provided
                context = None
                if context_func:
                    try:
                        context = context_func(*args, **kwargs)
                    except Exception:
                        pass
                
                # Handle error
                error_response = await error_handler.handle_error(
                    error=e,
                    context=context,
                    auto_retry=auto_retry
                )
                
                # Re-raise as BuildPlatformError for consistency
                raise BuildPlatformError(
                    code=error_response.code,
                    message=error_response.message,
                    category=error_response.category,
                    severity=error_response.severity,
                    details=error_response.details
                )
        
        return wrapper
    return decorator


# Error recovery utilities
def create_error_context(
    user_id: str = None,
    session_id: str = None,
    vm_id: str = None,
    operation: str = None,
    component: str = None,
    **kwargs
) -> ErrorContext:
    """Helper function to create error context."""
    return ErrorContext(
        user_id=user_id,
        session_id=session_id,
        vm_id=vm_id,
        operation=operation,
        component=component,
        additional_data=kwargs
    )


# Configuration
class ErrorHandlingConfig:
    """Error handling configuration."""
    
    # Circuit breaker settings
    CIRCUIT_BREAKER_FAILURE_THRESHOLD = 5
    CIRCUIT_BREAKER_RECOVERY_TIMEOUT = 60
    
    # Retry settings
    DEFAULT_MAX_RETRIES = 3
    DEFAULT_RETRY_BACKOFF_MULTIPLIER = 2.0
    DEFAULT_MAX_RETRY_DELAY = 60.0
    
    # Logging
    LOG_STACK_TRACES = True
    LOG_ERROR_DETAILS = True
    
    # Error response
    INCLUDE_STACK_TRACE_IN_RESPONSE = False  # Security: don't expose internals
    DEFAULT_ERROR_MESSAGE = "An internal error occurred"


# Example usage functions
async def example_vm_operation():
    """Example of VM operation with error handling."""
    try:
        # Simulate VM operation
        raise VMError(
            code=ErrorCode.VM_START_FAILED,
            message="Failed to start VM due to resource constraints",
            severity=ErrorSeverity.HIGH,
            details={"vm_id": "vm-123", "resource": "memory"},
            retry_strategy=RetryStrategy.EXPONENTIAL_BACKOFF,
            retry_after=30
        )
    except BuildPlatformError as e:
        return e.to_response()


@handle_errors(circuit_breaker="external_api")
async def example_external_api_call():
    """Example of external API call with circuit breaker."""
    # This would be an actual external API call
    raise Exception("External API temporarily unavailable")


if __name__ == "__main__":
    # Example usage
    import asyncio
    
    async def test_error_handling():
        response = await example_vm_operation()
        print(f"Error response: {response.model_dump_json(indent=2)}")
    
    asyncio.run(test_error_handling())