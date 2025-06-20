# Session 6 API Documentation

## Overview

Session 6 implements comprehensive session management and recovery capabilities for the Claude Code Platform. This document provides detailed API documentation for all components.

## Components

### 1. Session State Manager

Manages session lifecycle, state persistence, and cleanup operations.

#### SessionStateManager Class

```python
class SessionStateManager:
    def __init__(self, redis_client: redis.Redis)
```

**Methods:**

#### `initialize() -> None`
Initialize the session state manager and start cleanup tasks.

```python
await state_manager.initialize()
```

#### `create_session(user_id: str, vm_id: str, terminal_size: tuple = (80, 24), environment_vars: Dict[str, str] = None) -> str`
Create a new session.

**Parameters:**
- `user_id`: Unique identifier for the user
- `vm_id`: Unique identifier for the VM
- `terminal_size`: Terminal dimensions as (columns, rows)
- `environment_vars`: Environment variables for the session

**Returns:** Session ID string

**Example:**
```python
session_id = await state_manager.create_session(
    user_id="user_123",
    vm_id="vm_456",
    terminal_size=(120, 30),
    environment_vars={"TERM": "xterm-256color"}
)
```

#### `get_session(session_id: str) -> Optional[SessionContext]`
Retrieve session by ID.

**Parameters:**
- `session_id`: Session identifier

**Returns:** SessionContext object or None if not found

#### `update_session_state(session_id: str, new_state: SessionState) -> bool`
Update session state.

**Parameters:**
- `session_id`: Session identifier
- `new_state`: New session state (INITIALIZING, ACTIVE, IDLE, SUSPENDED, TERMINATED, ERROR)

**Returns:** True if successful, False otherwise

#### `delete_session(session_id: str) -> bool`
Delete a session and cleanup all related data.

**Parameters:**
- `session_id`: Session identifier

**Returns:** True if successful, False otherwise

#### `get_user_sessions(user_id: str) -> List[SessionContext]`
Get all sessions for a user.

**Parameters:**
- `user_id`: User identifier

**Returns:** List of SessionContext objects

### 2. WebSocket Gateway

Handles WebSocket connections and real-time communication.

#### WebSocketGateway Class

```python
class WebSocketGateway:
    def __init__(self, session_manager: SessionStateManager, auth_service, jwt_secret: str)
```

**Methods:**

#### `initialize() -> None`
Initialize the WebSocket gateway.

```python
await gateway.initialize()
```

#### `handle_connection(websocket, path: str) -> None`
Handle incoming WebSocket connections.

**Parameters:**
- `websocket`: WebSocket connection object
- `path`: Connection path

#### `send_to_session(session_id: str, message: Dict[str, Any]) -> bool`
Send message to all connections in a session.

**Parameters:**
- `session_id`: Target session ID
- `message`: Message to send

**Returns:** True if sent successfully

#### `get_connection_count() -> int`
Get total active connection count.

**Returns:** Number of active connections

### 3. Session Buffer Manager

Manages terminal buffer persistence and retrieval.

#### SessionBufferManager Class

```python
class SessionBufferManager:
    def __init__(self, redis_client: redis.Redis)
```

**Methods:**

#### `store_buffer(session_id: str, user_id: str, buffer_data: bytes, cursor_pos: tuple, scroll_pos: int = 0) -> bool`
Store session buffer data.

**Parameters:**
- `session_id`: Session identifier
- `user_id`: User identifier (for authorization)
- `buffer_data`: Terminal buffer content
- `cursor_pos`: Cursor position as (x, y)
- `scroll_pos`: Scroll position

**Returns:** True if stored successfully

**Example:**
```python
success = await buffer_manager.store_buffer(
    session_id="sess_123",
    user_id="user_456",
    buffer_data=b"Terminal output\n$ command\nresult\n",
    cursor_pos=(0, 2),
    scroll_pos=0
)
```

#### `retrieve_buffer(session_id: str, user_id: str) -> Optional[SessionBufferData]`
Retrieve session buffer data.

**Parameters:**
- `session_id`: Session identifier
- `user_id`: User identifier (for authorization)

**Returns:** SessionBufferData object or None

#### `clear_buffer(session_id: str, user_id: str) -> bool`
Clear session buffer.

**Parameters:**
- `session_id`: Session identifier
- `user_id`: User identifier (for authorization)

**Returns:** True if cleared successfully

### 4. Recovery Manager

Handles session recovery operations.

#### RecoveryManager Class

```python
class RecoveryManager:
    def __init__(self, state_manager: SessionStateManager, buffer_manager: SessionBufferManager, websocket_gateway: WebSocketGateway)
```

**Methods:**

#### `initialize() -> None`
Initialize the recovery manager.

```python
await recovery_manager.initialize()
```

#### `initiate_recovery(session_id: str, user_id: str, connection_id: str) -> bool`
Initiate session recovery process.

