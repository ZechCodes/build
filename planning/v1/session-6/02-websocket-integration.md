# Session 6.2: WebSocket Integration & Authentication

## Objective
Implement WebSocket gateway for real-time session communication with comprehensive authentication, connection management, and message routing.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for WebSocket connection monitoring and debugging
- **Session 2**: Integrates with JWT authentication for connection security
- **Session 4**: Extends terminal session WebSocket foundation
- **Session 5**: Connects to database for session validation and logging

## Core Implementation

### WebSocket Gateway Service
**Location**: `session-manager/websocket/gateway.py`

```python
# session-manager/websocket/gateway.py
import asyncio
import json
import time
from typing import Dict, Any, Optional, Set
from dataclasses import dataclass
import structlog
import logfire
from fastapi import WebSocket, WebSocketDisconnect, HTTPException
from jose import JWTError, jwt

logger = structlog.get_logger()

@dataclass
class ConnectionInfo:
    websocket: WebSocket
    user_id: str
    session_id: str
    connected_at: float
    last_ping: float
    is_authenticated: bool = False

class WebSocketGateway:
    def __init__(self, session_manager, auth_service, jwt_secret: str):
        self.session_manager = session_manager
        self.auth_service = auth_service
        self.jwt_secret = jwt_secret
        self.connections: Dict[str, ConnectionInfo] = {}
        self.user_connections: Dict[str, Set[str]] = {}
        self.heartbeat_task: Optional[asyncio.Task] = None
        self.cleanup_interval = 30  # seconds
        
    async def initialize(self):
        """Initialize WebSocket gateway"""
        self.heartbeat_task = asyncio.create_task(self._heartbeat_loop())
        logger.info("WebSocket gateway initialized")
        logfire.info("WebSocket gateway started", service="session-manager")
    
    async def handle_connection(self, websocket: WebSocket, token: str):
        """Handle new WebSocket connection"""
        connection_id = None
        try:
            # Validate authentication token
            user_data = await self._validate_token(token)
            if not user_data:
                await websocket.close(code=4001, reason="Authentication failed")
                return
                
            user_id = user_data["user_id"]
            connection_id = self._generate_connection_id(user_id)
            
            # Accept connection
            await websocket.accept()
            
            # Create connection info
            connection_info = ConnectionInfo(
                websocket=websocket,
                user_id=user_id,
                session_id="",  # Will be set when session is joined
                connected_at=time.time(),
                last_ping=time.time(),
                is_authenticated=True
            )
            
            # Store connection
            self.connections[connection_id] = connection_info
            if user_id not in self.user_connections:
                self.user_connections[user_id] = set()
            self.user_connections[user_id].add(connection_id)
            
            logger.info("WebSocket connection established", 
                       connection_id=connection_id, user_id=user_id)
            
            # Log to Logfire with tracing
            with logfire.span("websocket_connection_established") as span:
                span.set_attribute("connection_id", connection_id)
                span.set_attribute("user_id", user_id)
                span.set_attribute("connection_count", len(self.connections))
                
                logfire.info("New WebSocket connection", 
                           connection_id=connection_id,
                           user_id=user_id,
                           total_connections=len(self.connections))
            
            # Handle messages
            await self._handle_messages(connection_id)
            
        except WebSocketDisconnect:
            logger.info("WebSocket connection disconnected", 
                       connection_id=connection_id)
        except Exception as e:
            logger.error("WebSocket connection error", 
                        connection_id=connection_id, error=str(e))
        finally:
            if connection_id:
                await self._cleanup_connection(connection_id)
    
    async def _handle_messages(self, connection_id: str):
        """Handle incoming WebSocket messages"""
        connection = self.connections[connection_id]
        websocket = connection.websocket
        
        try:
            while True:
                # Receive message
                message = await websocket.receive_text()
                connection.last_ping = time.time()
                
                # Parse message
                try:
                    data = json.loads(message)
                    message_type = data.get("type")
                    
                    # Route message based on type
                    if message_type == "join_session":
                        await self._handle_join_session(connection_id, data)
                    elif message_type == "session_command":
                        await self._handle_session_command(connection_id, data)
                    elif message_type == "ping":
                        await self._handle_ping(connection_id)
                    else:
                        await self._send_error(connection_id, f"Unknown message type: {message_type}")
                        
                except json.JSONDecodeError:
                    await self._send_error(connection_id, "Invalid JSON message")
                    
        except WebSocketDisconnect:
            pass  # Normal disconnection
        except Exception as e:
            logger.error("Message handling error", 
                        connection_id=connection_id, error=str(e))
            
    async def _handle_join_session(self, connection_id: str, data: Dict[str, Any]):
        """Handle session join request"""
        try:
            session_id = data.get("session_id")
            if not session_id:
                await self._send_error(connection_id, "Missing session_id")
                return
                
            connection = self.connections[connection_id]
            user_id = connection.user_id
            
            # Validate session ownership
            session = await self.session_manager.get_session(session_id)
            if not session or session.user_id != user_id:
                await self._send_error(connection_id, "Session not found or access denied")
                return
                
            # Update connection with session ID
            connection.session_id = session_id
            
            # Send success response
            await self._send_message(connection_id, {
                "type": "session_joined",
                "session_id": session_id,
                "session_state": session.state.value
            })
            
            logger.info("User joined session via WebSocket", 
                       connection_id=connection_id, 
                       user_id=user_id, 
                       session_id=session_id)
            
            # Log to Logfire
            logfire.info("Session joined via WebSocket",
                        connection_id=connection_id,
                        user_id=user_id,
                        session_id=session_id)
                        
        except Exception as e:
            logger.error("Failed to handle join session", 
                        connection_id=connection_id, error=str(e))
            await self._send_error(connection_id, "Failed to join session")
```

