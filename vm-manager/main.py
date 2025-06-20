"""VM Manager Service - Main entry point for Firecracker VM management."""

import asyncio
import signal
import sys
from pathlib import Path
from typing import Optional

import structlog
import uvicorn
from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware

from config.settings import settings
from monitoring.health_checker import VMHealthChecker
from services.vm_manager import VMManagerService

# Configure structured logging
structlog.configure(
    processors=[
        structlog.stdlib.filter_by_level,
        structlog.stdlib.add_logger_name,
        structlog.stdlib.add_log_level,
        structlog.stdlib.PositionalArgumentsFormatter(),
        structlog.processors.StackInfoRenderer(),
        structlog.processors.format_exc_info,
        structlog.processors.UnicodeDecoder(),
        structlog.processors.JSONRenderer()
    ],
    context_class=dict,
    logger_factory=structlog.stdlib.LoggerFactory(),
    wrapper_class=structlog.stdlib.BoundLogger,
    cache_logger_on_first_use=True,
)

logger = structlog.get_logger(__name__)

# Global service instances
vm_manager: Optional[VMManagerService] = None
health_checker: Optional[VMHealthChecker] = None

app = FastAPI(
    title="VM Manager Service",
    description="Firecracker VM lifecycle management service",
    version="1.0.0",
    docs_url="/docs" if settings.debug else None,
    redoc_url="/redoc" if settings.debug else None,
)

# CORS middleware for development
if settings.debug:
    app.add_middleware(
        CORSMiddleware,
        allow_origins=["http://localhost:3000", "http://127.0.0.1:3000"],
        allow_credentials=True,
        allow_methods=["*"],
        allow_headers=["*"],
    )


@app.on_event("startup")
async def startup_event():
    """Initialize services on startup."""
    global vm_manager, health_checker
    
    logger.info("Starting VM Manager Service", version="1.0.0")
    
    try:
        # Initialize VM manager service
        vm_manager = VMManagerService()
        await vm_manager.initialize()
        
        # Initialize health checker
        health_checker = VMHealthChecker(check_interval=settings.health_check_interval)
        
        # Start health monitoring in background
        asyncio.create_task(health_checker.start_monitoring())
        
        logger.info("VM Manager Service started successfully")
        
    except Exception as e:
        logger.error("Failed to start VM Manager Service", error=str(e))
        raise


@app.on_event("shutdown")
async def shutdown_event():
    """Cleanup on shutdown."""
    global vm_manager, health_checker
    
    logger.info("Shutting down VM Manager Service")
    
    try:
        # Stop health monitoring
        if health_checker:
            await health_checker.stop_monitoring()
        
        # Cleanup VM manager
        if vm_manager:
            await vm_manager.cleanup()
        
        logger.info("VM Manager Service shutdown complete")
        
    except Exception as e:
        logger.error("Error during shutdown", error=str(e))


@app.get("/health")
async def health_check():
    """Health check endpoint."""
    return {
        "status": "healthy",
        "service": "vm-manager",
        "version": "1.0.0"
    }


@app.get("/")
async def root():
    """Root endpoint."""
    return {
        "service": "VM Manager",
        "description": "Firecracker VM lifecycle management service",
        "version": "1.0.0"
    }


def handle_signal(sig_num: int, frame):
    """Handle shutdown signals gracefully."""
    logger.info("Received shutdown signal", signal=sig_num)
    sys.exit(0)


async def main():
    """Main function to run the service."""
    # Setup signal handlers
    signal.signal(signal.SIGINT, handle_signal)
    signal.signal(signal.SIGTERM, handle_signal)
    
    # Run the service
    config = uvicorn.Config(
        app,
        host=settings.host,
        port=settings.port,
        log_level=settings.log_level.lower(),
        access_log=settings.debug,
        reload=settings.debug,
    )
    
    server = uvicorn.Server(config)
    await server.serve()


if __name__ == "__main__":
    asyncio.run(main())