# Session 12.1: Advanced Rate Limiting Engine

## Objective
Implement comprehensive multi-tier rate limiting system with Redis backend, providing granular protection against API abuse through per-IP, per-user, per-endpoint, and global rate limits with intelligent blocking and burst allowance capabilities.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for rate limiting analytics and monitoring integration
- **Session 2**: Protects authentication endpoints with stricter rate limits
- **Session 6**: Rate limits WebSocket connections and terminal session operations
- **Session 7**: Applies rate limits to VM snapshot operations and storage access
- **Session 8**: Rate limits frontend API calls and user interaction endpoints
- **Session 9**: Protects Git operations and repository access with specialized limits
- **Session 10**: Rate limits recording operations and playback requests
- **Session 11**: Integrates with metrics collection for rate limiting analytics

## Core Implementation

### Advanced Rate Limiting Engine
**Location**: `api/middleware/rate_limiting/advanced_limiter.py`

```python
# api/middleware/rate_limiting/advanced_limiter.py
import asyncio
import time
import hashlib
from typing import Dict, Any, Optional, List, Tuple
from dataclasses import dataclass
from enum import Enum
import redis.asyncio as redis
import structlog
from fastapi import Request, HTTPException, status

logger = structlog.get_logger()

class LimitType(Enum):
    PER_SECOND = "per_second"
    PER_MINUTE = "per_minute"
    PER_HOUR = "per_hour"
    PER_DAY = "per_day"
    BURST = "burst"

class LimitScope(Enum):
    GLOBAL = "global"
    PER_IP = "per_ip"
    PER_USER = "per_user"
    PER_ENDPOINT = "per_endpoint"
    PER_API_KEY = "per_api_key"

@dataclass
class RateLimit:
    scope: LimitScope
    limit_type: LimitType
    max_requests: int
    window_seconds: int
    burst_allowance: int = 0
    blocked_duration: int = 300  # 5 minutes default block

@dataclass
class RateLimitResult:
    allowed: bool
    remaining: int
    reset_time: float
    retry_after: Optional[int] = None
    blocked_until: Optional[float] = None

class AdvancedRateLimiter:
    def __init__(self, redis_client: redis.Redis):
        self.redis = redis_client
        self.rate_limits: Dict[str, List[RateLimit]] = {}
        self.blocked_ips: Dict[str, float] = {}
        self.whitelist: set = set()
        self.suspicious_patterns: Dict[str, int] = {}
        
        # Initialize default rate limits
        self._initialize_default_limits()
    
    def _initialize_default_limits(self):
        """Initialize default rate limiting rules"""
        # Global API limits
        self.rate_limits['global'] = [
            RateLimit(LimitScope.GLOBAL, LimitType.PER_SECOND, 1000, 1),
            RateLimit(LimitScope.GLOBAL, LimitType.PER_MINUTE, 10000, 60),
        ]
        
        # Per-IP limits
        self.rate_limits['per_ip'] = [
            RateLimit(LimitScope.PER_IP, LimitType.PER_SECOND, 10, 1, burst_allowance=5),
            RateLimit(LimitScope.PER_IP, LimitType.PER_MINUTE, 300, 60),
            RateLimit(LimitScope.PER_IP, LimitType.PER_HOUR, 3600, 3600),
        ]
        
        # Authentication endpoints (stricter)
        self.rate_limits['auth'] = [
            RateLimit(LimitScope.PER_IP, LimitType.PER_MINUTE, 5, 60, blocked_duration=900),
            RateLimit(LimitScope.PER_IP, LimitType.PER_HOUR, 20, 3600, blocked_duration=1800),
        ]
        
        # VM operations (resource intensive)
        self.rate_limits['vm_ops'] = [
            RateLimit(LimitScope.PER_USER, LimitType.PER_MINUTE, 10, 60),
            RateLimit(LimitScope.PER_USER, LimitType.PER_HOUR, 100, 3600),
        ]
        
        # File operations
        self.rate_limits['file_ops'] = [
            RateLimit(LimitScope.PER_USER, LimitType.PER_SECOND, 5, 1),
            RateLimit(LimitScope.PER_USER, LimitType.PER_MINUTE, 100, 60),
        ]
        
        # WebSocket connections
        self.rate_limits['websocket'] = [
            RateLimit(LimitScope.PER_IP, LimitType.PER_MINUTE, 10, 60),
            RateLimit(LimitScope.PER_USER, LimitType.PER_MINUTE, 20, 60),
        ]
    
    async def check_rate_limit(self, request: Request, endpoint_category: str = 'global',
                             user_id: Optional[str] = None) -> RateLimitResult:
        """Check if request is within rate limits"""
        try:
            client_ip = self._get_client_ip(request)
            
            # Check if IP is blocked
            if await self._is_ip_blocked(client_ip):
                return RateLimitResult(
                    allowed=False,
                    remaining=0,
                    reset_time=self.blocked_ips.get(client_ip, time.time() + 300),
                    retry_after=300,
                    blocked_until=self.blocked_ips.get(client_ip)
                )
            
            # Check if IP is whitelisted
            if client_ip in self.whitelist:
                return RateLimitResult(allowed=True, remaining=float('inf'), reset_time=0)
            
            # Get applicable rate limits
            limits = self.rate_limits.get(endpoint_category, self.rate_limits['global'])
            
            # Check each limit
            for rate_limit in limits:
                result = await self._check_individual_limit(
                    client_ip, user_id, rate_limit, endpoint_category
                )
                
                if not result.allowed:
                    # Check if this should trigger IP blocking
                    await self._evaluate_blocking(client_ip, rate_limit)
                    return result
            
            return RateLimitResult(allowed=True, remaining=1000, reset_time=time.time() + 60)
            
        except Exception as e:
            logger.error("Rate limit check failed", error=str(e))
            # Fail open for reliability
            return RateLimitResult(allowed=True, remaining=1000, reset_time=time.time() + 60)
    
    async def _check_individual_limit(self, client_ip: str, user_id: Optional[str],
                                    rate_limit: RateLimit, category: str) -> RateLimitResult:
        """Check an individual rate limit"""
        # Determine the key for this limit
        if rate_limit.scope == LimitScope.GLOBAL:
            key = f"rate_limit:global:{category}"
        elif rate_limit.scope == LimitScope.PER_IP:
            key = f"rate_limit:ip:{client_ip}:{category}"
        elif rate_limit.scope == LimitScope.PER_USER and user_id:
            key = f"rate_limit:user:{user_id}:{category}"
        else:
            # Default to IP-based if user not available
            key = f"rate_limit:ip:{client_ip}:{category}"
        
        current_time = time.time()
        window_start = current_time - rate_limit.window_seconds
        
        # Use sliding window log with Redis sorted sets
        pipe = self.redis.pipeline()
        
        # Remove old entries
        pipe.zremrangebyscore(key, 0, window_start)
        
        # Count current requests in window
        pipe.zcard(key)
        
        # Add current request
        pipe.zadd(key, {str(current_time): current_time})
        
        # Set expiration
        pipe.expire(key, rate_limit.window_seconds + 60)
        
        results = await pipe.execute()
        current_count = results[1]
        
        # Check if within limits (including burst allowance)
        max_allowed = rate_limit.max_requests + rate_limit.burst_allowance
        
        if current_count > max_allowed:
            # Remove the request we just added since it's not allowed
            await self.redis.zrem(key, str(current_time))
            
            return RateLimitResult(
                allowed=False,
                remaining=0,
                reset_time=current_time + rate_limit.window_seconds,
                retry_after=rate_limit.window_seconds
            )
        
        remaining = max_allowed - current_count
        reset_time = current_time + rate_limit.window_seconds
        
        return RateLimitResult(
            allowed=True,
            remaining=remaining,
            reset_time=reset_time
        )
    
    async def _evaluate_blocking(self, client_ip: str, rate_limit: RateLimit):
        """Evaluate if IP should be blocked for repeated violations"""
        violation_key = f"violations:ip:{client_ip}"
        current_time = time.time()
        
        # Record violation
        await self.redis.zadd(violation_key, {str(current_time): current_time})
        
        # Count violations in last hour
        hour_ago = current_time - 3600
        await self.redis.zremrangebyscore(violation_key, 0, hour_ago)
        violation_count = await self.redis.zcard(violation_key)
        
        # Block IP if too many violations
        if violation_count >= 10:  # 10 violations per hour
            block_until = current_time + rate_limit.blocked_duration
            self.blocked_ips[client_ip] = block_until
            
            # Store in Redis for persistence
            await self.redis.setex(
                f"blocked_ip:{client_ip}", 
                rate_limit.blocked_duration, 
                str(block_until)
            )
            
            logger.warning("IP blocked for rate limit violations", 
                         client_ip=client_ip, violations=violation_count,
                         blocked_until=block_until)
    
    async def _is_ip_blocked(self, client_ip: str) -> bool:
        """Check if IP is currently blocked"""
        # Check local cache first
        if client_ip in self.blocked_ips:
            if time.time() < self.blocked_ips[client_ip]:
                return True
            else:
                # Block expired
                del self.blocked_ips[client_ip]
        
        # Check Redis
        blocked_until_str = await self.redis.get(f"blocked_ip:{client_ip}")
        if blocked_until_str:
            blocked_until = float(blocked_until_str)
            if time.time() < blocked_until:
                self.blocked_ips[client_ip] = blocked_until
                return True
            else:
                # Clean up expired block
                await self.redis.delete(f"blocked_ip:{client_ip}")
        
        return False
    
    def _get_client_ip(self, request: Request) -> str:
        """Extract client IP from request"""
        # Check common headers for real IP
        forwarded_for = request.headers.get("X-Forwarded-For")
        if forwarded_for:
            # Take the first IP in the chain
            return forwarded_for.split(',')[0].strip()
        
        real_ip = request.headers.get("X-Real-IP")
        if real_ip:
            return real_ip
        
        # Fallback to client host
        return request.client.host if request.client else "unknown"
    
    async def add_to_whitelist(self, ip_address: str):
        """Add IP to whitelist"""
        self.whitelist.add(ip_address)
        await self.redis.sadd("rate_limit:whitelist", ip_address)
        logger.info("IP added to whitelist", ip_address=ip_address)
    
    async def remove_from_whitelist(self, ip_address: str):
        """Remove IP from whitelist"""
        self.whitelist.discard(ip_address)
        await self.redis.srem("rate_limit:whitelist", ip_address)
        logger.info("IP removed from whitelist", ip_address=ip_address)
    
    async def unblock_ip(self, ip_address: str):
        """Manually unblock an IP"""
        self.blocked_ips.pop(ip_address, None)
        await self.redis.delete(f"blocked_ip:{ip_address}")
        logger.info("IP manually unblocked", ip_address=ip_address)
```

