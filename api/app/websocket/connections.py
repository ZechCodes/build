"""Enhanced WebSocket connection manager for terminal sessions."""

import asyncio
import time
import uuid
import weakref
from typing import Dict, Set, Optional, Any, List
from fastapi import WebSocket, WebSocketDisconnect
from enum import Enum
import structlog

from .auth import WebSocketAuthenticator
from .security import (
    get_security_validator, 
    get_connection_token_manager, 
    get_replay_detector
)
from .encryption import get_message_encryption, get_secure_protocol
from .redis_storage import get_redis_storage, EncryptedRedisStorage
from .rate_limiting import get_websocket_rate_limiter
from .metrics import get_websocket_metrics
from .alerts import get_websocket_alert_manager, log_alert_handler
from .acknowledgment import get_message_ack_system, MessagePriority
from .operational_metrics import get_operational_metrics
from .audit_logger import (
    get_audit_logger, AuditEventType, AuditSeverity,
    audit_connection_established, audit_authentication_success, 
    audit_authentication_failure
)
from app.models.user import User

logger = structlog.get_logger(__name__)


class ConnectionState(Enum):
    CONNECTING = "connecting"
    CONNECTED = "connected"
    AUTHENTICATED = "authenticated"
    BOUND = "bound"  # Bound to a terminal session
    DISCONNECTED = "disconnected"
    ERROR = "error"


class TerminalWebSocketConnection:
    """Enhanced WebSocket connection for terminal sessions."""
    
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
        self.user: Optional[User] = None
        # Security enhancements
        self.security_info: Dict[str, Any] = {}
        self.connection_token: Optional[str] = None
        
    async def send_message(self, message: Dict[str, Any]):
        """Send a message to the WebSocket client with encryption for sensitive data."""
        try:
            # Apply encryption for sensitive messages
            secure_protocol = get_secure_protocol()
            encrypted_message = secure_protocol.prepare_outgoing_message(message, self.connection_id)
            
            # Record message metrics
            from .metrics import get_websocket_metrics
            metrics = await get_websocket_metrics()
            message_type = message.get("type", "unknown")
            message_size = len(str(encrypted_message))
            metrics.record_message_sent(self.connection_id, message_type, message_size)
            
            # Record operational metrics
            operational_metrics = await get_operational_metrics()
            operational_metrics.record_message_sent(self.connection_id, message_size)
            
            async with self.send_lock:
                await self.websocket.send_json(encrypted_message)
                logger.debug("Message sent", connection_id=self.connection_id, 
                           message_type=message.get("type"),
                           encrypted=encrypted_message.get("type") == "encrypted_message")
        except Exception as e:
            logger.error("Failed to send message", 
                        connection_id=self.connection_id, error=str(e))
            # Record message error
            from .metrics import get_websocket_metrics
            metrics = await get_websocket_metrics()
            metrics.record_message_error(self.connection_id, "send_failed", message.get("type"))
            self.state = ConnectionState.ERROR
            raise
    
    async def send_reliable_message(self, message: Dict[str, Any], 
                                  priority: MessagePriority = MessagePriority.NORMAL,
                                  timeout_seconds: float = 30.0) -> str:
        """Send a message that requires acknowledgment for reliable delivery."""
        try:
            # Get acknowledgment system
            ack_system = await get_message_ack_system()
            
            # Send message with acknowledgment tracking
            message_id = ack_system.send_message_with_ack(
                self.connection_id,
                message,
                timeout_seconds=timeout_seconds,
                priority=priority
            )
            
            # Send the message immediately
            await self.send_message(message)
            
            logger.debug("Reliable message sent", 
                        connection_id=self.connection_id,
                        message_id=message_id,
                        priority=priority.name)
            
            return message_id
            
        except Exception as e:
            logger.error("Failed to send reliable message", 
                        connection_id=self.connection_id, error=str(e))
            raise