## TDD Implementation Cycle

### Test-Driven Development Process

1. **Red Phase**: Write failing WebSocket tests
   ```bash
   # Create WebSocket test file
   touch session-manager/tests/test_websocket_gateway.py
   
   # Run failing test
   pytest session-manager/tests/test_websocket_gateway.py::test_websocket_authentication -v
   ```

2. **Green Phase**: Implement minimal WebSocket functionality
   ```bash
   # Implement basic WebSocket handling
   pytest session-manager/tests/test_websocket_gateway.py::test_websocket_authentication -v
   ```

3. **Refactor Phase**: Optimize WebSocket performance
   ```bash
   # Refactor connection management
   pytest session-manager/tests/ -v
   ```

4. **Commit**: Commit WebSocket functionality
   ```bash
   git add session-manager/websocket/ session-manager/tests/test_websocket_gateway.py
   git commit -m "feat: implement WebSocket gateway with authentication
   
   - Add WebSocketGateway class with connection management
   - Implement JWT token validation for WebSocket connections
   - Add message routing for session join and commands
   - Include heartbeat mechanism for connection health
   - Integrate with Logfire for WebSocket monitoring
   
   Tests: Added comprehensive WebSocket test suite with auth validation
   Security: JWT authentication and session ownership validation
   Integration: Connected to session manager and auth service"
   ```

### WebSocket Test Cases

```python
# session-manager/tests/test_websocket_gateway.py
import pytest
import asyncio
from unittest.mock import AsyncMock, MagicMock
from fastapi.testclient import TestClient
from fastapi import FastAPI
from session_manager.websocket.gateway import WebSocketGateway

@pytest.fixture
def auth_service_mock():
    """Mock authentication service"""
    service = AsyncMock()
    service.validate_token = AsyncMock(return_value={"user_id": "user123"})
    return service

@pytest.fixture
def session_manager_mock():
    """Mock session manager"""
    manager = AsyncMock()
    session_mock = MagicMock()
    session_mock.user_id = "user123"
    session_mock.state.value = "active"
    manager.get_session = AsyncMock(return_value=session_mock)
    return manager

@pytest.fixture
def websocket_gateway(session_manager_mock, auth_service_mock):
    """WebSocket gateway instance"""
    return WebSocketGateway(
        session_manager_mock, 
        auth_service_mock, 
        "test-jwt-secret"
    )

class TestWebSocketGateway:
    async def test_websocket_authentication_success(self, websocket_gateway):
        """Test successful WebSocket authentication"""
        # This would be implemented with a WebSocket test client
        # Testing authentication flow and connection establishment
        pass
        
    async def test_websocket_authentication_failure(self, websocket_gateway):
        """Test WebSocket authentication failure"""
        # Test invalid token handling
        pass
        
    async def test_session_join_success(self, websocket_gateway):
        """Test successful session join via WebSocket"""
        # Test session join message handling
        pass
        
    async def test_session_join_unauthorized(self, websocket_gateway):
        """Test unauthorized session join attempt"""
        # Test cross-user session access prevention
        pass
```

