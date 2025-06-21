"""
Unit tests for SessionBufferManager
"""
import pytest
import asyncio
import time
import gzip
from unittest.mock import AsyncMock, MagicMock

import sys
import os

# Add the session-manager directory to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '../..'))

from persistence.buffer_manager import SessionBufferManager, SessionBuffer


class TestSessionBufferManager:
    """Test suite for SessionBufferManager"""
    
    async def test_store_buffer_success(self, redis_mock):
        """Test successful buffer storage"""
        # Arrange
        buffer_manager = SessionBufferManager(redis_mock)
        session_id = "session_123"
        user_id = "user_456"
        buffer_data = b"terminal output data\nline 2\nline 3"
        cursor_pos = (10, 5)
        scroll_pos = 2
        
        # Act
        result = await buffer_manager.store_buffer(
            session_id, user_id, buffer_data, cursor_pos, scroll_pos
        )
        
        # Assert
        assert result is True
        
        # Verify Redis operations were called
        redis_mock.hset.assert_called()
        redis_mock.expire.assert_called()
        
        # Check the data stored
        hset_calls = redis_mock.hset.call_args_list
        buffer_call = hset_calls[0]
        metadata_call = hset_calls[1]
        
        # Verify buffer data was stored
        assert buffer_call[0][0] == "session:buffer:session_123"
        stored_data = buffer_call[1]["mapping"]
        assert stored_data["user_id"] == user_id
        assert stored_data["cursor_x"] == 10
        assert stored_data["cursor_y"] == 5
        assert stored_data["scroll_position"] == 2
        
        # Verify metadata was stored
        assert metadata_call[0][0] == "session:meta:session_123"
        metadata = metadata_call[1]["mapping"]
        assert metadata["user_id"] == user_id
        assert metadata["session_id"] == session_id
    
    async def test_retrieve_buffer_success(self, redis_mock):
        """Test successful buffer retrieval"""
        # Arrange
        buffer_manager = SessionBufferManager(redis_mock)
        session_id = "session_123"
        user_id = "user_456"
        buffer_data = b"terminal output data"
        
        # Mock Redis response
        redis_mock.hgetall.return_value = {
            b"user_id": b"user_456",
            b"buffer_data": buffer_data,
            b"cursor_x": b"10",
            b"cursor_y": b"5",
            b"scroll_position": b"0",
            b"last_updated": b"1234567890.0",
            b"size_bytes": b"19",
            b"line_count": b"1",
            b"compressed": b"false"
        }
        
        # Act
        buffer = await buffer_manager.retrieve_buffer(session_id, user_id)
        
        # Assert
        assert buffer is not None
        assert buffer.session_id == session_id
        assert buffer.user_id == user_id
        assert buffer.buffer_data == buffer_data
        assert buffer.cursor_position == (10, 5)
        assert buffer.scroll_position == 0
        
        # Verify Redis was called correctly
        redis_mock.hgetall.assert_called_with("session:buffer:session_123")
    
    async def test_buffer_compression(self, redis_mock):
        """Test buffer compression for large data"""
        pytest.skip("SessionBufferManager not implemented yet - Red phase of TDD")
        
        # This test will verify compression is applied for large buffers
    
    async def test_buffer_size_limits(self, redis_mock):
        """Test buffer size limit enforcement"""
        pytest.skip("SessionBufferManager not implemented yet - Red phase of TDD")
        
        # This test will verify large buffers are truncated
    
    async def test_cross_user_buffer_access_prevention(self, redis_mock):
        """Test prevention of cross-user buffer access"""
        # Arrange
        buffer_manager = SessionBufferManager(redis_mock)
        session_id = "session_123"
        owner_user_id = "user_456"
        unauthorized_user_id = "user_999"
        
        # Mock Redis response with buffer owned by user_456
        redis_mock.hgetall.return_value = {
            b"user_id": b"user_456",  # Owner
            b"buffer_data": b"sensitive data",
            b"cursor_x": b"10",
            b"cursor_y": b"5",
            b"scroll_position": b"0",
            b"last_updated": b"1234567890.0",
            b"size_bytes": b"14",
            b"line_count": b"1",
            b"compressed": b"false"
        }
        
        # Act - Try to access with wrong user
        buffer = await buffer_manager.retrieve_buffer(session_id, unauthorized_user_id)
        
        # Assert - Should deny access
        assert buffer is None
        
        # Act - Access with correct user
        buffer = await buffer_manager.retrieve_buffer(session_id, owner_user_id)
        
        # Assert - Should allow access
        assert buffer is not None
        assert buffer.user_id == owner_user_id
    
    async def test_buffer_metadata_tracking(self, redis_mock):
        """Test buffer metadata is properly tracked"""
        pytest.skip("SessionBufferManager not implemented yet - Red phase of TDD")
        
        # This test will verify metadata like size, line count, etc.
    
    async def test_buffer_expiration(self, redis_mock):
        """Test buffer automatic expiration"""
        pytest.skip("SessionBufferManager not implemented yet - Red phase of TDD")
        
        # This test will verify buffers expire automatically
    
    async def test_clear_buffer(self, redis_mock):
        """Test buffer clearing functionality"""
        pytest.skip("SessionBufferManager not implemented yet - Red phase of TDD")
        
    async def test_buffer_write_rate_limiting(self, redis_mock):
        """Test buffer write rate limiting functionality"""
        # Arrange
        buffer_manager = SessionBufferManager(redis_mock)
        user_id = "rate_limit_test_user"
        session_id = "rate_limit_session"
        test_data = b"test buffer data"
        
        # Mock Redis pipeline operations for rate limiting
        buffer_manager.redis.pipeline.return_value = buffer_manager.redis
        buffer_manager.redis.zremrangebyscore = AsyncMock()
        buffer_manager.redis.zcard = AsyncMock(return_value=5)  # Under limit
        buffer_manager.redis.zadd = AsyncMock()
        buffer_manager.redis.expire = AsyncMock()
        buffer_manager.redis.execute = AsyncMock(return_value=[None, 5, None, None])
        
        # Should allow writes under rate limit
        result = await buffer_manager.store_buffer(
            session_id=session_id,
            user_id=user_id,
            buffer_data=test_data,
            cursor_pos=(0, 1)
        )
        assert result is True
        
        # Test rate limit exceeded
        buffer_manager.redis.zcard = AsyncMock(return_value=65)  # Over limit
        buffer_manager.redis.execute = AsyncMock(return_value=[None, 65, None, None])
        
        # Should raise rate limit error
        with pytest.raises(ValueError, match="Buffer write rate limit exceeded"):
            await buffer_manager.store_buffer(
                session_id=session_id,
                user_id=user_id,
                buffer_data=test_data,
                cursor_pos=(0, 1)
            )
    
    async def test_rate_limit_check_method(self, redis_mock):
        """Test the rate limit check method directly"""
        # Arrange
        buffer_manager = SessionBufferManager(redis_mock)
        user_id = "direct_test_user"
        
        # Mock Redis operations
        buffer_manager.redis.pipeline.return_value = buffer_manager.redis
        buffer_manager.redis.execute = AsyncMock(return_value=[None, 30, None, None])
        
        # Should allow under limit
        result = await buffer_manager._check_buffer_write_rate_limit(user_id)
        assert result is True
        
        # Should block over limit
        buffer_manager.redis.execute = AsyncMock(return_value=[None, 70, None, None])
        result = await buffer_manager._check_buffer_write_rate_limit(user_id)
        assert result is False
    
    async def test_rate_limit_error_handling(self, redis_mock):
        """Test rate limit check handles Redis errors gracefully"""
        # Arrange
        buffer_manager = SessionBufferManager(redis_mock)
        user_id = "error_test_user"
        
        # Mock Redis failure
        buffer_manager.redis.pipeline.side_effect = Exception("Redis connection failed")
        
        # Should fail open (allow operation) when rate limiting fails
        result = await buffer_manager._check_buffer_write_rate_limit(user_id)
        assert result is True
        
        # This test will verify buffers can be cleared