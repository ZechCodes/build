"""Logfire middleware for enhanced request tracking."""

import time
import uuid
import structlog
from typing import Optional, Dict, Any
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import Response

try:
    import logfire
    LOGFIRE_AVAILABLE = True
except ImportError:
    LOGFIRE_AVAILABLE = False

from .logfire_setup import create_span

logger = structlog.get_logger(__name__)


class LogfireTrackingMiddleware(BaseHTTPMiddleware):
    """Enhanced request tracking with Logfire spans."""
    
    def __init__(self, app, **kwargs):
        super().__init__(app)
        self.include_request_body = kwargs.get("include_request_body", False)
        self.include_response_body = kwargs.get("include_response_body", False)
        self.sensitive_headers = {
            "authorization", "cookie", "x-api-key", "x-auth-token"
        }
    
    async def dispatch(self, request: Request, call_next):
        # Generate trace ID if not present
        trace_id = request.headers.get("x-trace-id", str(uuid.uuid4()))
        request.state.trace_id = trace_id
        
        # Extract request information
        request_info = await self._extract_request_info(request)
        
        # Create Logfire span
        with create_span(
            "HTTP Request",
            trace_id=trace_id,
            **request_info
        ) as span:
            start_time = time.time()
            
            try:
                response = await call_next(request)
                
                # Calculate processing time
                process_time = time.time() - start_time
                
                # Extract response information
                response_info = self._extract_response_info(response, process_time)
                
                # Update span with response data
                for key, value in response_info.items():
                    span.set_attribute(f"response.{key}", value)
                
                # Add trace ID to response headers
                response.headers["x-trace-id"] = trace_id
                
                # Log successful request
                logger.info(
                    "Request completed",
                    trace_id=trace_id,
                    method=request_info["method"],
                    path=request_info["path"],
                    status_code=response_info["status_code"],
                    process_time_ms=response_info["process_time_ms"]
                )
                
                return response
                
            except Exception as e:
                process_time = time.time() - start_time
                
                # Record exception in span
                span.record_exception(e)
                span.set_attribute("response.status_code", 500)
                span.set_attribute("response.process_time_ms", round(process_time * 1000, 2))
                span.set_attribute("error.type", type(e).__name__)
                span.set_attribute("error.message", str(e))
                
                # Log error
                logger.error(
                    "Request failed",
                    trace_id=trace_id,
                    method=request_info["method"],
                    path=request_info["path"],
                    error_type=type(e).__name__,
                    error_message=str(e),
                    process_time_ms=round(process_time * 1000, 2),
                    exc_info=e
                )
                
                raise
    
    async def _extract_request_info(self, request: Request) -> Dict[str, Any]:
        """Extract relevant request information."""
        # Get client IP
        client_ip = self._get_client_ip(request)
        
        # Get user agent
        user_agent = request.headers.get("user-agent", "")
        
        # Get user ID if available
        user_id = getattr(request.state, "user_id", None)
        
        # Get filtered headers
        headers = self._filter_sensitive_headers(dict(request.headers))
        
        request_info = {
            "method": request.method,
            "path": request.url.path,
            "query_string": str(request.url.query) if request.url.query else "",
            "client_ip": client_ip,
            "user_agent": user_agent,
            "headers": headers,
            "content_length": request.headers.get("content-length", 0)
        }
        
        if user_id:
            request_info["user_id"] = user_id
        
        # Include request body if configured (be careful with sensitive data)
        if self.include_request_body and request.method in ["POST", "PUT", "PATCH"]:
            try:
                content_type = request.headers.get("content-type", "")
                if "application/json" in content_type:
                    body = await request.body()
                    if len(body) < 10000:  # Limit body size
                        request_info["body_size"] = len(body)
                        # Don't log actual body content for security
            except Exception as e:
                logger.debug("Failed to read request body", error=str(e))
        
        return request_info
    
    def _extract_response_info(self, response: Response, process_time: float) -> Dict[str, Any]:
        """Extract relevant response information."""
        response_info = {
            "status_code": response.status_code,
            "process_time_ms": round(process_time * 1000, 2),
            "content_length": response.headers.get("content-length", 0)
        }
        
        # Add response headers (filtered)
        filtered_headers = self._filter_sensitive_headers(dict(response.headers))
        response_info["headers"] = filtered_headers
        
        return response_info
    
    def _get_client_ip(self, request: Request) -> str:
        """Get client IP address considering proxies."""
        forwarded_for = request.headers.get("x-forwarded-for")
        if forwarded_for:
            return forwarded_for.split(",")[0].strip()
        
        real_ip = request.headers.get("x-real-ip")
        if real_ip:
            return real_ip
        
        return request.client.host if request.client else "unknown"
    
    def _filter_sensitive_headers(self, headers: Dict[str, str]) -> Dict[str, str]:
        """Filter out sensitive headers."""
        filtered = {}
        for key, value in headers.items():
            if key.lower() in self.sensitive_headers:
                filtered[key] = "[REDACTED]"
            else:
                filtered[key] = value
        return filtered


