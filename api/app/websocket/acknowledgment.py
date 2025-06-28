"""Message acknowledgment system for reliable WebSocket delivery."""

import asyncio
import time
import uuid
from typing import Dict, Any, Optional, Set, List, Callable, Tuple
from dataclasses import dataclass, asdict
from enum import Enum
import structlog

logger = structlog.get_logger(__name__)


class AckStatus(Enum):
    """Acknowledgment status."""
    PENDING = "pending"
    ACKNOWLEDGED = "acknowledged"
    TIMEOUT = "timeout"
    FAILED = "failed"


class MessagePriority(Enum):
    """Message priority levels."""
    LOW = 0
    NORMAL = 1
    HIGH = 2
    CRITICAL = 3


@dataclass
class PendingMessage:
    """A message waiting for acknowledgment."""
    message_id: str
    connection_id: str
    message: Dict[str, Any]
    sent_at: float
    retry_count: int = 0
    max_retries: int = 3
    timeout_seconds: float = 30.0
    priority: MessagePriority = MessagePriority.NORMAL
    callback: Optional[Callable] = None
    
    @property
    def is_expired(self) -> bool:
        """Check if message has timed out."""
        return time.time() - self.sent_at > self.timeout_seconds
    
    @property
    def can_retry(self) -> bool:
        """Check if message can be retried."""
        return self.retry_count < self.max_retries


@dataclass
class AckResult:
    """Result of acknowledgment processing."""
    success: bool
    message_id: str
    status: AckStatus
    response_time_ms: float
    retry_count: int
    error_message: Optional[str] = None


