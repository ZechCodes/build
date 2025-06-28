"""Message handlers for WebSocket terminal communication."""

import asyncio
import json
import time
from typing import Dict, Any, Optional
from fastapi import WebSocketDisconnect
import structlog

from .connections import TerminalConnectionManager, ConnectionState
from .protocols import MessageProtocol, MessageType
from .mock_pty import MockPTYManager
from .sanitizer import get_terminal_sanitizer
from .encryption import get_secure_protocol
from .rate_limiting import get_websocket_rate_limiter
from .metrics import get_websocket_metrics
from .binary_validator import get_binary_validator, BinaryFormat
from .acknowledgment import get_message_ack_system, MessagePriority
from .audit_logger import get_audit_logger, AuditEventType, AuditSeverity, audit_message_sent
from .pattern_detector import get_pattern_detector, ThreatLevel

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
            MessageType.ACK: self._handle_acknowledgment,
        }
    
    async def route_message(self, connection_id: str, raw_message: str):
        """Route an incoming message to the appropriate handler with security validation and decryption."""
        try:
            # First decode the raw message
            message = self.protocol.decode_message(raw_message)
            
            # Apply decryption if message is encrypted
            secure_protocol = get_secure_protocol()
            decrypted_message = secure_protocol.process_incoming_message(message, connection_id)
            
            # Check if decryption resulted in an error
            if decrypted_message.get("type") == "error":
                error_data = decrypted_message.get("data", {})
                await self._send_error(connection_id, 
                                     error_data.get("code", "DECRYPTION_ERROR"),
                                     error_data.get("message", "Message processing failed"))
                return
            
            # Validate binary data if present
            if decrypted_message.get("binary") and "data" in decrypted_message:
                binary_validator = await get_binary_validator()
                validation_result = await binary_validator.validate_binary_data(
                    decrypted_message["data"], 
                    BinaryFormat.TERMINAL_DATA if decrypted_message["type"] == "terminal_data" else BinaryFormat.RAW_BYTES,
                    decrypted_message["type"]
                )
                
                if not validation_result.valid:
                    logger.warning("Binary data validation failed", 
                                 connection_id=connection_id,
                                 errors=validation_result.errors)
                    await self._send_error(connection_id, "BINARY_VALIDATION_FAILED", 
                                         f"Binary data validation failed: {'; '.join(validation_result.errors)}")
                    return
                
                # Pattern detection for binary data
                if isinstance(decrypted_message["data"], bytes):
                    binary_detection_result = pattern_detector.analyze_binary_data(
                        decrypted_message["data"], 
                        connection_id
                    )
                    
                    if binary_detection_result.detected and binary_detection_result.risk_score >= 50:
                        logger.warning("Malicious patterns detected in binary data",
                                     connection_id=connection_id,
                                     threat_level=binary_detection_result.threat_level.value,
                                     patterns=binary_detection_result.patterns_found,
                                     risk_score=binary_detection_result.risk_score)
                        
                        # Audit binary threat
                        audit_logger = await get_audit_logger()
                        await audit_logger.log_security_event(
                            AuditEventType.SECURITY_VIOLATION,
                            connection_id,
                            {
                                "violation_type": "binary_malicious_content",
                                "patterns_detected": binary_detection_result.patterns_found,
                                "threat_level": binary_detection_result.threat_level.value,
                                "risk_score": binary_detection_result.risk_score,
                                "data_size": len(decrypted_message["data"])
                            },
                            connection.user_id,
                            client_ip,
                            AuditSeverity.ERROR
                        )
                        
                        if binary_detection_result.risk_score >= 70:
                            await self._send_error(connection_id, "MALICIOUS_BINARY_DATA", 
                                                 "Malicious content detected in binary data.")
                            return
                
                if validation_result.warnings:
                    logger.warning("Binary data validation warnings", 
                                 connection_id=connection_id,
                                 warnings=validation_result.warnings)
            
            message_type = MessageType(decrypted_message["type"])
            
            connection = self.connection_manager.get_connection(connection_id)
            if not connection:
                logger.warning("Message from unknown connection", connection_id=connection_id)
                return
            
            # Rate limiting check
            rate_limiter = await get_websocket_rate_limiter()
            user_id = connection.user_id or "anonymous"
            client_ip = connection.security_info.get("x_real_ip") or connection.security_info.get("x_forwarded_for") or "unknown"
            message_size = len(str(decrypted_message))
            
            allowed, violation_reason = await rate_limiter.check_rate_limit(
                connection_id, user_id, client_ip, message_type.value, message_size
            )
            
            if not allowed:
                logger.warning("Message rate limited", 
                             connection_id=connection_id,
                             user_id=user_id,
                             message_type=message_type.value,
                             violation_reason=violation_reason)
                
                # Audit rate limit violation
                audit_logger = await get_audit_logger()
                await audit_logger.log_event(
                    AuditEventType.RATE_LIMIT_EXCEEDED,
                    AuditSeverity.WARNING,
                    connection_id=connection_id,
                    user_id=user_id,
                    client_ip=client_ip,
                    message_type=message_type.value,
                    details={
                        "violation_reason": violation_reason,
                        "message_size": message_size
                    }
                )
                
                await self._send_error(connection_id, "RATE_LIMITED", 
                                     f"Rate limit exceeded: {violation_reason}")
                return
            
            # Security validation for all messages
            if not await self.connection_manager.validate_message_security(connection_id, decrypted_message):
                logger.warning("Message failed security validation", 
                             connection_id=connection_id,
                             message_type=message_type.value)
                # Send security error to client
                await self._send_error(connection_id, "SECURITY_VALIDATION_FAILED", 
                                     "Message failed security validation")
                return
            
            # Malicious pattern detection
            pattern_detector = get_pattern_detector()
            
            # Analyze the message content
            message_text = json.dumps(decrypted_message)
            detection_result = pattern_detector.analyze_message(
                message_text, 
                connection_id, 
                message_type.value,
                {
                    "message_type": message_type.value,
                    "is_authenticated": connection.state.value == "authenticated",
                    "user_role": getattr(connection.user, "role", "user") if connection.user else "anonymous"
                }
            )
            
            # Handle threat detection
            if detection_result.detected:
                logger.warning("Malicious patterns detected in message",
                             connection_id=connection_id,
                             message_type=message_type.value,
                             threat_level=detection_result.threat_level.value,
                             patterns=detection_result.patterns_found,
                             risk_score=detection_result.risk_score)
                
                # Audit the security threat
                audit_logger = await get_audit_logger()
                await audit_logger.log_security_event(
                    AuditEventType.SECURITY_VIOLATION,
                    connection_id,
                    {
                        "violation_type": "malicious_pattern_detection",
                        "patterns_detected": detection_result.patterns_found,
                        "threat_level": detection_result.threat_level.value,
                        "risk_score": detection_result.risk_score,
                        "confidence": detection_result.confidence_score,
                        "recommendations": detection_result.recommendations
                    },
                    connection.user_id,
                    client_ip,
                    AuditSeverity.CRITICAL if detection_result.threat_level == ThreatLevel.CRITICAL else AuditSeverity.ERROR
                )
                
                # Take action based on threat level
                if detection_result.threat_level == ThreatLevel.CRITICAL:
                    logger.critical("Critical threat detected - disconnecting client",
                                  connection_id=connection_id,
                                  patterns=detection_result.patterns_found)
                    await self._send_error(connection_id, "CRITICAL_THREAT_DETECTED", 
                                         "Critical security threat detected. Connection terminated.")
                    await self.connection_manager.disconnect(connection_id)
                    return
                elif detection_result.risk_score >= 60:
                    logger.error("High-risk patterns detected",
                               connection_id=connection_id,
                               risk_score=detection_result.risk_score)
                    await self._send_error(connection_id, "HIGH_RISK_CONTENT", 
                                         "High-risk content detected. Message rejected.")
                    return
            
            # Record message received
            metrics = await get_websocket_metrics()
            message_size = len(str(decrypted_message))
            metrics.record_message_received(connection_id, message_type.value, message_size)
            
            # Audit message received
            audit_logger = await get_audit_logger()
            await audit_logger.log_message_event(
                AuditEventType.MESSAGE_RECEIVED,
                connection_id,
                message_type.value,
                message_size,
                connection.user_id,
                connection.session_id,
                decrypted_message.get("protocol_version"),
                decrypted_message.get("data"),
                connection.security_info
            )
            
            logger.debug("Routing message", 
                        connection_id=connection_id,
                        message_type=message_type.value,
                        user_id=connection.user_id)
            
            if message_type in self.handlers:
                await self.handlers[message_type](connection_id, decrypted_message)
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
            
            # Audit message failure
            audit_logger = await get_audit_logger()
            await audit_logger.log_message_event(
                AuditEventType.MESSAGE_FAILED,
                connection_id,
                "unknown",
                len(raw_message),
                None,
                None,
                None,
                None,
                {"error": str(e), "error_type": "routing_error"}
            )
            
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
            # Advanced malicious pattern detection for terminal data
            pattern_detector = get_pattern_detector()
            
            if isinstance(data, str):
                text_data = data
            else:
                text_data = data.decode('utf-8', errors='replace') if isinstance(data, bytes) else str(data)
            
            # Analyze terminal input for malicious patterns
            detection_result = pattern_detector.analyze_message(
                text_data,
                connection_id,
                "terminal_input",
                {
                    "message_type": "terminal_data",
                    "is_authenticated": True,
                    "user_role": getattr(connection.user, "role", "user") if connection.user else "user",
                    "session_id": connection.session_id
                }
            )
            
            if detection_result.detected:
                logger.warning("Malicious patterns detected in terminal input",
                             connection_id=connection_id,
                             session_id=connection.session_id,
                             threat_level=detection_result.threat_level.value,
                             patterns=detection_result.patterns_found,
                             risk_score=detection_result.risk_score)
                
                # Audit the terminal security threat
                audit_logger = await get_audit_logger()
                await audit_logger.log_security_event(
                    AuditEventType.SECURITY_VIOLATION,
                    connection_id,
                    {
                        "violation_type": "terminal_malicious_input",
                        "patterns_detected": detection_result.patterns_found,
                        "threat_level": detection_result.threat_level.value,
                        "risk_score": detection_result.risk_score,
                        "terminal_data": text_data[:100],  # First 100 chars for context
                        "session_id": connection.session_id
                    },
                    connection.user_id,
                    None,
                    AuditSeverity.CRITICAL if detection_result.threat_level == ThreatLevel.CRITICAL else AuditSeverity.ERROR
                )
                
                # Take action based on threat level
                if detection_result.threat_level == ThreatLevel.CRITICAL:
                    logger.critical("Critical threat detected in terminal input - blocking",
                                  connection_id=connection_id,
                                  session_id=connection.session_id,
                                  patterns=detection_result.patterns_found)
                    await self._send_error(connection_id, "CRITICAL_TERMINAL_THREAT", 
                                         "Critical security threat detected in terminal input. Command blocked.")
                    return
                elif detection_result.risk_score >= 70:
                    logger.error("High-risk terminal command blocked",
                               connection_id=connection_id,
                               session_id=connection.session_id,
                               risk_score=detection_result.risk_score)
                    await self._send_error(connection_id, "HIGH_RISK_TERMINAL_INPUT", 
                                         "High-risk terminal command detected. Command blocked.")
                    return
                elif detection_result.risk_score >= 40:
                    # Log but allow with warning
                    logger.warning("Medium-risk terminal command allowed with logging",
                                 connection_id=connection_id,
                                 session_id=connection.session_id,
                                 risk_score=detection_result.risk_score)
            
            # Sanitize input to prevent command injection and malicious sequences
            sanitizer = get_terminal_sanitizer()
            
            # Legacy threat detection (keeping for additional coverage)
            threats = sanitizer.detect_malicious_patterns(text_data)
            if threats["detected"] and threats["severity"] == "high":
                logger.warning("High-severity malicious pattern detected in terminal input (legacy detector)",
                             connection_id=connection_id,
                             session_id=connection.session_id,
                             threats=threats["threats"])
                await self._send_error(connection_id, "MALICIOUS_INPUT", "Potentially malicious input detected")
                return
            
            # Sanitize the input
            sanitized_data = sanitizer.sanitize_terminal_input(data)
            
            # Convert to bytes if string
            if isinstance(sanitized_data, str):
                sanitized_data = sanitized_data.encode('utf-8')
            
            # Send sanitized data to mock PTY
            await self.pty_manager.send_input(connection.session_id, sanitized_data)
            
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
                
                # Audit session creation
                audit_logger = await get_audit_logger()
                await audit_logger.log_session_event(
                    AuditEventType.SESSION_CREATED,
                    connection_id,
                    session_id,
                    connection.user_id,
                    {"vm_id": vm_id}
                )
                
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
                
                # Audit session join
                audit_logger = await get_audit_logger()
                await audit_logger.log_session_event(
                    AuditEventType.SESSION_JOINED,
                    connection_id,
                    session_id,
                    connection.user_id
                )
                
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
                # Sanitize historical terminal data before sending
                sanitizer = get_terminal_sanitizer()
                sanitized_history = []
                
                for history_item in history:
                    if isinstance(history_item, str):
                        sanitized_item = sanitizer.sanitize_terminal_output(history_item)
                    else:
                        # If it's bytes or other format, convert and sanitize
                        sanitized_item = sanitizer.sanitize_terminal_output(str(history_item))
                    sanitized_history.append(sanitized_item)
                
                # Send recovery message with sanitized history
                response = self.protocol.create_session_recovered_message(session_id, sanitized_history)
                await self.connection_manager.send_to_connection(connection_id, 
                                                               json.loads(response))
                
                # Start terminal output streaming
                asyncio.create_task(self._stream_terminal_output(session_id))
                
                # Audit session restore
                audit_logger = await get_audit_logger()
                await audit_logger.log_session_event(
                    AuditEventType.SESSION_RESTORED,
                    connection_id,
                    session_id,
                    connection.user_id,
                    {"history_lines": len(history)}
                )
                
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
    
    async def _handle_acknowledgment(self, connection_id: str, message: Dict[str, Any]):
        """Handle message acknowledgment from client."""
        try:
            message_id = message.get("message_id")
            if not message_id:
                logger.warning("Acknowledgment missing message_id", connection_id=connection_id)
                return
            
            # Get acknowledgment system
            ack_system = await get_message_ack_system()
            
            # Process the acknowledgment
            result = ack_system.process_acknowledgment(message_id, message.get("data"))
            
            if result.success:
                logger.debug("Message acknowledgment processed",
                           connection_id=connection_id,
                           message_id=message_id,
                           response_time_ms=result.response_time_ms)
            else:
                logger.warning("Failed to process acknowledgment",
                             connection_id=connection_id,
                             message_id=message_id,
                             error=result.error_message)
        
        except Exception as e:
            logger.error("Error processing acknowledgment",
                        connection_id=connection_id,
                        error=str(e))
    
    async def _stream_terminal_output(self, session_id: str):
        """Stream terminal output to all connected clients with XSS protection."""
        try:
            sanitizer = get_terminal_sanitizer()
            
            async for output_data in self.pty_manager.get_output_stream(session_id):
                if output_data:
                    # Sanitize output to prevent XSS and malicious sequences
                    sanitized_output = sanitizer.sanitize_terminal_output(output_data)
                    
                    # Detect and log any threats in the output
                    threats = sanitizer.detect_malicious_patterns(sanitized_output)
                    if threats["detected"]:
                        logger.warning("Malicious patterns detected in terminal output",
                                     session_id=session_id,
                                     threats=threats["threats"],
                                     severity=threats["severity"])
                    
                    # Create terminal data message with sanitized output
                    message_str = self.protocol.create_terminal_data_message(
                        sanitized_output, session_id
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