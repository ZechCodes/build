# Session 6.3: Redis Persistence & Recovery

## Objective
Implement robust Redis-based persistence for session state with recovery mechanisms, ensuring session data survives service restarts and provides efficient state synchronization.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for Redis operation monitoring and performance tracking
- **Session 5**: Connects to database for session metadata backup and validation
- **Redis Infrastructure**: Leverages Redis setup from previous infrastructure sessions

## Core Implementation

### Session Buffer Manager
**Location**: `session-manager/persistence/buffer_manager.py`

```python
# session-manager/persistence/buffer_manager.py
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
    session_id: str
    user_id: str
    buffer_data: bytes
    cursor_position: Tuple[int, int]
    scroll_position: int
    last_updated: float
    size_bytes: int
    line_count: int

class SessionBufferManager:
    def __init__(self, redis_client: redis.Redis, max_buffer_size: int = 1024 * 1024):
        self.redis = redis_client
        self.max_buffer_size = max_buffer_size
        self.buffer_key_prefix = "session:buffer:"
        self.metadata_key_prefix = "session:meta:"
        self.compression_threshold = 4096  # Compress buffers > 4KB
        
    async def store_buffer(self, session_id: str, user_id: str, 
                          buffer_data: bytes, cursor_pos: Tuple[int, int],
                          scroll_pos: int = 0) -> bool:
        """Store terminal buffer data in Redis"""
        try:
            start_time = time.time()
            
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
                "compressed": compressed
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
            
            buffer_key = f"{self.buffer_key_prefix}{session_id}"
            redis_data = await self.redis.hgetall(buffer_key)
            
            if not redis_data:
                return None
            
            # Validate user ownership
            stored_user_id = redis_data.get("user_id", "").decode('utf-8')
            if stored_user_id != user_id:
                logger.warning("Unauthorized buffer access attempt",
                             session_id=session_id,
                             requesting_user=user_id,
                             owner_user=stored_user_id)
                return None
            
            # Extract data
            buffer_data = redis_data["buffer_data"]
            compressed = redis_data.get("compressed", b"false") == b"true"
            
            # Decompress if needed
            if compressed:
                try:
                    buffer_data = gzip.decompress(buffer_data)
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
                    int(redis_data.get("cursor_x", 0)),
                    int(redis_data.get("cursor_y", 0))
                ),
                scroll_position=int(redis_data.get("scroll_position", 0)),
                last_updated=float(redis_data.get("last_updated", 0)),
                size_bytes=int(redis_data.get("size_bytes", len(buffer_data))),
                line_count=int(redis_data.get("line_count", 0))
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
```

### Recovery Manager
**Location**: `session-manager/persistence/recovery_manager.py`

