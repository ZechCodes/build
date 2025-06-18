"""Main FastAPI application."""

import structlog
from contextlib import asynccontextmanager
from fastapi import FastAPI, HTTPException, Depends
from fastapi.middleware.cors import CORSMiddleware
from fastapi.middleware.trustedhost import TrustedHostMiddleware

from app.api.v1.api import api_router
from app.core.config import get_settings
from app.core.logging import configure_logging
from app.core.redis import redis_manager, get_redis
from app.services.cache import get_cache_service

# Configure structured logging
configure_logging()
logger = structlog.get_logger(__name__)

# Get application settings
settings = get_settings()


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Application lifespan manager."""
    # Startup
    logger.info("Starting Build Platform API", version="0.1.0")
    
    try:
        # Connect to Redis
        await redis_manager.connect()
        logger.info("Redis connection established")
    except Exception as e:
        logger.error("Failed to connect to Redis", error=str(e))
        # Don't fail startup - Redis is important but not critical for basic functionality
    
    yield
    
    # Shutdown
    logger.info("Shutting down Build Platform API")
    try:
        await redis_manager.disconnect()
        logger.info("Redis connection closed")
    except Exception as e:
        logger.error("Error closing Redis connection", error=str(e))


# Create FastAPI application
app = FastAPI(
    title="Build Platform API",
    description="API for the Build platform - isolated development environments",
    version="0.1.0",
    openapi_url="/api/v1/openapi.json" if settings.environment != "production" else None,
    docs_url="/docs" if settings.enable_swagger_ui else None,
    redoc_url="/redoc" if settings.enable_redoc else None,
    lifespan=lifespan,
)

# Add middleware
if settings.enable_cors:
    app.add_middleware(
        CORSMiddleware,
        allow_origins=settings.allowed_origins,
        allow_credentials=True,
        allow_methods=["*"],
        allow_headers=["*"],
    )

# Add trusted host middleware for security
app.add_middleware(
    TrustedHostMiddleware,
    allowed_hosts=["localhost", "127.0.0.1", "*.build-platform.dev"],
)

# Include API router
app.include_router(api_router, prefix="/api/v1")




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
async def detailed_health_check(redis: redis_manager = Depends(get_redis)):
    """Detailed health check including all dependencies."""
    health_status = {
        "status": "healthy",
        "service": "build-api",
        "version": "0.1.0",
        "timestamp": structlog.stdlib.BoundLogger().info("health_check"),
        "dependencies": {}
    }
    
    # Check Redis
    try:
        redis_health = await redis.health_check()
        health_status["dependencies"]["redis"] = redis_health
        if not redis_health.get("connected", False):
            health_status["status"] = "degraded"
    except Exception as e:
        health_status["dependencies"]["redis"] = {
            "status": "unhealthy",
            "error": str(e)
        }
        health_status["status"] = "degraded"
    
    # Check database would go here
    # health_status["dependencies"]["database"] = await check_database()
    
    return health_status


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(
        "app.main:app",
        host=settings.api_host,
        port=settings.api_port,
        reload=settings.debug,
        log_level=settings.log_level.lower(),
    )