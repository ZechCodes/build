#!/usr/bin/env python3
"""Simple development server for testing WebSocket functionality."""

import os
import asyncio
import tempfile
from pathlib import Path

# Set minimal environment variables for development
os.environ.update({
    "DATABASE_URL": f"sqlite+aiosqlite:///{tempfile.gettempdir()}/build_dev.db",
    "REDIS_URL": "redis://localhost:6379/0",  # We'll mock this
    "REDIS_PASSWORD": "dev_password",
    "JWT_SECRET": "your-super-secret-jwt-key-for-development-only",
    "MINIO_ENDPOINT": "localhost:9000",
    "MINIO_ACCESS_KEY": "minioadmin", 
    "MINIO_SECRET_KEY": "minioadmin123",
    "ENVIRONMENT": "development",
    "DEBUG": "true",
    "LOG_LEVEL": "DEBUG"
})

# Mock Redis for development
class MockRedis:
    async def ping(self):
        return True
    
    async def health_check(self):
        return {"status": "healthy"}
    
    async def close(self):
        pass

# Patch Redis manager to use mock
import app.core.redis
app.core.redis.redis_manager = type('MockRedisManager', (), {
    'connect': lambda: None,
    'disconnect': lambda: None,
    'get_redis': lambda: MockRedis()
})()

if __name__ == "__main__":
    import uvicorn
    
    print("🚀 Starting Build Platform API in development mode")
    print("📱 WebSocket endpoint: ws://localhost:8000/ws/terminal")
    print("🔧 Using SQLite database for development")
    print("⚠️  Redis mocked for development")
    
    uvicorn.run(
        "app.main:app",
        host="0.0.0.0",
        port=8000,
        reload=True,
        log_level="debug"
    )