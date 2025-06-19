"""Dependency injection for FastAPI routes."""

import uuid
import structlog
from typing import Generator, Optional, AsyncGenerator
from fastapi import Depends, HTTPException, status, Request
from fastapi.security import HTTPBearer, HTTPAuthorizationCredentials
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select

from app.core.database import get_db_session
from app.core.redis import get_redis, RedisManager
from app.core.config import get_settings, Settings
from app.core.security import verify_token
from app.services.cache import get_cache_service, CacheService
from app.models.user import User

logger = structlog.get_logger(__name__)

security = HTTPBearer()


async def get_db() -> AsyncGenerator[AsyncSession, None]:
    """Database dependency with proper error handling."""
    async with get_db_session() as session:
        try:
            yield session
        except Exception as e:
            logger.error("Database session error", error=str(e))
            await session.rollback()
            raise
        finally:
            await session.close()


async def get_current_user(
    credentials: HTTPAuthorizationCredentials = Depends(security),
    db: AsyncSession = Depends(get_db)
) -> User:
    """Get current authenticated user."""
    credentials_exception = HTTPException(
        status_code=status.HTTP_401_UNAUTHORIZED,
        detail="Could not validate credentials",
        headers={"WWW-Authenticate": "Bearer"},
    )
    
    if not credentials:
        raise credentials_exception
    
    user_id = verify_token(credentials.credentials)
    if user_id is None:
        raise credentials_exception
    
    # Convert user_id to UUID
    try:
        user_uuid = uuid.UUID(user_id)
    except ValueError:
        raise credentials_exception
    
    # Fetch user from database
    result = await db.execute(select(User).where(User.id == user_uuid))
    user = result.scalar_one_or_none()
    
    if user is None:
        raise credentials_exception
    
    if not user.is_active:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Inactive user"
        )
    
    return user


async def get_current_active_superuser(
    current_user: User = Depends(get_current_user),
) -> User:
    """Get current active superuser."""
    if not current_user.is_superuser:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="The user doesn't have enough privileges"
        )
    return current_user


async def get_optional_current_user(
    credentials: Optional[HTTPAuthorizationCredentials] = Depends(security),
    db: AsyncSession = Depends(get_db)
) -> Optional[User]:
    """Get current user if authenticated, otherwise None."""
    if not credentials:
        return None
    
    user_id = verify_token(credentials.credentials)
    if user_id is None:
        return None
    
    # Convert user_id to UUID
    try:
        user_uuid = uuid.UUID(user_id)
    except ValueError:
        return None
    
    # Fetch user from database
    result = await db.execute(select(User).where(User.id == user_uuid))
    user = result.scalar_one_or_none()
    
    if user is None or not user.is_active:
        return None
    
    return user


# Redis and cache dependencies
async def get_redis_client() -> RedisManager:
    """Get Redis client dependency."""
    return await get_redis()


async def get_cache() -> CacheService:
    """Get cache service dependency."""
    redis_client = await get_redis_client()
    return await get_cache_service(redis_client)


# Configuration dependencies
def get_app_settings() -> Settings:
    """Get application settings dependency."""
    return get_settings()


# Rate limiting dependencies
class RateLimiter:
    """Rate limiter dependency."""
    
    def __init__(self, requests: int, window: int = 60):
        self.requests = requests
        self.window = window
    
    async def __call__(
        self,
        request: Request,
        cache: CacheService = Depends(get_cache)
    ):
        """Check rate limit for request."""
        # Get client identifier
        client_ip = request.client.host if request.client else "unknown"
        user_id = getattr(request.state, "user_id", None)
        
        if user_id:
            identifier = f"user:{user_id}"
        else:
            identifier = f"ip:{client_ip}"
        
        # Check rate limit
        is_allowed, current_count, time_until_reset = await cache.check_rate_limit(
            identifier, self.requests, self.window
        )
        
        if not is_allowed:
            raise HTTPException(
                status_code=status.HTTP_429_TOO_MANY_REQUESTS,
                detail="Rate limit exceeded",
                headers={
                    "Retry-After": str(time_until_reset),
                    "X-RateLimit-Limit": str(self.requests),
                    "X-RateLimit-Remaining": "0",
                    "X-RateLimit-Reset": str(time_until_reset)
                }
            )
        
        return True


# Common rate limiters
rate_limit_auth = RateLimiter(requests=5, window=300)  # 5 requests per 5 minutes
rate_limit_api = RateLimiter(requests=100, window=60)  # 100 requests per minute
rate_limit_strict = RateLimiter(requests=10, window=60)  # 10 requests per minute


# Pagination dependencies
class PaginationParams:
    """Pagination parameters."""
    
    def __init__(self, skip: int = 0, limit: int = 20):
        self.skip = max(0, skip)
        self.limit = min(100, max(1, limit))  # Max 100 items per page


def get_pagination_params(skip: int = 0, limit: int = 20) -> PaginationParams:
    """Get pagination parameters dependency."""
    return PaginationParams(skip, limit)


# Health check dependencies
async def check_database_health(db: AsyncSession = Depends(get_db)) -> dict:
    """Check database health."""
    try:
        # Simple query to test database connectivity
        result = await db.execute("SELECT 1")
        await result.fetchone()
        return {"status": "healthy", "latency_ms": None}
    except Exception as e:
        logger.error("Database health check failed", error=str(e))
        return {"status": "unhealthy", "error": str(e)}


async def check_redis_health(redis: RedisManager = Depends(get_redis_client)) -> dict:
    """Check Redis health."""
    try:
        health_info = await redis.health_check()
        return health_info
    except Exception as e:
        logger.error("Redis health check failed", error=str(e))
        return {"status": "unhealthy", "error": str(e)}


async def check_system_health() -> dict:
    """Check system resource health."""
    try:
        from ..middleware.monitoring import get_system_metrics
        metrics = await get_system_metrics()
        
        # Determine health status based on resource usage
        cpu_usage = metrics.get("cpu", {}).get("usage_percent", 0)
        memory_usage = metrics.get("memory", {}).get("percent", 0)
        disk_usage = metrics.get("disk", {}).get("percent", 0)
        
        status = "healthy"
        warnings = []
        
        if cpu_usage > 80:
            status = "warning"
            warnings.append(f"High CPU usage: {cpu_usage}%")
        
        if memory_usage > 85:
            status = "warning"
            warnings.append(f"High memory usage: {memory_usage}%")
        
        if disk_usage > 90:
            status = "critical"
            warnings.append(f"High disk usage: {disk_usage}%")
        
        return {
            "status": status,
            "metrics": metrics,
            "warnings": warnings
        }
    except Exception as e:
        logger.error("System health check failed", error=str(e))
        return {"status": "unhealthy", "error": str(e)}