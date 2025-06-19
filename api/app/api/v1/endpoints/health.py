"""Health check endpoints."""

import time
import structlog
from fastapi import APIRouter, Depends, Response, status
from sqlalchemy.ext.asyncio import AsyncSession

from app.core.database import get_db_session
from app.core.deps import (
    check_database_health,
    check_redis_health, 
    check_system_health,
    get_cache
)
from app.core.config import get_settings

logger = structlog.get_logger(__name__)
router = APIRouter()
settings = get_settings()


@router.get("/")
async def health_check():
    """Basic health check for load balancers."""
    return {"status": "healthy", "service": "build-api", "timestamp": int(time.time())}


@router.get("/ready")
async def readiness_check(
    db_health: dict = Depends(check_database_health),
    redis_health: dict = Depends(check_redis_health)
):
    """Readiness check for Kubernetes."""
    # Service is ready if critical dependencies are healthy
    is_ready = (
        db_health.get("status") == "healthy" and 
        redis_health.get("status") in ["healthy", "degraded"]
    )
    
    if not is_ready:
        return Response(
            content='{"status": "not ready"}',
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
            media_type="application/json"
        )
    
    return {"status": "ready", "timestamp": int(time.time())}


@router.get("/live")
async def liveness_check():
    """Liveness check for Kubernetes."""
    # Simple check that the application is responding
    return {"status": "alive", "timestamp": int(time.time())}


@router.get("/detailed")
async def detailed_health_check(
    db_health: dict = Depends(check_database_health),
    redis_health: dict = Depends(check_redis_health),
    system_health: dict = Depends(check_system_health)
):
    """Comprehensive health check with all dependencies."""
    # Determine overall status
    overall_status = "healthy"
    dependencies = {
        "database": db_health,
        "redis": redis_health,
        "system": system_health
    }
    
    # Check if any critical dependency is unhealthy
    critical_services = ["database"]
    for service in critical_services:
        if dependencies[service].get("status") == "unhealthy":
            overall_status = "unhealthy"
            break
    
    # Check for warnings
    if overall_status == "healthy":
        for service, health in dependencies.items():
            if health.get("status") in ["warning", "degraded"]:
                overall_status = "degraded"
                break
    
    response_data = {
        "status": overall_status,
        "service": "build-api",
        "version": "1.0.0",
        "environment": settings.environment,
        "timestamp": int(time.time()),
        "dependencies": dependencies
    }
    
    # Return appropriate HTTP status code
    if overall_status == "unhealthy":
        return Response(
            content=response_data,
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
            media_type="application/json"
        )
    elif overall_status == "degraded":
        return Response(
            content=response_data,
            status_code=status.HTTP_200_OK,
            media_type="application/json",
            headers={"X-Health-Status": "degraded"}
        )
    
    return response_data


@router.get("/cache")
async def cache_health_check(cache = Depends(get_cache)):
    """Cache-specific health check."""
    try:
        stats = await cache.get_cache_stats()
        
        # Analyze cache performance
        hit_ratio = 0
        if stats.get("keyspace_hits", 0) + stats.get("keyspace_misses", 0) > 0:
            hit_ratio = stats["keyspace_hits"] / (stats["keyspace_hits"] + stats["keyspace_misses"])
        
        cache_status = "healthy"
        warnings = []
        
        if hit_ratio < 0.8:  # Less than 80% hit ratio
            cache_status = "warning"
            warnings.append(f"Low cache hit ratio: {hit_ratio:.2%}")
        
        memory_usage = stats.get("memory_usage", {})
        if memory_usage.get("used_memory", 0) > 0.9 * memory_usage.get("maxmemory", float('inf')):
            cache_status = "warning"
            warnings.append("High memory usage")
        
        return {
            "status": cache_status,
            "hit_ratio": hit_ratio,
            "metrics": stats,
            "warnings": warnings,
            "timestamp": int(time.time())
        }
    except Exception as e:
        logger.error("Cache health check failed", error=str(e))
        return Response(
            content=f'{{"status": "unhealthy", "error": "{str(e)}"}}',
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
            media_type="application/json"
        )