### Rate Limiting Middleware
**Location**: `api/middleware/rate_limiting/middleware.py`

```python
# api/middleware/rate_limiting/middleware.py
import time
from typing import Callable, Awaitable
from fastapi import Request, Response, HTTPException, status
from fastapi.middleware.base import BaseHTTPMiddleware
import structlog
import logfire
from .advanced_limiter import AdvancedRateLimiter, RateLimitResult

logger = structlog.get_logger()

class RateLimitingMiddleware(BaseHTTPMiddleware):
    def __init__(self, app, rate_limiter: AdvancedRateLimiter):
        super().__init__(app)
        self.rate_limiter = rate_limiter
        
        # Endpoint category mapping
        self.endpoint_categories = {
            "/auth/": "auth",
            "/api/v1/vms/": "vm_ops",
            "/api/v1/files/": "file_ops",
            "/ws/": "websocket",
            "/api/v1/sessions/": "session_ops",
            "/api/v1/repos/": "git_ops",
            "/api/v1/recordings/": "recording_ops"
        }

    async def dispatch(self, request: Request, call_next: Callable[[Request], Awaitable[Response]]) -> Response:
        start_time = time.time()
        
        # Determine endpoint category
        endpoint_category = self._get_endpoint_category(request.url.path)
        
        # Extract user ID if available
        user_id = self._extract_user_id(request)
        
        # Check rate limits
        rate_limit_result = await self.rate_limiter.check_rate_limit(
            request, endpoint_category, user_id
        )
        
        # Log rate limiting attempt
        logfire.debug("Rate limit check",
                     endpoint=request.url.path,
                     category=endpoint_category,
                     client_ip=self.rate_limiter._get_client_ip(request),
                     allowed=rate_limit_result.allowed,
                     remaining=rate_limit_result.remaining)
        
        if not rate_limit_result.allowed:
            # Log rate limit violation
            logger.warning("Rate limit exceeded",
                         endpoint=request.url.path,
                         client_ip=self.rate_limiter._get_client_ip(request),
                         user_id=user_id,
                         retry_after=rate_limit_result.retry_after)
            
            # Return rate limit error response
            response = Response(
                content="{\"error\": \"Rate limit exceeded\", \"retry_after\": " + 
                       str(rate_limit_result.retry_after) + "}",
                status_code=status.HTTP_429_TOO_MANY_REQUESTS,
                media_type="application/json"
            )
            
            # Add rate limiting headers
            response.headers["X-RateLimit-Limit"] = "0"
            response.headers["X-RateLimit-Remaining"] = "0"
            response.headers["X-RateLimit-Reset"] = str(int(rate_limit_result.reset_time))
            response.headers["Retry-After"] = str(rate_limit_result.retry_after)
            
            if rate_limit_result.blocked_until:
                response.headers["X-RateLimit-Blocked-Until"] = str(int(rate_limit_result.blocked_until))
            
            return response
        
        # Process request
        response = await call_next(request)
        
        # Add rate limiting headers to successful responses
        response.headers["X-RateLimit-Remaining"] = str(rate_limit_result.remaining)
        response.headers["X-RateLimit-Reset"] = str(int(rate_limit_result.reset_time))
        
        # Log successful request with rate limiting info
        processing_time = time.time() - start_time
        logfire.info("Request processed with rate limiting",
                   endpoint=request.url.path,
                   status_code=response.status_code,
                   processing_time_ms=processing_time * 1000,
                   remaining_requests=rate_limit_result.remaining)
        
        return response
    
    def _get_endpoint_category(self, path: str) -> str:
        """Determine endpoint category for rate limiting"""
        for prefix, category in self.endpoint_categories.items():
            if path.startswith(prefix):
                return category
        return "global"
    
    def _extract_user_id(self, request: Request) -> Optional[str]:
        """Extract user ID from request"""
        # Try to get user ID from JWT token or session
        auth_header = request.headers.get("Authorization")
        if auth_header:
            # This would integrate with your authentication system
            # to extract user ID from JWT token
            pass
        
        # Try to get from session cookie
        session_id = request.cookies.get("session_id")
        if session_id:
            # This would look up user ID from session
            pass
        
        return None
```

