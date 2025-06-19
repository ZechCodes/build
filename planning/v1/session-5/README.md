# Session 5: WebSocket Communication Layer

## Objective
Implement robust WebSocket handling for real-time terminal communication, providing seamless bidirectional data flow between frontend and backend services.

## Overview
This session builds upon the PTY layer from Session 4 to create a comprehensive WebSocket gateway that handles authentication, message routing, connection management, and provides reliability features like reconnection and message queuing.

## Prerequisites
- Session 1-4 completed successfully
- PTY layer operational
- Authentication system functional
- VM management working

## Components to Implement

### 1. WebSocket Connection Manager
**Location**: `websocket-gateway/connections/`

#### Connection Lifecycle Management
```python
# websocket-gateway/connections/manager.py
import asyncio
import weakref
from typing import Dict, Set, Optional, Any
from fastapi import WebSocket, WebSocketDisconnect
from enum import Enum
import time
import uuid
import structlog

logger = structlog.get_logger()

class ConnectionState(Enum):
    CONNECTING = "connecting"
    CONNECTED = "connected"
    AUTHENTICATED = "authenticated"
    DISCONNECTED = "disconnected"
    ERROR = "error"

class WebSocketConnection:
    def __init__(self, websocket: WebSocket, connection_id: str):
        self.websocket = websocket
        self.connection_id = connection_id
        self.user_id: Optional[str] = None
        self.session_id: Optional[str] = None
        self.state = ConnectionState.CONNECTING
        self.connected_at = time.time()
        self.last_heartbeat = time.time()
        self.message_queue: asyncio.Queue = asyncio.Queue(maxsize=1000)
        self.send_lock = asyncio.Lock()
        
    async def send_message(self, message: Dict[str, Any]):
        """Send a message to the WebSocket client"""
        try:
            async with self.send_lock:
                await self.websocket.send_json(message)
        except Exception as e:
            logger.error("Failed to send message", 
                        connection_id=self.connection_id, error=str(e))
            self.state = ConnectionState.ERROR
            raise

class WebSocketConnectionManager:
    def __init__(self):
        self.connections: Dict[str, WebSocketConnection] = {}
        self.user_connections: Dict[str, Set[str]] = {}
        self.session_connections: Dict[str, Set[str]] = {}
        self.heartbeat_interval = 30  # seconds
        self.heartbeat_task: Optional[asyncio.Task] = None
        
    async def start(self):
        """Start the connection manager"""
        self.heartbeat_task = asyncio.create_task(self._heartbeat_loop())
        logger.info("WebSocket connection manager started")
    
    async def stop(self):
        """Stop the connection manager"""
        if self.heartbeat_task:
            self.heartbeat_task.cancel()
        
        # Close all connections
        for connection in list(self.connections.values()):
            await self.disconnect(connection.connection_id)
    
    async def connect(self, websocket: WebSocket) -> str:
        """Accept a new WebSocket connection"""
        connection_id = str(uuid.uuid4())
        connection = WebSocketConnection(websocket, connection_id)
        
        await websocket.accept()
        connection.state = ConnectionState.CONNECTED
        
        self.connections[connection_id] = connection
        
        logger.info("WebSocket connected", connection_id=connection_id)
        return connection_id
    
    async def authenticate(self, connection_id: str, user_id: str) -> bool:
        """Authenticate a WebSocket connection"""
        if connection_id not in self.connections:
            return False
        
        connection = self.connections[connection_id]
        connection.user_id = user_id
        connection.state = ConnectionState.AUTHENTICATED
        
        # Track user connections
        if user_id not in self.user_connections:
            self.user_connections[user_id] = set()
        self.user_connections[user_id].add(connection_id)
        
        logger.info("WebSocket authenticated", 
                   connection_id=connection_id, user_id=user_id)
        return True
    
    async def bind_session(self, connection_id: str, session_id: str) -> bool:
        """Bind connection to a terminal session"""
        if connection_id not in self.connections:
            return False
        
        connection = self.connections[connection_id]
        connection.session_id = session_id
        
        # Track session connections
        if session_id not in self.session_connections:
            self.session_connections[session_id] = set()
        self.session_connections[session_id].add(connection_id)
        
        logger.info("WebSocket bound to session", 
                   connection_id=connection_id, session_id=session_id)
        return True
    
    async def disconnect(self, connection_id: str):
        """Disconnect a WebSocket connection"""
        if connection_id not in self.connections:
            return
        
        connection = self.connections[connection_id]
        connection.state = ConnectionState.DISCONNECTED
        
        # Clean up tracking
        if connection.user_id:
            self.user_connections.get(connection.user_id, set()).discard(connection_id)
        if connection.session_id:
            self.session_connections.get(connection.session_id, set()).discard(connection_id)
        
        try:
            await connection.websocket.close()
        except:
            pass  # Connection might already be closed
        
        del self.connections[connection_id]
        
        logger.info("WebSocket disconnected", connection_id=connection_id)
    
    async def send_to_connection(self, connection_id: str, message: Dict[str, Any]) -> bool:
        """Send message to specific connection"""
        if connection_id not in self.connections:
            return False
        
        connection = self.connections[connection_id]
        try:
            await connection.send_message(message)
            return True
        except:
            await self.disconnect(connection_id)
            return False
    
    async def send_to_session(self, session_id: str, message: Dict[str, Any]) -> int:
        """Send message to all connections for a session"""
        if session_id not in self.session_connections:
            return 0
        
        sent_count = 0
        for connection_id in list(self.session_connections[session_id]):
            if await self.send_to_connection(connection_id, message):
                sent_count += 1
        
        return sent_count
    
    async def send_to_user(self, user_id: str, message: Dict[str, Any]) -> int:
        """Send message to all connections for a user"""
        if user_id not in self.user_connections:
            return 0
        
        sent_count = 0
        for connection_id in list(self.user_connections[user_id]):
            if await self.send_to_connection(connection_id, message):
                sent_count += 1
        
        return sent_count
    
    async def _heartbeat_loop(self):
        """Send periodic heartbeat messages"""
        while True:
            try:
                await asyncio.sleep(self.heartbeat_interval)
                current_time = time.time()
                
                # Send heartbeat to all connections
                heartbeat_message = {
                    "type": "heartbeat",
                    "timestamp": current_time
                }
                
                for connection_id in list(self.connections.keys()):
                    connection = self.connections.get(connection_id)
                    if connection and connection.state == ConnectionState.AUTHENTICATED:
                        try:
                            await connection.send_message(heartbeat_message)
                            connection.last_heartbeat = current_time
                        except:
                            await self.disconnect(connection_id)
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Heartbeat loop error", error=str(e))
```