class MessageAcknowledgmentSystem:
    """Advanced message acknowledgment system for reliable delivery."""
    
    def __init__(self):
        # Message tracking
        self.pending_messages: Dict[str, PendingMessage] = {}
        self.acknowledged_messages: Dict[str, float] = {}  # message_id -> ack_time
        
        # Configuration
        self.default_timeout = 30.0  # seconds
        self.max_retries = 3
        self.retry_backoff_factor = 2.0
        self.max_pending_per_connection = 100
        
        # Background tasks
        self.cleanup_task: Optional[asyncio.Task] = None
        self.retry_task: Optional[asyncio.Task] = None
        self.cleanup_interval = 60  # seconds
        self.retry_interval = 5  # seconds
        
        # Statistics
        self.stats = {
            "messages_sent": 0,
            "messages_acknowledged": 0,
            "messages_timeout": 0,
            "messages_failed": 0,
            "total_retries": 0,
            "average_response_time": 0.0
        }
        
        # Callbacks
        self.ack_handlers: List[Callable[[AckResult], None]] = []
        self.timeout_handlers: List[Callable[[PendingMessage], None]] = []
        
    async def start(self):
        """Start the acknowledgment system."""
        self.cleanup_task = asyncio.create_task(self._cleanup_loop())
        self.retry_task = asyncio.create_task(self._retry_loop())
        logger.info("Message acknowledgment system started")
    
    async def stop(self):
        """Stop the acknowledgment system."""
        if self.cleanup_task:
            self.cleanup_task.cancel()
        if self.retry_task:
            self.retry_task.cancel()
        
        # Process any remaining pending messages
        await self._process_pending_timeouts()
        
        logger.info("Message acknowledgment system stopped")
    
    def send_message_with_ack(self, 
                            connection_id: str,
                            message: Dict[str, Any],
                            timeout_seconds: Optional[float] = None,
                            max_retries: Optional[int] = None,
                            priority: MessagePriority = MessagePriority.NORMAL,
                            callback: Optional[Callable] = None) -> str:
        """
        Send a message that requires acknowledgment.
        
        Args:
            connection_id: Target connection ID
            message: Message to send
            timeout_seconds: Timeout for acknowledgment
            max_retries: Maximum retry attempts
            priority: Message priority
            callback: Callback for acknowledgment result
        
        Returns:
            Message ID for tracking
        """
        # Check connection limits
        connection_pending = [m for m in self.pending_messages.values() 
                            if m.connection_id == connection_id]
        
        if len(connection_pending) >= self.max_pending_per_connection:
            raise ValueError(f"Too many pending messages for connection {connection_id}")
        
        # Generate message ID
        message_id = str(uuid.uuid4())
        
        # Add acknowledgment fields to message
        ack_message = {
            **message,
            "message_id": message_id,
            "requires_ack": True,
            "timestamp": time.time()
        }
        
        # Create pending message
        pending = PendingMessage(
            message_id=message_id,
            connection_id=connection_id,
            message=ack_message,
            sent_at=time.time(),
            timeout_seconds=timeout_seconds or self.default_timeout,
            max_retries=max_retries or self.max_retries,
            priority=priority,
            callback=callback
        )
        
        self.pending_messages[message_id] = pending
        self.stats["messages_sent"] += 1
        
        logger.debug("Message queued for acknowledgment",
                    message_id=message_id,
                    connection_id=connection_id,
                    timeout=pending.timeout_seconds,
                    priority=priority.name)
        
        return message_id
    
    def process_acknowledgment(self, message_id: str, ack_data: Optional[Dict[str, Any]] = None) -> AckResult:
        """
        Process an acknowledgment from client.
        
        Args:
            message_id: ID of acknowledged message
            ack_data: Optional acknowledgment data
        
        Returns:
            AckResult with processing details
        """
        if message_id not in self.pending_messages:
            # Check if already acknowledged
            if message_id in self.acknowledged_messages:
                return AckResult(
                    success=False,
                    message_id=message_id,
                    status=AckStatus.ACKNOWLEDGED,
                    response_time_ms=0,
                    retry_count=0,
                    error_message="Message already acknowledged"
                )
            
            return AckResult(
                success=False,
                message_id=message_id,
                status=AckStatus.FAILED,
                response_time_ms=0,
                retry_count=0,
                error_message="Message not found"
            )
        
        pending = self.pending_messages[message_id]
        current_time = time.time()
        response_time_ms = (current_time - pending.sent_at) * 1000
        
        # Remove from pending
        del self.pending_messages[message_id]
        
        # Mark as acknowledged
        self.acknowledged_messages[message_id] = current_time
        
        # Create result
        result = AckResult(
            success=True,
            message_id=message_id,
            status=AckStatus.ACKNOWLEDGED,
            response_time_ms=response_time_ms,
            retry_count=pending.retry_count
        )
        
        # Update statistics
        self.stats["messages_acknowledged"] += 1
        self._update_average_response_time(response_time_ms)
        
        # Call message callback if provided
        if pending.callback:
            try:
                if asyncio.iscoroutinefunction(pending.callback):
                    asyncio.create_task(pending.callback(result))
                else:
                    pending.callback(result)
            except Exception as e:
                logger.error("Error in message callback", message_id=message_id, error=str(e))
        
        # Call global acknowledgment handlers
        for handler in self.ack_handlers:
            try:
                if asyncio.iscoroutinefunction(handler):
                    asyncio.create_task(handler(result))
                else:
                    handler(result)
            except Exception as e:
                logger.error("Error in ack handler", error=str(e))
        
        logger.debug("Message acknowledged",
                    message_id=message_id,
                    response_time_ms=response_time_ms,
                    retry_count=pending.retry_count)
        
        return result
    
    def get_pending_messages(self, connection_id: Optional[str] = None) -> List[PendingMessage]:
        """Get pending messages, optionally filtered by connection."""
        messages = list(self.pending_messages.values())
        
        if connection_id:
            messages = [m for m in messages if m.connection_id == connection_id]
        
        # Sort by priority and send time
        messages.sort(key=lambda m: (m.priority.value, m.sent_at), reverse=True)
        
        return messages
    
    def get_message_status(self, message_id: str) -> Optional[AckStatus]:
        """Get status of a message."""
        if message_id in self.pending_messages:
            pending = self.pending_messages[message_id]
            if pending.is_expired:
                return AckStatus.TIMEOUT
            return AckStatus.PENDING
        
        if message_id in self.acknowledged_messages:
            return AckStatus.ACKNOWLEDGED
        
        return None  # Message not found
    
    def cancel_message(self, message_id: str) -> bool:
        """Cancel a pending message."""
        if message_id in self.pending_messages:
            pending = self.pending_messages[message_id]
            del self.pending_messages[message_id]
            
            logger.debug("Message cancelled", message_id=message_id)
            return True
        
        return False
    
    def cancel_connection_messages(self, connection_id: str) -> int:
        """Cancel all pending messages for a connection."""
        cancelled = 0
        
        to_cancel = [msg_id for msg_id, pending in self.pending_messages.items()
                    if pending.connection_id == connection_id]
        
        for msg_id in to_cancel:
            if self.cancel_message(msg_id):
                cancelled += 1
        
        logger.info("Connection messages cancelled", 
                   connection_id=connection_id, count=cancelled)
        
        return cancelled
    
    def add_ack_handler(self, handler: Callable[[AckResult], None]):
        """Add global acknowledgment handler."""
        self.ack_handlers.append(handler)
    
    def add_timeout_handler(self, handler: Callable[[PendingMessage], None]):
        """Add timeout handler."""
        self.timeout_handlers.append(handler)
    
    async def _cleanup_loop(self):
        """Background task to clean up expired acknowledgments."""
        while True:
            try:
                await asyncio.sleep(self.cleanup_interval)
                await self._process_pending_timeouts()
                await self._cleanup_acknowledged_messages()
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Cleanup loop error", error=str(e))
    
    async def _retry_loop(self):
        """Background task to retry failed messages."""
        while True:
            try:
                await asyncio.sleep(self.retry_interval)
                await self._process_retries()
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Retry loop error", error=str(e))
    
    async def _process_pending_timeouts(self):
        """Process messages that have timed out."""
        current_time = time.time()
        timed_out = []
        
        for message_id, pending in list(self.pending_messages.items()):
            if pending.is_expired:
                timed_out.append((message_id, pending))
        
        for message_id, pending in timed_out:
            # Remove from pending
            del self.pending_messages[message_id]
            
            # Update statistics
            self.stats["messages_timeout"] += 1
            
            # Create timeout result
            result = AckResult(
                success=False,
                message_id=message_id,
                status=AckStatus.TIMEOUT,
                response_time_ms=(current_time - pending.sent_at) * 1000,
                retry_count=pending.retry_count,
                error_message="Message acknowledgment timeout"
            )
            
            # Call timeout handlers
            for handler in self.timeout_handlers:
                try:
                    if asyncio.iscoroutinefunction(handler):
                        await handler(pending)
                    else:
                        handler(pending)
                except Exception as e:
                    logger.error("Error in timeout handler", error=str(e))
            
            # Call message callback if provided
            if pending.callback:
                try:
                    if asyncio.iscoroutinefunction(pending.callback):
                        await pending.callback(result)
                    else:
                        pending.callback(result)
                except Exception as e:
                    logger.error("Error in timeout callback", error=str(e))
            
            logger.warning("Message timed out",
                          message_id=message_id,
                          connection_id=pending.connection_id,
                          timeout_seconds=pending.timeout_seconds,
                          retry_count=pending.retry_count)
        
        if timed_out:
            logger.info("Processed message timeouts", count=len(timed_out))
    
    async def _process_retries(self):
        """Process message retries."""
        current_time = time.time()
        to_retry = []
        
        for message_id, pending in list(self.pending_messages.items()):
            # Check if message should be retried (not expired, but past initial timeout)
            if (not pending.is_expired and 
                pending.can_retry and
                current_time - pending.sent_at > pending.timeout_seconds / (pending.retry_count + 1)):
                to_retry.append((message_id, pending))
        
        for message_id, pending in to_retry:
            # Update retry count
            pending.retry_count += 1
            pending.sent_at = current_time
            
            # Apply backoff
            pending.timeout_seconds *= self.retry_backoff_factor
            
            self.stats["total_retries"] += 1
            
            logger.debug("Message queued for retry",
                        message_id=message_id,
                        retry_count=pending.retry_count,
                        new_timeout=pending.timeout_seconds)
        
        if to_retry:
            logger.info("Processed message retries", count=len(to_retry))
    
    async def _cleanup_acknowledged_messages(self):
        """Clean up old acknowledged messages."""
        current_time = time.time()
        cutoff_time = current_time - 3600  # Keep for 1 hour
        
        old_acks = [msg_id for msg_id, ack_time in self.acknowledged_messages.items()
                   if ack_time < cutoff_time]
        
        for msg_id in old_acks:
            del self.acknowledged_messages[msg_id]
        
        if old_acks:
            logger.debug("Cleaned up old acknowledgments", count=len(old_acks))
    
    def _update_average_response_time(self, new_response_time: float):
        """Update average response time statistic."""
        current_avg = self.stats["average_response_time"]
        ack_count = self.stats["messages_acknowledged"]
        
        if ack_count == 1:
            self.stats["average_response_time"] = new_response_time
        else:
            # Calculate running average
            self.stats["average_response_time"] = (
                (current_avg * (ack_count - 1) + new_response_time) / ack_count
            )
    
    def get_stats(self) -> Dict[str, Any]:
        """Get acknowledgment system statistics."""
        current_time = time.time()
        
        # Calculate additional stats
        pending_count = len(self.pending_messages)
        expired_count = sum(1 for p in self.pending_messages.values() if p.is_expired)
        
        connection_stats = {}
        for pending in self.pending_messages.values():
            conn_id = pending.connection_id
            if conn_id not in connection_stats:
                connection_stats[conn_id] = 0
            connection_stats[conn_id] += 1
        
        return {
            **self.stats,
            "current_pending": pending_count,
            "current_expired": expired_count,
            "acknowledged_cache_size": len(self.acknowledged_messages),
            "connection_pending_counts": connection_stats,
            "config": {
                "default_timeout": self.default_timeout,
                "max_retries": self.max_retries,
                "retry_backoff_factor": self.retry_backoff_factor,
                "max_pending_per_connection": self.max_pending_per_connection
            }
        }
    
    def create_ack_message(self, message_id: str, success: bool = True, 
                          error: Optional[str] = None, data: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
        """Create an acknowledgment message."""
        ack_message = {
            "type": "ack",
            "message_id": message_id,
            "success": success,
            "timestamp": time.time()
        }
        
        if error:
            ack_message["error"] = error
        
        if data:
            ack_message["data"] = data
        
        return ack_message


# Global instance
_message_ack_system = None


async def get_message_ack_system() -> MessageAcknowledgmentSystem:
    """Get global message acknowledgment system instance."""
    global _message_ack_system
    if _message_ack_system is None:
        _message_ack_system = MessageAcknowledgmentSystem()
        await _message_ack_system.start()
        logger.info("Message acknowledgment system initialized")
    return _message_ack_system