## Security Checklist for WebSocket Integration

### WebSocket Security Controls
- [ ] JWT token validation on all WebSocket connections
- [ ] Connection rate limiting (10 connections per user max)
- [ ] Session ownership validation before allowing session join
- [ ] Message size limits to prevent DoS attacks (1MB max per message)
- [ ] Connection timeout for idle connections (10 minutes)
- [ ] Secure WebSocket protocol (WSS) enforcement in production
- [ ] Cross-origin request validation for WebSocket upgrades
- [ ] Input validation and sanitization for all WebSocket messages
- [ ] Audit logging for all WebSocket authentication attempts
- [ ] Protection against WebSocket hijacking attacks

### Message Security
- [ ] Message type validation with allow-list approach
- [ ] JSON schema validation for all incoming messages
- [ ] Command authorization based on session ownership
- [ ] Protection against message injection attacks
- [ ] Rate limiting on message frequency (100 messages/minute per connection)
- [ ] Secure error messages that don't leak system information
- [ ] Message encryption for sensitive session data
- [ ] Protection against replay attacks with message timestamps
- [ ] Validation of message size and structure
- [ ] Monitoring for suspicious message patterns

### Integration Security
- [ ] Secure integration with session manager authentication
- [ ] Protected session state access through proper authorization
- [ ] Database connection security for session validation
- [ ] Logfire integration excludes sensitive authentication data
- [ ] Error handling prevents information disclosure through WebSocket errors

## Performance Requirements

### WebSocket Operations
- Connection establishment < 500ms
- Message processing latency < 50ms
- Heartbeat interval every 30 seconds
- Connection cleanup within 60 seconds of disconnect
- Memory usage < 50KB per active connection
- Support for 1000+ concurrent connections

### Message Throughput
- Message routing latency < 10ms
- Session join operation < 200ms
- Authentication validation < 100ms
- Error response generation < 50ms
- Broadcast to multiple connections < 100ms
- Message queue processing rate > 1000 messages/second

## Integration Testing

### Authentication Integration Tests
```python
async def test_jwt_authentication_integration(websocket_gateway, auth_service):
    """Test WebSocket JWT authentication integration"""
    # Test real JWT validation flow
    pass

async def test_session_manager_integration(websocket_gateway, session_manager):
    """Test session manager integration for WebSocket operations"""
    # Test session validation and state management
    pass
```

### Database Integration Tests
```python
async def test_connection_logging_integration(websocket_gateway, db_session):
    """Test WebSocket connection logging to database"""
    # Verify connection events are logged to database
    pass
```

### Logfire Integration Tests
```python
async def test_logfire_monitoring_integration(websocket_gateway):
    """Test Logfire integration for WebSocket monitoring"""
    # Verify WebSocket events are logged to Logfire
    pass
```

## Error Handling and Recovery

### Connection Recovery
- Automatic reconnection logic on client side
- Session state preservation during brief disconnections
- Graceful degradation when WebSocket is unavailable
- Fallback to HTTP polling for compatibility
- Connection state synchronization after reconnection

### Error Response Patterns
```python
async def _send_error(self, connection_id: str, error_message: str):
    """Send standardized error response"""
    error_response = {
        "type": "error",
        "message": error_message,
        "timestamp": time.time(),
        "connection_id": connection_id
    }
    await self._send_message(connection_id, error_response)
```

## Monitoring and Observability

### Logfire Integration
- Connection establishment and disconnection events
- Message routing and processing metrics
- Authentication success/failure rates
- Performance metrics for WebSocket operations
- Error tracking and debugging information

### Metrics Collection
- Active connection count per user
- Message processing latency distribution
- Authentication failure rates
- Connection duration statistics
- Memory usage per connection

## Next Implementation Steps

1. **Complete WebSocket gateway implementation** with all message types
2. **Add comprehensive authentication validation** with JWT integration
3. **Implement connection pooling and management** for scalability
4. **Create WebSocket client library** for frontend integration
5. **Add monitoring and metrics collection** with Logfire
6. **Implement security controls** for rate limiting and validation
7. **Create integration tests** with other session manager components

## Commit Guidelines

Each commit should include:
- **Feature implementation** with proper error handling
- **Security validation** for all user inputs and connections
- **Test coverage** for new functionality (>80%)
- **Integration verification** with existing services
- **Performance considerations** and optimization
- **Documentation updates** for API changes