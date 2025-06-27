"""Message handlers for WebSocket terminal communication."""

import asyncio
from typing import Dict, Any, Optional
from fastapi import WebSocketDisconnect
import structlog

from .connections import TerminalConnectionManager, ConnectionState
from .protocols import MessageProtocol, MessageType
from .mock_pty import MockPTYManager

logger = structlog.get_logger(__name__)


class MessageRouter:
    """Routes and handles WebSocket messages."""
    
    def __init__(self, connection_manager: TerminalConnectionManager):
        self.connection_manager = connection_manager
        self.protocol = MessageProtocol()
        self.pty_manager = MockPTYManager()
        
        # Message handlers mapping
        self.handlers = {
            MessageType.TERMINAL_DATA: self._handle_terminal_data,
            MessageType.TERMINAL_RESIZE: self._handle_terminal_resize,
            MessageType.SESSION_CREATE: self._handle_session_create,
            MessageType.SESSION_JOIN: self._handle_session_join,
            MessageType.SESSION_RESTORE: self._handle_session_restore,
            MessageType.HEARTBEAT_RESPONSE: self._handle_heartbeat_response,
        }
    
    async def route_message(self, connection_id: str, raw_message: str):
        """Route an incoming message to the appropriate handler."""
        try:
            message = self.protocol.decode_message(raw_message)
            message_type = MessageType(message["type"])
            
            connection = self.connection_manager.get_connection(connection_id)
            if not connection:
                logger.warning("Message from unknown connection", connection_id=connection_id)
                return
            
            logger.debug("Routing message", 
                        connection_id=connection_id,
                        message_type=message_type.value,
                        user_id=connection.user_id)
            
            if message_type in self.handlers:
                await self.handlers[message_type](connection_id, message)
            else:
                logger.warning("Unknown message type", 
                             type=message_type.value, 
                             connection_id=connection_id)
                await self._send_error(connection_id, "UNKNOWN_MESSAGE_TYPE", 
                                     f"Unknown message type: {message_type.value}")
                
        except Exception as e:
            logger.error("Message routing error", 
                        connection_id=connection_id, 
                        error=str(e))
            await self._send_error(connection_id, "ROUTING_ERROR", str(e))
    
    async def _handle_terminal_data(self, connection_id: str, message: Dict[str, Any]):
        """Handle terminal data input from client."""
        connection = self.connection_manager.get_connection(connection_id)
        if not connection or connection.state != ConnectionState.BOUND:
            await self._send_error(connection_id, "NOT_BOUND", "Connection not bound to session")
            return
        
        if not connection.session_id:
            await self._send_error(connection_id, "NO_SESSION", "No active session")
            return
        
        data = message.get("data")
        if not self.protocol.validate_terminal_data(data):
            await self._send_error(connection_id, "INVALID_DATA", "Invalid terminal data")
            return
        
        try:
            # Convert to bytes if string
            if isinstance(data, str):
                data = data.encode('utf-8')
            
            # Send to mock PTY
            await self.pty_manager.send_input(connection.session_id, data)
            
            logger.debug("Terminal data sent to PTY", 
                        session_id=connection.session_id,
                        data_size=len(data))
            
        except Exception as e:
            logger.error("Failed to send terminal data", 
                        session_id=connection.session_id,
                        error=str(e))
            await self._send_error(connection_id, "PTY_ERROR", "Failed to send data to terminal")
    
    async def _handle_terminal_resize(self, connection_id: str, message: Dict[str, Any]):
        """Handle terminal resize request."""
        connection = self.connection_manager.get_connection(connection_id)
        if not connection or connection.state != ConnectionState.BOUND:
            await self._send_error(connection_id, "NOT_BOUND", "Connection not bound to session")
            return
        
        if not connection.session_id:
            await self._send_error(connection_id, "NO_SESSION", "No active session")
            return
        
        resize_data = message.get("data")
        if not self.protocol.validate_resize_data(resize_data):
            await self._send_error(connection_id, "INVALID_RESIZE", "Invalid resize data")
            return
        
        rows = resize_data["rows"]
        cols = resize_data["cols"]
        
        try:
            # Send resize to mock PTY
            await self.pty_manager.resize_terminal(connection.session_id, rows, cols)
            
            logger.info("Terminal resized", 
                       session_id=connection.session_id,
                       rows=rows, cols=cols)
            
        except Exception as e:
            logger.error("Failed to resize terminal", 
                        session_id=connection.session_id,
                        error=str(e))
            await self._send_error(connection_id, "RESIZE_ERROR", "Failed to resize terminal")
    
    async def _handle_session_create(self, connection_id: str, message: Dict[str, Any]):
        """Handle session creation request."""
        connection = self.connection_manager.get_connection(connection_id)
        if not connection or connection.state != ConnectionState.AUTHENTICATED:
            await self._send_error(connection_id, "NOT_AUTHENTICATED", "Connection not authenticated")
            return
        
        vm_id = message.get("data", {}).get("vm_id", "default-vm")
        
        try:
            # Create mock terminal session
            session_id = await self.pty_manager.create_session(vm_id, connection.user_id)
            
            # Bind connection to session
            if await self.connection_manager.bind_session(connection_id, session_id):
                # Send session created response
                response = self.protocol.create_session_created_message(session_id)
                await self.connection_manager.send_to_connection(connection_id, 
                                                               json.loads(response))
                
                # Start terminal output streaming
                asyncio.create_task(self._stream_terminal_output(session_id))
                
                logger.info("Session created and bound", 
                           session_id=session_id,
                           connection_id=connection_id,
                           user_id=connection.user_id)
            else:
                await self._send_error(connection_id, "BIND_FAILED", "Failed to bind to session")
                
        except Exception as e:
            logger.error("Failed to create session", 
                        connection_id=connection_id,
                        error=str(e))
            await self._send_error(connection_id, "SESSION_CREATE_FAILED", str(e))
    
    async def _handle_session_join(self, connection_id: str, message: Dict[str, Any]):
        """Handle session join request."""
        connection = self.connection_manager.get_connection(connection_id)
        if not connection or connection.state != ConnectionState.AUTHENTICATED:
            await self._send_error(connection_id, "NOT_AUTHENTICATED", "Connection not authenticated")
            return
        
        session_id = message.get("session_id")
        if not session_id:
            await self._send_error(connection_id, "NO_SESSION_ID", "Session ID required")
            return
        
        try:
            # Verify session exists and user has access
            if not await self.pty_manager.session_exists(session_id):
                await self._send_error(connection_id, "SESSION_NOT_FOUND", "Session not found")
                return
            
            # Bind connection to session
            if await self.connection_manager.bind_session(connection_id, session_id):
                # Send session status
                response = self.protocol.create_session_status_message(session_id, "joined")
                await self.connection_manager.send_to_connection(connection_id, 
                                                               json.loads(response))
                
                logger.info("Session joined", 
                           session_id=session_id,
                           connection_id=connection_id,
                           user_id=connection.user_id)
            else:
                await self._send_error(connection_id, "BIND_FAILED", "Failed to bind to session")
                
        except Exception as e:
            logger.error("Failed to join session", 
                        session_id=session_id,
                        error=str(e))
            await self._send_error(connection_id, "SESSION_JOIN_FAILED", str(e))
    
    async def _handle_session_restore(self, connection_id: str, message: Dict[str, Any]):
        """Handle session restore request."""
        connection = self.connection_manager.get_connection(connection_id)
        if not connection or connection.state != ConnectionState.AUTHENTICATED:
            await self._send_error(connection_id, "NOT_AUTHENTICATED", "Connection not authenticated")
            return
        
        session_id = message.get("session_id")
        if not session_id:
            await self._send_error(connection_id, "NO_SESSION_ID", "Session ID required")
            return
        
        try:
            # Get session history from PTY manager
            history = await self.pty_manager.get_session_history(session_id)
            
            if history is None:
                await self._send_error(connection_id, "SESSION_NOT_FOUND", "Session not found")
                return
            
            # Bind connection to session
            if await self.connection_manager.bind_session(connection_id, session_id):
                # Send recovery message with history
                response = self.protocol.create_session_recovered_message(session_id, history)
                await self.connection_manager.send_to_connection(connection_id, 
                                                               json.loads(response))
                
                # Start terminal output streaming
                asyncio.create_task(self._stream_terminal_output(session_id))
                
                logger.info("Session restored", 
                           session_id=session_id,
                           connection_id=connection_id,
                           user_id=connection.user_id,
                           history_lines=len(history))
            else:
                await self._send_error(connection_id, "BIND_FAILED", "Failed to bind to session")
                
        except Exception as e:
            logger.error("Failed to restore session", 
                        session_id=session_id,
                        error=str(e))
            await self._send_error(connection_id, "SESSION_RESTORE_FAILED", str(e))
    
    async def _handle_heartbeat_response(self, connection_id: str, message: Dict[str, Any]):
        """Handle heartbeat response from client."""
        connection = self.connection_manager.get_connection(connection_id)
        if connection:
            connection.last_heartbeat = message.get("timestamp", time.time())
            logger.debug("Heartbeat received", connection_id=connection_id)
    
    async def _stream_terminal_output(self, session_id: str):
        """Stream terminal output to all connected clients."""
        try:
            async for output_data in self.pty_manager.get_output_stream(session_id):
                if output_data:
                    # Create terminal data message
                    message_str = self.protocol.create_terminal_data_message(
                        output_data, session_id
                    )
                    message = json.loads(message_str)
                    
                    # Send to all connections for this session
                    sent_count = await self.connection_manager.send_to_session(
                        session_id, message
                    )
                    
                    if sent_count == 0:
                        # No connections, stop streaming
                        break
                        
        except Exception as e:
            logger.error("Terminal output streaming error", 
                        session_id=session_id,
                        error=str(e))
    
    async def _send_error(self, connection_id: str, error_code: str, error_message: str):
        """Send an error message to the client."""
        try:
            error_msg = self.protocol.create_error_message(error_code, error_message)
            await self.connection_manager.send_to_connection(
                connection_id, json.loads(error_msg)
            )
        except Exception as e:
            logger.error("Failed to send error message", 
                        connection_id=connection_id,
                        error=str(e))


import json
import time