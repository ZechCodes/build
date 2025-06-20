"""
WebSocket Gateway for Session Management

Provides real-time WebSocket communication with comprehensive authentication,
connection management, and message routing for terminal sessions.
"""
import asyncio
import json
import time
import uuid
from typing import Dict, Any, Optional, Set
from dataclasses import dataclass
import structlog
import logfire
from fastapi import WebSocket, WebSocketDisconnect, HTTPException
from jose import JWTError, jwt

logger = structlog.get_logger()


@dataclass
class ConnectionInfo:
    """WebSocket connection information"""
    websocket: WebSocket
    user_id: str
    session_id: str
    connected_at: float
    last_ping: float
    is_authenticated: bool = False


class WebSocketGateway:
    """WebSocket gateway for real-time session communication"""
    
    def __init__(self, session_manager, auth_service, jwt_secret: str):
        self.session_manager = session_manager
        self.auth_service = auth_service
        self.jwt_secret = jwt_secret
        self.connections: Dict[str, ConnectionInfo] = {}
        self.user_connections: Dict[str, Set[str]] = {}
        self.heartbeat_task: Optional[asyncio.Task] = None
        self.cleanup_interval = 30  # seconds
        self.max_connections_per_user = 10
        
    async def initialize(self):
        """Initialize WebSocket gateway"""
        self.heartbeat_task = asyncio.create_task(self._heartbeat_loop())
        logger.info("WebSocket gateway initialized")
        logfire.info("WebSocket gateway started", service="session-manager")
    
    async def stop(self):
        """Stop WebSocket gateway"""
        if self.heartbeat_task:
            self.heartbeat_task.cancel()
            try:
                await self.heartbeat_task
            except asyncio.CancelledError:
                pass
        
        # Close all connections
        for connection_info in list(self.connections.values()):
            try:
                await connection_info.websocket.close()
            except Exception:
                pass
        
        self.connections.clear()
        self.user_connections.clear()
        
        logger.info("WebSocket gateway stopped")
    
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
            
            # Check connection limits
            if not await self._check_connection_limits(user_id):
                await websocket.close(code=4003, reason="Connection limit exceeded")
                return
            
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
    
    async def _validate_token(self, token: str) -> Optional[Dict[str, Any]]:
        """Validate JWT token"""
        try:
            # Decode JWT token
            payload = jwt.decode(
                token, 
                self.jwt_secret, 
                algorithms=["HS256"]
            )
            
            # Check expiration
            if payload.get("exp", 0) < time.time():
                logger.warning("Expired JWT token")
                return None
            
            # Validate required fields
            user_id = payload.get("user_id")
            if not user_id:
                logger.warning("JWT token missing user_id")
                return None
            
            return {"user_id": user_id}
            
        except JWTError as e:
            logger.warning("JWT validation failed", error=str(e))
            return None
        except Exception as e:
            logger.error("Token validation error", error=str(e))
            return None
    
    async def _check_connection_limits(self, user_id: str) -> bool:
        """Check if user has exceeded connection limits"""
        try:
            user_connection_count = len(self.user_connections.get(user_id, set()))
            
            if user_connection_count >= self.max_connections_per_user:
                logger.warning("User connection limit exceeded", 
                             user_id=user_id, 
                             current_connections=user_connection_count,
                             limit=self.max_connections_per_user)
                return False
            
            return True
            
        except Exception as e:
            logger.error("Connection limit check failed", error=str(e))
            return False
    
    def _generate_connection_id(self, user_id: str) -> str:
        """Generate unique connection ID"""
        base_id = str(uuid.uuid4())
        timestamp = str(int(time.time() * 1000))
        return f"conn_{base_id}_{timestamp}"
    
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
                logfire.warning("Unauthorized session access attempt",
                              connection_id=connection_id,
                              user_id=user_id,
                              session_id=session_id)
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
    
    async def _handle_session_command(self, connection_id: str, data: Dict[str, Any]):
        """Handle session command"""
        try:
            # This would route commands to the appropriate session
            # For now, just acknowledge
            await self._send_message(connection_id, {
                "type": "command_ack",
                "status": "received"
            })
            
        except Exception as e:
            logger.error("Failed to handle session command", 
                        connection_id=connection_id, error=str(e))
            await self._send_error(connection_id, "Failed to process command")
    
    async def _handle_ping(self, connection_id: str):
        """Handle ping message"""
        try:
            await self._send_message(connection_id, {
                "type": "pong",
                "timestamp": time.time()
            })
            
        except Exception as e:
            logger.error("Failed to handle ping", 
                        connection_id=connection_id, error=str(e))
    
    async def _send_message(self, connection_id: str, message: Dict[str, Any]):
        """Send message to WebSocket connection"""
        try:
            if connection_id not in self.connections:
                return False
            
            connection = self.connections[connection_id]
            await connection.websocket.send_text(json.dumps(message))
            return True
            
        except Exception as e:
            logger.error("Failed to send WebSocket message", 
                        connection_id=connection_id, error=str(e))
            return False
    
    async def _send_error(self, connection_id: str, error_message: str):
        """Send error response"""
        error_response = {
            "type": "error",
            "message": error_message,
            "timestamp": time.time(),
            "connection_id": connection_id
        }
        await self._send_message(connection_id, error_response)
    
    async def _cleanup_connection(self, connection_id: str):
        """Cleanup connection state"""
        try:
            if connection_id not in self.connections:
                return
            
            connection = self.connections[connection_id]
            user_id = connection.user_id
            
            # Remove from connections
            self.connections.pop(connection_id, None)
            
            # Remove from user connections
            if user_id in self.user_connections:
                self.user_connections[user_id].discard(connection_id)
                if not self.user_connections[user_id]:
                    self.user_connections.pop(user_id, None)
            
            logger.info("WebSocket connection cleaned up", 
                       connection_id=connection_id, user_id=user_id)
            
        except Exception as e:
            logger.error("Connection cleanup error", 
                        connection_id=connection_id, error=str(e))
    
    async def _heartbeat_loop(self):
        """Heartbeat loop for connection health"""
        while True:
            try:
                await asyncio.sleep(self.cleanup_interval)
                await self._check_connection_health()
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Heartbeat loop error", error=str(e))
    
    async def _check_connection_health(self):
        """Check connection health and cleanup stale connections"""
        try:
            current_time = time.time()
            stale_connections = []
            
            for connection_id, connection in self.connections.items():
                # Consider connections stale after 5 minutes without ping
                if current_time - connection.last_ping > 300:
                    stale_connections.append(connection_id)
            
            for connection_id in stale_connections:
                logger.info("Cleaning up stale connection", connection_id=connection_id)
                await self._cleanup_connection(connection_id)
                
        except Exception as e:
            logger.error("Connection health check error", error=str(e))
    
    async def send_to_connection(self, connection_id: str, message: Dict[str, Any]) -> bool:
        """Send message to specific connection (external interface)"""
        return await self._send_message(connection_id, message)
    
    async def broadcast_to_user(self, user_id: str, message: Dict[str, Any]) -> int:
        """Broadcast message to all user connections"""
        try:
            sent_count = 0
            user_connection_ids = self.user_connections.get(user_id, set())
            
            for connection_id in list(user_connection_ids):
                if await self._send_message(connection_id, message):
                    sent_count += 1
            
            return sent_count
            
        except Exception as e:
            logger.error("Broadcast to user failed", user_id=user_id, error=str(e))
            return 0
    
    def get_connection_count(self) -> int:
        """Get total connection count"""
        return len(self.connections)
    
    def get_user_connection_count(self, user_id: str) -> int:
        """Get connection count for specific user"""
        return len(self.user_connections.get(user_id, set()))