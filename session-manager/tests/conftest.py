"""
Test configuration and fixtures for session manager tests
"""
import pytest
import asyncio
from unittest.mock import AsyncMock, MagicMock
import redis.asyncio as redis


@pytest.fixture
def event_loop():
    """Create an instance of the default event loop for the test session."""
    loop = asyncio.get_event_loop_policy().new_event_loop()
    yield loop
    loop.close()


@pytest.fixture
async def redis_mock():
    """Mock Redis client for testing"""
    redis_client = AsyncMock(spec=redis.Redis)
    
    # Configure common Redis operations
    redis_client.hset = AsyncMock()
    redis_client.hgetall = AsyncMock()
    redis_client.get = AsyncMock()
    redis_client.set = AsyncMock()
    redis_client.delete = AsyncMock()
    redis_client.expire = AsyncMock()
    redis_client.pipeline = MagicMock()
    redis_client.sadd = AsyncMock()
    redis_client.srem = AsyncMock()
    redis_client.smembers = AsyncMock()
    redis_client.scan_iter = AsyncMock()
    redis_client.keys = AsyncMock()
    
    # Pipeline mock
    pipe_mock = AsyncMock()
    pipe_mock.hset = AsyncMock()
    pipe_mock.expire = AsyncMock()
    pipe_mock.sadd = AsyncMock()
    pipe_mock.srem = AsyncMock()
    pipe_mock.delete = AsyncMock()
    pipe_mock.execute = AsyncMock()
    redis_client.pipeline.return_value = pipe_mock
    
    return redis_client


@pytest.fixture
def sample_session_data():
    """Sample session data for testing"""
    return {
        "session_id": "session_123",
        "user_id": "user_456", 
        "vm_id": "vm_789",
        "terminal_size": (80, 24),
        "environment_vars": {"TERM": "xterm-256color"},
        "working_directory": "/home/user"
    }


@pytest.fixture
async def state_manager_with_cleanup(redis_mock):
    """SessionStateManager with proper cleanup"""
    import sys
    import os
    
    # Add the session-manager directory to the path  
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..'))
    
    from core.state_manager import SessionStateManager
    
    state_manager = SessionStateManager(redis_mock)
    await state_manager.initialize()
    
    yield state_manager
    
    # Cleanup
    await state_manager.stop()