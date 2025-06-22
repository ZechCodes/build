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
        session_id = "sess_12345678-1234-5678-9abc-123456789abc_1234567890"
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
        assert buffer_call[0][0] == f"session:buffer:{session_id}"
        stored_data = buffer_call[1]["mapping"]
        assert stored_data["user_id"] == user_id
        assert stored_data["cursor_x"] == 10
        assert stored_data["cursor_y"] == 5
        assert stored_data["scroll_position"] == 2
        
        # Verify metadata was stored
        assert metadata_call[0][0] == f"session:meta:{session_id}"
        metadata = metadata_call[1]["mapping"]
        assert metadata["user_id"] == user_id
        assert metadata["session_id"] == session_id
    
    async def test_retrieve_buffer_success(self, redis_mock):
        """Test successful buffer retrieval"""
        # Arrange
        buffer_manager = SessionBufferManager(redis_mock)
        session_id = "sess_12345678-1234-5678-9abc-123456789abc_1234567890"
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
        redis_mock.hgetall.assert_called_with(f"session:buffer:{session_id}")
    
    async def test_buffer_compression(self, redis_mock):
        """Test buffer compression for large data"""
        # Arrange
        buffer_manager = SessionBufferManager(redis_mock)
        session_id = "sess_12345678-1234-5678-9abc-123456789abc_1234567890"
        user_id = "user_456"
        # Large buffer data (over compression threshold of 8KB)
        large_data = b"A" * 10000  # 10KB of data
        cursor_pos = (0, 0)
        
        # Mock rate limiting to pass
        buffer_manager.redis.pipeline.return_value = buffer_manager.redis
        buffer_manager.redis.execute = AsyncMock(return_value=[None, 5, None, None])
        
        # Act
        result = await buffer_manager.store_buffer(
            session_id, user_id, large_data, cursor_pos
        )
        
        # Assert
        assert result is True
        
        # Verify compression was applied (stored data should be smaller than original)
        hset_calls = redis_mock.hset.call_args_list
        buffer_call = hset_calls[0]
        stored_data = buffer_call[1]["mapping"]
        
        # Check that compressed flag was set
        assert stored_data["compressed"] == "true"
        # Compressed data should be smaller than original
        assert len(stored_data["buffer_data"]) < len(large_data)
    
    async def test_buffer_size_limits(self, redis_mock):
        """Test buffer size limit enforcement"""
        # Arrange
        buffer_manager = SessionBufferManager(redis_mock)
        session_id = "sess_12345678-1234-5678-9abc-123456789abc_1234567890"
        user_id = "user_456"
        # Buffer data larger than 1MB limit
        oversized_data = b"X" * (1024 * 1024 + 1000)  # 1MB + 1000 bytes
        cursor_pos = (0, 0)
        
        # Mock rate limiting to pass
        buffer_manager.redis.pipeline.return_value = buffer_manager.redis
        buffer_manager.redis.execute = AsyncMock(return_value=[None, 5, None, None])
        
        # Act
        result = await buffer_manager.store_buffer(
            session_id, user_id, oversized_data, cursor_pos
        )
        
        # Assert
        assert result is True
        
        # Verify buffer was truncated
        hset_calls = redis_mock.hset.call_args_list
        buffer_call = hset_calls[0]
        stored_data = buffer_call[1]["mapping"]
        
        # Stored data should be smaller than original due to truncation
        if stored_data["compressed"] == "true":
            # If compressed, decompress to check actual size
            import gzip
            actual_data = gzip.decompress(stored_data["buffer_data"])
        else:
            actual_data = stored_data["buffer_data"]
        
        # Should be within the 1MB limit
        assert len(actual_data) <= 1024 * 1024
    
    async def test_cross_user_buffer_access_prevention(self, redis_mock):
        """Test prevention of cross-user buffer access"""
        # Arrange
        buffer_manager = SessionBufferManager(redis_mock)
        session_id = "sess_12345678-1234-5678-9abc-123456789abc_1234567890"
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
        # Arrange
        buffer_manager = SessionBufferManager(redis_mock)
        session_id = "sess_12345678-1234-5678-9abc-123456789abc_1234567890"
        user_id = "user_456"
        buffer_data = b"line 1\nline 2\nline 3\nline 4"
        cursor_pos = (5, 2)
        scroll_pos = 1
        
        # Mock rate limiting to pass
        buffer_manager.redis.pipeline.return_value = buffer_manager.redis
        buffer_manager.redis.execute = AsyncMock(return_value=[None, 5, None, None])
        
        # Act
        result = await buffer_manager.store_buffer(
            session_id, user_id, buffer_data, cursor_pos, scroll_pos
        )
        
        # Assert
        assert result is True
        
        # Verify metadata was stored correctly
        hset_calls = redis_mock.hset.call_args_list
        buffer_call = hset_calls[0]
        metadata_call = hset_calls[1]
        
        buffer_data_stored = buffer_call[1]["mapping"]
        metadata_stored = metadata_call[1]["mapping"]
        
        # Check buffer metadata
        assert buffer_data_stored["size_bytes"] == len(buffer_data)
        assert buffer_data_stored["line_count"] == 3  # 3 newlines = 4 lines - 1
        assert buffer_data_stored["cursor_x"] == 5
        assert buffer_data_stored["cursor_y"] == 2
        assert buffer_data_stored["scroll_position"] == 1
        
        # Check separate metadata storage
        assert metadata_stored["user_id"] == user_id
        assert metadata_stored["session_id"] == session_id
    
    async def test_buffer_expiration(self, redis_mock):
        """Test buffer automatic expiration"""
        # Arrange
        buffer_manager = SessionBufferManager(redis_mock)
        session_id = "sess_12345678-1234-5678-9abc-123456789abc_1234567890"
        user_id = "user_456"
        buffer_data = b"test data"
        cursor_pos = (0, 0)
        
        # Mock rate limiting to pass
        buffer_manager.redis.pipeline.return_value = buffer_manager.redis
        buffer_manager.redis.execute = AsyncMock(return_value=[None, 5, None, None])
        
        # Act
        result = await buffer_manager.store_buffer(
            session_id, user_id, buffer_data, cursor_pos
        )
        
        # Assert
        assert result is True
        
        # Verify expiration was set
        expire_calls = redis_mock.expire.call_args_list
        assert len(expire_calls) >= 2  # At least buffer and metadata expiration
        
        # Find the 24-hour expiration calls (filter out rate limiting calls)
        day_expire_calls = [call for call in expire_calls if call[0][1] == 86400]
        assert len(day_expire_calls) >= 2  # Buffer and metadata both have 24-hour expiration
        
        # Verify the keys are correct
        day_expire_keys = [call[0][0] for call in day_expire_calls]
        assert f"session:buffer:{session_id}" in day_expire_keys
        assert f"session:meta:{session_id}" in day_expire_keys
    
    async def test_clear_buffer(self, redis_mock):
        """Test buffer clearing functionality"""
        # Arrange
        buffer_manager = SessionBufferManager(redis_mock)
        session_id = "sess_12345678-1234-5678-9abc-123456789abc_1234567890"
        user_id = "user_456"
        
        # Mock retrieve_buffer to return a valid buffer (so clear can proceed)
        redis_mock.hgetall.return_value = {
            b"user_id": b"user_456",
            b"buffer_data": b"test data",
            b"cursor_x": b"0",
            b"cursor_y": b"0",
            b"scroll_position": b"0",
            b"last_updated": b"1234567890.0",
            b"size_bytes": b"9",
            b"line_count": b"0",
            b"compressed": b"false"
        }
        
        # Mock pipeline for deletion
        redis_mock.pipeline.return_value = redis_mock
        redis_mock.execute = AsyncMock(return_value=[None, None])
        
        # Act
        result = await buffer_manager.clear_buffer(session_id, user_id)
        
        # Assert
        assert result is True
        
        # Verify Redis delete operations were called through pipeline
        delete_calls = redis_mock.delete.call_args_list
        assert len(delete_calls) >= 2  # Buffer and metadata deletion
        
        # Check that correct keys were deleted
        deleted_keys = [call[0][0] for call in delete_calls]
        expected_buffer_key = f"session:buffer:{session_id}"
        expected_metadata_key = f"session:meta:{session_id}"
        
        assert expected_buffer_key in deleted_keys
        assert expected_metadata_key in deleted_keys
        
    async def test_buffer_write_rate_limiting(self, redis_mock):
        """Test buffer write rate limiting functionality"""
        # Arrange
        buffer_manager = SessionBufferManager(redis_mock)
        user_id = "rate_limit_test_user"
        session_id = "sess_12345678-1234-5678-9abc-123456789abc_1234567890"
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