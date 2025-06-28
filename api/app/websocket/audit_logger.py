"""Enhanced audit logging system for WebSocket messages and events."""

import asyncio
import json
import time
import hashlib
import hmac
import secrets
from typing import Dict, Any, Optional, List, Union
from dataclasses import dataclass, asdict
from enum import Enum
import structlog
from pathlib import Path

logger = structlog.get_logger(__name__)


class AuditEventType(Enum):
    """Types of audit events."""
    CONNECTION_ESTABLISHED = "connection_established"
    CONNECTION_FAILED = "connection_failed"
    CONNECTION_CLOSED = "connection_closed"
    AUTHENTICATION_ATTEMPT = "authentication_attempt"
    AUTHENTICATION_SUCCESS = "authentication_success"
    AUTHENTICATION_FAILURE = "authentication_failure"
    SESSION_CREATED = "session_created"
    SESSION_JOINED = "session_joined"
    SESSION_RESTORED = "session_restored"
    SESSION_BOUND = "session_bound"
    MESSAGE_SENT = "message_sent"
    MESSAGE_RECEIVED = "message_received"
    MESSAGE_FAILED = "message_failed"
    SECURITY_VIOLATION = "security_violation"
    RATE_LIMIT_EXCEEDED = "rate_limit_exceeded"
    PROTOCOL_NEGOTIATED = "protocol_negotiated"
    PROTOCOL_VIOLATION = "protocol_violation"
    ERROR_OCCURRED = "error_occurred"
    HEARTBEAT_MISSED = "heartbeat_missed"
    ACKNOWLEDGMENT_TIMEOUT = "acknowledgment_timeout"
    COMPRESSION_EVENT = "compression_event"
    ENCRYPTION_EVENT = "encryption_event"


class AuditSeverity(Enum):
    """Severity levels for audit events."""
    INFO = "info"
    WARNING = "warning"
    ERROR = "error"
    CRITICAL = "critical"