## TDD Implementation Cycle

### Red Phase: Rate Limiting Test Creation
```python
# api/tests/test_rate_limiting.py
import pytest
import asyncio
from fastapi import FastAPI, Request
from fastapi.testclient import TestClient
from api.middleware.rate_limiting.advanced_limiter import AdvancedRateLimiter, RateLimit, LimitScope, LimitType

@pytest.mark.asyncio
async def test_rate_limiter_initialization():
    """Test rate limiter initializes with default limits"""
    # This test should initially fail (Red phase)
    assert False, "Rate limiter initialization not implemented yet"

@pytest.mark.asyncio
async def test_per_ip_rate_limiting():
    """Test per-IP rate limiting works correctly"""
    # This test should initially fail (Red phase)
    assert False, "Per-IP rate limiting not implemented yet"

@pytest.mark.asyncio
async def test_rate_limit_blocking():
    """Test IP blocking after repeated violations"""
    # This test should initially fail (Red phase)
    assert False, "Rate limit blocking not implemented yet"

@pytest.mark.asyncio
async def test_sliding_window_algorithm():
    """Test sliding window rate limiting accuracy"""
    # This test should initially fail (Red phase)
    assert False, "Sliding window algorithm not implemented yet"

@pytest.mark.asyncio
async def test_rate_limit_middleware_integration():
    """Test rate limiting middleware integration"""
    # This test should initially fail (Red phase)
    assert False, "Rate limiting middleware not implemented yet"
```