### 2. Message Protocol Handler
**Location**: `websocket-gateway/protocols/`

#### Terminal Message Protocol
```python
# websocket-gateway/protocols/terminal.py
from typing import Dict, Any, Optional, Union
from enum import Enum
import json
import msgpack
import base64
import structlog

logger = structlog.get_logger()

class MessageType(Enum):
    # Terminal data
    TERMINAL_DATA = "terminal_data"
    TERMINAL_RESIZE = "terminal_resize"
    
    # Session management
    SESSION_CREATE = "session_create"
    SESSION_RESTORE = "session_restore"
    SESSION_STATUS = "session_status"
    
    # Authentication
    AUTH_REQUEST = "auth_request"
    AUTH_RESPONSE = "auth_response"
    
    # System
    HEARTBEAT = "heartbeat"
    ERROR = "error"
    ACK = "ack"

class MessageProtocol:
    def __init__(self, use_compression: bool = True):
        self.use_compression = use_compression
        self.max_message_size = 1024 * 1024  # 1MB
    
    def encode_message(self, message_type: MessageType, data: Any, 
                      binary: bool = False) -> Union[str, bytes]:
        """Encode a message for transmission"""
        message = {
            "type": message_type.value,
            "data": data,
            "timestamp": time.time()
        }
        
        if binary and self.use_compression:
            # Use MessagePack for binary data
            encoded = msgpack.packb(message)
            if len(encoded) > self.max_message_size:
                raise ValueError("Message too large")
            return encoded
        else:
            # Use JSON for text data
            if binary and isinstance(data, bytes):
                # Base64 encode binary data for JSON
                message["data"] = base64.b64encode(data).decode('ascii')
                message["binary"] = True
            
            encoded = json.dumps(message)
            if len(encoded.encode()) > self.max_message_size:
                raise ValueError("Message too large")
            return encoded
    
    def decode_message(self, raw_message: Union[str, bytes]) -> Dict[str, Any]:
        """Decode a received message"""
        try:
            if isinstance(raw_message, bytes):
                # MessagePack format
                message = msgpack.unpackb(raw_message, raw=False)
            else:
                # JSON format
                message = json.loads(raw_message)
                
                # Decode base64 binary data if present
                if message.get("binary") and "data" in message:
                    message["data"] = base64.b64decode(message["data"])
            
            # Validate message structure
            if not isinstance(message, dict) or "type" not in message:
                raise ValueError("Invalid message structure")
            
            return message
            
        except Exception as e:
            logger.error("Failed to decode message", error=str(e))
            raise ValueError(f"Message decode error: {e}")
    
    def create_terminal_data_message(self, data: bytes) -> bytes:
        """Create a terminal data message"""
        return self.encode_message(
            MessageType.TERMINAL_DATA, 
            data, 
            binary=True
        )
    
    def create_resize_message(self, rows: int, cols: int) -> str:
        """Create a terminal resize message"""
        return self.encode_message(
            MessageType.TERMINAL_RESIZE,
            {"rows": rows, "cols": cols}
        )
    
    def create_error_message(self, error_code: str, message: str) -> str:
        """Create an error message"""
        return self.encode_message(
            MessageType.ERROR,
            {"code": error_code, "message": message}
        )
    
    def create_session_status_message(self, session_id: str, status: str) -> str:
        """Create a session status message"""
        return self.encode_message(
            MessageType.SESSION_STATUS,
            {"session_id": session_id, "status": status}
        )

# websocket-gateway/protocols/message_router.py
class MessageRouter:
    def __init__(self, connection_manager: WebSocketConnectionManager):
        self.connection_manager = connection_manager
        self.protocol = MessageProtocol()
        self.handlers = {
            MessageType.TERMINAL_DATA: self._handle_terminal_data,
            MessageType.TERMINAL_RESIZE: self._handle_terminal_resize,
            MessageType.SESSION_CREATE: self._handle_session_create,
            MessageType.SESSION_RESTORE: self._handle_session_restore,
            MessageType.AUTH_REQUEST: self._handle_auth_request,
            MessageType.HEARTBEAT: self._handle_heartbeat,
        }
    
    async def route_message(self, connection_id: str, raw_message: Union[str, bytes]):
        """Route an incoming message to the appropriate handler"""
        try:
            message = self.protocol.decode_message(raw_message)
            message_type = MessageType(message["type"])
            
            if message_type in self.handlers:
                await self.handlers[message_type](connection_id, message)
            else:
                logger.warning("Unknown message type", 
                             type=message_type, connection_id=connection_id)
                
        except Exception as e:
            logger.error("Message routing error", 
                        connection_id=connection_id, error=str(e))
            await self._send_error(connection_id, "ROUTING_ERROR", str(e))
    
    async def _handle_terminal_data(self, connection_id: str, message: Dict[str, Any]):
        """Handle terminal data input"""
        connection = self.connection_manager.connections.get(connection_id)
        if not connection or not connection.session_id:
            return
        
        # Forward to session manager
        from ..session_manager import session_manager
        await session_manager.send_to_session(
            connection.session_id, 
            message["data"]
        )
    
    async def _handle_terminal_resize(self, connection_id: str, message: Dict[str, Any]):
        """Handle terminal resize request"""
        connection = self.connection_manager.connections.get(connection_id)
        if not connection or not connection.session_id:
            return
        
        data = message["data"]
        rows, cols = data["rows"], data["cols"]
        
        # Forward to session manager
        from ..session_manager import session_manager
        await session_manager.resize_session(
            connection.session_id, 
            rows, 
            cols
        )
    
    async def _send_error(self, connection_id: str, error_code: str, error_message: str):
        """Send an error message to the client"""
        error_msg = self.protocol.create_error_message(error_code, error_message)
        await self.connection_manager.send_to_connection(connection_id, error_msg)
```

