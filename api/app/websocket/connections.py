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
        
    async def send_message(self, message: Dict[str, Any]):
        """Send a message to the WebSocket client with error handling."""
        try:
            async with self.send_lock:
                await self.websocket.send_json(message)
                logger.debug("Message sent", connection_id=self.connection_id, 
                           message_type=message.get("type"))
        except Exception as e:
            logger.error("Failed to send message", 
                        connection_id=self.connection_id, error=str(e))
            self.state = ConnectionState.ERROR
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
        
    async def start(self):
        """Start the connection manager."""
        self.heartbeat_task = asyncio.create_task(self._heartbeat_loop())
        logger.info("Terminal connection manager started")
    
    async def stop(self):
        """Stop the connection manager."""
        if self.heartbeat_task:
            self.heartbeat_task.cancel()
        
        # Close all connections
        for connection_id in list(self.connections.keys()):
            await self.disconnect(connection_id)
            
        logger.info("Terminal connection manager stopped")
    
    async def connect(self, websocket: WebSocket) -> str:
        """Accept a new WebSocket connection."""
        connection_id = str(uuid.uuid4())
        connection = TerminalWebSocketConnection(websocket, connection_id)
        
        await websocket.accept()
        connection.state = ConnectionState.CONNECTED
        
        self.connections[connection_id] = connection
        
        logger.info("WebSocket connected", connection_id=connection_id)
        return connection_id
    
    async def authenticate(self, connection_id: str) -> bool:
        """Authenticate a WebSocket connection."""
        if connection_id not in self.connections:
            return False
        
        connection = self.connections[connection_id]
        
        try:
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
            
            logger.info("WebSocket authenticated", 
                       connection_id=connection_id, 
                       user_id=user_id,
                       email=user.email)
            return True
            
        except Exception as e:
            logger.error("WebSocket authentication failed", 
                        connection_id=connection_id, error=str(e))
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
        
        del self.connections[connection_id]
        
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