**Parameters:**
- `session_id`: Session to recover
- `user_id`: User requesting recovery
- `connection_id`: WebSocket connection ID

**Returns:** True if recovery initiated successfully

**Example:**
```python
success = await recovery_manager.initiate_recovery(
    session_id="sess_123",
    user_id="user_456",
    connection_id="conn_789"
)
```

#### `get_active_recoveries() -> Dict[str, RecoveryContext]`
Get currently active recovery operations.

**Returns:** Dictionary mapping session IDs to recovery contexts

#### `get_recovery_stats() -> Dict[str, Any]`
Get recovery statistics.

**Returns:** Statistics dictionary including total attempts, success rate, etc.

### 5. Performance Monitor

Monitors and tracks session performance metrics.

#### SessionPerformanceMonitor Class

```python
class SessionPerformanceMonitor:
    def __init__(self, state_manager: SessionStateManager, websocket_gateway: WebSocketGateway, buffer_manager: SessionBufferManager)
```

**Methods:**

#### `initialize() -> None`
Initialize performance monitoring.

```python
await monitor.initialize()
```

#### `record_operation(operation_name: str, duration_ms: float = None, success: bool = True) -> None`
Record operation performance.

**Parameters:**
- `operation_name`: Name of the operation
- `duration_ms`: Operation duration in milliseconds
- `success`: Whether operation was successful

**Example:**
```python
monitor.record_operation("session_created", duration_ms=150, success=True)
```

#### `get_current_metrics() -> PerformanceMetrics`
Get current performance metrics.

**Returns:** PerformanceMetrics object with current statistics

#### `get_performance_summary(duration_minutes: int = 60) -> Dict[str, Any]`
Get performance summary for specified duration.

**Parameters:**
- `duration_minutes`: Time window for summary

**Returns:** Performance summary dictionary

#### `set_thresholds(thresholds: Dict[str, float]) -> None`
Set performance thresholds for alerting.

**Parameters:**
- `thresholds`: Dictionary of threshold values

## Data Models

### SessionContext

```python
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
```

### SessionState Enum

```python
class SessionState(Enum):
    INITIALIZING = "initializing"
    ACTIVE = "active"
    IDLE = "idle"
    SUSPENDED = "suspended"
    TERMINATED = "terminated"
    ERROR = "error"
```

### SessionBufferData

```python
@dataclass
class SessionBufferData:
    session_id: str
    user_id: str
    buffer_data: bytes
    cursor_position: tuple[int, int]
    scroll_position: int
    last_updated: float
    size_bytes: int
    line_count: int
```

### PerformanceMetrics

```python
@dataclass
class PerformanceMetrics:
    timestamp: float
    session_count: int
    active_connections: int
    operations_per_second: float
    average_response_time: float
    error_rate: float
    memory_usage_mb: float
    redis_latency_ms: float
```

## Error Handling

All API methods use structured error handling:

### Common Exception Types

- `SessionNotFoundError`: Session does not exist
- `UnauthorizedAccessError`: User lacks permission for operation
- `SessionStateError`: Invalid state transition
- `BufferStorageError`: Buffer storage operation failed
- `RecoveryError`: Recovery operation failed
- `RateLimitExceededError`: Too many requests

### Example Error Handling

```python
try:
    session_id = await state_manager.create_session(user_id, vm_id)
except ValueError as e:
    logger.error("Invalid session parameters", error=str(e))
except Exception as e:
    logger.error("Session creation failed", error=str(e))
    raise
```

## WebSocket Message Protocol

### Message Types

#### Join Session
```json
{
    "type": "join_session",
    "session_id": "sess_123",
    "auth_token": "jwt_token_here"
}
```

#### Session Command
```json
{
    "type": "session_command",
    "session_id": "sess_123",
    "command": "ls -la",
    "timestamp": 1640995200
}
```

#### Ping/Heartbeat
```json
{
    "type": "ping",
    "timestamp": 1640995200
}
```

### Response Messages

#### Session Joined
```json
{
    "type": "session_joined",
    "session_id": "sess_123",
    "status": "success"
}
```

#### Session Output
```json
{
    "type": "session_output",
    "session_id": "sess_123",
    "data": "terminal output here",
    "timestamp": 1640995200
}
```

#### Error Response
```json
{
    "type": "error",
    "error_code": "UNAUTHORIZED",
    "message": "Invalid session access",
    "timestamp": 1640995200
}
```

## Configuration

### Environment Variables

- `REDIS_URL`: Redis connection URL
- `JWT_SECRET`: JWT signing secret
- `SESSION_TIMEOUT`: Session timeout in seconds (default: 3600)
- `BUFFER_SIZE_LIMIT`: Maximum buffer size in bytes (default: 1048576)
- `MAX_CONNECTIONS_PER_USER`: Maximum WebSocket connections per user (default: 10)
- `RECOVERY_RATE_LIMIT`: Recovery attempts per hour (default: 10)

### Redis Configuration

