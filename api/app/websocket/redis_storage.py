"""Redis storage for WebSocket connection state with encryption."""

import asyncio
import json
import time
from typing import Dict, Any, Optional, List
import redis.asyncio as redis
import structlog

from app.core.config import get_settings
from .encryption import get_message_encryption

logger = structlog.get_logger(__name__)


class EncryptedRedisStorage:
    """Encrypted Redis storage for WebSocket connection state."""
    
    def __init__(self):
        self.settings = get_settings()
        self.encryption = get_message_encryption()
        self.redis_client: Optional[redis.Redis] = None
        self.connection_prefix = "ws:connection:"
        self.session_prefix = "ws:session:"
        self.user_prefix = "ws:user:"
        self.metrics_prefix = "ws:metrics:"
        
        # Connection state TTL (in seconds)
        self.connection_ttl = 3600  # 1 hour
        self.session_ttl = 7200     # 2 hours
        self.metrics_ttl = 86400    # 24 hours
    
    async def initialize(self):
        """Initialize Redis connection."""
        try:
            self.redis_client = redis.from_url(
                self.settings.redis_url,
                password=self.settings.redis_password,
                socket_connect_timeout=self.settings.redis_socket_connect_timeout,
                socket_timeout=self.settings.redis_socket_timeout,
                health_check_interval=self.settings.redis_health_check_interval,
                max_connections=self.settings.redis_max_connections,
                decode_responses=True
            )
            
            # Test connection
            await self.redis_client.ping()
            logger.info("Redis connection for WebSocket storage established")
            
        except Exception as e:
            logger.error("Failed to initialize Redis storage", error=str(e))
            self.redis_client = None
            raise
    
    async def close(self):
        """Close Redis connection."""
        if self.redis_client:
            await self.redis_client.close()
            logger.info("Redis WebSocket storage connection closed")
    
    async def store_connection_state(self, connection_id: str, state_data: Dict[str, Any]) -> bool:
        """Store encrypted connection state in Redis."""
        if not self.redis_client:
            logger.warning("Redis not available, cannot store connection state")
            return False
        
        try:
            # Encrypt the state data
            encrypted_state = self.encryption.encrypt_connection_state(state_data)
            
            # Store in Redis with TTL
            key = f"{self.connection_prefix}{connection_id}"
            await self.redis_client.setex(key, self.connection_ttl, encrypted_state)
            
            logger.debug("Connection state stored", connection_id=connection_id)
            return True
            
        except Exception as e:
            logger.error("Failed to store connection state", 
                        connection_id=connection_id, error=str(e))
            return False
    
    async def retrieve_connection_state(self, connection_id: str) -> Optional[Dict[str, Any]]:
        """Retrieve and decrypt connection state from Redis."""
        if not self.redis_client:
            logger.warning("Redis not available, cannot retrieve connection state")
            return None
        
        try:
            key = f"{self.connection_prefix}{connection_id}"
            encrypted_state = await self.redis_client.get(key)
            
            if not encrypted_state:
                return None
            
            # Decrypt the state data
            state_data = self.encryption.decrypt_connection_state(encrypted_state)
            
            logger.debug("Connection state retrieved", connection_id=connection_id)
            return state_data
            
        except Exception as e:
            logger.error("Failed to retrieve connection state", 
                        connection_id=connection_id, error=str(e))
            return None
    
    async def delete_connection_state(self, connection_id: str) -> bool:
        """Delete connection state from Redis."""
        if not self.redis_client:
            return False
        
        try:
            key = f"{self.connection_prefix}{connection_id}"
            result = await self.redis_client.delete(key)
            
            logger.debug("Connection state deleted", connection_id=connection_id)
            return result > 0
            
        except Exception as e:
            logger.error("Failed to delete connection state", 
                        connection_id=connection_id, error=str(e))
            return False
    
    async def store_session_connections(self, session_id: str, connection_ids: List[str]) -> bool:
        """Store session-to-connections mapping."""
        if not self.redis_client:
            return False
        
        try:
            key = f"{self.session_prefix}{session_id}"
            
            # Store as encrypted JSON
            session_data = {
                "connection_ids": connection_ids,
                "last_updated": time.time()
            }
            encrypted_data = self.encryption.encrypt_connection_state(session_data)
            
            await self.redis_client.setex(key, self.session_ttl, encrypted_data)
            
            logger.debug("Session connections stored", 
                        session_id=session_id, 
                        connection_count=len(connection_ids))
            return True
            
        except Exception as e:
            logger.error("Failed to store session connections", 
                        session_id=session_id, error=str(e))
            return False
    
    async def retrieve_session_connections(self, session_id: str) -> List[str]:
        """Retrieve connections for a session."""
        if not self.redis_client:
            return []
        
        try:
            key = f"{self.session_prefix}{session_id}"
            encrypted_data = await self.redis_client.get(key)
            
            if not encrypted_data:
                return []
            
            session_data = self.encryption.decrypt_connection_state(encrypted_data)
            return session_data.get("connection_ids", [])
            
        except Exception as e:
            logger.error("Failed to retrieve session connections", 
                        session_id=session_id, error=str(e))
            return []
    
    async def store_user_connections(self, user_id: str, connection_ids: List[str]) -> bool:
        """Store user-to-connections mapping."""
        if not self.redis_client:
            return False
        
        try:
            key = f"{self.user_prefix}{user_id}"
            
            user_data = {
                "connection_ids": connection_ids,
                "last_updated": time.time()
            }
            encrypted_data = self.encryption.encrypt_connection_state(user_data)
            
            await self.redis_client.setex(key, self.connection_ttl, encrypted_data)
            
            logger.debug("User connections stored", 
                        user_id=user_id, 
                        connection_count=len(connection_ids))
            return True
            
        except Exception as e:
            logger.error("Failed to store user connections", 
                        user_id=user_id, error=str(e))
            return False
    
    async def retrieve_user_connections(self, user_id: str) -> List[str]:
        """Retrieve connections for a user."""
        if not self.redis_client:
            return []
        
        try:
            key = f"{self.user_prefix}{user_id}"
            encrypted_data = await self.redis_client.get(key)
            
            if not encrypted_data:
                return []
            
            user_data = self.encryption.decrypt_connection_state(encrypted_data)
            return user_data.get("connection_ids", [])
            
        except Exception as e:
            logger.error("Failed to retrieve user connections", 
                        user_id=user_id, error=str(e))
            return []
    
    async def store_connection_metrics(self, connection_id: str, metrics: Dict[str, Any]) -> bool:
        """Store connection metrics."""
        if not self.redis_client:
            return False
        
        try:
            key = f"{self.metrics_prefix}{connection_id}"
            
            # Add timestamp
            metrics_data = {
                **metrics,
                "timestamp": time.time()
            }
            
            # Store metrics (encrypted for security)
            encrypted_metrics = self.encryption.encrypt_connection_state(metrics_data)
            await self.redis_client.setex(key, self.metrics_ttl, encrypted_metrics)
            
            logger.debug("Connection metrics stored", connection_id=connection_id)
            return True
            
        except Exception as e:
            logger.error("Failed to store connection metrics", 
                        connection_id=connection_id, error=str(e))
            return False
    
    async def retrieve_connection_metrics(self, connection_id: str) -> Optional[Dict[str, Any]]:
        """Retrieve connection metrics."""
        if not self.redis_client:
            return None
        
        try:
            key = f"{self.metrics_prefix}{connection_id}"
            encrypted_metrics = await self.redis_client.get(key)
            
            if not encrypted_metrics:
                return None
            
            metrics_data = self.encryption.decrypt_connection_state(encrypted_metrics)
            return metrics_data
            
        except Exception as e:
            logger.error("Failed to retrieve connection metrics", 
                        connection_id=connection_id, error=str(e))
            return None
    
    async def cleanup_expired_data(self) -> int:
        """Clean up expired connection data (Redis TTL should handle this, but manual cleanup for safety)."""
        if not self.redis_client:
            return 0
        
        try:
            cleaned_count = 0
            current_time = time.time()
            
            # Get all connection keys
            connection_keys = await self.redis_client.keys(f"{self.connection_prefix}*")
            
            for key in connection_keys:
                try:
                    encrypted_data = await self.redis_client.get(key)
                    if encrypted_data:
                        state_data = self.encryption.decrypt_connection_state(encrypted_data)
                        
                        # Check if connection is too old (beyond TTL + buffer)
                        created_at = state_data.get("connected_at", current_time)
                        if current_time - created_at > self.connection_ttl + 300:  # 5 min buffer
                            await self.redis_client.delete(key)
                            cleaned_count += 1
                            
                except Exception as e:
                    # If we can't decrypt or parse, delete the key
                    logger.warning("Cleaning up corrupted connection state", key=key, error=str(e))
                    await self.redis_client.delete(key)
                    cleaned_count += 1
            
            if cleaned_count > 0:
                logger.info("Cleaned up expired connection data", count=cleaned_count)
            
            return cleaned_count
            
        except Exception as e:
            logger.error("Failed to cleanup expired data", error=str(e))
            return 0
    
    async def get_global_stats(self) -> Dict[str, Any]:
        """Get global WebSocket statistics."""
        if not self.redis_client:
            return {}
        
        try:
            stats = {
                "total_connections": 0,
                "total_sessions": 0,
                "total_users": 0,
                "timestamp": time.time()
            }
            
            # Count connections
            connection_keys = await self.redis_client.keys(f"{self.connection_prefix}*")
            stats["total_connections"] = len(connection_keys)
            
            # Count sessions
            session_keys = await self.redis_client.keys(f"{self.session_prefix}*")
            stats["total_sessions"] = len(session_keys)
            
            # Count users
            user_keys = await self.redis_client.keys(f"{self.user_prefix}*")
            stats["total_users"] = len(user_keys)
            
            return stats
            
        except Exception as e:
            logger.error("Failed to get global stats", error=str(e))
            return {}


# Global instance
_redis_storage = None


async def get_redis_storage() -> EncryptedRedisStorage:
    """Get global encrypted Redis storage instance."""
    global _redis_storage
    if _redis_storage is None:
        _redis_storage = EncryptedRedisStorage()
        await _redis_storage.initialize()
    return _redis_storage


async def cleanup_redis_storage():
    """Cleanup Redis storage on shutdown."""
    global _redis_storage
    if _redis_storage:
        await _redis_storage.close()
        _redis_storage = None