```python
# session-manager/persistence/recovery_manager.py
import asyncio
import time
from typing import Dict, List, Optional, Set
from dataclasses import dataclass
import structlog
import logfire

logger = structlog.get_logger()

@dataclass
class RecoveryInfo:
    session_id: str
    user_id: str
    last_seen: float
    recovery_attempts: int
    is_recoverable: bool

class RecoveryManager:
    def __init__(self, redis_client, session_manager, buffer_manager):
        self.redis = redis_client
        self.session_manager = session_manager
        self.buffer_manager = buffer_manager
        self.recovery_timeout = 300  # 5 minutes
        self.max_recovery_attempts = 3
        self.recovery_task: Optional[asyncio.Task] = None
        
    async def initialize(self):
        """Initialize recovery manager"""
        self.recovery_task = asyncio.create_task(self._recovery_loop())
        logger.info("Recovery manager initialized")
        logfire.info("Session recovery manager started")
    
    async def _recovery_loop(self):
        """Main recovery loop"""
        while True:
            try:
                await asyncio.sleep(60)  # Check every minute
                await self._scan_for_recoverable_sessions()
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Recovery loop error", error=str(e))
    
    async def _scan_for_recoverable_sessions(self):
        """Scan Redis for sessions that need recovery"""
        try:
            # Get all session metadata keys
            pattern = f"{self.buffer_manager.metadata_key_prefix}*"
            keys = await self.redis.keys(pattern)
            
            current_time = time.time()
            recoverable_sessions = []
            
            for key in keys:
                try:
                    session_id = key.decode('utf-8').split(':')[-1]
                    metadata = await self.redis.hgetall(key)
                    
                    if not metadata:
                        continue
                    
                    last_updated = float(metadata.get("last_updated", 0))
                    user_id = metadata.get("user_id", "").decode('utf-8')
                    
                    # Check if session needs recovery
                    if (current_time - last_updated > self.recovery_timeout and
                        session_id not in self.session_manager.sessions):
                        
                        recoverable_sessions.append(RecoveryInfo(
                            session_id=session_id,
                            user_id=user_id,
                            last_seen=last_updated,
                            recovery_attempts=0,
                            is_recoverable=True
                        ))
                        
                except Exception as e:
                    logger.error("Error processing recovery key", 
                               key=key, error=str(e))
            
            # Attempt recovery for found sessions
            if recoverable_sessions:
                logger.info("Found recoverable sessions", 
                           count=len(recoverable_sessions))
                
                for recovery_info in recoverable_sessions:
                    await self._attempt_session_recovery(recovery_info)
                    
        except Exception as e:
            logger.error("Session scan error", error=str(e))
    
    async def _attempt_session_recovery(self, recovery_info: RecoveryInfo):
        """Attempt to recover a session"""
        try:
            session_id = recovery_info.session_id
            user_id = recovery_info.user_id
            
            logger.info("Attempting session recovery", 
                       session_id=session_id, user_id=user_id)
            
            # Retrieve buffer data
            buffer_data = await self.buffer_manager.retrieve_buffer(
                session_id, user_id
            )
            
            if not buffer_data:
                logger.warning("No buffer data found for recovery", 
                             session_id=session_id)
                return False
            
            # Recreate session context (this would integrate with session manager)
            # For now, we'll mark the session as recovered in Redis
            recovery_key = f"session:recovered:{session_id}"
            recovery_data = {
                "user_id": user_id,
                "recovered_at": time.time(),
                "buffer_size": buffer_data.size_bytes,
                "last_activity": buffer_data.last_updated
            }
            
            await self.redis.hset(recovery_key, mapping=recovery_data)
            await self.redis.expire(recovery_key, 3600)  # 1 hour
            
            logger.info("Session recovery completed", 
                       session_id=session_id,
                       buffer_size=buffer_data.size_bytes)
            
            # Log to Logfire
            logfire.info("Session recovered successfully",
                        session_id=session_id,
                        user_id=user_id,
                        buffer_size=buffer_data.size_bytes,
                        last_activity=buffer_data.last_updated)
            
            return True
            
        except Exception as e:
            logger.error("Session recovery failed", 
                        session_id=recovery_info.session_id, error=str(e))
            logfire.error("Session recovery failed",
                         session_id=recovery_info.session_id, error=str(e))
            return False
```

## TDD Implementation Cycle

### Test-Driven Development Process

1. **Red Phase**: Write failing persistence tests
   ```bash
   # Create persistence test files
   touch session-manager/tests/test_buffer_manager.py
   touch session-manager/tests/test_recovery_manager.py
   
   # Run failing tests
   pytest session-manager/tests/test_buffer_manager.py::test_store_buffer -v
   ```

2. **Green Phase**: Implement minimal persistence functionality
   ```bash
   # Implement basic Redis operations
   pytest session-manager/tests/test_buffer_manager.py::test_store_buffer -v
   ```

3. **Refactor Phase**: Optimize Redis operations
   ```bash
   # Add compression and performance optimizations
   pytest session-manager/tests/ -v
   ```

