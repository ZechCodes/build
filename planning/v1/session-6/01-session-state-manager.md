# Session 6.1: Session State Manager Implementation

## Objective
Implement the core session state management system with Redis persistence, providing reliable session recovery and state synchronization across the platform.

## Integration with Previous Sessions
- **Session 1**: Extends existing Logfire integration for session monitoring
- **Session 2**: Integrates with authentication system for session security
- **Session 3**: Connects to VM manager for VM session state tracking
- **Session 4**: Uses WebSocket connections established in terminal sessions
- **Session 5**: Leverages database models for session metadata storage

## Core Implementation

### SessionStateManager
**Location**: `session-manager/core/state_manager.py`

```python
# session-manager/core/state_manager.py
import asyncio
import json
import time
from typing import Dict, Any, Optional, List
from dataclasses import dataclass, asdict
from enum import Enum
import redis.asyncio as redis
import structlog
import logfire

logger = structlog.get_logger()

class SessionState(Enum):
    INITIALIZING = "initializing"
    ACTIVE = "active"
    IDLE = "idle"
    SUSPENDED = "suspended"
    TERMINATED = "terminated"
    ERROR = "error"

@dataclass
class SessionContext:
    session_id: str
    user_id: str
    vm_id: str
    state: SessionState
    created_at: float
    last_activity: float
    terminal_size: tuple[int, int]
    environment_vars: Dict[str, str]
    working_directory: str
    active_processes: List[Dict[str, Any]]
    metadata: Dict[str, Any]

class SessionStateManager:
    def __init__(self, redis_client: redis.Redis):
        self.redis = redis_client
        self.sessions: Dict[str, SessionContext] = {}
        self.state_key_prefix = "session:state:"
        self.activity_key_prefix = "session:activity:"
        self.cleanup_task: Optional[asyncio.Task] = None
        
    async def initialize(self):
        """Initialize the session state manager"""
        # Start cleanup task for expired sessions
        self.cleanup_task = asyncio.create_task(self._cleanup_loop())
        
        # Load active sessions from Redis
        await self._load_active_sessions()
        
        logger.info("Session state manager initialized", 
                   active_sessions=len(self.sessions))
        
        # Log to Logfire
        logfire.info("Session state manager started", 
                    session_count=len(self.sessions))
    
    async def create_session(self, user_id: str, vm_id: str, 
                           terminal_size: tuple[int, int] = (80, 24),
                           environment_vars: Dict[str, str] = None) -> str:
        """Create a new session"""
        try:
            session_id = self._generate_session_id(user_id, vm_id)
            current_time = time.time()
            
            context = SessionContext(
                session_id=session_id,
                user_id=user_id,
                vm_id=vm_id,
                state=SessionState.INITIALIZING,
                created_at=current_time,
                last_activity=current_time,
                terminal_size=terminal_size,
                environment_vars=environment_vars or {},
                working_directory="/home/user",
                active_processes=[],
                metadata={}
            )
            
            # Store in memory
            self.sessions[session_id] = context
            
            # Persist to Redis
            await self._persist_session(context)
            
            logger.info("Session created", session_id=session_id, 
                       user_id=user_id, vm_id=vm_id)
            
            # Log to Logfire with tracing
            logfire.info("Session created successfully",
                        session_id=session_id,
                        user_id=user_id,
                        vm_id=vm_id,
                        terminal_size=terminal_size)
            
            return session_id
            
        except Exception as e:
            logger.error("Failed to create session", user_id=user_id, 
                        vm_id=vm_id, error=str(e))
            logfire.error("Session creation failed", 
                         user_id=user_id, vm_id=vm_id, error=str(e))
            raise
    
    async def update_session_state(self, session_id: str, 
                                 new_state: SessionState) -> bool:
        """Update session state"""
        try:
            if session_id not in self.sessions:
                return False
            
            context = self.sessions[session_id]
            old_state = context.state
            context.state = new_state
            context.last_activity = time.time()
            
            # Persist to Redis
            await self._persist_session(context)
            
            logger.info("Session state updated", session_id=session_id,
                       old_state=old_state.value, new_state=new_state.value)
            
            # Log state transition to Logfire
            logfire.info("Session state transition",
                        session_id=session_id,
                        from_state=old_state.value,
                        to_state=new_state.value,
                        user_id=context.user_id)
            
            return True
            
        except Exception as e:
            logger.error("Failed to update session state", 
                        session_id=session_id, error=str(e))
            return False
```

## TDD Implementation Cycle

### Test-Driven Development Process

1. **Red Phase**: Write failing tests
   ```bash
   # Create test file first
   touch session-manager/tests/test_state_manager.py
   
   # Write failing test
   pytest session-manager/tests/test_state_manager.py::test_create_session -v
   ```

2. **Green Phase**: Implement minimal code to pass
   ```bash
   # Implement just enough to pass the test
   pytest session-manager/tests/test_state_manager.py::test_create_session -v
   ```

3. **Refactor Phase**: Clean up and optimize
   ```bash
   # Refactor and ensure tests still pass
   pytest session-manager/tests/ -v
   ```