class LogfirePerformanceMiddleware(BaseHTTPMiddleware):
    """Track performance metrics with Logfire."""
    
    def __init__(self, app, **kwargs):
        super().__init__(app)
        self.slow_request_threshold = kwargs.get("slow_request_threshold", 1.0)  # 1 second
        self.track_database_queries = kwargs.get("track_database_queries", True)
        self.track_cache_operations = kwargs.get("track_cache_operations", True)
    
    async def dispatch(self, request: Request, call_next):
        # Skip performance tracking for health checks and metrics endpoints
        if request.url.path in ["/health", "/metrics", "/health/detailed"]:
            return await call_next(request)
        
        with create_span(
            "Performance Tracking",
            endpoint=self._normalize_endpoint(request.url.path),
            method=request.method
        ) as span:
            start_time = time.time()
            
            # Track memory usage before request
            memory_before = self._get_memory_usage()
            
            response = await call_next(request)
            
            # Calculate metrics
            duration = time.time() - start_time
            memory_after = self._get_memory_usage()
            memory_delta = memory_after - memory_before if memory_after and memory_before else 0
            
            # Update span with performance data
            span.set_attribute("performance.duration_ms", round(duration * 1000, 2))
            span.set_attribute("performance.memory_delta_mb", round(memory_delta / 1024 / 1024, 2))
            span.set_attribute("performance.status_code", response.status_code)
            
            # Log slow requests
            if duration > self.slow_request_threshold:
                span.set_attribute("performance.slow_request", True)
                logger.warning(
                    "Slow request detected",
                    method=request.method,
                    path=request.url.path,
                    duration_ms=round(duration * 1000, 2),
                    memory_delta_mb=round(memory_delta / 1024 / 1024, 2),
                    status_code=response.status_code
                )
            
            # Track additional metrics if available
            if hasattr(request.state, "db_query_count"):
                span.set_attribute("performance.db_queries", request.state.db_query_count)
            
            if hasattr(request.state, "cache_operations"):
                span.set_attribute("performance.cache_operations", request.state.cache_operations)
            
            return response
    
    def _normalize_endpoint(self, path: str) -> str:
        """Normalize endpoint path for metrics grouping."""
        import re
        
        # Replace UUIDs
        path = re.sub(
            r'/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}',
            '/{uuid}',
            path,
            flags=re.IGNORECASE
        )
        
        # Replace numeric IDs
        path = re.sub(r'/\d+', '/{id}', path)
        
        return path
    
    def _get_memory_usage(self) -> Optional[int]:
        """Get current memory usage in bytes."""
        try:
            import psutil
            process = psutil.Process()
            return process.memory_info().rss
        except ImportError:
            return None
        except Exception:
            return None


class LogfireUserActivityMiddleware(BaseHTTPMiddleware):
    """Track user activity and sessions with Logfire."""
    
    def __init__(self, app, **kwargs):
        super().__init__(app)
        self.track_anonymous_users = kwargs.get("track_anonymous_users", False)
    
    async def dispatch(self, request: Request, call_next):
        # Extract user context
        user_id = getattr(request.state, "user_id", None)
        session_id = getattr(request.state, "session_id", None)
        
        if not user_id and not self.track_anonymous_users:
            return await call_next(request)
        
        activity_data = {
            "user_id": user_id or "anonymous",
            "session_id": session_id,
            "endpoint": request.url.path,
            "method": request.method,
            "client_ip": self._get_client_ip(request),
            "user_agent": request.headers.get("user-agent", "")
        }
        
        with create_span("User Activity", **activity_data) as span:
            response = await call_next(request)
            
            # Update activity data with response info
            span.set_attribute("activity.status_code", response.status_code)
            span.set_attribute("activity.successful", response.status_code < 400)
            
            # Log user activity
            logger.info(
                "User activity recorded",
                **activity_data,
                status_code=response.status_code
            )
            
            return response
    
    def _get_client_ip(self, request: Request) -> str:
        """Get client IP address."""
        forwarded_for = request.headers.get("x-forwarded-for")
        if forwarded_for:
            return forwarded_for.split(",")[0].strip()
        
        return request.client.host if request.client else "unknown"