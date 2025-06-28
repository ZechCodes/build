"""WebSocket metrics collection and monitoring."""

import asyncio
import time
from collections import defaultdict, deque
from typing import Dict, Any, Optional, List, Union
from dataclasses import dataclass, asdict
from enum import Enum
import structlog

from .redis_storage import get_redis_storage

logger = structlog.get_logger(__name__)


class MetricType(Enum):
    """Types of metrics to collect."""
    COUNTER = "counter"          # Cumulative count
    GAUGE = "gauge"             # Current value
    HISTOGRAM = "histogram"     # Distribution of values
    TIMER = "timer"            # Duration measurements


@dataclass
class MetricPoint:
    """A single metric data point."""
    name: str
    value: Union[int, float]
    timestamp: float
    tags: Dict[str, str]
    metric_type: MetricType


class WebSocketMetricsCollector:
    """Collect and track WebSocket-specific metrics."""
    
    def __init__(self):
        self.redis_storage = None
        
        # In-memory metric storage
        self.counters: Dict[str, int] = defaultdict(int)
        self.gauges: Dict[str, float] = {}
        self.histograms: Dict[str, deque] = defaultdict(lambda: deque(maxlen=1000))
        self.timers: Dict[str, List[float]] = defaultdict(list)
        
        # Connection tracking
        self.active_connections: Dict[str, Dict[str, Any]] = {}
        self.connection_history: deque = deque(maxlen=10000)  # Last 10k connections
        
        # Rate tracking (for establishment/failure rates)
        self.connection_events: deque = deque(maxlen=1000)
        self.auth_events: deque = deque(maxlen=1000)
        self.message_events: deque = deque(maxlen=10000)
        
        # Performance tracking
        self.response_times: deque = deque(maxlen=1000)
        self.error_counts: Dict[str, int] = defaultdict(int)
        
        # Start time for uptime calculation
        self.start_time = time.time()
        
    async def initialize(self):
        """Initialize metrics collector with Redis storage."""
        try:
            self.redis_storage = await get_redis_storage()
            logger.info("WebSocket metrics collector initialized with Redis")
        except Exception as e:
            logger.warning("WebSocket metrics collector initialized without Redis", error=str(e))
    
    # Connection Metrics
    
    def record_connection_attempt(self, connection_id: str, client_ip: str, user_agent: str):
        """Record a connection attempt."""
        timestamp = time.time()
        
        self.counters["connections_attempted"] += 1
        self.connection_events.append({
            "type": "attempt",
            "connection_id": connection_id,
            "client_ip": client_ip,
            "user_agent": user_agent,
            "timestamp": timestamp
        })
        
        logger.debug("Connection attempt recorded", connection_id=connection_id)
    
    def record_connection_established(self, connection_id: str, user_id: Optional[str] = None):
        """Record a successful connection establishment."""
        timestamp = time.time()
        
        self.counters["connections_established"] += 1
        self.gauges["active_connections"] = len(self.active_connections) + 1
        
        # Track connection details
        self.active_connections[connection_id] = {
            "user_id": user_id,
            "connected_at": timestamp,
            "last_activity": timestamp,
            "messages_sent": 0,
            "messages_received": 0,
            "bytes_sent": 0,
            "bytes_received": 0
        }
        
        self.connection_events.append({
            "type": "established",
            "connection_id": connection_id,
            "user_id": user_id,
            "timestamp": timestamp
        })
        
        logger.debug("Connection established recorded", connection_id=connection_id)
    
    def record_connection_failed(self, connection_id: str, reason: str):
        """Record a failed connection attempt."""
        timestamp = time.time()
        
        self.counters["connections_failed"] += 1
        self.error_counts[f"connection_failed_{reason}"] += 1
        
        self.connection_events.append({
            "type": "failed",
            "connection_id": connection_id,
            "reason": reason,
            "timestamp": timestamp
        })
        
        logger.debug("Connection failure recorded", connection_id=connection_id, reason=reason)
    
    def record_connection_closed(self, connection_id: str, reason: str = "normal"):
        """Record a connection closure."""
        timestamp = time.time()
        
        if connection_id in self.active_connections:
            connection_info = self.active_connections[connection_id]
            duration = timestamp - connection_info["connected_at"]
            
            # Record duration in histogram
            self.histograms["connection_duration"].append(duration)
            
            # Move to history
            self.connection_history.append({
                **connection_info,
                "connection_id": connection_id,
                "disconnected_at": timestamp,
                "duration": duration,
                "close_reason": reason
            })
            
            del self.active_connections[connection_id]
        
        self.counters["connections_closed"] += 1
        self.gauges["active_connections"] = len(self.active_connections)
        
        self.connection_events.append({
            "type": "closed",
            "connection_id": connection_id,
            "reason": reason,
            "timestamp": timestamp
        })
        
        logger.debug("Connection closure recorded", connection_id=connection_id, reason=reason)
    
    # Authentication Metrics
    
    def record_auth_attempt(self, connection_id: str, user_id: str):
        """Record an authentication attempt."""
        timestamp = time.time()
        
        self.counters["auth_attempts"] += 1
        self.auth_events.append({
            "type": "attempt",
            "connection_id": connection_id,
            "user_id": user_id,
            "timestamp": timestamp
        })
    
    def record_auth_success(self, connection_id: str, user_id: str):
        """Record successful authentication."""
        timestamp = time.time()
        
        self.counters["auth_successes"] += 1
        
        # Update connection info
        if connection_id in self.active_connections:
            self.active_connections[connection_id]["user_id"] = user_id
            self.active_connections[connection_id]["authenticated_at"] = timestamp
        
        self.auth_events.append({
            "type": "success",
            "connection_id": connection_id,
            "user_id": user_id,
            "timestamp": timestamp
        })
    
    def record_auth_failure(self, connection_id: str, reason: str):
        """Record authentication failure."""
        timestamp = time.time()
        
        self.counters["auth_failures"] += 1
        self.error_counts[f"auth_failed_{reason}"] += 1
        
        self.auth_events.append({
            "type": "failure",
            "connection_id": connection_id,
            "reason": reason,
            "timestamp": timestamp
        })
    
    # Message Metrics
    
    def record_message_sent(self, connection_id: str, message_type: str, size: int):
        """Record a message sent to client."""
        timestamp = time.time()
        
        self.counters["messages_sent"] += 1
        self.counters[f"messages_sent_{message_type}"] += 1
        self.counters["bytes_sent"] += size
        
        # Update connection stats
        if connection_id in self.active_connections:
            self.active_connections[connection_id]["messages_sent"] += 1
            self.active_connections[connection_id]["bytes_sent"] += size
            self.active_connections[connection_id]["last_activity"] = timestamp
        
        # Track message size distribution
        self.histograms[f"message_size_{message_type}"].append(size)
        
        self.message_events.append({
            "type": "sent",
            "connection_id": connection_id,
            "message_type": message_type,
            "size": size,
            "timestamp": timestamp
        })
    
    def record_message_received(self, connection_id: str, message_type: str, size: int):
        """Record a message received from client."""
        timestamp = time.time()
        
        self.counters["messages_received"] += 1
        self.counters[f"messages_received_{message_type}"] += 1
        self.counters["bytes_received"] += size
        
        # Update connection stats
        if connection_id in self.active_connections:
            self.active_connections[connection_id]["messages_received"] += 1
            self.active_connections[connection_id]["bytes_received"] += size
            self.active_connections[connection_id]["last_activity"] = timestamp
        
        # Track message size distribution
        self.histograms[f"message_size_{message_type}"].append(size)
        
        self.message_events.append({
            "type": "received",
            "connection_id": connection_id,
            "message_type": message_type,
            "size": size,
            "timestamp": timestamp
        })
    
    def record_message_error(self, connection_id: str, error_type: str, message_type: str = None):
        """Record a message processing error."""
        self.counters["message_errors"] += 1
        self.error_counts[f"message_error_{error_type}"] += 1
        
        if message_type:
            self.error_counts[f"message_error_{error_type}_{message_type}"] += 1
    
    # Performance Metrics
    
    def record_response_time(self, operation: str, duration: float):
        """Record response time for an operation."""
        self.response_times.append(duration)
        self.histograms[f"response_time_{operation}"].append(duration)
        self.timers[operation].append(duration)
    
    def record_error(self, error_type: str, details: Dict[str, Any] = None):
        """Record an error occurrence."""
        self.counters["total_errors"] += 1
        self.error_counts[error_type] += 1
        
        if details:
            logger.debug("Error recorded", error_type=error_type, details=details)
    
    # Rate Calculations
    
    def get_connection_rate(self, window_seconds: int = 60) -> Dict[str, float]:
        """Calculate connection establishment and failure rates."""
        current_time = time.time()
        window_start = current_time - window_seconds
        
        recent_events = [e for e in self.connection_events if e["timestamp"] >= window_start]
        
        establishments = len([e for e in recent_events if e["type"] == "established"])
        failures = len([e for e in recent_events if e["type"] == "failed"])
        attempts = len([e for e in recent_events if e["type"] == "attempt"])
        
        return {
            "establishment_rate": establishments / window_seconds,
            "failure_rate": failures / window_seconds,
            "attempt_rate": attempts / window_seconds,
            "success_percentage": (establishments / attempts * 100) if attempts > 0 else 0,
            "failure_percentage": (failures / attempts * 100) if attempts > 0 else 0
        }
    
    def get_auth_rate(self, window_seconds: int = 60) -> Dict[str, float]:
        """Calculate authentication success and failure rates."""
        current_time = time.time()
        window_start = current_time - window_seconds
        
        recent_events = [e for e in self.auth_events if e["timestamp"] >= window_start]
        
        successes = len([e for e in recent_events if e["type"] == "success"])
        failures = len([e for e in recent_events if e["type"] == "failure"])
        attempts = len([e for e in recent_events if e["type"] == "attempt"])
        
        return {
            "success_rate": successes / window_seconds,
            "failure_rate": failures / window_seconds,
            "attempt_rate": attempts / window_seconds,
            "success_percentage": (successes / attempts * 100) if attempts > 0 else 0,
            "failure_percentage": (failures / attempts * 100) if attempts > 0 else 0
        }
    
    def get_message_rate(self, window_seconds: int = 60) -> Dict[str, float]:
        """Calculate message throughput rates."""
        current_time = time.time()
        window_start = current_time - window_seconds
        
        recent_events = [e for e in self.message_events if e["timestamp"] >= window_start]
        
        sent = len([e for e in recent_events if e["type"] == "sent"])
        received = len([e for e in recent_events if e["type"] == "received"])
        
        sent_bytes = sum(e["size"] for e in recent_events if e["type"] == "sent")
        received_bytes = sum(e["size"] for e in recent_events if e["type"] == "received")
        
        return {
            "messages_sent_rate": sent / window_seconds,
            "messages_received_rate": received / window_seconds,
            "bytes_sent_rate": sent_bytes / window_seconds,
            "bytes_received_rate": received_bytes / window_seconds
        }
    
    # Duration Statistics
    
    def get_connection_duration_stats(self) -> Dict[str, float]:
        """Get connection duration statistics."""
        durations = list(self.histograms["connection_duration"])
        
        if not durations:
            return {
                "min": 0, "max": 0, "mean": 0, 
                "median": 0, "p95": 0, "p99": 0
            }
        
        durations.sort()
        count = len(durations)
        
        return {
            "min": durations[0],
            "max": durations[-1],
            "mean": sum(durations) / count,
            "median": durations[count // 2],
            "p95": durations[int(count * 0.95)],
            "p99": durations[int(count * 0.99)]
        }
    
    def get_response_time_stats(self) -> Dict[str, float]:
        """Get response time statistics."""
        times = list(self.response_times)
        
        if not times:
            return {
                "min": 0, "max": 0, "mean": 0, 
                "median": 0, "p95": 0, "p99": 0
            }
        
        times.sort()
        count = len(times)
        
        return {
            "min": times[0],
            "max": times[-1],
            "mean": sum(times) / count,
            "median": times[count // 2],
            "p95": times[int(count * 0.95)],
            "p99": times[int(count * 0.99)]
        }
    
    # Comprehensive Metrics Report
    
    def get_comprehensive_metrics(self) -> Dict[str, Any]:
        """Get a comprehensive metrics report."""
        current_time = time.time()
        uptime = current_time - self.start_time
        
        return {
            "timestamp": current_time,
            "uptime_seconds": uptime,
            
            # Connection metrics
            "connections": {
                "active": len(self.active_connections),
                "total_attempted": self.counters["connections_attempted"],
                "total_established": self.counters["connections_established"],
                "total_failed": self.counters["connections_failed"],
                "total_closed": self.counters["connections_closed"],
                "rates": self.get_connection_rate(60),
                "duration_stats": self.get_connection_duration_stats()
            },
            
            # Authentication metrics
            "authentication": {
                "total_attempts": self.counters["auth_attempts"],
                "total_successes": self.counters["auth_successes"],
                "total_failures": self.counters["auth_failures"],
                "rates": self.get_auth_rate(60)
            },
            
            # Message metrics
            "messages": {
                "total_sent": self.counters["messages_sent"],
                "total_received": self.counters["messages_received"],
                "total_bytes_sent": self.counters["bytes_sent"],
                "total_bytes_received": self.counters["bytes_received"],
                "total_errors": self.counters["message_errors"],
                "rates": self.get_message_rate(60)
            },
            
            # Performance metrics
            "performance": {
                "response_time_stats": self.get_response_time_stats(),
                "total_errors": self.counters["total_errors"],
                "error_breakdown": dict(self.error_counts)
            },
            
            # Resource metrics
            "resources": {
                "memory_connections": len(self.active_connections),
                "memory_history_size": len(self.connection_history),
                "memory_events_size": len(self.connection_events) + len(self.auth_events) + len(self.message_events)
            }
        }
    
    async def store_metrics_in_redis(self):
        """Store metrics snapshot in Redis for persistence."""
        if not self.redis_storage:
            return
        
        try:
            metrics = self.get_comprehensive_metrics()
            key = f"ws:metrics:snapshot:{int(time.time())}"
            
            # Store with 24 hour TTL
            await self.redis_storage.redis_client.setex(
                key, 
                86400,  # 24 hours
                self.redis_storage.encryption.encrypt_connection_state(metrics)
            )
            
            logger.debug("Metrics stored in Redis", key=key)
            
        except Exception as e:
            logger.error("Failed to store metrics in Redis", error=str(e))
    
    def cleanup_old_data(self):
        """Clean up old metric data to prevent memory growth."""
        current_time = time.time()
        
        # Clean events older than 1 hour
        cutoff_time = current_time - 3600
        
        # Clean connection events
        while self.connection_events and self.connection_events[0]["timestamp"] < cutoff_time:
            self.connection_events.popleft()
        
        # Clean auth events
        while self.auth_events and self.auth_events[0]["timestamp"] < cutoff_time:
            self.auth_events.popleft()
        
        # Clean message events (keep only last hour)
        while self.message_events and self.message_events[0]["timestamp"] < cutoff_time:
            self.message_events.popleft()
        
        # Clean timer data older than 1 hour
        for operation in list(self.timers.keys()):
            self.timers[operation] = [
                t for t in self.timers[operation] 
                if current_time - t < 3600
            ]
            if not self.timers[operation]:
                del self.timers[operation]


# Global instance
_websocket_metrics = None


async def get_websocket_metrics() -> WebSocketMetricsCollector:
    """Get global WebSocket metrics collector instance."""
    global _websocket_metrics
    if _websocket_metrics is None:
        _websocket_metrics = WebSocketMetricsCollector()
        await _websocket_metrics.initialize()
    return _websocket_metrics