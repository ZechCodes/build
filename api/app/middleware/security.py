"""Security middleware components."""

import time
import uuid
import structlog
from typing import Optional, List, Dict, Any
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import Response, JSONResponse
from starlette.status import HTTP_429_TOO_MANY_REQUESTS, HTTP_403_FORBIDDEN

from ..core.redis import get_redis
from ..services.cache import get_cache_service
from ..core.config import get_settings

logger = structlog.get_logger(__name__)


class SecurityHeadersMiddleware(BaseHTTPMiddleware):
    """Add security headers to all responses."""
    
    def __init__(self, app, **kwargs):
        super().__init__(app)
        self.settings = get_settings()
    
    async def dispatch(self, request: Request, call_next):
        response = await call_next(request)
        
        # Security headers
        response.headers["X-Content-Type-Options"] = "nosniff"
        response.headers["X-Frame-Options"] = "DENY"
        response.headers["X-XSS-Protection"] = "1; mode=block"
        response.headers["Referrer-Policy"] = "strict-origin-when-cross-origin"
        response.headers["Permissions-Policy"] = "camera=(), microphone=(), geolocation=()"
        
        # HSTS header for HTTPS
        if request.url.scheme == "https":
            response.headers["Strict-Transport-Security"] = "max-age=31536000; includeSubDomains"
        
        # CSP header
        if self.settings.environment == "production":
            csp = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:;"
            response.headers["Content-Security-Policy"] = csp
        
        # Remove server information
        response.headers["Server"] = "Build-API"
        
        return response


class RateLimitMiddleware(BaseHTTPMiddleware):
    """Rate limiting middleware using Redis."""
    
    def __init__(self, app, **kwargs):
        super().__init__(app)
        self.settings = get_settings()
        self.default_limit = self.settings.rate_limit_per_minute
        self.window_seconds = 60
        
        # Define rate limits for different endpoints (Session 2 requirements)
        self.rate_limits = {
            "/api/v1/auth/login": (5, 900),  # 5 requests per 15 minutes (Session 2)
            "/api/v1/auth/register": (3, 3600),  # 3 requests per hour (Session 2)
            "/api/v1/auth/request-password-reset": (3, 3600),  # Session 2
            "/api/v1/auth/reset-password": (3, 3600),  # Session 2
            "/api/v1/auth/forgot-password": (3, 3600),  # Legacy endpoint
        }
    
    async def dispatch(self, request: Request, call_next):
        # Skip rate limiting for health checks and internal endpoints
        if request.url.path in ["/health", "/health/detailed", "/docs", "/redoc", "/openapi.json"]:
            return await call_next(request)
        
        try:
            redis = await get_redis()
            cache_service = await get_cache_service(redis)
            
            # Get client identifier (IP + User-Agent for anonymous, user_id for authenticated)
            client_ip = self.get_client_ip(request)
            user_agent = request.headers.get("user-agent", "unknown")
            user_id = getattr(request.state, "user_id", None)
            
            if user_id:
                identifier = f"user:{user_id}"
            else:
                identifier = f"ip:{client_ip}:{hash(user_agent) % 10000}"
            
            # Get rate limit for this endpoint
            endpoint = request.url.path
            limit, window = self.rate_limits.get(endpoint, (self.default_limit, self.window_seconds))
            
            # Check rate limit
            is_allowed, current_count, time_until_reset = await cache_service.check_rate_limit(
                identifier, limit, window
            )
            
            if not is_allowed:
                logger.warning(
                    "Rate limit exceeded",
                    identifier=identifier,
                    endpoint=endpoint,
                    current_count=current_count,
                    limit=limit,
                    client_ip=client_ip
                )
                
                return JSONResponse(
                    status_code=HTTP_429_TOO_MANY_REQUESTS,
                    content={
                        "detail": "Rate limit exceeded",
                        "retry_after": time_until_reset
                    },
                    headers={
                        "Retry-After": str(time_until_reset),
                        "X-RateLimit-Limit": str(limit),
                        "X-RateLimit-Remaining": str(max(0, limit - current_count)),
                        "X-RateLimit-Reset": str(int(time.time() + time_until_reset))
                    }
                )
            
            response = await call_next(request)
            
            # Add rate limit headers
            response.headers["X-RateLimit-Limit"] = str(limit)
            response.headers["X-RateLimit-Remaining"] = str(max(0, limit - current_count))
            response.headers["X-RateLimit-Reset"] = str(int(time.time() + time_until_reset))
            
            return response
            
        except Exception as e:
            logger.error("Rate limiting error", error=str(e))
            # Fail open - allow request if rate limiting fails
            return await call_next(request)
    
    def get_client_ip(self, request: Request) -> str:
        """Get client IP address considering proxies."""
        # Check for forwarded headers (in order of preference)
        forwarded_for = request.headers.get("X-Forwarded-For")
        if forwarded_for:
            # Take the first IP (client IP)
            return forwarded_for.split(",")[0].strip()
        
        real_ip = request.headers.get("X-Real-IP")
        if real_ip:
            return real_ip
        
        # Fallback to direct connection IP
        return request.client.host if request.client else "unknown"


