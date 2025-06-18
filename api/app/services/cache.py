"""Cache service using Redis."""

import json
import logging
from typing import Any, Optional, Union, Dict, List
from datetime import datetime, timedelta

from ..core.redis import RedisManager

logger = logging.getLogger(__name__)


class CacheService:
    """High-level cache service for application data."""
    
    def __init__(self, redis_manager: RedisManager):
        self.redis = redis_manager
        
    # User session management
    async def set_user_session(
        self, 
        session_token: str, 
        user_data: Dict[str, Any], 
        ttl_seconds: int = 3600
    ) -> bool:
        """Store user session data."""
        try:
            key = f"session:{session_token}"
            value = json.dumps(user_data, default=str)
            return await self.redis.set(key, value, ex=ttl_seconds)
        except Exception as e:
            logger.error(f"Failed to set user session {session_token}: {e}")
            return False
    
    async def get_user_session(self, session_token: str) -> Optional[Dict[str, Any]]:
        """Retrieve user session data."""
        try:
            key = f"session:{session_token}"
            value = await self.redis.get(key)
            if value:
                return json.loads(value)
            return None
        except Exception as e:
            logger.error(f"Failed to get user session {session_token}: {e}")
            return None
    
    async def delete_user_session(self, session_token: str) -> bool:
        """Delete user session."""
        try:
            key = f"session:{session_token}"
            result = await self.redis.delete(key)
            return result > 0
        except Exception as e:
            logger.error(f"Failed to delete user session {session_token}: {e}")
            return False
    
    async def extend_user_session(self, session_token: str, ttl_seconds: int = 3600) -> bool:
        """Extend user session TTL."""
        try:
            key = f"session:{session_token}"
            return await self.redis.expire(key, ttl_seconds)
        except Exception as e:
            logger.error(f"Failed to extend user session {session_token}: {e}")
            return False
    
    # VM state caching
    async def set_vm_state(self, vm_id: str, state_data: Dict[str, Any], ttl_seconds: int = 300) -> bool:
        """Cache VM state data."""
        try:
            key = f"vm:state:{vm_id}"
            value = json.dumps(state_data, default=str)
            return await self.redis.set(key, value, ex=ttl_seconds)
        except Exception as e:
            logger.error(f"Failed to set VM state {vm_id}: {e}")
            return False
    
    async def get_vm_state(self, vm_id: str) -> Optional[Dict[str, Any]]:
        """Retrieve cached VM state."""
        try:
            key = f"vm:state:{vm_id}"
            value = await self.redis.get(key)
            if value:
                return json.loads(value)
            return None
        except Exception as e:
            logger.error(f"Failed to get VM state {vm_id}: {e}")
            return None
    
    async def delete_vm_state(self, vm_id: str) -> bool:
        """Delete cached VM state."""
        try:
            key = f"vm:state:{vm_id}"
            result = await self.redis.delete(key)
            return result > 0
        except Exception as e:
            logger.error(f"Failed to delete VM state {vm_id}: {e}")
            return False
    
    # Rate limiting
    async def check_rate_limit(
        self, 
        identifier: str, 
        limit: int, 
        window_seconds: int = 60
    ) -> tuple[bool, int, int]:
        """
        Check rate limit for identifier.
        Returns: (is_allowed, current_count, time_until_reset)
        """
        try:
            key = f"rate_limit:{identifier}"
            current_count = await self.redis.incr(key)
            
            if current_count == 1:
                # First request in window
                await self.redis.expire(key, window_seconds)
                return True, current_count, window_seconds
            
            ttl = await self.redis.ttl(key)
            if ttl == -1:
                # Key exists but no TTL, reset it
                await self.redis.expire(key, window_seconds)
                ttl = window_seconds
            
            is_allowed = current_count <= limit
            return is_allowed, current_count, ttl
            
        except Exception as e:
            logger.error(f"Failed to check rate limit for {identifier}: {e}")
            # Fail open - allow request if cache is down
            return True, 0, 0
    
    async def reset_rate_limit(self, identifier: str) -> bool:
        """Reset rate limit for identifier."""
        try:
            key = f"rate_limit:{identifier}"
            result = await self.redis.delete(key)
            return result > 0
        except Exception as e:
            logger.error(f"Failed to reset rate limit for {identifier}: {e}")
            return False
    
    # User active tracking
    async def track_user_activity(self, user_id: str, ttl_seconds: int = 900) -> bool:
        """Track user as active."""
        try:
            key = f"user:active:{user_id}"
            timestamp = datetime.utcnow().isoformat()
            return await self.redis.set(key, timestamp, ex=ttl_seconds)
        except Exception as e:
            logger.error(f"Failed to track user activity {user_id}: {e}")
            return False
    
    async def is_user_active(self, user_id: str) -> bool:
        """Check if user is currently active."""
        try:
            key = f"user:active:{user_id}"
            result = await self.redis.exists(key)
            return result > 0
        except Exception as e:
            logger.error(f"Failed to check user activity {user_id}: {e}")
            return False
    
    async def get_active_users(self) -> List[str]:
        """Get list of active user IDs."""
        try:
            # This is not ideal for large user bases - should use a different pattern
            keys = await self.redis.client.keys("user:active:*")
            user_ids = [key.split(":")[-1] for key in keys]
            return user_ids
        except Exception as e:
            logger.error(f"Failed to get active users: {e}")
            return []
    
    # VM resource tracking
    async def track_vm_resources(self, user_id: str, vm_id: str, resources: Dict[str, Any]) -> bool:
        """Track VM resource usage."""
        try:
            key = f"user:vms:{user_id}"
            field = vm_id
            value = json.dumps(resources, default=str)
            result = await self.redis.hset(key, field, value)
            # Set expiration for the hash
            await self.redis.expire(key, 3600)  # 1 hour
            return result > 0
        except Exception as e:
            logger.error(f"Failed to track VM resources {vm_id} for user {user_id}: {e}")
            return False
    
    async def get_user_vm_resources(self, user_id: str) -> Dict[str, Dict[str, Any]]:
        """Get all VM resources for a user."""
        try:
            key = f"user:vms:{user_id}"
            data = await self.redis.hgetall(key)
            result = {}
            for vm_id, resources_str in data.items():
                try:
                    result[vm_id] = json.loads(resources_str)
                except json.JSONDecodeError:
                    logger.warning(f"Failed to parse VM resources for {vm_id}")
            return result
        except Exception as e:
            logger.error(f"Failed to get VM resources for user {user_id}: {e}")
            return {}
    
    async def remove_vm_tracking(self, user_id: str, vm_id: str) -> bool:
        """Remove VM from resource tracking."""
        try:
            key = f"user:vms:{user_id}"
            result = await self.redis.hdel(key, vm_id)
            return result > 0
        except Exception as e:
            logger.error(f"Failed to remove VM tracking {vm_id} for user {user_id}: {e}")
            return False
    
    # Generic caching
    async def set_cache(
        self, 
        key: str, 
        value: Any, 
        ttl_seconds: Optional[int] = None
    ) -> bool:
        """Set generic cache value."""
        try:
            serialized_value = json.dumps(value, default=str)
            return await self.redis.set(f"cache:{key}", serialized_value, ex=ttl_seconds)
        except Exception as e:
            logger.error(f"Failed to set cache {key}: {e}")
            return False
    
    async def get_cache(self, key: str) -> Optional[Any]:
        """Get generic cache value."""
        try:
            value = await self.redis.get(f"cache:{key}")
            if value:
                return json.loads(value)
            return None
        except Exception as e:
            logger.error(f"Failed to get cache {key}: {e}")
            return None
    
    async def delete_cache(self, key: str) -> bool:
        """Delete generic cache value."""
        try:
            result = await self.redis.delete(f"cache:{key}")
            return result > 0
        except Exception as e:
            logger.error(f"Failed to delete cache {key}: {e}")
            return False
    
    # Cache statistics
    async def get_cache_stats(self) -> Dict[str, Any]:
        """Get cache usage statistics."""
        try:
            info = await self.redis.info("stats")
            keyspace = await self.redis.info("keyspace")
            memory = await self.redis.get_memory_usage()
            
            return {
                "total_commands_processed": info.get("total_commands_processed", 0),
                "total_connections_received": info.get("total_connections_received", 0),
                "expired_keys": info.get("expired_keys", 0),
                "evicted_keys": info.get("evicted_keys", 0),
                "keyspace_hits": info.get("keyspace_hits", 0),
                "keyspace_misses": info.get("keyspace_misses", 0),
                "memory_usage": memory,
                "keyspace": keyspace,
            }
        except Exception as e:
            logger.error(f"Failed to get cache stats: {e}")
            return {}
    
    async def clear_user_cache(self, user_id: str) -> int:
        """Clear all cache entries for a user."""
        try:
            patterns = [
                f"session:*",  # We'll filter by user data
                f"user:active:{user_id}",
                f"user:vms:{user_id}",
                f"vm:state:*",  # We'll filter by user ownership
            ]
            
            deleted_count = 0
            for pattern in patterns:
                if pattern.endswith(user_id):
                    # Direct key deletion
                    result = await self.redis.delete(pattern)
                    deleted_count += result
                # For patterns with wildcards, we'd need to implement key scanning
                # This is simplified for this implementation
            
            return deleted_count
        except Exception as e:
            logger.error(f"Failed to clear user cache {user_id}: {e}")
            return 0


async def get_cache_service(redis_manager: RedisManager) -> CacheService:
    """Get cache service instance."""
    return CacheService(redis_manager)