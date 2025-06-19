"""Enhanced security middleware with comprehensive protection."""

import re
import time
import hashlib
import structlog
from typing import Optional, List, Dict, Any, Set
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import Response, JSONResponse
from starlette.status import HTTP_429_TOO_MANY_REQUESTS, HTTP_403_FORBIDDEN, HTTP_400_BAD_REQUEST

from ..core.redis import get_redis
from ..services.cache import get_cache_service
from ..services.audit import log_security_event
from ..services.security_validation import SecurityValidationService
from ..core.config import get_settings
from ..core.database import get_db_session


logger = structlog.get_logger(__name__)
settings = get_settings()


class EnhancedSecurityHeadersMiddleware(BaseHTTPMiddleware):
    """Enhanced security headers with additional protection."""
    
    def __init__(self, app, **kwargs):
        super().__init__(app)
        self.settings = get_settings()
    
    async def dispatch(self, request: Request, call_next):
        response = await call_next(request)
        
        # Standard security headers
        response.headers["X-Content-Type-Options"] = "nosniff"
        response.headers["X-Frame-Options"] = "DENY"
        response.headers["X-XSS-Protection"] = "1; mode=block"
        response.headers["Referrer-Policy"] = "strict-origin-when-cross-origin"
        response.headers["Permissions-Policy"] = (
            "camera=(), microphone=(), geolocation=(), payment=(), "
            "usb=(), magnetometer=(), gyroscope=(), accelerometer=(), "
            "ambient-light-sensor=(), autoplay=(), encrypted-media=(), "
            "fullscreen=(self), picture-in-picture=()"
        )
        
        # Enhanced HSTS for HTTPS
        if request.url.scheme == "https":
            response.headers["Strict-Transport-Security"] = (
                "max-age=31536000; includeSubDomains; preload"
            )
        
        # Enhanced CSP based on environment
        if self.settings.environment == "production":
            csp_directives = [
                "default-src 'self'",
                "script-src 'self' 'unsafe-inline' 'unsafe-eval'",
                "style-src 'self' 'unsafe-inline'",
                "img-src 'self' data: https:",
                "font-src 'self' https:",
                "connect-src 'self' wss: https:",
                "media-src 'self'",
                "object-src 'none'",
                "base-uri 'self'",
                "form-action 'self'",
                "frame-ancestors 'none'",
                "upgrade-insecure-requests"
            ]
            response.headers["Content-Security-Policy"] = "; ".join(csp_directives)
        else:
            # More permissive CSP for development
            csp_directives = [
                "default-src 'self' 'unsafe-inline' 'unsafe-eval' localhost:* 127.0.0.1:*",
                "connect-src 'self' ws: wss: localhost:* 127.0.0.1:*",
                "img-src 'self' data: blob: localhost:* 127.0.0.1:*"
            ]
            response.headers["Content-Security-Policy"] = "; ".join(csp_directives)
        
        # Additional security headers
        response.headers["X-Permitted-Cross-Domain-Policies"] = "none"
        response.headers["Cross-Origin-Embedder-Policy"] = "require-corp"
        response.headers["Cross-Origin-Opener-Policy"] = "same-origin"
        response.headers["Cross-Origin-Resource-Policy"] = "same-origin"
        
        # Remove sensitive server information
        response.headers["Server"] = "Build-API/1.0"
        
        # Add cache control for sensitive endpoints
        if request.url.path.startswith("/api/v1/auth") or "token" in request.url.path:
            response.headers["Cache-Control"] = "no-store, no-cache, must-revalidate, private"
            response.headers["Pragma"] = "no-cache"
            response.headers["Expires"] = "0"
        
        return response


