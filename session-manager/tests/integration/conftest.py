"""
Integration test configuration and fixtures
"""
import pytest
import asyncio
from unittest.mock import AsyncMock, MagicMock
from typing import Dict, Any
import redis.asyncio as redis


class StatefulRedis:
    """Stateful Redis mock that actually stores data for integration tests"""
    
    def __init__(self):
        self.data: Dict[str, Any] = {}
        self.expiry: Dict[str, float] = {}
        self.sets: Dict[str, set] = {}
    
    async def get(self, key: str):
        return self.data.get(key)
    
    async def set(self, key: str, value: Any, ex: int = None):
        self.data[key] = value
        if ex:
            import time
            self.expiry[key] = time.time() + ex
        return True
    
    async def hset(self, key: str, mapping: Dict[str, Any] = None, **kwargs):
        if key not in self.data:
            self.data[key] = {}
        if mapping:
            self.data[key].update(mapping)
        if kwargs:
            self.data[key].update(kwargs)
        return len(mapping or kwargs)
    
    async def hgetall(self, key: str):
        # Convert strings to bytes to match Redis behavior
        data = self.data.get(key, {})
        byte_data = {}
        for k, v in data.items():
            byte_key = k.encode() if isinstance(k, str) else k
            if isinstance(v, str):
                byte_value = v.encode()
            elif isinstance(v, bytes):
                byte_value = v
            else:
                byte_value = str(v).encode()
            byte_data[byte_key] = byte_value
        return byte_data
    
    async def delete(self, *keys):
        count = 0
        for key in keys:
            if key in self.data:
                del self.data[key]
                count += 1
        return count
    
    async def expire(self, key: str, seconds: int):
        if key in self.data:
            import time
            self.expiry[key] = time.time() + seconds
            return True
        return False
    
    async def sadd(self, key: str, *values):
        if key not in self.sets:
            self.sets[key] = set()
        before_size = len(self.sets[key])
        self.sets[key].update(values)
        return len(self.sets[key]) - before_size
    
    async def srem(self, key: str, *values):
        if key not in self.sets:
            return 0
        before_size = len(self.sets[key])
        self.sets[key] -= set(values)
        return before_size - len(self.sets[key])
    
    async def smembers(self, key: str):
        return self.sets.get(key, set())
    
    async def keys(self, pattern: str = "*"):
        import fnmatch
        return [k for k in self.data.keys() if fnmatch.fnmatch(k, pattern)]
    
    def pipeline(self):
        return StatefulRedisPipeline(self)


class StatefulRedisPipeline:
    """Pipeline mock for stateful Redis"""
    
    def __init__(self, redis_instance):
        self.redis = redis_instance
        self.commands = []
    
    def hset(self, key: str, mapping: Dict[str, Any] = None, **kwargs):
        self.commands.append(('hset', key, mapping, kwargs))
        return self
    
    def expire(self, key: str, seconds: int):
        self.commands.append(('expire', key, seconds))
        return self
    
    def sadd(self, key: str, *values):
        self.commands.append(('sadd', key, values))
        return self
    
    def srem(self, key: str, *values):
        self.commands.append(('srem', key, values))
        return self
    
    def delete(self, *keys):
        self.commands.append(('delete', keys))
        return self
    
    async def execute(self):
        results = []
        for command in self.commands:
            if command[0] == 'hset':
                result = await self.redis.hset(command[1], command[2], **command[3])
                results.append(result)
            elif command[0] == 'expire':
                result = await self.redis.expire(command[1], command[2])
                results.append(result)
            elif command[0] == 'sadd':
                result = await self.redis.sadd(command[1], *command[2])
                results.append(result)
            elif command[0] == 'srem':
                result = await self.redis.srem(command[1], *command[2])
                results.append(result)
            elif command[0] == 'delete':
                result = await self.redis.delete(*command[1])
                results.append(result)
        self.commands.clear()
        return results


@pytest.fixture
async def stateful_redis_mock():
    """Stateful Redis mock for integration tests"""
    return StatefulRedis()


@pytest.fixture
def auth_service_mock():
    """Mock authentication service"""
    service = AsyncMock()
    service.validate_token = AsyncMock(return_value={"user_id": "user123"})
    return service


@pytest.fixture  
def websocket_mock():
    """Mock WebSocket connection"""
    websocket = AsyncMock()
    websocket.accept = AsyncMock()
    websocket.close = AsyncMock()
    websocket.send_text = AsyncMock()
    websocket.receive_text = AsyncMock()
    return websocket


@pytest.fixture
def valid_jwt_token():
    """Valid JWT token for testing"""
    from jose import jwt
    import time
    
    payload = {
        "user_id": "user123",
        "exp": int(time.time()) + 3600,  # 1 hour from now
        "iat": int(time.time())
    }
    
    return jwt.encode(payload, "test-secret", algorithm="HS256")