### 3. Rate Limiting & Flow Control
**Location**: `websocket-gateway/middleware/`

#### WebSocket Rate Limiting
```python
# websocket-gateway/middleware/rate_limiting.py
import time
from typing import Dict, Any
from collections import defaultdict, deque
import asyncio
import structlog

logger = structlog.get_logger()

class WebSocketRateLimiter:
    def __init__(self):
        # Per-connection rate limits
        self.message_limits = {
            "terminal_data": {"rate": 100, "window": 1},  # 100 messages per second
            "terminal_resize": {"rate": 10, "window": 1},  # 10 resizes per second
            "default": {"rate": 50, "window": 1}  # 50 messages per second default
        }
        
        # Per-connection tracking
        self.connection_counters: Dict[str, Dict[str, deque]] = defaultdict(
            lambda: defaultdict(deque)
        )
        
        # Global rate limits (per user)
        self.global_limits = {
            "messages_per_minute": 1000,
            "bytes_per_minute": 10 * 1024 * 1024  # 10MB
        }
        
        self.user_counters: Dict[str, Dict[str, deque]] = defaultdict(
            lambda: defaultdict(deque)
        )
    
    async def check_rate_limit(self, connection_id: str, user_id: str, 
                              message_type: str, message_size: int) -> bool:
        """Check if message is within rate limits"""
        current_time = time.time()
        
        # Check connection-level rate limit
        if not self._check_connection_limit(connection_id, message_type, current_time):
            logger.warning("Connection rate limit exceeded", 
                          connection_id=connection_id, message_type=message_type)
            return False
        
        # Check user-level rate limit
        if not self._check_user_limit(user_id, message_size, current_time):
            logger.warning("User rate limit exceeded", 
                          user_id=user_id, message_size=message_size)
            return False
        
        return True
    
    def _check_connection_limit(self, connection_id: str, message_type: str, 
                               current_time: float) -> bool:
        """Check connection-level rate limit"""
        limit_config = self.message_limits.get(message_type, self.message_limits["default"])
        rate = limit_config["rate"]
        window = limit_config["window"]
        
        counter = self.connection_counters[connection_id][message_type]
        
        # Remove old entries outside the window
        while counter and counter[0] <= current_time - window:
            counter.popleft()
        
        # Check if we're within the limit
        if len(counter) >= rate:
            return False
        
        # Add current timestamp
        counter.append(current_time)
        return True
    
    def _check_user_limit(self, user_id: str, message_size: int, 
                         current_time: float) -> bool:
        """Check user-level rate limit"""
        message_counter = self.user_counters[user_id]["messages"]
        bytes_counter = self.user_counters[user_id]["bytes"]
        
        # Clean old entries (1 minute window)
        while message_counter and message_counter[0] <= current_time - 60:
            message_counter.popleft()
        
        while bytes_counter and bytes_counter[0][0] <= current_time - 60:
            bytes_counter.popleft()
        
        # Check message count limit
        if len(message_counter) >= self.global_limits["messages_per_minute"]:
            return False
        
        # Check bytes limit
        total_bytes = sum(entry[1] for entry in bytes_counter)
        if total_bytes + message_size > self.global_limits["bytes_per_minute"]:
            return False
        
        # Add current message
        message_counter.append(current_time)
        bytes_counter.append((current_time, message_size))
        
        return True
    
    def cleanup_old_data(self):
        """Cleanup old rate limiting data"""
        current_time = time.time()
        
        # Cleanup connection counters
        for connection_id in list(self.connection_counters.keys()):
            for message_type in list(self.connection_counters[connection_id].keys()):
                counter = self.connection_counters[connection_id][message_type]
                while counter and counter[0] <= current_time - 60:
                    counter.popleft()
                
                if not counter:
                    del self.connection_counters[connection_id][message_type]
            
            if not self.connection_counters[connection_id]:
                del self.connection_counters[connection_id]
        
        # Cleanup user counters
        for user_id in list(self.user_counters.keys()):
            for counter_type in list(self.user_counters[user_id].keys()):
                counter = self.user_counters[user_id][counter_type]
                if counter_type == "messages":
                    while counter and counter[0] <= current_time - 60:
                        counter.popleft()
                else:  # bytes counter
                    while counter and counter[0][0] <= current_time - 60:
                        counter.popleft()
                
                if not counter:
                    del self.user_counters[user_id][counter_type]
            
            if not self.user_counters[user_id]:
                del self.user_counters[user_id]
```