4. **Commit**: Commit persistence functionality
   ```bash
   git add session-manager/persistence/ session-manager/tests/test_*_manager.py
   git commit -m "feat: implement Redis persistence with session recovery
   
   - Add SessionBufferManager for terminal buffer persistence
   - Implement compression for large buffers to optimize storage
   - Add RecoveryManager for automatic session recovery after crashes
   - Include comprehensive error handling and user validation
   - Integrate with Logfire for Redis operation monitoring
   
   Tests: Added comprehensive test suite for persistence operations
   Security: User ownership validation and secure data handling
   Performance: Buffer compression and optimized Redis operations"
   ```

### Persistence Test Cases

```python
# session-manager/tests/test_buffer_manager.py
import pytest
import asyncio
from unittest.mock import AsyncMock, MagicMock
from session_manager.persistence.buffer_manager import SessionBufferManager, SessionBuffer

@pytest.fixture
async def redis_mock():
    """Mock Redis client"""
    redis_client = AsyncMock()
    redis_client.hset = AsyncMock()
    redis_client.hgetall = AsyncMock()
    redis_client.expire = AsyncMock()
    redis_client.keys = AsyncMock()
    return redis_client

@pytest.fixture
def buffer_manager(redis_mock):
    """SessionBufferManager instance"""
    return SessionBufferManager(redis_mock)

class TestSessionBufferManager:
    async def test_store_buffer_success(self, buffer_manager):
        """Test successful buffer storage"""
        # Arrange
        session_id = "session123"
        user_id = "user456"
        buffer_data = b"terminal output data"
        cursor_pos = (10, 5)
        
        # Act
        result = await buffer_manager.store_buffer(
            session_id, user_id, buffer_data, cursor_pos
        )
        
        # Assert
        assert result is True
        buffer_manager.redis.hset.assert_called()
        buffer_manager.redis.expire.assert_called()
    
    async def test_retrieve_buffer_success(self, buffer_manager):
        """Test successful buffer retrieval"""
        # Arrange
        session_id = "session123"
        user_id = "user456"
        
        # Mock Redis response
        buffer_manager.redis.hgetall.return_value = {
            "user_id": b"user456",
            "buffer_data": b"terminal output data",
            "cursor_x": b"10",
            "cursor_y": b"5",
            "scroll_position": b"0",
            "last_updated": b"1234567890.0",
            "size_bytes": b"19",
            "line_count": b"1",
            "compressed": b"false"
        }
        
        # Act
        buffer = await buffer_manager.retrieve_buffer(session_id, user_id)
        
        # Assert
        assert buffer is not None
        assert buffer.session_id == session_id
        assert buffer.user_id == user_id
        assert buffer.buffer_data == b"terminal output data"
    
    async def test_retrieve_buffer_unauthorized(self, buffer_manager):
        """Test unauthorized buffer retrieval"""
        # Arrange
        session_id = "session123"
        user_id = "user456"
        
        # Mock Redis response with different user
        buffer_manager.redis.hgetall.return_value = {
            "user_id": b"different_user",
            "buffer_data": b"sensitive data"
        }
        
        # Act
        buffer = await buffer_manager.retrieve_buffer(session_id, user_id)
        
        # Assert
        assert buffer is None  # Should deny access
```

## Security Checklist for Redis Persistence

### Data Security Controls
- [ ] User ownership validation on all buffer operations
- [ ] Cross-user data access prevention with strict authorization
- [ ] Redis key encryption for sensitive session identifiers  
- [ ] Buffer data encryption at rest in Redis
- [ ] Secure Redis authentication and connection encryption
- [ ] Rate limiting on Redis operations (100 ops/second per user)
- [ ] Redis key expiration to prevent data accumulation
- [ ] Audit logging for all Redis persistence operations
- [ ] Protection against Redis injection attacks
- [ ] Secure buffer size limits to prevent DoS attacks

