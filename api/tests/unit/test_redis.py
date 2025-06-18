"""Tests for Redis functionality."""

import pytest
import uuid
from unittest.mock import AsyncMock, MagicMock

from app.core.redis import RedisManager
from app.services.cache import CacheService


class TestRedisManager:
    """Test Redis manager functionality."""
    
    @pytest.fixture
    def redis_manager(self):
        """Create Redis manager for testing."""
        manager = RedisManager()
        # Mock the client for unit tests
        manager.client = AsyncMock()
        return manager
    
    @pytest.mark.asyncio
    async def test_ping(self, redis_manager):
        """Test Redis ping."""
        redis_manager.client.ping.return_value = True
        result = await redis_manager.ping()
        assert result is True
        redis_manager.client.ping.assert_called_once()
    
    @pytest.mark.asyncio
    async def test_ping_failure(self, redis_manager):
        """Test Redis ping failure."""
        redis_manager.client.ping.side_effect = Exception("Connection failed")
        result = await redis_manager.ping()
        assert result is False
    
    @pytest.mark.asyncio
    async def test_get_set(self, redis_manager):
        """Test Redis get and set operations."""
        # Test set
        redis_manager.client.set.return_value = True
        result = await redis_manager.set("test_key", "test_value")
        assert result is True
        redis_manager.client.set.assert_called_with("test_key", "test_value", ex=None, px=None, nx=False, xx=False)
        
        # Test get
        redis_manager.client.get.return_value = "test_value"
        result = await redis_manager.get("test_key")
        assert result == "test_value"
        redis_manager.client.get.assert_called_with("test_key")
    
    @pytest.mark.asyncio
    async def test_delete(self, redis_manager):
        """Test Redis delete operation."""
        redis_manager.client.delete.return_value = 1
        result = await redis_manager.delete("test_key")
        assert result == 1
        redis_manager.client.delete.assert_called_with("test_key")
    
    @pytest.mark.asyncio
    async def test_exists(self, redis_manager):
        """Test Redis exists operation."""
        redis_manager.client.exists.return_value = 1
        result = await redis_manager.exists("test_key")
        assert result == 1
        redis_manager.client.exists.assert_called_with("test_key")
    
    @pytest.mark.asyncio
    async def test_expire(self, redis_manager):
        """Test Redis expire operation."""
        redis_manager.client.expire.return_value = True
        result = await redis_manager.expire("test_key", 3600)
        assert result is True
        redis_manager.client.expire.assert_called_with("test_key", 3600)
    
    @pytest.mark.asyncio
    async def test_hash_operations(self, redis_manager):
        """Test Redis hash operations."""
        # Test hset
        redis_manager.client.hset.return_value = 1
        result = await redis_manager.hset("test_hash", "field1", "value1")
        assert result == 1
        
        # Test hget
        redis_manager.client.hget.return_value = "value1"
        result = await redis_manager.hget("test_hash", "field1")
        assert result == "value1"
        
        # Test hgetall
        redis_manager.client.hgetall.return_value = {"field1": "value1", "field2": "value2"}
        result = await redis_manager.hgetall("test_hash")
        assert result == {"field1": "value1", "field2": "value2"}
        
        # Test hdel
        redis_manager.client.hdel.return_value = 1
        result = await redis_manager.hdel("test_hash", "field1")
        assert result == 1
    
    @pytest.mark.asyncio
    async def test_set_operations(self, redis_manager):
        """Test Redis set operations."""
        # Test sadd
        redis_manager.client.sadd.return_value = 2
        result = await redis_manager.sadd("test_set", "member1", "member2")
        assert result == 2
        
        # Test smembers
        redis_manager.client.smembers.return_value = {"member1", "member2"}
        result = await redis_manager.smembers("test_set")
        assert result == {"member1", "member2"}
        
        # Test sismember
        redis_manager.client.sismember.return_value = True
        result = await redis_manager.sismember("test_set", "member1")
        assert result is True
        
        # Test srem
        redis_manager.client.srem.return_value = 1
        result = await redis_manager.srem("test_set", "member1")
        assert result == 1
    
    @pytest.mark.asyncio
    async def test_incr_decr(self, redis_manager):
        """Test Redis increment and decrement operations."""
        # Test incr
        redis_manager.client.incr.return_value = 5
        result = await redis_manager.incr("counter", 2)
        assert result == 5
        redis_manager.client.incr.assert_called_with("counter", 2)
        
        # Test decr
        redis_manager.client.decr.return_value = 3
        result = await redis_manager.decr("counter", 1)
        assert result == 3
        redis_manager.client.decr.assert_called_with("counter", 1)
    
    @pytest.mark.asyncio
    async def test_ttl(self, redis_manager):
        """Test Redis TTL operation."""
        redis_manager.client.ttl.return_value = 3600
        result = await redis_manager.ttl("test_key")
        assert result == 3600
        redis_manager.client.ttl.assert_called_with("test_key")
    
    @pytest.mark.asyncio
    async def test_health_check(self, redis_manager):
        """Test Redis health check."""
        redis_manager.client.ping.return_value = True
        redis_manager.client.info.side_effect = [
            {"used_memory": 1024, "redis_version": "7.0.0", "uptime_in_seconds": 86400},
            {"db0": "keys=100,expires=0,avg_ttl=0"}
        ]
        
        # Mock get_memory_usage method
        async def mock_get_memory_usage():
            return {"used_memory": 1024, "used_memory_human": "1KB"}
        
        redis_manager.get_memory_usage = mock_get_memory_usage
        
        result = await redis_manager.health_check()
        
        assert result["connected"] is True
        assert result["status"] == "healthy"
        assert result["version"] == "7.0.0"
        assert result["uptime_seconds"] == 86400
    
    @pytest.mark.asyncio
    async def test_health_check_failure(self, redis_manager):
        """Test Redis health check failure."""
        redis_manager.client.ping.side_effect = Exception("Connection failed")
        
        result = await redis_manager.health_check()
        
        assert result["connected"] is False
        assert result["status"] == "unhealthy"
        assert "error" in result