## Critical Decisions

### Message Size Limits
- **Maximum Message Size**: 1MB per message
- **Terminal Data Chunks**: 64KB recommended
- **Queue Size**: 1000 messages per connection
- **Compression**: MessagePack for binary, JSON for text

### Connection Limits
- **Per User**: Maximum 10 concurrent connections
- **Per Session**: Maximum 5 connections (for collaboration)
- **Global**: 10,000 concurrent connections per server
- **Timeout**: 1 hour inactivity timeout

### Rate Limiting
- **Terminal Data**: 100 messages/second per connection
- **Resize Events**: 10/second per connection
- **Global User Limit**: 1000 messages/minute, 10MB/minute
- **Heartbeat**: Every 30 seconds

### Reconnection Strategy
- **Auto-reconnect**: Exponential backoff (1s, 2s, 4s, 8s, max 30s)
- **Session Recovery**: 1 hour window for session restoration
- **Message Queue**: Persist undelivered messages during reconnection
- **State Synchronization**: Full state sync on reconnection

## Security Checklist ✅

### Connection Security
- [ ] Authentication required before WebSocket upgrade
- [ ] Token validation on every connection attempt
- [ ] Origin validation to prevent unauthorized access
- [ ] Connection hijacking prevention with connection tokens
- [ ] Secure WebSocket (WSS) enforced in production
- [ ] Rate limiting per connection and per user
- [ ] Connection timeout and cleanup procedures
- [ ] Session binding validation (users can only access their sessions)
- [ ] Cross-user data isolation enforced
- [ ] Connection state encryption in Redis