class RequestLoggingMiddleware(BaseHTTPMiddleware):
    """Log all requests with structured logging."""
    
    def __init__(self, app, **kwargs):
        super().__init__(app)
        self.settings = get_settings()
    
    async def dispatch(self, request: Request, call_next):
        # Generate request ID for tracing
        request_id = str(uuid.uuid4())
        request.state.request_id = request_id
        
        # Start timing
        start_time = time.time()
        
        # Get client information
        client_ip = self.get_client_ip(request)
        user_agent = request.headers.get("user-agent", "")
        
        # Log request start
        logger.info(
            "Request started",
            request_id=request_id,
            method=request.method,
            path=request.url.path,
            query_params=dict(request.query_params),
            client_ip=client_ip,
            user_agent=user_agent,
            content_length=request.headers.get("content-length", 0)
        )
        
        try:
            response = await call_next(request)
            
            # Calculate processing time
            process_time = time.time() - start_time
            
            # Log response
            logger.info(
                "Request completed",
                request_id=request_id,
                method=request.method,
                path=request.url.path,
                status_code=response.status_code,
                process_time_ms=round(process_time * 1000, 2),
                client_ip=client_ip,
                content_length=response.headers.get("content-length", 0)
            )
            
            # Add request ID to response headers
            response.headers["X-Request-ID"] = request_id
            response.headers["X-Process-Time"] = f"{process_time:.3f}"
            
            return response
            
        except Exception as e:
            process_time = time.time() - start_time
            
            logger.error(
                "Request failed",
                request_id=request_id,
                method=request.method,
                path=request.url.path,
                error=str(e),
                error_type=type(e).__name__,
                process_time_ms=round(process_time * 1000, 2),
                client_ip=client_ip
            )
            
            raise
    
    def get_client_ip(self, request: Request) -> str:
        """Get client IP address considering proxies."""
        forwarded_for = request.headers.get("X-Forwarded-For")
        if forwarded_for:
            return forwarded_for.split(",")[0].strip()
        
        real_ip = request.headers.get("X-Real-IP")
        if real_ip:
            return real_ip
        
        return request.client.host if request.client else "unknown"


class UserContextMiddleware(BaseHTTPMiddleware):
    """Add user context to requests for authenticated users."""
    
    async def dispatch(self, request: Request, call_next):
        # Extract user information from JWT token if present
        authorization = request.headers.get("Authorization")
        
        if authorization and authorization.startswith("Bearer "):
            try:
                # This would be implemented when we add JWT authentication
                # For now, just pass through
                pass
            except Exception as e:
                logger.warning("Failed to extract user context", error=str(e))
        
        return await call_next(request)


class IPWhitelistMiddleware(BaseHTTPMiddleware):
    """IP whitelist middleware for sensitive endpoints."""
    
    def __init__(self, app, allowed_ips: Optional[List[str]] = None, **kwargs):
        super().__init__(app)
        self.allowed_ips = set(allowed_ips or [])
        self.settings = get_settings()
        
        # Always allow localhost in development
        if self.settings.environment == "development":
            self.allowed_ips.update(["127.0.0.1", "::1", "localhost"])
    
    async def dispatch(self, request: Request, call_next):
        # Only apply whitelist to admin endpoints
        if not request.url.path.startswith("/api/v1/admin"):
            return await call_next(request)
        
        if not self.allowed_ips:
            # No whitelist configured, allow all
            return await call_next(request)
        
        client_ip = self.get_client_ip(request)
        
        if client_ip not in self.allowed_ips:
            logger.warning(
                "IP access denied",
                client_ip=client_ip,
                path=request.url.path,
                allowed_ips=list(self.allowed_ips)
            )
            
            return JSONResponse(
                status_code=HTTP_403_FORBIDDEN,
                content={"detail": "Access denied from this IP address"}
            )
        
        return await call_next(request)
    
    def get_client_ip(self, request: Request) -> str:
        """Get client IP address considering proxies."""
        forwarded_for = request.headers.get("X-Forwarded-For")
        if forwarded_for:
            return forwarded_for.split(",")[0].strip()
        
        real_ip = request.headers.get("X-Real-IP")
        if real_ip:
            return real_ip
        
        return request.client.host if request.client else "unknown"


class RequestSizeLimitMiddleware(BaseHTTPMiddleware):
    """Limit request body size to prevent DoS attacks."""
    
    def __init__(self, app, max_size: int = 10 * 1024 * 1024, **kwargs):  # 10MB default
        super().__init__(app)
        self.max_size = max_size
    
    async def dispatch(self, request: Request, call_next):
        content_length = request.headers.get("content-length")
        
        if content_length and int(content_length) > self.max_size:
            logger.warning(
                "Request body too large",
                content_length=content_length,
                max_size=self.max_size,
                path=request.url.path
            )
            
            return JSONResponse(
                status_code=413,  # Payload Too Large
                content={"detail": f"Request body too large. Maximum size: {self.max_size} bytes"}
            )
        
        return await call_next(request)