class TerminalConnectionManager:
    """Enhanced connection manager for terminal WebSocket connections."""
    
    def __init__(self, authenticator: WebSocketAuthenticator):
        self.authenticator = authenticator
        self.connections: Dict[str, TerminalWebSocketConnection] = {}
        self.user_connections: Dict[str, Set[str]] = {}  # user_id -> connection_ids
        self.session_connections: Dict[str, Set[str]] = {}  # session_id -> connection_ids
        self.heartbeat_interval = 30  # seconds
        self.max_connections_per_user = 5
        self.max_connections_per_session = 3
        self.heartbeat_task: Optional[asyncio.Task] = None
        self.cleanup_task: Optional[asyncio.Task] = None
        self.redis_storage: Optional[EncryptedRedisStorage] = None
        self.alert_manager = None
        
    async def start(self):
        """Start the connection manager with Redis storage."""
        try:
            # Initialize Redis storage
            self.redis_storage = await get_redis_storage()
            
            # Initialize alert manager
            self.alert_manager = await get_websocket_alert_manager()
            self.alert_manager.add_notification_handler(log_alert_handler)
            
            # Start background tasks
            self.heartbeat_task = asyncio.create_task(self._heartbeat_loop())
            self.cleanup_task = asyncio.create_task(self._cleanup_loop())
            
            logger.info("Terminal connection manager started with Redis storage and alerting")
            
        except Exception as e:
            logger.error("Failed to start connection manager", error=str(e))
            # Continue without Redis if it fails
            self.heartbeat_task = asyncio.create_task(self._heartbeat_loop())
            logger.warning("Terminal connection manager started without Redis storage")
    
    async def stop(self):
        """Stop the connection manager."""
        if self.heartbeat_task:
            self.heartbeat_task.cancel()
        
        if self.cleanup_task:
            self.cleanup_task.cancel()
        
        # Close all connections
        for connection_id in list(self.connections.keys()):
            await self.disconnect(connection_id)
        
        # Close Redis storage
        if self.redis_storage:
            await self.redis_storage.close()
        
        # Shutdown alert manager
        if self.alert_manager:
            await self.alert_manager.shutdown()
            
        logger.info("Terminal connection manager stopped")
    
    async def connect(self, websocket: WebSocket) -> str:
        """Accept a new WebSocket connection with security validation."""
        connection_id = str(uuid.uuid4())
        
        # Record connection attempt
        metrics = await get_websocket_metrics()
        client_ip = websocket.headers.get("x-real-ip") or websocket.headers.get("x-forwarded-for") or "unknown"
        user_agent = websocket.headers.get("user-agent") or "unknown"
        metrics.record_connection_attempt(connection_id, client_ip, user_agent)
        
        # Perform security validation before accepting connection
        security_validator = get_security_validator()
        security_result = security_validator.validate_connection_security(websocket)
        
        if not security_result["valid"]:
            logger.warning("WebSocket connection rejected due to security validation", 
                         connection_id=connection_id,
                         errors=security_result["errors"])
            metrics.record_connection_failed(connection_id, "security_validation")
            await websocket.close(code=1008)  # Policy violation
            raise ValueError(f"Connection security validation failed: {security_result['errors']}")
        
        # Check connection rate limits
        client_ip = security_result["security_info"].get("x_real_ip") or \
                   security_result["security_info"].get("x_forwarded_for") or "unknown"
        
        rate_limiter = await get_websocket_rate_limiter()
        allowed, violation_reason = await rate_limiter.check_rate_limit(
            connection_id, "anonymous", client_ip, "connection", 0
        )
        
        if not allowed:
            logger.warning("WebSocket connection rejected due to rate limiting", 
                         connection_id=connection_id,
                         client_ip=client_ip,
                         violation_reason=violation_reason)
            metrics.record_connection_failed(connection_id, f"rate_limit_{violation_reason}")
            await websocket.close(code=1008)  # Policy violation
            raise ValueError(f"Connection rate limit exceeded: {violation_reason}")
        
        # Log security warnings if any
        if security_result["warnings"]:
            logger.warning("WebSocket connection security warnings", 
                         connection_id=connection_id,
                         warnings=security_result["warnings"])
        
        connection = TerminalWebSocketConnection(websocket, connection_id)
        connection.security_info = security_result["security_info"]
        
        # Generate connection token for hijacking prevention
        token_manager = get_connection_token_manager()
        connection.connection_token = token_manager.generate_connection_token(
            connection_id, connection.security_info
        )
        
        await websocket.accept()
        connection.state = ConnectionState.CONNECTED
        
        self.connections[connection_id] = connection
        
        # Store connection state in Redis
        if self.redis_storage:
            connection_state = {
                "connection_id": connection_id,
                "connected_at": connection.connected_at,
                "security_info": connection.security_info,
                "connection_token": connection.connection_token,
                "state": connection.state.value
            }
            await self.redis_storage.store_connection_state(connection_id, connection_state)
        
        # Record successful connection establishment
        metrics.record_connection_established(connection_id)
        
        # Register with operational metrics
        operational_metrics = await get_operational_metrics()
        operational_metrics.register_connection(connection_id)
        
        # Audit connection establishment
        await audit_connection_established(
            connection_id, 
            client_ip,
            user_agent
        )
        
        logger.info("WebSocket connected with security validation", 
                   connection_id=connection_id,
                   origin=connection.security_info.get("origin"),
                   user_agent=connection.security_info.get("user_agent"))
        return connection_id
    
    async def authenticate(self, connection_id: str) -> bool:
        """Authenticate a WebSocket connection."""
        if connection_id not in self.connections:
            return False
        
        connection = self.connections[connection_id]
        
        try:
            # Record auth attempt
            metrics = await get_websocket_metrics()
            metrics.record_auth_attempt(connection_id, "unknown")
            
            user = await self.authenticator.authenticate_websocket(connection.websocket)
            
            # Check connection limits
            user_id = str(user.id)
            if user_id in self.user_connections:
                if len(self.user_connections[user_id]) >= self.max_connections_per_user:
                    logger.warning("Max connections per user exceeded", 
                                 user_id=user_id, 
                                 max_connections=self.max_connections_per_user)
                    await connection.websocket.close(code=1008)  # Policy violation
                    return False
            
            connection.user_id = user_id
            connection.user = user
            connection.state = ConnectionState.AUTHENTICATED
            
            # Track user connections
            if user_id not in self.user_connections:
                self.user_connections[user_id] = set()
            self.user_connections[user_id].add(connection_id)
            
            # Record successful authentication
            metrics.record_auth_success(connection_id, user_id)
            
            # Update operational metrics with user info
            operational_metrics = await get_operational_metrics()
            if connection_id in operational_metrics.connection_resources:
                operational_metrics.connection_resources[connection_id].user_id = user_id
            
            # Audit successful authentication
            await audit_authentication_success(
                connection_id, 
                user_id, 
                connection.security_info.get("x_real_ip") or connection.security_info.get("x_forwarded_for")
            )
            
            logger.info("WebSocket authenticated", 
                       connection_id=connection_id, 
                       user_id=user_id,
                       email=user.email)
            return True
            
        except Exception as e:
            logger.error("WebSocket authentication failed", 
                        connection_id=connection_id, error=str(e))
            # Record authentication failure
            metrics.record_auth_failure(connection_id, str(e))
            
            # Audit authentication failure
            await audit_authentication_failure(
                connection_id,
                connection.security_info.get("x_real_ip") or connection.security_info.get("x_forwarded_for"),
                str(e)
            )
            
            await connection.websocket.close(code=1008)  # Policy violation
            return False
    
    async def bind_session(self, connection_id: str, session_id: str) -> bool:
        """Bind connection to a terminal session."""
        if connection_id not in self.connections:
            return False
        
        connection = self.connections[connection_id]
        
        if connection.state != ConnectionState.AUTHENTICATED:
            logger.warning("Cannot bind unauthenticated connection", 
                          connection_id=connection_id)
            return False
        
        # Check session connection limits
        if session_id in self.session_connections:
            if len(self.session_connections[session_id]) >= self.max_connections_per_session:
                logger.warning("Max connections per session exceeded", 
                             session_id=session_id,
                             max_connections=self.max_connections_per_session)
                return False
        
        connection.session_id = session_id
        connection.state = ConnectionState.BOUND
        
        # Track session connections
        if session_id not in self.session_connections:
            self.session_connections[session_id] = set()
        self.session_connections[session_id].add(connection_id)
        
        # Update operational metrics with session info
        operational_metrics = await get_operational_metrics()
        if connection_id in operational_metrics.connection_resources:
            operational_metrics.connection_resources[connection_id].session_id = session_id
        
        # Audit session binding
        audit_logger = await get_audit_logger()
        await audit_logger.log_session_event(
            AuditEventType.SESSION_BOUND,
            connection_id,
            session_id,
            connection.user_id
        )
        
        logger.info("WebSocket bound to session", 
                   connection_id=connection_id, 
                   session_id=session_id,
                   user_id=connection.user_id)
        return True
    
    async def disconnect(self, connection_id: str):
        """Disconnect a WebSocket connection."""
        if connection_id not in self.connections:
            return
        
        connection = self.connections[connection_id]
        connection.state = ConnectionState.DISCONNECTED
        
        # Clean up tracking
        if connection.user_id:
            user_connections = self.user_connections.get(connection.user_id, set())
            user_connections.discard(connection_id)
            if not user_connections:
                del self.user_connections[connection.user_id]
        
        if connection.session_id:
            session_connections = self.session_connections.get(connection.session_id, set())
            session_connections.discard(connection_id)
            if not session_connections:
                del self.session_connections[connection.session_id]
        
        try:
            await connection.websocket.close()
        except:
            pass  # Connection might already be closed
        
        # Clean up Redis state
        if self.redis_storage:
            await self.redis_storage.delete_connection_state(connection_id)
            
            # Update user and session mappings in Redis
            if connection.user_id:
                user_connections = list(self.user_connections.get(connection.user_id, set()))
                await self.redis_storage.store_user_connections(connection.user_id, user_connections)
            
            if connection.session_id:
                session_connections = list(self.session_connections.get(connection.session_id, set()))
                await self.redis_storage.store_session_connections(connection.session_id, session_connections)
        
        del self.connections[connection_id]
        
        # Record connection closure
        metrics = await get_websocket_metrics()
        metrics.record_connection_closed(connection_id, "normal")
        
        # Unregister from operational metrics
        operational_metrics = await get_operational_metrics()
        operational_metrics.unregister_connection(connection_id)
        
        # Audit connection closure
        audit_logger = await get_audit_logger()
        await audit_logger.log_connection_event(
            AuditEventType.CONNECTION_CLOSED,
            connection_id,
            connection.security_info.get("x_real_ip") or connection.security_info.get("x_forwarded_for") or "unknown",
            connection.security_info.get("user_agent") or "unknown",
            connection.user_id,
            details={
                "session_id": connection.session_id,
                "connection_duration": time.time() - connection.connected_at
            }
        )
        
        logger.info("WebSocket disconnected", 
                   connection_id=connection_id,
                   user_id=connection.user_id,
                   session_id=connection.session_id)
    
    async def send_to_connection(self, connection_id: str, message: Dict[str, Any]) -> bool:
        """Send message to specific connection."""
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
        """Send message to all connections for a session."""
        if session_id not in self.session_connections:
            return 0
        
        sent_count = 0
        for connection_id in list(self.session_connections[session_id]):
            if await self.send_to_connection(connection_id, message):
                sent_count += 1
        
        return sent_count
    
    async def send_to_user(self, user_id: str, message: Dict[str, Any]) -> int:
        """Send message to all connections for a user."""
        if user_id not in self.user_connections:
            return 0
        
        sent_count = 0
        for connection_id in list(self.user_connections[user_id]):
            if await self.send_to_connection(connection_id, message):
                sent_count += 1
        
        return sent_count
    
    def get_connection(self, connection_id: str) -> Optional[TerminalWebSocketConnection]:
        """Get connection by ID."""
        return self.connections.get(connection_id)
    
    def get_user_connections(self, user_id: str) -> List[str]:
        """Get all connection IDs for a user."""
        return list(self.user_connections.get(user_id, set()))
    
    def get_session_connections(self, session_id: str) -> List[str]:
        """Get all connection IDs for a session."""
        return list(self.session_connections.get(session_id, set()))
    
    def get_connection_count(self) -> int:
        """Get total number of active connections."""
        return len(self.connections)
    
    async def validate_message_security(self, connection_id: str, message: Dict[str, Any]) -> bool:
        """Validate message security including replay detection."""
        if connection_id not in self.connections:
            return False
            
        connection = self.connections[connection_id]
        
        # 1. Validate connection token (hijacking prevention)
        token_manager = get_connection_token_manager()
        if not token_manager.validate_connection_token(
            connection_id, 
            connection.connection_token, 
            connection.security_info
        ):
            logger.warning("Connection token validation failed - potential hijacking", 
                         connection_id=connection_id)
            
            # Audit security violation
            audit_logger = await get_audit_logger()
            await audit_logger.log_security_event(
                AuditEventType.SECURITY_VIOLATION,
                connection_id,
                {
                    "violation_type": "connection_hijacking",
                    "details": "Connection token validation failed"
                },
                connection.user_id,
                connection.security_info.get("x_real_ip"),
                AuditSeverity.CRITICAL
            )
            
            await self.disconnect(connection_id)
            return False
        
        # 2. Check for replay attacks
        replay_detector = get_replay_detector()
        if replay_detector.is_replay(message):
            logger.warning("Replay attack detected", connection_id=connection_id)
            
            # Audit security violation
            audit_logger = await get_audit_logger()
            await audit_logger.log_security_event(
                AuditEventType.SECURITY_VIOLATION,
                connection_id,
                {
                    "violation_type": "replay_attack",
                    "details": "Message replay detected"
                },
                connection.user_id,
                connection.security_info.get("x_real_ip"),
                AuditSeverity.ERROR
            )
            
            return False
        
        # 3. Validate message security
        security_validator = get_security_validator()
        if not security_validator.validate_message_security(message, connection.security_info):
            logger.warning("Message security validation failed", connection_id=connection_id)
            
            # Audit security violation
            audit_logger = await get_audit_logger()
            await audit_logger.log_security_event(
                AuditEventType.SECURITY_VIOLATION,
                connection_id,
                {
                    "violation_type": "message_security_validation",
                    "details": "Message failed security validation"
                },
                connection.user_id,
                connection.security_info.get("x_real_ip"),
                AuditSeverity.WARNING
            )
            
            return False
        
        return True
    
    def get_user_connection_count(self, user_id: str) -> int:
        """Get connection count for a specific user."""
        return len(self.user_connections.get(user_id, set()))
    
    async def _heartbeat_loop(self):
        """Send periodic heartbeat messages."""
        while True:
            try:
                await asyncio.sleep(self.heartbeat_interval)
                current_time = time.time()
                
                # Send heartbeat to all authenticated connections
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
    
    async def _cleanup_loop(self):
        """Periodic cleanup of expired data in Redis."""
        while True:
            try:
                # Run cleanup every 5 minutes
                await asyncio.sleep(300)
                
                if self.redis_storage:
                    cleaned_count = await self.redis_storage.cleanup_expired_data()
                    if cleaned_count > 0:
                        logger.info("Cleaned up expired Redis data", count=cleaned_count)
                
                # Also cleanup token manager
                token_manager = get_connection_token_manager()
                token_manager.cleanup_expired_tokens()
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Cleanup loop error", error=str(e))


# Global connection manager instance
connection_manager: Optional[TerminalConnectionManager] = None


def get_connection_manager() -> TerminalConnectionManager:
    """Get the global connection manager instance."""
    global connection_manager
    if connection_manager is None:
        from app.security.jwt import JWTManager
        from app.core.config import get_settings
        settings = get_settings()
        jwt_manager = JWTManager(secret_key=settings.jwt_secret)
        authenticator = WebSocketAuthenticator(jwt_manager)
        connection_manager = TerminalConnectionManager(authenticator)
    return connection_manager