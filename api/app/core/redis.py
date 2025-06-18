"""Redis connection and management."""

import asyncio
import logging
from typing import Optional, Union, Any, List
from urllib.parse import urlparse

import redis.asyncio as redis
from redis.asyncio.sentinel import Sentinel
from redis.asyncio import ConnectionPool
from redis.exceptions import ConnectionError, TimeoutError, AuthenticationError

from .config import get_settings

logger = logging.getLogger(__name__)


class RedisManager:
    """Redis connection manager with Sentinel support."""
    
    def __init__(self):
        self.settings = get_settings()
        self.pool: Optional[ConnectionPool] = None
        self.client: Optional[redis.Redis] = None
        self.sentinel: Optional[Sentinel] = None
        self._master_name = "mymaster"
        
    async def connect(self) -> None:
        """Establish Redis connection with Sentinel failover support."""
        try:
            if self.settings.environment == "production":
                await self._connect_with_sentinel()
            else:
                await self._connect_direct()
            
            # Test connection
            await self.ping()
            logger.info("Redis connection established successfully")
            
        except Exception as e:
            logger.error(f"Failed to connect to Redis: {e}")
            raise
    
    async def _connect_with_sentinel(self) -> None:
        """Connect to Redis using Sentinel for high availability."""
        sentinel_hosts = [
            ("127.0.0.1", 26379),
            ("127.0.0.1", 26380), 
            ("127.0.0.1", 26381)
        ]
        
        self.sentinel = Sentinel(
            sentinel_hosts,
            password=self.settings.redis_password,
            socket_timeout=5.0,
            socket_connect_timeout=5.0,
        )
        
        # Get master connection
        self.client = self.sentinel.master_for(
            self._master_name,
            password=self.settings.redis_password,
            socket_timeout=5.0,
            socket_connect_timeout=5.0,
            retry_on_timeout=True,
            health_check_interval=30,
            max_connections=20,
        )
        
    async def _connect_direct(self) -> None:
        """Connect directly to Redis (development/testing)."""
        redis_url = self.settings.redis_url
        parsed_url = urlparse(redis_url)
        
        self.pool = ConnectionPool.from_url(
            redis_url,
            max_connections=20,
            retry_on_timeout=True,
            socket_timeout=5.0,
            socket_connect_timeout=5.0,
            health_check_interval=30,
        )
        
        self.client = redis.Redis(
            connection_pool=self.pool,
            decode_responses=True,
        )
    
    async def disconnect(self) -> None:
        """Close Redis connections."""
        try:
            if self.client:
                await self.client.aclose()
            if self.pool:
                await self.pool.disconnect()
            if self.sentinel:
                await self.sentinel.close()
            logger.info("Redis connection closed successfully")
        except Exception as e:
            logger.error(f"Error closing Redis connection: {e}")
    
    async def ping(self) -> bool:
        """Test Redis connection."""
        if not self.client:
            return False
        try:
            result = await self.client.ping()
            return result
        except Exception as e:
            logger.error(f"Redis ping failed: {e}")
            return False
    
    async def get(self, key: str) -> Optional[str]:
        """Get value by key."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.get(key)
        except Exception as e:
            logger.error(f"Redis GET error for key {key}: {e}")
            raise
    
    async def set(
        self, 
        key: str, 
        value: Union[str, int, float], 
        ex: Optional[int] = None,
        px: Optional[int] = None,
        nx: bool = False,
        xx: bool = False
    ) -> bool:
        """Set key-value pair with optional expiration."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.set(key, value, ex=ex, px=px, nx=nx, xx=xx)
        except Exception as e:
            logger.error(f"Redis SET error for key {key}: {e}")
            raise
    
    async def delete(self, *keys: str) -> int:
        """Delete one or more keys."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.delete(*keys)
        except Exception as e:
            logger.error(f"Redis DELETE error for keys {keys}: {e}")
            raise
    
    async def exists(self, *keys: str) -> int:
        """Check if keys exist."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.exists(*keys)
        except Exception as e:
            logger.error(f"Redis EXISTS error for keys {keys}: {e}")
            raise
    
    async def expire(self, key: str, time: int) -> bool:
        """Set expiration time for key."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.expire(key, time)
        except Exception as e:
            logger.error(f"Redis EXPIRE error for key {key}: {e}")
            raise
    
    async def hget(self, name: str, key: str) -> Optional[str]:
        """Get field value from hash."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.hget(name, key)
        except Exception as e:
            logger.error(f"Redis HGET error for hash {name}, key {key}: {e}")
            raise
    
    async def hset(self, name: str, key: str, value: Union[str, int, float]) -> int:
        """Set field value in hash."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.hset(name, key, value)
        except Exception as e:
            logger.error(f"Redis HSET error for hash {name}, key {key}: {e}")
            raise
    
    async def hgetall(self, name: str) -> dict:
        """Get all fields and values from hash."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.hgetall(name)
        except Exception as e:
            logger.error(f"Redis HGETALL error for hash {name}: {e}")
            raise
    
    async def hdel(self, name: str, *keys: str) -> int:
        """Delete fields from hash."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.hdel(name, *keys)
        except Exception as e:
            logger.error(f"Redis HDEL error for hash {name}, keys {keys}: {e}")
            raise
    
    async def sadd(self, name: str, *values: Union[str, int]) -> int:
        """Add members to set."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.sadd(name, *values)
        except Exception as e:
            logger.error(f"Redis SADD error for set {name}: {e}")
            raise
    
    async def srem(self, name: str, *values: Union[str, int]) -> int:
        """Remove members from set."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.srem(name, *values)
        except Exception as e:
            logger.error(f"Redis SREM error for set {name}: {e}")
            raise
    
    async def smembers(self, name: str) -> set:
        """Get all members of set."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.smembers(name)
        except Exception as e:
            logger.error(f"Redis SMEMBERS error for set {name}: {e}")
            raise
    
    async def sismember(self, name: str, value: Union[str, int]) -> bool:
        """Check if value is member of set."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.sismember(name, value)
        except Exception as e:
            logger.error(f"Redis SISMEMBER error for set {name}, value {value}: {e}")
            raise
    
    async def incr(self, name: str, amount: int = 1) -> int:
        """Increment value of key."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.incr(name, amount)
        except Exception as e:
            logger.error(f"Redis INCR error for key {name}: {e}")
            raise
    
    async def decr(self, name: str, amount: int = 1) -> int:
        """Decrement value of key."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.decr(name, amount)
        except Exception as e:
            logger.error(f"Redis DECR error for key {name}: {e}")
            raise
    
    async def ttl(self, name: str) -> int:
        """Get TTL of key."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.ttl(name)
        except Exception as e:
            logger.error(f"Redis TTL error for key {name}: {e}")
            raise
    
    async def flushdb(self, asynchronous: bool = False) -> bool:
        """Flush current database (development only)."""
        if self.settings.environment == "production":
            raise PermissionError("FLUSHDB not allowed in production")
        
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.flushdb(asynchronous=asynchronous)
        except Exception as e:
            logger.error(f"Redis FLUSHDB error: {e}")
            raise
    
    async def info(self, section: Optional[str] = None) -> dict:
        """Get Redis server info."""
        if not self.client:
            raise ConnectionError("Redis client not connected")
        try:
            return await self.client.info(section)
        except Exception as e:
            logger.error(f"Redis INFO error: {e}")
            raise
    
    async def get_memory_usage(self) -> dict:
        """Get Redis memory usage statistics."""
        try:
            info = await self.info("memory")
            return {
                "used_memory": info.get("used_memory", 0),
                "used_memory_human": info.get("used_memory_human", "0B"),
                "used_memory_peak": info.get("used_memory_peak", 0),
                "used_memory_peak_human": info.get("used_memory_peak_human", "0B"),
                "maxmemory": info.get("maxmemory", 0),
                "maxmemory_human": info.get("maxmemory_human", "0B"),
            }
        except Exception as e:
            logger.error(f"Error getting Redis memory usage: {e}")
            return {}
    
    async def health_check(self) -> dict:
        """Comprehensive Redis health check."""
        try:
            # Basic connectivity
            ping_result = await self.ping()
            
            # Memory usage
            memory_stats = await self.get_memory_usage()
            
            # Server info
            server_info = await self.info("server")
            
            # Keyspace info
            keyspace_info = await self.info("keyspace")
            
            return {
                "connected": ping_result,
                "memory": memory_stats,
                "version": server_info.get("redis_version", "unknown"),
                "uptime_seconds": server_info.get("uptime_in_seconds", 0),
                "keyspace": keyspace_info,
                "status": "healthy" if ping_result else "unhealthy"
            }
        except Exception as e:
            logger.error(f"Redis health check failed: {e}")
            return {
                "connected": False,
                "status": "unhealthy",
                "error": str(e)
            }


# Global Redis manager instance
redis_manager = RedisManager()


async def get_redis() -> RedisManager:
    """Dependency to get Redis manager instance."""
    return redis_manager