class TestCacheService:
    """Test cache service functionality."""
    
    @pytest.fixture
    def cache_service(self):
        """Create cache service for testing."""
        redis_manager = RedisManager()
        redis_manager.client = AsyncMock()
        return CacheService(redis_manager)
    
    @pytest.mark.asyncio
    async def test_user_session_management(self, cache_service):
        """Test user session cache operations."""
        user_data = {"user_id": "123", "username": "testuser", "email": "test@example.com"}
        session_token = "session_123"
        
        # Test set session
        cache_service.redis.set.return_value = True
        result = await cache_service.set_user_session(session_token, user_data, 3600)
        assert result is True
        
        # Test get session
        import json
        cache_service.redis.get.return_value = json.dumps(user_data, default=str)
        result = await cache_service.get_user_session(session_token)
        assert result == user_data
        
        # Test delete session
        cache_service.redis.delete.return_value = 1
        result = await cache_service.delete_user_session(session_token)
        assert result is True
        
        # Test extend session
        cache_service.redis.expire.return_value = True
        result = await cache_service.extend_user_session(session_token, 7200)
        assert result is True
    
    @pytest.mark.asyncio
    async def test_vm_state_caching(self, cache_service):
        """Test VM state cache operations."""
        vm_id = str(uuid.uuid4())
        state_data = {"status": "running", "cpu_usage": 45.2, "memory_usage": 1024}
        
        # Test set VM state
        cache_service.redis.set.return_value = True
        result = await cache_service.set_vm_state(vm_id, state_data, 300)
        assert result is True
        
        # Test get VM state
        import json
        cache_service.redis.get.return_value = json.dumps(state_data, default=str)
        result = await cache_service.get_vm_state(vm_id)
        assert result == state_data
        
        # Test delete VM state
        cache_service.redis.delete.return_value = 1
        result = await cache_service.delete_vm_state(vm_id)
        assert result is True
    
    @pytest.mark.asyncio
    async def test_rate_limiting(self, cache_service):
        """Test rate limiting functionality."""
        identifier = "user:123"
        limit = 10
        window_seconds = 60
        
        # First request - should be allowed
        cache_service.redis.incr.return_value = 1
        cache_service.redis.expire.return_value = True
        cache_service.redis.ttl.return_value = 60
        
        is_allowed, count, ttl = await cache_service.check_rate_limit(identifier, limit, window_seconds)
        assert is_allowed is True
        assert count == 1
        assert ttl == 60
        
        # Request at limit - should be allowed
        cache_service.redis.incr.return_value = 10
        cache_service.redis.ttl.return_value = 30
        
        is_allowed, count, ttl = await cache_service.check_rate_limit(identifier, limit, window_seconds)
        assert is_allowed is True
        assert count == 10
        assert ttl == 30
        
        # Request over limit - should be denied
        cache_service.redis.incr.return_value = 11
        cache_service.redis.ttl.return_value = 15
        
        is_allowed, count, ttl = await cache_service.check_rate_limit(identifier, limit, window_seconds)
        assert is_allowed is False
        assert count == 11
        assert ttl == 15
    
    @pytest.mark.asyncio
    async def test_user_activity_tracking(self, cache_service):
        """Test user activity tracking."""
        user_id = str(uuid.uuid4())
        
        # Test track activity
        cache_service.redis.set.return_value = True
        result = await cache_service.track_user_activity(user_id, 900)
        assert result is True
        
        # Test check activity
        cache_service.redis.exists.return_value = 1
        result = await cache_service.is_user_active(user_id)
        assert result is True
        
        # Test inactive user
        cache_service.redis.exists.return_value = 0
        result = await cache_service.is_user_active(user_id)
        assert result is False
    
    @pytest.mark.asyncio
    async def test_vm_resource_tracking(self, cache_service):
        """Test VM resource tracking."""
        user_id = str(uuid.uuid4())
        vm_id = str(uuid.uuid4())
        resources = {"cpu": 2, "memory": 4096, "disk": 50}
        
        # Test track resources
        cache_service.redis.hset.return_value = 1
        cache_service.redis.expire.return_value = True
        result = await cache_service.track_vm_resources(user_id, vm_id, resources)
        assert result is True
        
        # Test get resources
        import json
        cache_service.redis.hgetall.return_value = {vm_id: json.dumps(resources, default=str)}
        result = await cache_service.get_user_vm_resources(user_id)
        assert result == {vm_id: resources}
        
        # Test remove tracking
        cache_service.redis.hdel.return_value = 1
        result = await cache_service.remove_vm_tracking(user_id, vm_id)
        assert result is True
    
    @pytest.mark.asyncio
    async def test_generic_caching(self, cache_service):
        """Test generic cache operations."""
        key = "test_cache_key"
        value = {"data": "test_value", "number": 42}
        
        # Test set cache
        cache_service.redis.set.return_value = True
        result = await cache_service.set_cache(key, value, 1800)
        assert result is True
        
        # Test get cache
        import json
        cache_service.redis.get.return_value = json.dumps(value, default=str)
        result = await cache_service.get_cache(key)
        assert result == value
        
        # Test delete cache
        cache_service.redis.delete.return_value = 1
        result = await cache_service.delete_cache(key)
        assert result is True
    
    @pytest.mark.asyncio
    async def test_cache_stats(self, cache_service):
        """Test cache statistics."""
        stats_data = {
            "total_commands_processed": 1000,
            "keyspace_hits": 800,
            "keyspace_misses": 200
        }
        memory_data = {
            "used_memory": 1024000,
            "used_memory_human": "1.02M"
        }
        keyspace_data = {"db0": "keys=100,expires=10"}
        
        cache_service.redis.info.side_effect = [stats_data, keyspace_data]
        cache_service.redis.get_memory_usage.return_value = memory_data
        
        result = await cache_service.get_cache_stats()
        
        assert result["total_commands_processed"] == 1000
        assert result["keyspace_hits"] == 800
        assert result["keyspace_misses"] == 200
        assert result["memory_usage"] == memory_data
        assert result["keyspace"] == keyspace_data