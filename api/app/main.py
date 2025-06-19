"""Main FastAPI application."""

import time
import structlog
from contextlib import asynccontextmanager
from fastapi import FastAPI, HTTPException, Depends, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.middleware.trustedhost import TrustedHostMiddleware
from fastapi.responses import PlainTextResponse

try:
    from prometheus_client import generate_latest, CONTENT_TYPE_LATEST
    PROMETHEUS_AVAILABLE = True
except ImportError:
    PROMETHEUS_AVAILABLE = False

try:
    from app.api.v1.api import api_router
    API_ROUTER_AVAILABLE = True
except ImportError as e:
    # logger not available yet at module level
    print(f"Warning: API router not available - {e}")
    API_ROUTER_AVAILABLE = False
from app.core.config import get_settings
from app.core.logging import configure_logging
from app.core.redis import redis_manager, get_redis
from app.core.deps import (
    check_database_health, 
    check_redis_health, 
    check_system_health,
    get_cache
)
from app.services.cache import get_cache_service

# Import monitoring components
try:
    from app.monitoring.logfire_setup import (
        setup_logfire_monitoring, 
        setup_structured_logging,
        LOGFIRE_AVAILABLE
    )
    from app.monitoring.logfire_middleware import (
        LogfireTrackingMiddleware,
        LogfirePerformanceMiddleware,
        LogfireUserActivityMiddleware
    )
    from app.monitoring.metrics import start_metrics_collection_task
    LOGFIRE_MONITORING_AVAILABLE = True
except ImportError as e:
    print(f"Warning: Logfire monitoring not available - {e}")
    LOGFIRE_MONITORING_AVAILABLE = False

# Import enhanced security middleware
try:
    from app.middleware.security_enhanced import (
        EnhancedSecurityHeadersMiddleware,
        AdvancedRateLimitMiddleware,
        InputValidationMiddleware,
        SecurityMonitoringMiddleware
    )
    ENHANCED_SECURITY_AVAILABLE = True
except ImportError as e:
    print(f"Warning: Enhanced security middleware not available - {e}")
    ENHANCED_SECURITY_AVAILABLE = False

try:
    from app.middleware.security import (
        SecurityHeadersMiddleware,
        RateLimitMiddleware, 
        RequestLoggingMiddleware,
        UserContextMiddleware,
        IPWhitelistMiddleware,
        RequestSizeLimitMiddleware
    )
    SECURITY_MIDDLEWARE_AVAILABLE = True
except ImportError as e:
    print(f"Warning: Security middleware not available - {e}")
    SECURITY_MIDDLEWARE_AVAILABLE = False

try:
    from app.middleware.monitoring import (
        PrometheusMiddleware,
        HealthMetricsMiddleware,
        ErrorTrackingMiddleware,
        PerformanceMonitoringMiddleware
    )
    MONITORING_MIDDLEWARE_AVAILABLE = True
except ImportError as e:
    print(f"Warning: Monitoring middleware not available - {e}")
    MONITORING_MIDDLEWARE_AVAILABLE = False

# Configure structured logging
configure_logging()
logger = structlog.get_logger(__name__)