### Message Security
- [ ] Message size limits enforced (max 1MB)
- [ ] Input validation for all message types
- [ ] XSS prevention in terminal output
- [ ] Message replay attack prevention
- [ ] Binary data validation and sanitization
- [ ] Message compression with integrity checks
- [ ] Rate limiting on message frequency
- [ ] Malicious message pattern detection
- [ ] Message encryption for sensitive data
- [ ] Audit logging for all messages

### Transport Security
- [ ] WSS (secure WebSocket) required for production
- [ ] Certificate validation and pinning
- [ ] Perfect forward secrecy for connections
- [ ] HSTS enforcement for WebSocket upgrades
- [ ] Encrypted message payload for sensitive data
- [ ] Secure cookie handling for authentication
- [ ] Protection against man-in-the-middle attacks
- [ ] Network-level DDoS protection
- [ ] Connection state integrity verification
- [ ] Secure token transmission and storage

### Protocol Security
- [ ] Message protocol version validation
- [ ] Command injection prevention in terminal data
- [ ] Binary data integrity verification
- [ ] Message ordering and sequence validation
- [ ] Protocol downgrade attack prevention
- [ ] Message tampering detection
- [ ] Unauthorized command filtering
- [ ] Protocol-level rate limiting
- [ ] Message source authentication
- [ ] Protocol state machine validation

### Session Security
- [ ] Session ownership validation on binding
- [ ] Session hijacking prevention
- [ ] Session token encryption and rotation
- [ ] Multi-connection session management
- [ ] Session state integrity verification
- [ ] Unauthorized session access prevention
- [ ] Session timeout enforcement
- [ ] Session recovery security validation
- [ ] Cross-session data isolation
- [ ] Session audit trail maintenance

## Functional Targets

### Connection Functionality
- WebSocket connection establishment
- Message routing functionality
- Heartbeat responsiveness
- Connection cleanup functionality
- Memory usage monitoring

### Message Functionality
- Terminal data handling functionality
- Message queue processing
- Protocol encoding/decoding
- Rate limiting functionality
- Compression functionality for terminal data

### Scalability
- Support concurrent connections per server
- Handle multiple messages per second
- Support concurrent sessions
- Memory usage scaling with connections
- CPU usage monitoring under load

## Monitoring & Alerting

### Connection Metrics
- Active WebSocket connections
- Connection establishment rate
- Connection failure rate
- Authentication success/failure rate
- Connection duration distribution

### Message Metrics
- Message counts (messages/second)
- Message size distribution
- Rate limiting trigger frequency
- Message queue depth
- Protocol error rates

### Operational Metrics
- WebSocket responsiveness (connection, message)
- Memory usage per connection
- CPU usage for WebSocket handling
- Network bandwidth utilization
- Error rates and types

### Alert Conditions
- Connection failures > 5%
- Message delays detected
- Rate limiting triggers > 10% of connections
- Memory usage > 80%
- WebSocket errors > 1%

## Documentation Deliverables

### API Documentation
- [ ] WebSocket protocol specification
- [ ] Message format documentation
- [ ] Authentication flow for WebSockets
- [ ] Error codes and handling
- [ ] Rate limiting policies

### Integration Documentation
- [ ] Frontend WebSocket client integration
- [ ] Session manager integration
- [ ] Authentication service integration
- [ ] Monitoring and logging setup
- [ ] Testing WebSocket connections

## Next Steps

Upon successful completion of Session 5:
1. WebSocket gateway operational with full authentication
2. Message routing and protocol handling functional
3. Rate limiting and security measures active
4. Connection management and recovery working
5. Performance targets met under load
6. Proceed to Session 6: Session Management & Recovery

## Risk Mitigation

### Technical Risks
1. **WebSocket instability**: Connection pooling, automatic recovery
2. **Message loss**: Queue persistence, acknowledgment system
3. **Performance degradation**: Connection limits, rate limiting
4. **Memory leaks**: Connection cleanup, monitoring
5. **Protocol attacks**: Input validation, rate limiting

### Security Risks
1. **Connection hijacking**: Token validation, secure cookies
2. **DDoS attacks**: Rate limiting, connection limits
3. **Data injection**: Input validation, sanitization
4. **Session confusion**: Strong session binding, validation
5. **Protocol vulnerabilities**: Regular security audits

---

**Session 5 Success Criteria:**
- WebSocket gateway operational with full feature set
- Security measures implemented and tested
- Performance targets achieved under load
- Integration with authentication and session management
- Ready for Session 6 session persistence implementation