Session 6 requires Redis with the following features:
- Persistence enabled
- Memory limit configured
- Eviction policy: `allkeys-lru`

## Performance Targets

Session 6 meets the following performance targets:

- **Session Creation**: < 200ms response time
- **Session Retrieval**: < 50ms response time
- **State Updates**: < 25ms response time
- **Buffer Operations**: < 100ms write, < 25ms read
- **Recovery Initiation**: < 200ms response time
- **Memory Usage**: < 100MB per 1000 sessions

## Security Features

### Authentication & Authorization
- JWT token validation for all operations
- User ownership validation for session access
- Cross-user access prevention

### Data Protection
- Session ID cryptographic randomness (256-bit entropy)
- Buffer data size limits and validation
- Input sanitization and injection protection

### Rate Limiting
- Session creation: 5 sessions/minute per user
- WebSocket connections: 10 connections per user
- Recovery attempts: 10 attempts/hour per user

### Monitoring & Auditing
- All operations logged with structured logging
- Security events tracked and alerted
- Performance metrics collected and monitored

## Usage Examples

### Basic Session Workflow

```python
import asyncio
from session_manager import SessionStateManager, SessionBufferManager
import redis.asyncio as redis

async def basic_workflow():
    # Initialize components
    redis_client = redis.from_url("redis://localhost:6379")
    state_manager = SessionStateManager(redis_client)
    buffer_manager = SessionBufferManager(redis_client)
    
    await state_manager.initialize()
    
    try:
        # Create session
        session_id = await state_manager.create_session(
            user_id="user_123",
            vm_id="vm_456"
        )
        
        # Store buffer data
        await buffer_manager.store_buffer(
            session_id=session_id,
            user_id="user_123",
            buffer_data=b"Welcome to terminal!\n$ ",
            cursor_pos=(2, 1)
        )
        
        # Update session state
        await state_manager.update_session_state(
            session_id, SessionState.ACTIVE
        )
        
        # Retrieve session
        session = await state_manager.get_session(session_id)
        print(f"Session state: {session.state}")
        
    finally:
        await state_manager.stop()

# Run the workflow
asyncio.run(basic_workflow())
```

### WebSocket Integration

```python
import websockets
import json

async def websocket_client():
    uri = "ws://localhost:8000/ws"
    
    async with websockets.connect(uri) as websocket:
        # Join session
        join_message = {
            "type": "join_session",
            "session_id": "sess_123",
            "auth_token": "your_jwt_token"
        }
        await websocket.send(json.dumps(join_message))
        
        # Listen for messages
        async for message in websocket:
            data = json.loads(message)
            print(f"Received: {data}")

# Run the client
asyncio.run(websocket_client())
```

### Recovery Operation

```python
async def recovery_example():
    # Setup components (state_manager, buffer_manager, gateway)
    recovery_manager = RecoveryManager(
        state_manager, buffer_manager, gateway
    )
    await recovery_manager.initialize()
    
    try:
        # Initiate recovery
        success = await recovery_manager.initiate_recovery(
            session_id="sess_123",
            user_id="user_456",
            connection_id="conn_789"
        )
        
        if success:
            print("Recovery initiated successfully")
            
            # Check recovery status
            active_recoveries = recovery_manager.get_active_recoveries()
            if "sess_123" in active_recoveries:
                print("Recovery in progress")
        
    finally:
        await recovery_manager.stop()

# Run recovery
asyncio.run(recovery_example())
```

## Troubleshooting

### Common Issues

1. **Redis Connection Errors**
   - Verify Redis is running and accessible
   - Check Redis connection URL format
   - Ensure Redis has sufficient memory

2. **Session Creation Failures**
   - Validate user_id and vm_id parameters
   - Check Redis write permissions
   - Monitor Redis memory usage

3. **WebSocket Authentication Failures**
   - Verify JWT token validity
   - Check JWT secret configuration
   - Ensure token has required claims

4. **Buffer Storage Issues**
   - Check buffer size limits
   - Verify user authorization
   - Monitor Redis storage capacity

5. **Recovery Failures**
   - Check session ownership
   - Verify session exists and is recoverable
   - Monitor recovery rate limits

### Debugging

Enable debug logging:
```python
import logging
logging.getLogger("session_manager").setLevel(logging.DEBUG)
```

Monitor performance:
```python
metrics = await monitor.get_current_metrics()
print(f"Active sessions: {metrics.session_count}")
print(f"Response time: {metrics.average_response_time}ms")
```

## Migration Guide

When upgrading from previous versions:

1. **Database Schema**: No schema changes required for Session 6
2. **Configuration**: Add new environment variables
3. **API Changes**: New methods added, existing methods unchanged
4. **Dependencies**: Update Redis client to support async operations

## Contributing

For contributing to Session 6 development:

1. Follow TDD approach with comprehensive test coverage
2. Ensure all security tests pass
3. Validate performance targets are met
4. Update documentation for API changes
5. Use structured logging with appropriate log levels