# Get application settings
settings = get_settings()


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Application lifespan manager."""
    # Startup
    logger.info("Starting Build Platform API", 
               version="0.1.0", 
               environment=settings.environment)
    
    # Setup Logfire monitoring
    if LOGFIRE_MONITORING_AVAILABLE:
        try:
            setup_logfire_monitoring(app)
            setup_structured_logging()
            logger.info("Logfire monitoring initialized")
        except Exception as e:
            logger.error("Failed to setup Logfire monitoring", error=str(e))
    
    try:
        # Connect to Redis
        await redis_manager.connect()
        logger.info("Redis connection established")
    except Exception as e:
        logger.error("Failed to connect to Redis", error=str(e))
        # Don't fail startup - Redis is important but not critical for basic functionality
    
    # Start metrics collection task
    if LOGFIRE_MONITORING_AVAILABLE:
        try:
            await start_metrics_collection_task()
            logger.info("Metrics collection task started")
        except Exception as e:
            logger.error("Failed to start metrics collection", error=str(e))
    
    # Log startup completion
    logger.info("Build Platform API startup completed")
    
    yield
    
    # Shutdown
    logger.info("Shutting down Build Platform API")
    try:
        await redis_manager.disconnect()
        logger.info("Redis connection closed")
    except Exception as e:
        logger.error("Error closing Redis connection", error=str(e))
    
    logger.info("Build Platform API shutdown completed")


# Create FastAPI application
app = FastAPI(
    title="Build Platform API",
    description="Cloud development environment platform with VM isolation",
    version="1.0.0",
    openapi_url="/api/v1/openapi.json" if settings.environment != "production" else None,
    docs_url="/docs" if settings.enable_swagger_ui else None,
    redoc_url="/redoc" if settings.enable_redoc else None,
    lifespan=lifespan,
)

# Add middleware in order (last added = first executed)

# Add Logfire middleware if available
if LOGFIRE_MONITORING_AVAILABLE:
    # 1. Logfire tracking (outermost for complete request tracing)
    app.add_middleware(LogfireTrackingMiddleware, include_request_body=False, include_response_body=False)
    
    # 2. Logfire performance monitoring
    app.add_middleware(LogfirePerformanceMiddleware, slow_request_threshold=1.0)
    
    # 3. Logfire user activity tracking
    app.add_middleware(LogfireUserActivityMiddleware, track_anonymous_users=True)

# Add enhanced security middleware if available
if ENHANCED_SECURITY_AVAILABLE:
    # 4. Enhanced security headers
    app.add_middleware(EnhancedSecurityHeadersMiddleware)
    
    # 5. Security monitoring and threat detection
    app.add_middleware(SecurityMonitoringMiddleware)
    
    # 6. Input validation and attack detection
    app.add_middleware(InputValidationMiddleware, max_body_size=50 * 1024 * 1024)
    
    # 7. Advanced rate limiting with attack detection
    app.add_middleware(AdvancedRateLimitMiddleware)

elif SECURITY_MIDDLEWARE_AVAILABLE and MONITORING_MIDDLEWARE_AVAILABLE:
    # Fallback to basic security middleware
    # 4. Security headers
    app.add_middleware(SecurityHeadersMiddleware)

    # 5. Request size limiting
    app.add_middleware(RequestSizeLimitMiddleware, max_size=50 * 1024 * 1024)  # 50MB

    # 6. Rate limiting (basic)
    app.add_middleware(RateLimitMiddleware)

else:
    logger.warning("Security middleware disabled due to missing dependencies")

# Add remaining middleware if available
if SECURITY_MIDDLEWARE_AVAILABLE and MONITORING_MIDDLEWARE_AVAILABLE:
    # 8. IP whitelist for admin endpoints
    if settings.environment == "production":
        app.add_middleware(IPWhitelistMiddleware, allowed_ips=["127.0.0.1"])

    # 9. Request logging and tracing
    app.add_middleware(RequestLoggingMiddleware)

    # 10. Performance monitoring
    app.add_middleware(PerformanceMonitoringMiddleware)

    # 11. Error tracking
    app.add_middleware(ErrorTrackingMiddleware)

    # 12. Health metrics collection
    app.add_middleware(HealthMetricsMiddleware)

    # 13. Prometheus metrics collection  
    app.add_middleware(PrometheusMiddleware)

    # 14. User context extraction
    app.add_middleware(UserContextMiddleware)

# 15. CORS middleware
if settings.enable_cors:
    app.add_middleware(
        CORSMiddleware,
        allow_origins=settings.allowed_origins,
        allow_credentials=True,
        allow_methods=["GET", "POST", "PUT", "DELETE", "OPTIONS"],
        allow_headers=["*"],
        expose_headers=["X-Request-ID", "X-Process-Time", "X-RateLimit-*"]
    )

# 16. Trusted host middleware (innermost, closest to routes)
app.add_middleware(
    TrustedHostMiddleware,
    allowed_hosts=[
        "localhost", 
        "127.0.0.1", 
        "*.getbuild.ing",
        "*.build-platform.dev"
    ] if settings.environment == "production" else ["*"]
)

# Include API router
if API_ROUTER_AVAILABLE:
    app.include_router(api_router, prefix="/api/v1")
else:
    logger.warning("API router not included due to import errors")




@app.get("/")
async def root():
    """Root endpoint."""
    return {
        "message": "Build Platform API",
        "version": "0.1.0",
        "status": "running",
        "docs": "/docs" if settings.enable_swagger_ui else None,
    }


@app.get("/health")
async def health_check():
    """Basic health check endpoint."""
    return {"status": "healthy", "service": "build-api"}


@app.get("/health/detailed")
async def detailed_health_check(
    db_health: dict = Depends(check_database_health),
    redis_health: dict = Depends(check_redis_health),
    system_health: dict = Depends(check_system_health)
):
    """Detailed health check including all dependencies."""
    import time
    
    # Determine overall status
    overall_status = "healthy"
    dependencies = {
        "database": db_health,
        "redis": redis_health,
        "system": system_health
    }
    
    # Check if any dependency is unhealthy
    for service, health in dependencies.items():
        if health.get("status") == "unhealthy":
            overall_status = "unhealthy"
            break
        elif health.get("status") in ["warning", "degraded"]:
            overall_status = "degraded"
    
    return {
        "status": overall_status,
        "service": "build-api",
        "version": "1.0.0",
        "environment": settings.environment,
        "timestamp": int(time.time()),
        "dependencies": dependencies
    }


@app.get("/metrics")
async def get_metrics():
    """Prometheus metrics endpoint."""
    if not PROMETHEUS_AVAILABLE:
        return {"error": "Prometheus client not available"}
    
    return PlainTextResponse(
        content=generate_latest(),
        media_type=CONTENT_TYPE_LATEST
    )


@app.get("/api/v1/health/system")
async def get_system_metrics(system_health: dict = Depends(check_system_health)):
    """Get detailed system metrics."""
    return system_health


@app.get("/api/v1/health/cache")
async def get_cache_metrics(cache = Depends(get_cache)):
    """Get cache performance metrics."""
    try:
        stats = await cache.get_cache_stats()
        return {"status": "healthy", "metrics": stats}
    except Exception as e:
        logger.error("Failed to get cache metrics", error=str(e))
        return {"status": "unhealthy", "error": str(e)}


@app.get("/api/v1/metrics/logfire")
async def get_logfire_metrics():
    """Get Logfire and monitoring metrics."""
    if not LOGFIRE_MONITORING_AVAILABLE:
        return {"error": "Logfire monitoring not available"}
    
    try:
        from app.monitoring.metrics import metrics_collector
        from app.monitoring.database_tracking import get_database_metrics
        
        metrics_summary = metrics_collector.get_all_metrics_summary()
        db_metrics = get_database_metrics()
        
        return {
            "status": "healthy",
            "logfire_enabled": LOGFIRE_MONITORING_AVAILABLE,
            "metrics": metrics_summary,
            "database": db_metrics,
            "timestamp": time.time()
        }
    except Exception as e:
        logger.error("Failed to get Logfire metrics", error=str(e))
        return {"status": "unhealthy", "error": str(e)}


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(
        "app.main:app",
        host=settings.api_host,
        port=settings.api_port,
        reload=settings.debug,
        log_level=settings.log_level.lower(),
    )