class AdvancedRateLimitMiddleware(BaseHTTPMiddleware):
    """Advanced rate limiting with adaptive limits and attack detection."""
    
    def __init__(self, app, **kwargs):
        super().__init__(app)
        self.settings = get_settings()
        self.suspicious_ips: Set[str] = set()
        
        # Endpoint-specific rate limits (requests, window_seconds, burst_limit)
        self.rate_limits = {
            "/api/v1/auth/login": (5, 300, 2),  # 5 per 5 min, burst of 2
            "/api/v1/auth/register": (3, 3600, 1),  # 3 per hour, no burst
            "/api/v1/auth/forgot-password": (3, 3600, 1),
            "/api/v1/auth/reset-password": (5, 3600, 1),
            "/api/v1/auth/verify-email": (10, 3600, 3),
            "/api/v1/vms/create": (10, 3600, 2),
            "/api/v1/vms/": (100, 3600, 20),  # General VM operations
            "/api/v1/snapshots/create": (20, 3600, 5),
        }
        
        # Default limits for unspecified endpoints
        self.default_limits = {
            "authenticated": (1000, 3600, 100),  # 1000 per hour for auth users
            "anonymous": (100, 3600, 20),  # 100 per hour for anonymous
        }
    
    async def dispatch(self, request: Request, call_next):
        # Skip rate limiting for health and monitoring endpoints
        if request.url.path in ["/health", "/health/detailed", "/metrics", "/docs", "/redoc", "/openapi.json"]:
            return await call_next(request)
        
        try:
            redis = await get_redis()
            cache_service = await get_cache_service(redis)
            
            # Get client information
            client_ip = self._get_client_ip(request)
            user_agent = request.headers.get("user-agent", "unknown")
            user_id = getattr(request.state, "user_id", None)
            
            # Check if IP is suspicious
            if client_ip in self.suspicious_ips:
                await log_security_event(
                    event_type="suspicious_ip_blocked",
                    severity="high",
                    ip_address=client_ip,
                    user_agent=user_agent,
                    details={"path": request.url.path}
                )
                return JSONResponse(
                    status_code=HTTP_403_FORBIDDEN,
                    content={"detail": "Access denied due to suspicious activity"}
                )
            
            # Determine identifier and limits
            if user_id:
                identifier = f"user:{user_id}"
                default_limit, default_window, default_burst = self.default_limits["authenticated"]
            else:
                identifier = f"ip:{client_ip}:{hash(user_agent) % 10000}"
                default_limit, default_window, default_burst = self.default_limits["anonymous"]
            
            # Get endpoint-specific limits
            endpoint = request.url.path
            limit, window, burst_limit = self.rate_limits.get(
                endpoint, (default_limit, default_window, default_burst)
            )
            
            # Check main rate limit
            is_allowed, current_count, time_until_reset = await cache_service.check_rate_limit(
                identifier, limit, window
            )
            
            # Check burst limit (shorter window, lower limit)
            burst_identifier = f"{identifier}:burst"
            burst_allowed, burst_count, burst_reset = await cache_service.check_rate_limit(
                burst_identifier, burst_limit, 60  # 1 minute burst window
            )
            
            if not is_allowed or not burst_allowed:
                # Log rate limit violation
                await log_security_event(
                    event_type="rate_limit_exceeded",
                    severity="medium",
                    ip_address=client_ip,
                    user_agent=user_agent,
                    user_id=user_id,
                    details={
                        "endpoint": endpoint,
                        "current_count": current_count,
                        "limit": limit,
                        "burst_count": burst_count,
                        "burst_limit": burst_limit
                    }
                )
                
                # Check for potential attack (multiple rapid violations)
                violation_key = f"violations:{client_ip}"
                violation_count = await cache_service.increment_counter(violation_key, ttl=3600)
                
                if violation_count > 10:  # More than 10 violations in an hour
                    self.suspicious_ips.add(client_ip)
                    await log_security_event(
                        event_type="potential_dos_attack",
                        severity="critical",
                        ip_address=client_ip,
                        user_agent=user_agent,
                        details={"violation_count": violation_count}
                    )
                
                return JSONResponse(
                    status_code=HTTP_429_TOO_MANY_REQUESTS,
                    content={
                        "detail": "Rate limit exceeded",
                        "retry_after": min(time_until_reset, burst_reset) if not burst_allowed else time_until_reset
                    },
                    headers={
                        "Retry-After": str(int(min(time_until_reset, burst_reset) if not burst_allowed else time_until_reset)),
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
            logger.error("Advanced rate limiting error", error=str(e))
            return await call_next(request)
    
    def _get_client_ip(self, request: Request) -> str:
        """Get client IP with proxy support."""
        forwarded_for = request.headers.get("X-Forwarded-For")
        if forwarded_for:
            return forwarded_for.split(",")[0].strip()
        
        real_ip = request.headers.get("X-Real-IP")
        if real_ip:
            return real_ip
        
        return request.client.host if request.client else "unknown"


class InputValidationMiddleware(BaseHTTPMiddleware):
    """Middleware for input validation and attack detection."""
    
    def __init__(self, app, **kwargs):
        super().__init__(app)
        self.max_body_size = kwargs.get("max_body_size", 10 * 1024 * 1024)  # 10MB
        self.max_header_size = kwargs.get("max_header_size", 8192)  # 8KB
        self.max_query_params = kwargs.get("max_query_params", 50)
    
    async def dispatch(self, request: Request, call_next):
        client_ip = self._get_client_ip(request)
        user_agent = request.headers.get("user-agent", "")
        
        # Validate request size
        content_length = request.headers.get("content-length")
        if content_length and int(content_length) > self.max_body_size:
            await log_security_event(
                event_type="oversized_request",
                severity="medium",
                ip_address=client_ip,
                user_agent=user_agent,
                details={"content_length": content_length, "max_size": self.max_body_size}
            )
            return JSONResponse(
                status_code=413,
                content={"detail": f"Request body too large. Maximum size: {self.max_body_size} bytes"}
            )
        
        # Validate headers
        for name, value in request.headers.items():
            if len(value) > self.max_header_size:
                await log_security_event(
                    event_type="oversized_header",
                    severity="medium",
                    ip_address=client_ip,
                    user_agent=user_agent,
                    details={"header": name, "size": len(value)}
                )
                return JSONResponse(
                    status_code=HTTP_400_BAD_REQUEST,
                    content={"detail": "Request header too large"}
                )
        
        # Validate query parameters
        if len(request.query_params) > self.max_query_params:
            await log_security_event(
                event_type="too_many_query_params",
                severity="medium",
                ip_address=client_ip,
                user_agent=user_agent,
                details={"param_count": len(request.query_params)}
            )
            return JSONResponse(
                status_code=HTTP_400_BAD_REQUEST,
                content={"detail": "Too many query parameters"}
            )
        
        # Check for suspicious patterns in query parameters
        for key, value in request.query_params.items():
            if self._is_suspicious_param(key, value):
                await log_security_event(
                    event_type="suspicious_query_param",
                    severity="high",
                    ip_address=client_ip,
                    user_agent=user_agent,
                    details={"param": key, "value": value[:100]}
                )
        
        # Validate path parameters
        if self._has_path_traversal(request.url.path):
            await log_security_event(
                event_type="path_traversal_attempt",
                severity="high",
                ip_address=client_ip,
                user_agent=user_agent,
                details={"path": request.url.path}
            )
            return JSONResponse(
                status_code=HTTP_400_BAD_REQUEST,
                content={"detail": "Invalid request path"}
            )
        
        # Validate JSON body for POST/PUT requests
        if request.method in ["POST", "PUT", "PATCH"] and "application/json" in request.headers.get("content-type", ""):
            try:
                # For now, just pass through - detailed validation will be done at the endpoint level
                pass
            except Exception as e:
                await log_security_event(
                    event_type="invalid_json",
                    severity="low",
                    ip_address=client_ip,
                    user_agent=user_agent,
                    details={"error": str(e)}
                )
        
        return await call_next(request)
    
    def _get_client_ip(self, request: Request) -> str:
        """Get client IP with proxy support."""
        forwarded_for = request.headers.get("X-Forwarded-For")
        if forwarded_for:
            return forwarded_for.split(",")[0].strip()
        return request.client.host if request.client else "unknown"
    
    def _is_suspicious_param(self, key: str, value: str) -> bool:
        """Check if query parameter contains suspicious content."""
        suspicious_patterns = [
            r"<script",
            r"javascript:",
            r"on\w+\s*=",
            r"union\s+select",
            r"drop\s+table",
            r"exec\s*\(",
            r"eval\s*\(",
            r"\.\.\/",
            r"\.\.\\",
            r"\/etc\/passwd",
            r"cmd\.exe",
            r"powershell"
        ]
        
        combined_value = f"{key}={value}".lower()
        
        for pattern in suspicious_patterns:
            if re.search(pattern, combined_value, re.IGNORECASE):
                return True
        
        return False
    
    def _has_path_traversal(self, path: str) -> bool:
        """Check for path traversal attempts."""
        return ".." in path or "~" in path or path.count("/") > 10


class SecurityMonitoringMiddleware(BaseHTTPMiddleware):
    """Monitor requests for security threats and anomalies."""
    
    def __init__(self, app, **kwargs):
        super().__init__(app)
        self.bot_user_agents = {
            "bot", "crawler", "spider", "scraper", "curl", "wget", "python-requests",
            "scanner", "masscan", "nmap", "sqlmap", "burp", "nikto", "dirb"
        }
    
    async def dispatch(self, request: Request, call_next):
        client_ip = self._get_client_ip(request)
        user_agent = request.headers.get("user-agent", "").lower()
        
        # Detect potential bots/scanners
        if any(bot in user_agent for bot in self.bot_user_agents):
            await log_security_event(
                event_type="bot_detection",
                severity="low",
                ip_address=client_ip,
                user_agent=user_agent,
                details={"path": request.url.path}
            )
        
        # Monitor for rapid sequential requests (potential scanning)
        async with get_db_session() as db:
            validation_service = SecurityValidationService(db)
            ip_reputation = await validation_service.check_ip_reputation(client_ip)
            
            if ip_reputation["is_suspicious"]:
                await log_security_event(
                    event_type="suspicious_ip_activity",
                    severity="medium",
                    ip_address=client_ip,
                    user_agent=user_agent,
                    details={
                        "risk_score": ip_reputation["risk_score"],
                        "reasons": ip_reputation["reasons"]
                    }
                )
        
        # Check for unusual request patterns
        unusual_headers = self._check_unusual_headers(request.headers)
        if unusual_headers:
            await log_security_event(
                event_type="unusual_headers",
                severity="low",
                ip_address=client_ip,
                user_agent=user_agent,
                details={"unusual_headers": unusual_headers}
            )
        
        start_time = time.time()
        response = await call_next(request)
        response_time = time.time() - start_time
        
        # Monitor for error patterns that might indicate attacks
        if response.status_code >= 400:
            await log_security_event(
                event_type="error_response",
                severity="low",
                ip_address=client_ip,
                user_agent=user_agent,
                details={
                    "status_code": response.status_code,
                    "path": request.url.path,
                    "method": request.method,
                    "response_time": response_time
                }
            )
        
        return response
    
    def _get_client_ip(self, request: Request) -> str:
        """Get client IP with proxy support."""
        forwarded_for = request.headers.get("X-Forwarded-For")
        if forwarded_for:
            return forwarded_for.split(",")[0].strip()
        return request.client.host if request.client else "unknown"
    
    def _check_unusual_headers(self, headers) -> List[str]:
        """Check for unusual or suspicious headers."""
        unusual = []
        
        # Check for non-standard headers that might indicate attacks
        suspicious_headers = [
            "x-real-ip", "x-originating-ip", "x-forwarded-host",
            "x-remote-ip", "x-client-ip", "cf-connecting-ip"
        ]
        
        for header in headers:
            if header.lower() in suspicious_headers and header.lower() not in ["x-forwarded-for", "x-real-ip"]:
                unusual.append(header)
        
        return unusual