4. **Commit**: Commit the working feature
   ```bash
   git add session-manager/core/state_manager.py session-manager/tests/test_state_manager.py
   git commit -m "feat: implement session state manager with Redis persistence
   
   - Add SessionStateManager class with create/update operations
   - Implement SessionContext dataclass for state tracking
   - Add Redis persistence for session recovery
   - Include comprehensive error handling and logging
   - Integrate with Logfire for observability
   
   Tests: Added test coverage for session creation and state transitions"
   ```

### Core Test Cases

```python
# session-manager/tests/test_state_manager.py
import pytest
import asyncio
from unittest.mock import AsyncMock, MagicMock
from session_manager.core.state_manager import SessionStateManager, SessionState

@pytest.fixture
async def redis_mock():
    """Mock Redis client"""
    redis_client = AsyncMock()
    redis_client.set = AsyncMock()
    redis_client.get = AsyncMock()
    redis_client.delete = AsyncMock()
    return redis_client

@pytest.fixture
async def state_manager(redis_mock):
    """SessionStateManager instance with mocked Redis"""
    manager = SessionStateManager(redis_mock)
    await manager.initialize()
    return manager

class TestSessionStateManager:
    async def test_create_session_success(self, state_manager):
        """Test successful session creation"""
        # Arrange
        user_id = "user123"
        vm_id = "vm456"
        
        # Act
        session_id = await state_manager.create_session(user_id, vm_id)
        
        # Assert
        assert session_id is not None
        assert session_id in state_manager.sessions
        session = state_manager.sessions[session_id]
        assert session.user_id == user_id
        assert session.vm_id == vm_id
        assert session.state == SessionState.INITIALIZING
    
    async def test_update_session_state(self, state_manager):
        """Test session state updates"""
        # Arrange
        session_id = await state_manager.create_session("user123", "vm456")
        
        # Act
        result = await state_manager.update_session_state(
            session_id, SessionState.ACTIVE
        )
        
        # Assert
        assert result is True
        session = state_manager.sessions[session_id]
        assert session.state == SessionState.ACTIVE
```

## Security Checklist for Session State Management

### Session Security Controls
- [ ] Session ID generation uses cryptographically secure random values
- [ ] Session ownership validation on all state operations  
- [ ] Cross-user session access prevention with authorization checks
- [ ] Session enumeration protection through opaque identifiers
- [ ] Rate limiting on session creation (5 sessions per user max)
- [ ] Session timeout enforcement for idle sessions (30 minutes)
- [ ] Secure session cleanup with proper state transitions
- [ ] Audit logging for all session state changes
- [ ] Redis connection security with authentication and encryption
- [ ] Session data encryption in Redis storage

### Integration Security
- [ ] Authentication integration validates user permissions before session creation
- [ ] VM manager integration verifies VM ownership before session association  
- [ ] WebSocket connection validation ensures session ownership
- [ ] Database integration uses parameterized queries to prevent injection
- [ ] Logfire integration excludes sensitive session data from logs

## Performance Requirements

### Session Operations
- Session creation latency < 200ms
- State update operations < 50ms  
- Session recovery from Redis < 500ms
- Memory usage < 1MB per 1000 active sessions
- Redis operations timeout after 5 seconds
- Cleanup cycle runs every 5 minutes

### Integration Performance
- Database queries complete within 100ms
- Redis persistence operations < 100ms
- Logfire logging overhead < 10ms per operation
- VM manager integration calls < 200ms
- WebSocket state synchronization < 50ms

## Integration Testing

### Redis Integration Tests
```python
async def test_redis_persistence(state_manager):
    """Test session persistence to Redis"""
    session_id = await state_manager.create_session("user123", "vm456")
    
    # Verify Redis storage
    state_manager.redis.set.assert_called()
    call_args = state_manager.redis.set.call_args
    assert call_args[0][0].startswith("session:state:")
    assert session_id in call_args[0][0]
```

### Database Integration Tests  
```python
async def test_database_integration(state_manager, db_session):
    """Test session metadata storage in database"""
    session_id = await state_manager.create_session("user123", "vm456")
    
    # Verify database record
    session_record = await db_session.get(SessionModel, session_id)
    assert session_record is not None
    assert session_record.state == "initializing"
```

## Next Implementation Steps

1. **Complete SessionStateManager class** with all CRUD operations
2. **Implement Redis persistence layer** with error handling
3. **Add session recovery mechanisms** for service restarts  
4. **Create comprehensive test suite** with >80% coverage
5. **Integrate with authentication system** for security validation
6. **Add Logfire monitoring** for session lifecycle tracking
7. **Implement cleanup and maintenance** procedures

## Commit Guidelines

Each commit should follow this pattern:
```bash
git add <files>
git commit -m "<type>: <description>

- <specific change 1>
- <specific change 2>  
- <specific change 3>

Tests: <test coverage description>
Integration: <integration points validated>
Security: <security measures implemented>"
```

Example commit types: `feat`, `fix`, `test`, `refactor`, `security`, `perf`