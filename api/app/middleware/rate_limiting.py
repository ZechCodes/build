"""Rate limiting middleware according to Session 2 requirements."""

from fastapi import HTTPException, Request, status
from starlette.middleware.base import BaseHTTPMiddleware
import redis.asyncio as redis
from datetime import datetime, timedelta, timezone
import structlog

logger = structlog.get_logger(__name__)


class RateLimitMiddleware(BaseHTTPMiddleware):
    """Rate limiting middleware for authentication endpoints."""

    def __init__(self, app, redis_url: str):
        """Initialize rate limiting middleware."""
        super().__init__(app)
        self.redis = redis.from_url(redis_url)
        
        # Session 2 rate limiting requirements
        self.rate_limits = {
            "/auth/login": {"max_requests": 5, "window_minutes": 15},
            "/auth/register": {"max_requests": 3, "window_minutes": 60},
            "/auth/reset-password": {"max_requests": 3, "window_minutes": 60},
        }

    async def dispatch(self, request: Request, call_next):
        """Process request with rate limiting checks."""
        client_ip = self.get_client_ip(request)
        endpoint = request.url.path
        
        # Only apply rate limiting to configured endpoints
        if endpoint in self.rate_limits:
            try:
                if await self.is_rate_limited(client_ip, endpoint):
                    logger.warning(
                        "Rate limit exceeded",
                        client_ip=client_ip,
                        endpoint=endpoint
                    )
                    raise HTTPException(
                        status_code=status.HTTP_429_TOO_MANY_REQUESTS,
                        detail="Rate limit exceeded. Please try again later."
                    )
            except Exception as e:
                # If Redis is unavailable, log error but allow request
                logger.error(
                    "Rate limiting check failed",
                    client_ip=client_ip,
                    endpoint=endpoint,
                    error=str(e)
                )
        
        # Process the request
        response = await call_next(request)
        
        # Record successful requests for rate limiting
        if endpoint in self.rate_limits and response.status_code < 400:
            try:
                await self.record_request(client_ip, endpoint)
            except Exception as e:
                logger.error(
                    "Failed to record request for rate limiting",
                    client_ip=client_ip,
                    endpoint=endpoint,
                    error=str(e)
                )
        
        return response

    def get_client_ip(self, request: Request) -> str:
        """Extract client IP address from request."""
        # Check for X-Forwarded-For header (proxy/load balancer)
        forwarded_for = request.headers.get("x-forwarded-for")
        if forwarded_for:
            # Return first IP in the chain
            return forwarded_for.split(",")[0].strip()
        
        # Check for X-Real-IP header
        real_ip = request.headers.get("x-real-ip")
        if real_ip:
            return real_ip.strip()
        
        # Fall back to direct connection IP
        return request.client.host

    async def is_rate_limited(self, client_ip: str, endpoint: str) -> bool:
        """Check if client is rate limited for endpoint."""
        limit_config = self.rate_limits[endpoint]
        key = f"rate_limit:{client_ip}:{endpoint}"
        
        try:
            current_requests = await self.redis.get(key)
            if current_requests is None:
                return False
            
            return int(current_requests) >= limit_config["max_requests"]
        except Exception as e:
            logger.error(
                "Redis rate limit check failed",
                key=key,
                error=str(e)
            )
            # If Redis fails, don't rate limit (fail open)
            return False

    async def record_request(self, client_ip: str, endpoint: str):
        """Record request for rate limiting."""
        limit_config = self.rate_limits[endpoint]
        key = f"rate_limit:{client_ip}:{endpoint}"
        window_seconds = limit_config["window_minutes"] * 60
        
        try:
            # Increment counter
            current_count = await self.redis.incr(key)
            
            # Set expiration on first request
            if current_count == 1:
                await self.redis.expire(key, window_seconds)
            
            logger.debug(
                "Rate limit request recorded",
                client_ip=client_ip,
                endpoint=endpoint,
                current_count=current_count,
                max_requests=limit_config["max_requests"]
            )
            
        except Exception as e:
            logger.error(
                "Failed to record rate limit request",
                key=key,
                error=str(e)
            )

    async def get_remaining_requests(self, client_ip: str, endpoint: str) -> dict:
        """Get remaining requests for client and endpoint."""
        if endpoint not in self.rate_limits:
            return {"remaining": -1, "reset_time": None}  # No limit
        
        limit_config = self.rate_limits[endpoint]
        key = f"rate_limit:{client_ip}:{endpoint}"
        
        try:
            current_requests = await self.redis.get(key)
            if current_requests is None:
                return {
                    "remaining": limit_config["max_requests"],
                    "reset_time": None
                }
            
            current_count = int(current_requests)
            remaining = max(0, limit_config["max_requests"] - current_count)
            
            # Get TTL for reset time
            ttl = await self.redis.ttl(key)
            reset_time = None
            if ttl > 0:
                reset_time = datetime.now(timezone.utc) + timedelta(seconds=ttl)
            
            return {
                "remaining": remaining,
                "reset_time": reset_time,
                "current_count": current_count,
                "max_requests": limit_config["max_requests"]
            }
            
        except Exception as e:
            logger.error(
                "Failed to get remaining requests",
                key=key,
                error=str(e)
            )
            return {"remaining": -1, "reset_time": None}

    async def reset_rate_limit(self, client_ip: str, endpoint: str) -> bool:
        """Reset rate limit for client and endpoint (admin function)."""
        if endpoint not in self.rate_limits:
            return False
        
        key = f"rate_limit:{client_ip}:{endpoint}"
        
        try:
            await self.redis.delete(key)
            logger.info(
                "Rate limit reset",
                client_ip=client_ip,
                endpoint=endpoint
            )
            return True
        except Exception as e:
            logger.error(
                "Failed to reset rate limit",
                key=key,
                error=str(e)
            )
            return False