### Green Phase: Rate Limiting Implementation
```python
# Implement rate limiting features to make tests pass
# This involves adding sliding window logic, Redis integration, and middleware
```

### Refactor Phase: Rate Limiting Optimization
```python
# Optimize rate limiting for performance and accuracy
# Add advanced blocking strategies and whitelist management
# Enhance Redis operations and error handling
```

## Security Checklist ✅

### Rate Limiting Core Security
- [ ] Rate limit bypass prevention through header manipulation
- [ ] Distributed rate limiting across multiple servers
- [ ] Rate limit configuration stored securely
- [ ] Protection against rate limit enumeration
- [ ] Secure rate limit key generation
- [ ] Rate limit data encryption in Redis
- [ ] Protection against rate limit storage exhaustion
- [ ] Audit logging for rate limit violations
- [ ] Rate limit whitelist security validation
- [ ] Emergency rate limit override procedures

### Implementation Security
- [ ] Redis connection security and authentication
- [ ] Protection against Redis command injection
- [ ] Secure client IP extraction and validation
- [ ] Protection against IP spoofing attacks
- [ ] Rate limit violation logging without sensitive data
- [ ] Secure handling of rate limit exceptions
- [ ] Protection against timing attacks on rate limits
- [ ] Secure rate limit cache management
- [ ] Rate limit configuration validation
- [ ] Protection against rate limit cache poisoning

