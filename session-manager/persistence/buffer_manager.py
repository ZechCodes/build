"""
Session Buffer Manager

Manages terminal buffer persistence with compression, user validation,
and efficient Redis storage for session recovery.
"""
import asyncio
import json
import time
import gzip
from typing import Dict, Any, Optional, List, Tuple
from dataclasses import dataclass, asdict
import redis.asyncio as redis
import structlog
import logfire

logger = structlog.get_logger()


@dataclass
class SessionBuffer:
    """Session buffer data structure"""
    session_id: str
    user_id: str
    buffer_data: bytes
    cursor_position: Tuple[int, int]
    scroll_position: int
    last_updated: float
    size_bytes: int
    line_count: int


class SessionBufferManager:
    """Manages session buffer persistence with Redis"""
    
    def __init__(self, redis_client: redis.Redis, max_buffer_size: int = 1024 * 1024):
        self.redis = redis_client
        self.max_buffer_size = max_buffer_size  # 1MB default
        self.buffer_key_prefix = "session:buffer:"
        self.metadata_key_prefix = "session:meta:"
        self.compression_threshold = 4096  # Compress buffers > 4KB
        
    async def store_buffer(self, session_id: str, user_id: str, 
                          buffer_data: bytes, cursor_pos: Tuple[int, int],
                          scroll_pos: int = 0) -> bool:
        """Store terminal buffer data in Redis"""
        try:
            start_time = time.time()
            
            # Validate inputs
            if not session_id or not user_id:
                raise ValueError("Session ID and User ID are required")
            
            if not isinstance(buffer_data, bytes):
                raise ValueError("Buffer data must be bytes")
            
            # Check buffer size limits
            if len(buffer_data) > self.max_buffer_size:
                logger.warning("Buffer size exceeds limit", 
                             session_id=session_id, 
                             size=len(buffer_data),
                             limit=self.max_buffer_size)
                # Truncate buffer to last N lines
                buffer_data = self._truncate_buffer(buffer_data)
            
            # Create buffer object
            session_buffer = SessionBuffer(
                session_id=session_id,
                user_id=user_id,
                buffer_data=buffer_data,
                cursor_position=cursor_pos,
                scroll_position=scroll_pos,
                last_updated=time.time(),
                size_bytes=len(buffer_data),
                line_count=buffer_data.count(b'\n')
            )
            
            # Compress if large
            store_data = buffer_data
            compressed = False
            if len(buffer_data) > self.compression_threshold:
                store_data = gzip.compress(buffer_data)
                compressed = True
                logger.debug("Buffer compressed", 
                           session_id=session_id,
                           original_size=len(buffer_data),
                           compressed_size=len(store_data))
            
            # Prepare Redis data
            redis_data = {
                "user_id": user_id,
                "buffer_data": store_data,
                "cursor_x": cursor_pos[0],
                "cursor_y": cursor_pos[1], 
                "scroll_position": scroll_pos,
                "last_updated": session_buffer.last_updated,
                "size_bytes": len(buffer_data),
                "line_count": session_buffer.line_count,
                "compressed": str(compressed).lower()
            }
            
            # Store in Redis with expiration
            buffer_key = f"{self.buffer_key_prefix}{session_id}"
            await self.redis.hset(buffer_key, mapping=redis_data)
            await self.redis.expire(buffer_key, 86400)  # 24 hour expiration
            
            # Store metadata separately for faster access
            metadata = {
                "session_id": session_id,
                "user_id": user_id,
                "last_updated": session_buffer.last_updated,
                "size_bytes": len(buffer_data),
                "line_count": session_buffer.line_count
            }
            
            meta_key = f"{self.metadata_key_prefix}{session_id}"
            await self.redis.hset(meta_key, mapping=metadata)
            await self.redis.expire(meta_key, 86400)
            
            # Performance logging
            duration = time.time() - start_time
            logger.debug("Buffer stored successfully", 
                        session_id=session_id,
                        size_bytes=len(buffer_data),
                        compressed=compressed,
                        duration_ms=duration * 1000)
            
            # Log to Logfire with performance metrics
            logfire.info("Session buffer stored",
                        session_id=session_id,
                        user_id=user_id,
                        buffer_size=len(buffer_data),
                        compressed=compressed,
                        duration_ms=duration * 1000,
                        line_count=session_buffer.line_count)
            
            return True
            
        except Exception as e:
            logger.error("Failed to store buffer", 
                        session_id=session_id, error=str(e))
            logfire.error("Buffer storage failed",
                         session_id=session_id, error=str(e))
            return False
    
    async def retrieve_buffer(self, session_id: str, user_id: str) -> Optional[SessionBuffer]:
        """Retrieve terminal buffer data from Redis"""
        try:
            start_time = time.time()
            
            # Validate inputs
            if not session_id or not user_id:
                raise ValueError("Session ID and User ID are required")
            
            buffer_key = f"{self.buffer_key_prefix}{session_id}"
            redis_data = await self.redis.hgetall(buffer_key)
            
            if not redis_data:
                logger.debug("No buffer found", session_id=session_id)
                return None
            
            # Validate user ownership
            stored_user_id = redis_data.get(b"user_id", b"").decode('utf-8')
            if stored_user_id != user_id:
                logger.warning("Unauthorized buffer access attempt",
                             session_id=session_id,
                             requesting_user=user_id,
                             owner_user=stored_user_id)
                logfire.warning("Unauthorized buffer access attempt",
                              session_id=session_id,
                              requesting_user=user_id,
                              owner_user=stored_user_id)
                return None
            
            # Extract data
            buffer_data = redis_data[b"buffer_data"]
            compressed = redis_data.get(b"compressed", b"false") == b"true"
            
            # Decompress if needed
            if compressed:
                try:
                    buffer_data = gzip.decompress(buffer_data)
                    logger.debug("Buffer decompressed", 
                               session_id=session_id,
                               compressed_size=len(redis_data[b"buffer_data"]),
                               decompressed_size=len(buffer_data))
                except Exception as e:
                    logger.error("Failed to decompress buffer", 
                               session_id=session_id, error=str(e))
                    return None
            
            # Create SessionBuffer object
            session_buffer = SessionBuffer(
                session_id=session_id,
                user_id=user_id,
                buffer_data=buffer_data,
                cursor_position=(
                    int(redis_data.get(b"cursor_x", b"0")),
                    int(redis_data.get(b"cursor_y", b"0"))
                ),
                scroll_position=int(redis_data.get(b"scroll_position", b"0")),
                last_updated=float(redis_data.get(b"last_updated", b"0")),
                size_bytes=int(redis_data.get(b"size_bytes", len(buffer_data))),
                line_count=int(redis_data.get(b"line_count", b"0"))
            )
            
            # Performance logging
            duration = time.time() - start_time
            logger.debug("Buffer retrieved successfully",
                        session_id=session_id,
                        size_bytes=session_buffer.size_bytes,
                        duration_ms=duration * 1000)
            
            # Log to Logfire
            logfire.info("Session buffer retrieved",
                        session_id=session_id,
                        user_id=user_id,
                        buffer_size=session_buffer.size_bytes,
                        duration_ms=duration * 1000)
            
            return session_buffer
            
        except Exception as e:
            logger.error("Failed to retrieve buffer", 
                        session_id=session_id, error=str(e))
            logfire.error("Buffer retrieval failed",
                         session_id=session_id, error=str(e))
            return None
    
    async def clear_buffer(self, session_id: str, user_id: str) -> bool:
        """Clear session buffer"""
        try:
            # Validate ownership first
            buffer = await self.retrieve_buffer(session_id, user_id)
            if not buffer:
                logger.warning("Cannot clear buffer - not found or access denied",
                             session_id=session_id, user_id=user_id)
                return False
            
            # Clear buffer and metadata from Redis
            buffer_key = f"{self.buffer_key_prefix}{session_id}"
            metadata_key = f"{self.metadata_key_prefix}{session_id}"
            
            pipe = self.redis.pipeline()
            pipe.delete(buffer_key)
            pipe.delete(metadata_key)
            await pipe.execute()
            
            logger.info("Session buffer cleared", 
                       session_id=session_id, user_id=user_id)
            logfire.info("Session buffer cleared",
                        session_id=session_id, user_id=user_id)
            
            return True
            
        except Exception as e:
            logger.error("Failed to clear buffer", 
                        session_id=session_id, error=str(e))
            return False
    
    async def get_buffer_metadata(self, session_id: str, user_id: str) -> Optional[Dict[str, Any]]:
        """Get buffer metadata without loading full buffer"""
        try:
            # First verify ownership via main buffer
            buffer_key = f"{self.buffer_key_prefix}{session_id}"
            user_check = await self.redis.hget(buffer_key, "user_id")
            
            if not user_check or user_check.decode('utf-8') != user_id:
                return None
            
            # Get metadata
            meta_key = f"{self.metadata_key_prefix}{session_id}"
            metadata = await self.redis.hgetall(meta_key)
            
            if not metadata:
                return None
            
            # Convert to dict with proper types
            return {
                "session_id": metadata.get(b"session_id", b"").decode('utf-8'),
                "user_id": metadata.get(b"user_id", b"").decode('utf-8'),
                "last_updated": float(metadata.get(b"last_updated", b"0")),
                "size_bytes": int(metadata.get(b"size_bytes", b"0")),
                "line_count": int(metadata.get(b"line_count", b"0"))
            }
            
        except Exception as e:
            logger.error("Failed to get buffer metadata", 
                        session_id=session_id, error=str(e))
            return None
    
    def _truncate_buffer(self, buffer_data: bytes) -> bytes:
        """Truncate buffer to fit within size limits"""
        try:
            lines = buffer_data.split(b'\n')
            
            # Keep last N lines that fit within limit
            truncated_lines = []
            current_size = 0
            
            for line in reversed(lines):
                line_size = len(line) + 1  # +1 for newline
                if current_size + line_size > self.max_buffer_size:
                    break
                truncated_lines.insert(0, line)
                current_size += line_size
            
            truncated_buffer = b'\n'.join(truncated_lines)
            
            logger.info("Buffer truncated", 
                       original_size=len(buffer_data),
                       truncated_size=len(truncated_buffer),
                       lines_kept=len(truncated_lines))
            
            return truncated_buffer
            
        except Exception as e:
            logger.error("Buffer truncation failed", error=str(e))
            # Return last max_buffer_size bytes as fallback
            return buffer_data[-self.max_buffer_size:]
    
    async def list_user_buffers(self, user_id: str) -> List[str]:
        """List all buffer session IDs for a user"""
        try:
            # Scan for metadata keys and check ownership
            user_sessions = []
            pattern = f"{self.metadata_key_prefix}*"
            
            async for key in self.redis.scan_iter(match=pattern):
                try:
                    metadata = await self.redis.hgetall(key)
                    if metadata and metadata.get(b"user_id", b"").decode('utf-8') == user_id:
                        session_id = metadata.get(b"session_id", b"").decode('utf-8')
                        if session_id:
                            user_sessions.append(session_id)
                except Exception:
                    continue
            
            return user_sessions
            
        except Exception as e:
            logger.error("Failed to list user buffers", user_id=user_id, error=str(e))
            return []
    
    async def cleanup_expired_buffers(self) -> int:
        """Cleanup expired buffers (usually handled by Redis TTL)"""
        try:
            # This is mostly handled by Redis expiration, but we can do additional cleanup
            cleaned_count = 0
            current_time = time.time()
            
            # Check metadata for very old buffers that should be cleaned
            pattern = f"{self.metadata_key_prefix}*"
            async for key in self.redis.scan_iter(match=pattern):
                try:
                    metadata = await self.redis.hgetall(key)
                    if metadata:
                        last_updated = float(metadata.get(b"last_updated", b"0"))
                        # Clean buffers older than 48 hours (double the TTL)
                        if current_time - last_updated > 172800:
                            session_id = metadata.get(b"session_id", b"").decode('utf-8')
                            user_id = metadata.get(b"user_id", b"").decode('utf-8')
                            
                            if session_id and user_id:
                                await self.clear_buffer(session_id, user_id)
                                cleaned_count += 1
                                
                except Exception:
                    continue
            
            if cleaned_count > 0:
                logger.info("Cleaned up expired buffers", count=cleaned_count)
            
            return cleaned_count
            
        except Exception as e:
            logger.error("Buffer cleanup failed", error=str(e))
            return 0