@dataclass
class AuditEvent:
    """An audit event record."""
    event_id: str
    event_type: AuditEventType
    severity: AuditSeverity
    timestamp: float
    
    # Connection information
    connection_id: Optional[str]
    user_id: Optional[str]
    session_id: Optional[str]
    client_ip: Optional[str]
    user_agent: Optional[str]
    
    # Event details
    message_type: Optional[str]
    message_size: Optional[int]
    data_hash: Optional[str]
    protocol_version: Optional[str]
    
    # Security context
    security_context: Dict[str, Any]
    
    # Additional event data
    details: Dict[str, Any]
    
    # Integrity verification
    integrity_hash: Optional[str] = None
    
    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary for serialization."""
        result = asdict(self)
        # Convert enums to their string values
        result['event_type'] = self.event_type.value
        result['severity'] = self.severity.value
        return result
    
    def calculate_integrity_hash(self, secret_key: bytes) -> str:
        """Calculate integrity hash for the event."""
        # Create canonical representation excluding integrity_hash
        event_data = {k: v for k, v in self.to_dict().items() if k != 'integrity_hash'}
        event_str = json.dumps(event_data, sort_keys=True, separators=(',', ':'))
        return hmac.new(
            secret_key,
            event_str.encode('utf-8'),
            hashlib.sha256
        ).hexdigest()


class AuditLogger:
    """Enhanced audit logging system for WebSocket events."""
    
    def __init__(self, log_file_path: Optional[str] = None):
        # Configuration
        self.log_file_path = log_file_path or "/tmp/websocket_audit.log"
        self.enable_file_logging = True
        self.enable_structured_logging = True
        self.enable_real_time_monitoring = True
        
        # Security
        self.integrity_key = secrets.token_bytes(32)
        self.enable_integrity_checks = True
        
        # Event tracking
        self.event_sequence = 0
        self.events_logged = 0
        self.failed_log_attempts = 0
        
        # Real-time monitoring
        self.event_handlers: List[callable] = []
        self.security_handlers: List[callable] = []
        
        # Rate limiting for logging
        self.max_events_per_second = 1000
        self.event_timestamps = []
        
        # Background tasks
        self.log_writer_task: Optional[asyncio.Task] = None
        self.log_queue: asyncio.Queue = asyncio.Queue(maxsize=10000)
        
        # Statistics
        self.stats = {
            "events_logged": 0,
            "events_by_type": {},
            "events_by_severity": {},
            "security_violations": 0,
            "failed_log_attempts": 0,
            "last_log_time": 0
        }
    
    async def start(self):
        """Start the audit logging system."""
        self.log_writer_task = asyncio.create_task(self._log_writer_loop())
        
        # Ensure log directory exists
        log_path = Path(self.log_file_path)
        log_path.parent.mkdir(parents=True, exist_ok=True)
        
        logger.info("Audit logging system started", 
                   log_file=self.log_file_path,
                   integrity_enabled=self.enable_integrity_checks)
    
    async def stop(self):
        """Stop the audit logging system."""
        if self.log_writer_task:
            self.log_writer_task.cancel()
        
        # Process remaining queued events
        await self._flush_log_queue()
        
        logger.info("Audit logging system stopped", 
                   events_logged=self.events_logged)
    
    async def log_event(self, 
                       event_type: AuditEventType,
                       severity: AuditSeverity = AuditSeverity.INFO,
                       connection_id: Optional[str] = None,
                       user_id: Optional[str] = None,
                       session_id: Optional[str] = None,
                       client_ip: Optional[str] = None,
                       user_agent: Optional[str] = None,
                       message_type: Optional[str] = None,
                       message_size: Optional[int] = None,
                       data: Optional[Union[str, bytes, Dict[str, Any]]] = None,
                       protocol_version: Optional[str] = None,
                       security_context: Optional[Dict[str, Any]] = None,
                       details: Optional[Dict[str, Any]] = None):
        """Log an audit event."""
        try:
            # Rate limiting check
            if not self._check_rate_limit():
                self.failed_log_attempts += 1
                return
            
            # Generate event ID
            self.event_sequence += 1
            event_id = f"audit_{int(time.time())}_{self.event_sequence}"
            
            # Calculate data hash if data provided
            data_hash = None
            if data is not None:
                if isinstance(data, bytes):
                    data_hash = hashlib.sha256(data).hexdigest()
                elif isinstance(data, (str, dict)):
                    data_str = json.dumps(data) if isinstance(data, dict) else data
                    data_hash = hashlib.sha256(data_str.encode()).hexdigest()
            
            # Create audit event
            event = AuditEvent(
                event_id=event_id,
                event_type=event_type,
                severity=severity,
                timestamp=time.time(),
                connection_id=connection_id,
                user_id=user_id,
                session_id=session_id,
                client_ip=client_ip,
                user_agent=user_agent,
                message_type=message_type,
                message_size=message_size,
                data_hash=data_hash,
                protocol_version=protocol_version,
                security_context=security_context or {},
                details=details or {}
            )
            
            # Calculate integrity hash
            if self.enable_integrity_checks:
                event.integrity_hash = event.calculate_integrity_hash(self.integrity_key)
            
            # Queue for background processing
            await self.log_queue.put(event)
            
            # Update statistics
            self.stats["events_logged"] += 1
            self.stats["events_by_type"][event_type.value] = \
                self.stats["events_by_type"].get(event_type.value, 0) + 1
            self.stats["events_by_severity"][severity.value] = \
                self.stats["events_by_severity"].get(severity.value, 0) + 1
            self.stats["last_log_time"] = time.time()
            
            # Handle security events specially
            if severity in [AuditSeverity.ERROR, AuditSeverity.CRITICAL] or \
               event_type in [AuditEventType.SECURITY_VIOLATION, AuditEventType.AUTHENTICATION_FAILURE]:
                self.stats["security_violations"] += 1
                await self._handle_security_event(event)
            
            # Call real-time handlers
            if self.enable_real_time_monitoring:
                await self._notify_event_handlers(event)
            
        except Exception as e:
            self.failed_log_attempts += 1
            logger.error("Failed to log audit event", 
                        event_type=event_type.value, error=str(e))
    
    async def log_connection_event(self, event_type: AuditEventType, 
                                 connection_id: str,
                                 client_ip: str,
                                 user_agent: str,
                                 user_id: Optional[str] = None,
                                 details: Optional[Dict[str, Any]] = None):
        """Log a connection-related event."""
        await self.log_event(
            event_type=event_type,
            severity=AuditSeverity.INFO if event_type == AuditEventType.CONNECTION_ESTABLISHED else AuditSeverity.WARNING,
            connection_id=connection_id,
            user_id=user_id,
            client_ip=client_ip,
            user_agent=user_agent,
            details=details
        )
    
    async def log_message_event(self, event_type: AuditEventType,
                              connection_id: str,
                              message_type: str,
                              message_size: int,
                              user_id: Optional[str] = None,
                              session_id: Optional[str] = None,
                              protocol_version: Optional[str] = None,
                              data: Optional[Any] = None,
                              security_context: Optional[Dict[str, Any]] = None):
        """Log a message-related event."""
        severity = AuditSeverity.INFO
        if event_type == AuditEventType.MESSAGE_FAILED:
            severity = AuditSeverity.ERROR
        
        await self.log_event(
            event_type=event_type,
            severity=severity,
            connection_id=connection_id,
            user_id=user_id,
            session_id=session_id,
            message_type=message_type,
            message_size=message_size,
            protocol_version=protocol_version,
            data=data,
            security_context=security_context
        )
    
    async def log_security_event(self, event_type: AuditEventType,
                               connection_id: str,
                               violation_details: Dict[str, Any],
                               user_id: Optional[str] = None,
                               client_ip: Optional[str] = None,
                               severity: AuditSeverity = AuditSeverity.WARNING):
        """Log a security-related event."""
        await self.log_event(
            event_type=event_type,
            severity=severity,
            connection_id=connection_id,
            user_id=user_id,
            client_ip=client_ip,
            security_context=violation_details,
            details=violation_details
        )
    
    async def log_authentication_event(self, success: bool,
                                     connection_id: str,
                                     user_id: Optional[str] = None,
                                     client_ip: Optional[str] = None,
                                     error_reason: Optional[str] = None):
        """Log an authentication event."""
        event_type = AuditEventType.AUTHENTICATION_SUCCESS if success else AuditEventType.AUTHENTICATION_FAILURE
        severity = AuditSeverity.INFO if success else AuditSeverity.WARNING
        
        details = {}
        if error_reason:
            details["error_reason"] = error_reason
        
        await self.log_event(
            event_type=event_type,
            severity=severity,
            connection_id=connection_id,
            user_id=user_id,
            client_ip=client_ip,
            details=details
        )
    
    async def log_session_event(self, event_type: AuditEventType,
                              connection_id: str,
                              session_id: str,
                              user_id: str,
                              details: Optional[Dict[str, Any]] = None):
        """Log a session-related event."""
        await self.log_event(
            event_type=event_type,
            severity=AuditSeverity.INFO,
            connection_id=connection_id,
            user_id=user_id,
            session_id=session_id,
            details=details
        )
    
    def add_event_handler(self, handler: callable):
        """Add a real-time event handler."""
        self.event_handlers.append(handler)
    
    def add_security_handler(self, handler: callable):
        """Add a security event handler."""
        self.security_handlers.append(handler)
    
    def get_audit_stats(self) -> Dict[str, Any]:
        """Get audit logging statistics."""
        return {
            **self.stats,
            "failed_log_attempts": self.failed_log_attempts,
            "queue_size": self.log_queue.qsize(),
            "max_queue_size": self.log_queue.maxsize,
            "rate_limit_max": self.max_events_per_second
        }
    
    async def search_events(self, 
                           event_type: Optional[AuditEventType] = None,
                           connection_id: Optional[str] = None,
                           user_id: Optional[str] = None,
                           start_time: Optional[float] = None,
                           end_time: Optional[float] = None,
                           limit: int = 100) -> List[Dict[str, Any]]:
        """Search audit events (simplified implementation for demo)."""
        # This is a simplified implementation
        # In production, you'd want to use a proper database or search system
        events = []
        
        try:
            if Path(self.log_file_path).exists():
                with open(self.log_file_path, 'r') as f:
                    for line in f:
                        try:
                            event_data = json.loads(line.strip())
                            
                            # Apply filters
                            if event_type and event_data.get("event_type") != event_type.value:
                                continue
                            if connection_id and event_data.get("connection_id") != connection_id:
                                continue
                            if user_id and event_data.get("user_id") != user_id:
                                continue
                            if start_time and event_data.get("timestamp", 0) < start_time:
                                continue
                            if end_time and event_data.get("timestamp", 0) > end_time:
                                continue
                            
                            events.append(event_data)
                            
                            if len(events) >= limit:
                                break
                                
                        except json.JSONDecodeError:
                            continue
        except Exception as e:
            logger.error("Failed to search audit events", error=str(e))
        
        return events[-limit:]  # Return most recent events
    
    def _check_rate_limit(self) -> bool:
        """Check if event logging is within rate limits."""
        current_time = time.time()
        
        # Remove timestamps older than 1 second
        self.event_timestamps = [ts for ts in self.event_timestamps 
                               if current_time - ts < 1.0]
        
        if len(self.event_timestamps) >= self.max_events_per_second:
            return False
        
        self.event_timestamps.append(current_time)
        return True
    
    async def _log_writer_loop(self):
        """Background task to write audit events to file."""
        while True:
            try:
                # Get event from queue with timeout
                event = await asyncio.wait_for(self.log_queue.get(), timeout=1.0)
                
                # Write to file
                if self.enable_file_logging:
                    await self._write_event_to_file(event)
                
                # Write to structured logger
                if self.enable_structured_logging:
                    self._write_event_to_structured_log(event)
                
                self.events_logged += 1
                
            except asyncio.TimeoutError:
                continue
            except asyncio.CancelledError:
                break
            except Exception as e:
                self.failed_log_attempts += 1
                logger.error("Log writer error", error=str(e))
    
    async def _write_event_to_file(self, event: AuditEvent):
        """Write event to audit log file."""
        try:
            log_line = json.dumps(event.to_dict(), separators=(',', ':')) + '\n'
            
            # Append to file
            with open(self.log_file_path, 'a') as f:
                f.write(log_line)
                f.flush()
                
        except Exception as e:
            logger.error("Failed to write to audit log file", 
                        file=self.log_file_path, error=str(e))
    
    def _write_event_to_structured_log(self, event: AuditEvent):
        """Write event to structured logger."""
        try:
            log_data = {
                "audit_event": True,
                "event_id": event.event_id,
                "event_type": event.event_type.value,
                "severity": event.severity.value,
                "connection_id": event.connection_id,
                "user_id": event.user_id,
                "session_id": event.session_id,
                "message_type": event.message_type,
                "details": event.details
            }
            
            # Log with appropriate level
            if event.severity == AuditSeverity.CRITICAL:
                logger.critical("Audit event", **log_data)
            elif event.severity == AuditSeverity.ERROR:
                logger.error("Audit event", **log_data)
            elif event.severity == AuditSeverity.WARNING:
                logger.warning("Audit event", **log_data)
            else:
                logger.info("Audit event", **log_data)
                
        except Exception as e:
            logger.error("Failed to write to structured log", error=str(e))
    
    async def _handle_security_event(self, event: AuditEvent):
        """Handle security-related events."""
        try:
            # Call security handlers
            for handler in self.security_handlers:
                if asyncio.iscoroutinefunction(handler):
                    await handler(event)
                else:
                    handler(event)
        except Exception as e:
            logger.error("Error in security event handler", error=str(e))
    
    async def _notify_event_handlers(self, event: AuditEvent):
        """Notify real-time event handlers."""
        try:
            for handler in self.event_handlers:
                if asyncio.iscoroutinefunction(handler):
                    await handler(event)
                else:
                    handler(event)
        except Exception as e:
            logger.error("Error in event handler", error=str(e))
    
    async def _flush_log_queue(self):
        """Flush remaining events in the queue."""
        while not self.log_queue.empty():
            try:
                event = self.log_queue.get_nowait()
                if self.enable_file_logging:
                    await self._write_event_to_file(event)
                if self.enable_structured_logging:
                    self._write_event_to_structured_log(event)
            except asyncio.QueueEmpty:
                break
            except Exception as e:
                logger.error("Error flushing log queue", error=str(e))


# Global instance
_audit_logger = None


async def get_audit_logger() -> AuditLogger:
    """Get global audit logger instance."""
    global _audit_logger
    if _audit_logger is None:
        _audit_logger = AuditLogger()
        await _audit_logger.start()
        logger.info("Audit logger initialized")
    return _audit_logger


# Convenience functions for common audit events
async def audit_connection_established(connection_id: str, client_ip: str, user_agent: str):
    """Audit connection establishment."""
    audit_logger = await get_audit_logger()
    await audit_logger.log_connection_event(
        AuditEventType.CONNECTION_ESTABLISHED,
        connection_id, client_ip, user_agent
    )


async def audit_authentication_success(connection_id: str, user_id: str, client_ip: str):
    """Audit successful authentication."""
    audit_logger = await get_audit_logger()
    await audit_logger.log_authentication_event(
        True, connection_id, user_id, client_ip
    )


async def audit_authentication_failure(connection_id: str, client_ip: str, reason: str):
    """Audit authentication failure."""
    audit_logger = await get_audit_logger()
    await audit_logger.log_authentication_event(
        False, connection_id, None, client_ip, reason
    )


async def audit_message_sent(connection_id: str, message_type: str, size: int, 
                           user_id: str = None, session_id: str = None):
    """Audit message sent."""
    audit_logger = await get_audit_logger()
    await audit_logger.log_message_event(
        AuditEventType.MESSAGE_SENT, connection_id, message_type, size,
        user_id, session_id
    )


async def audit_security_violation(connection_id: str, violation_type: str, 
                                 details: Dict[str, Any], client_ip: str = None):
    """Audit security violation."""
    audit_logger = await get_audit_logger()
    await audit_logger.log_security_event(
        AuditEventType.SECURITY_VIOLATION, connection_id, 
        {"violation_type": violation_type, **details}, 
        client_ip=client_ip, severity=AuditSeverity.ERROR
    )