### Recovery Security
- [ ] Session ownership validation during recovery operations
- [ ] Recovery attempt rate limiting (3 attempts per session max)
- [ ] Secure recovery token generation and validation
- [ ] Recovery operation audit logging
- [ ] Protection against recovery enumeration attacks
- [ ] Secure cleanup of recovery artifacts
- [ ] Recovery access controls based on user permissions
- [ ] Recovery state integrity validation
- [ ] Protection against replay attacks during recovery
- [ ] Secure recovery notification mechanisms

### Integration Security
- [ ] Secure integration with session manager authentication
- [ ] Protected Redis connection with authentication and encryption
- [ ] Database integration uses secure connection parameters
- [ ] Logfire integration excludes sensitive buffer content
- [ ] Error handling prevents information disclosure through Redis errors

## Performance Requirements

### Redis Operations
- Buffer storage latency < 100ms
- Buffer retrieval latency < 50ms
- Compression/decompression < 50ms for 1MB buffers
- Redis connection pool efficiency > 90%
- Memory usage optimization with compression
- Concurrent operation support (100+ simultaneous)

### Recovery Operations
- Recovery scan completion < 30 seconds
- Individual session recovery < 5 seconds
- Recovery loop interval every 60 seconds
- Recovery attempt timeout after 30 seconds
- Memory efficient recovery processing
- Minimal impact on active session performance

## Integration Testing

### Redis Integration Tests
```python
async def test_redis_persistence_integration(buffer_manager, redis_client):
    """Test actual Redis persistence operations"""
    # Test with real Redis instance
    session_id = "test_session_123"
    user_id = "test_user_456"
    buffer_data = b"test terminal output"
    
    # Store buffer
    result = await buffer_manager.store_buffer(
        session_id, user_id, buffer_data, (10, 5)
    )
    assert result is True
    
    # Retrieve buffer
    retrieved = await buffer_manager.retrieve_buffer(session_id, user_id)
    assert retrieved is not None
    assert retrieved.buffer_data == buffer_data
```

### Database Integration Tests
```python
async def test_metadata_backup_integration(buffer_manager, db_session):
    """Test session metadata backup to database"""
    # Verify buffer metadata is backed up to database
    pass
```

### Logfire Integration Tests
```python
async def test_logfire_monitoring_integration(buffer_manager):
    """Test Logfire integration for Redis operations"""
    # Verify Redis operations are logged to Logfire
    pass
```

## Error Handling and Recovery

### Redis Connection Handling
- Automatic retry logic for Redis operations
- Connection pool management with health checks
- Graceful degradation when Redis is unavailable
- Fallback to database storage for critical sessions
- Circuit breaker pattern for Redis failures

### Data Integrity
- Checksum validation for buffer data
- Atomic operations for buffer updates
- Transaction rollback on partial failures
- Data corruption detection and recovery
- Backup verification and restoration testing

## Monitoring and Observability

### Logfire Integration
- Redis operation performance metrics
- Buffer storage and retrieval statistics
- Compression efficiency tracking
- Recovery operation success rates
- Error rate monitoring and alerting

### Performance Metrics
- Redis operation latency percentiles
- Buffer size distribution statistics
- Compression ratio effectiveness
- Memory usage patterns
- Recovery time measurements

## Next Implementation Steps

1. **Complete Redis persistence layer** with all CRUD operations
2. **Implement buffer compression optimization** for large terminal outputs
3. **Add session recovery automation** with comprehensive testing
4. **Create Redis connection pooling** for scalability
5. **Add comprehensive monitoring** with Logfire integration
6. **Implement security controls** for data protection
7. **Create integration tests** with session manager and database

## Commit Guidelines

Focus on:
- **Atomic commits** for individual persistence features
- **Security validation** for all Redis operations
- **Test coverage** for persistence and recovery scenarios
- **Performance optimization** with compression and caching
- **Integration verification** with existing session management
- **Documentation updates** for persistence API changes