### Operational Security
- [ ] Rate limiting system monitoring and alerting
- [ ] Secure rate limit configuration management
- [ ] Protection against rate limiting system compromise
- [ ] Secure deployment of rate limiting updates
- [ ] Rate limiting system backup and recovery
- [ ] Incident response for rate limiting failures
- [ ] Security testing of rate limiting implementation
- [ ] Rate limiting system access controls
- [ ] Protection against rate limiting denial of service
- [ ] Regular security assessment of rate limiting system

### Integration Security
- [ ] Secure integration with authentication systems
- [ ] Protection against authentication bypass via rate limiting
- [ ] Secure user identification for rate limiting
- [ ] Rate limiting coordination with load balancers
- [ ] Secure integration with monitoring systems
- [ ] Protection against cross-service rate limit bypass
- [ ] Secure rate limit data sharing between services
- [ ] Rate limiting integration with CDN providers
- [ ] Protection against rate limiting information disclosure
- [ ] Secure handling of rate limiting edge cases

## Performance Requirements

### Rate Limiting Performance
- Rate limit check latency < 5ms
- Redis operation response time < 2ms
- Rate limiting overhead < 1% of request processing
- Concurrent rate limit checks > 10,000/second
- Memory usage optimization
- CPU usage efficiency

### Accuracy Requirements
- Sliding window accuracy > 99%
- Rate limit enforcement precision ± 1%
- IP blocking effectiveness > 95%
- Whitelist bypass reliability > 99.9%
- Rate limit reset timing accuracy ± 5 seconds
- Violation detection accuracy > 98%

### Scalability Requirements
- Support 1M+ rate limit checks per minute
- Handle 100K+ concurrent IP addresses
- Scale to 1000+ rate limit rules
- Support 10+ rate limiting categories
- Manage 100MB+ rate limiting data in Redis
- Handle 50+ concurrent rate limit operations

## Commit Instructions

After implementing the rate limiting engine:

```bash
git add api/middleware/rate_limiting/
git commit -m "Add advanced multi-tier rate limiting engine with Redis backend

- Implement AdvancedRateLimiter with sliding window algorithm
- Add multi-scope rate limiting (global, per-IP, per-user, per-endpoint)
- Implement intelligent IP blocking with violation tracking
- Add configurable rate limits for different endpoint categories
- Include burst allowance and graduated blocking durations
- Add comprehensive whitelist and manual override capabilities
- Implement RateLimitingMiddleware for FastAPI integration
- Add Redis persistence for distributed rate limiting
- Include detailed rate limiting headers and error responses
- Add TDD cycle with Red-Green-Refactor for rate limiting
- Ensure >90% rate limiting test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete rate limiting test suite:

```bash
# Run all rate limiting tests
pytest api/tests/test_rate_limiting.py -v --timeout=300

# Run specific rate limiting test categories
pytest api/tests/rate_limiting/ -k "sliding_window" -v
pytest api/tests/rate_limiting/ -k "ip_blocking" -v
pytest api/tests/rate_limiting/ -k "middleware" -v

# Run rate limiting performance tests
pytest api/tests/rate_limiting/performance/ -v

# Run rate limiting Redis integration tests
pytest api/tests/rate_limiting/test_redis_integration.py -v
```

Validate rate limiting test coverage:
```bash
pytest api/tests/rate_limiting/ --cov=api.middleware.rate_limiting --cov-report=html --cov-fail-under=90
```

## Integration Testing

Test rate limiting integration with platform components:
```bash
# Test integration with Session 2 (Authentication)
pytest api/tests/integration/test_rate_limiting_auth_integration.py -v

# Test integration with Session 11 (Metrics)
pytest api/tests/integration/test_rate_limiting_metrics_integration.py -v

# Test Redis failover behavior
pytest api/tests/integration/test_rate_limiting_redis_failover.py -v
```