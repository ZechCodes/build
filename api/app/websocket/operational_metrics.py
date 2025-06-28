"""Operational metrics for WebSocket connections including memory, CPU, and bandwidth tracking."""

import asyncio
import psutil
import time
import gc
import sys
from typing import Dict, Any, Optional, List
from dataclasses import dataclass, asdict
from collections import defaultdict, deque
import structlog

logger = structlog.get_logger(__name__)


@dataclass
class ConnectionResourceUsage:
    """Resource usage for a specific connection."""
    connection_id: str
    user_id: Optional[str]
    session_id: Optional[str]
    
    # Memory metrics (bytes)
    allocated_memory: int
    peak_memory: int
    message_queue_memory: int
    
    # Bandwidth metrics (bytes)
    bytes_sent: int
    bytes_received: int
    bytes_sent_rate: float  # bytes/second
    bytes_received_rate: float  # bytes/second
    
    # Connection metrics
    connection_duration: float  # seconds
    message_count_sent: int
    message_count_received: int
    
    # Performance metrics
    avg_response_time: float  # milliseconds
    error_count: int
    
    # Timestamps
    created_at: float
    last_updated: float


@dataclass
class SystemResourceSnapshot:
    """System-wide resource snapshot."""
    timestamp: float
    
    # CPU metrics
    cpu_percent: float
    cpu_count: int
    load_average: tuple
    
    # Memory metrics
    memory_total: int
    memory_available: int
    memory_used: int
    memory_percent: float
    
    # Process-specific metrics
    process_memory_rss: int
    process_memory_vms: int
    process_cpu_percent: float
    process_threads: int
    process_open_files: int
    
    # WebSocket-specific metrics
    active_connections: int
    total_bandwidth_in: int
    total_bandwidth_out: int


class OperationalMetricsCollector:
    """Advanced operational metrics collector for WebSocket connections."""
    
    def __init__(self):
        # System process handle
        self.process = psutil.Process()
        
        # Connection tracking
        self.connection_resources: Dict[str, ConnectionResourceUsage] = {}
        self.system_snapshots: deque = deque(maxlen=1440)  # 24 hours of minute snapshots
        
        # Rate tracking
        self.bandwidth_samples: Dict[str, deque] = defaultdict(lambda: deque(maxlen=60))  # 1 minute of samples
        
        # Collection settings
        self.collection_interval = 60  # seconds
        self.detailed_collection_interval = 10  # seconds for connection-specific metrics
        
        # Background tasks
        self.system_collection_task: Optional[asyncio.Task] = None
        self.connection_collection_task: Optional[asyncio.Task] = None
        
        # Thresholds for alerts
        self.memory_warning_threshold = 80  # percent
        self.cpu_warning_threshold = 80  # percent
        self.bandwidth_warning_threshold = 100 * 1024 * 1024  # 100 MB/s
        
        # Cache for expensive operations
        self._last_system_snapshot: Optional[SystemResourceSnapshot] = None
        self._last_collection_time = 0
    
    async def start(self):
        """Start the operational metrics collection."""
        self.system_collection_task = asyncio.create_task(self._system_collection_loop())
        self.connection_collection_task = asyncio.create_task(self._connection_collection_loop())
        logger.info("Operational metrics collection started")
    
    async def stop(self):
        """Stop the operational metrics collection."""
        if self.system_collection_task:
            self.system_collection_task.cancel()
        if self.connection_collection_task:
            self.connection_collection_task.cancel()
        logger.info("Operational metrics collection stopped")
    
    def register_connection(self, connection_id: str, user_id: Optional[str] = None, 
                          session_id: Optional[str] = None):
        """Register a new connection for tracking."""
        current_time = time.time()
        
        self.connection_resources[connection_id] = ConnectionResourceUsage(
            connection_id=connection_id,
            user_id=user_id,
            session_id=session_id,
            allocated_memory=0,
            peak_memory=0,
            message_queue_memory=0,
            bytes_sent=0,
            bytes_received=0,
            bytes_sent_rate=0.0,
            bytes_received_rate=0.0,
            connection_duration=0.0,
            message_count_sent=0,
            message_count_received=0,
            avg_response_time=0.0,
            error_count=0,
            created_at=current_time,
            last_updated=current_time
        )
        
        logger.debug("Connection registered for metrics", 
                    connection_id=connection_id, user_id=user_id)
    
    def unregister_connection(self, connection_id: str):
        """Unregister a connection from tracking."""
        if connection_id in self.connection_resources:
            resource_usage = self.connection_resources[connection_id]
            
            # Log final resource usage
            logger.info("Connection unregistered",
                       connection_id=connection_id,
                       duration=time.time() - resource_usage.created_at,
                       bytes_sent=resource_usage.bytes_sent,
                       bytes_received=resource_usage.bytes_received,
                       peak_memory=resource_usage.peak_memory,
                       message_count=resource_usage.message_count_sent + resource_usage.message_count_received)
            
            del self.connection_resources[connection_id]
            
            # Clean up bandwidth samples
            if connection_id in self.bandwidth_samples:
                del self.bandwidth_samples[connection_id]
    
    def record_message_sent(self, connection_id: str, size: int, response_time_ms: float = 0):
        """Record a sent message for a connection."""
        if connection_id not in self.connection_resources:
            return
        
        resource = self.connection_resources[connection_id]
        resource.bytes_sent += size
        resource.message_count_sent += 1
        resource.last_updated = time.time()
        
        # Update response time average
        if response_time_ms > 0:
            current_avg = resource.avg_response_time
            count = resource.message_count_sent
            resource.avg_response_time = ((current_avg * (count - 1)) + response_time_ms) / count
        
        # Record bandwidth sample
        self.bandwidth_samples[connection_id].append({
            'timestamp': time.time(),
            'bytes_out': size,
            'bytes_in': 0
        })
    
    def record_message_received(self, connection_id: str, size: int):
        """Record a received message for a connection."""
        if connection_id not in self.connection_resources:
            return
        
        resource = self.connection_resources[connection_id]
        resource.bytes_received += size
        resource.message_count_received += 1
        resource.last_updated = time.time()
        
        # Record bandwidth sample
        self.bandwidth_samples[connection_id].append({
            'timestamp': time.time(),
            'bytes_out': 0,
            'bytes_in': size
        })
    
    def record_error(self, connection_id: str):
        """Record an error for a connection."""
        if connection_id in self.connection_resources:
            self.connection_resources[connection_id].error_count += 1
    
    def estimate_connection_memory(self, connection_id: str) -> int:
        """Estimate memory usage for a specific connection."""
        if connection_id not in self.connection_resources:
            return 0
        
        # Basic estimation based on message queue and data structures
        resource = self.connection_resources[connection_id]
        
        # Base connection overhead (estimated)
        base_memory = 1024 * 10  # 10KB base overhead
        
        # Message queue estimation (rough approximation)
        queue_memory = resource.message_count_sent * 100  # ~100 bytes per message in queue
        
        # Bandwidth tracking memory
        bandwidth_memory = len(self.bandwidth_samples.get(connection_id, [])) * 50
        
        estimated_memory = base_memory + queue_memory + bandwidth_memory
        
        # Update resource tracking
        resource.allocated_memory = estimated_memory
        if estimated_memory > resource.peak_memory:
            resource.peak_memory = estimated_memory
        
        return estimated_memory
    
    def calculate_bandwidth_rates(self, connection_id: str) -> tuple:
        """Calculate current bandwidth rates for a connection."""
        if connection_id not in self.bandwidth_samples:
            return 0.0, 0.0
        
        samples = self.bandwidth_samples[connection_id]
        if len(samples) < 2:
            return 0.0, 0.0
        
        current_time = time.time()
        
        # Calculate rates over last 10 seconds
        recent_samples = [s for s in samples if current_time - s['timestamp'] <= 10]
        
        if len(recent_samples) < 2:
            return 0.0, 0.0
        
        total_bytes_out = sum(s['bytes_out'] for s in recent_samples)
        total_bytes_in = sum(s['bytes_in'] for s in recent_samples)
        time_span = current_time - recent_samples[0]['timestamp']
        
        bytes_out_rate = total_bytes_out / time_span if time_span > 0 else 0
        bytes_in_rate = total_bytes_in / time_span if time_span > 0 else 0
        
        return bytes_out_rate, bytes_in_rate
    
    def get_system_snapshot(self) -> SystemResourceSnapshot:
        """Get current system resource snapshot."""
        current_time = time.time()
        
        # Use cache if recent
        if (self._last_system_snapshot and 
            current_time - self._last_collection_time < 5):
            return self._last_system_snapshot
        
        try:
            # CPU metrics
            cpu_percent = psutil.cpu_percent(interval=0.1)
            cpu_count = psutil.cpu_count()
            try:
                load_avg = psutil.getloadavg()
            except AttributeError:
                # Windows doesn't have getloadavg
                load_avg = (0, 0, 0)
            
            # Memory metrics
            memory = psutil.virtual_memory()
            
            # Process metrics
            process_memory = self.process.memory_info()
            process_cpu = self.process.cpu_percent()
            process_threads = self.process.num_threads()
            
            try:
                process_files = self.process.num_fds()  # Unix
            except (AttributeError, psutil.AccessDenied):
                try:
                    process_files = self.process.num_handles()  # Windows
                except (AttributeError, psutil.AccessDenied):
                    process_files = 0
            
            # WebSocket metrics
            total_bandwidth_in = sum(r.bytes_received for r in self.connection_resources.values())
            total_bandwidth_out = sum(r.bytes_sent for r in self.connection_resources.values())
            
            snapshot = SystemResourceSnapshot(
                timestamp=current_time,
                cpu_percent=cpu_percent,
                cpu_count=cpu_count,
                load_average=load_avg,
                memory_total=memory.total,
                memory_available=memory.available,
                memory_used=memory.used,
                memory_percent=memory.percent,
                process_memory_rss=process_memory.rss,
                process_memory_vms=process_memory.vms,
                process_cpu_percent=process_cpu,
                process_threads=process_threads,
                process_open_files=process_files,
                active_connections=len(self.connection_resources),
                total_bandwidth_in=total_bandwidth_in,
                total_bandwidth_out=total_bandwidth_out
            )
            
            self._last_system_snapshot = snapshot
            self._last_collection_time = current_time
            
            return snapshot
            
        except Exception as e:
            logger.error("Failed to collect system snapshot", error=str(e))
            # Return a minimal snapshot
            return SystemResourceSnapshot(
                timestamp=current_time,
                cpu_percent=0, cpu_count=1, load_average=(0, 0, 0),
                memory_total=0, memory_available=0, memory_used=0, memory_percent=0,
                process_memory_rss=0, process_memory_vms=0, process_cpu_percent=0,
                process_threads=0, process_open_files=0,
                active_connections=len(self.connection_resources),
                total_bandwidth_in=0, total_bandwidth_out=0
            )
    
    def get_connection_metrics(self, connection_id: str) -> Optional[ConnectionResourceUsage]:
        """Get metrics for a specific connection."""
        if connection_id not in self.connection_resources:
            return None
        
        resource = self.connection_resources[connection_id]
        
        # Update calculated fields
        current_time = time.time()
        resource.connection_duration = current_time - resource.created_at
        
        # Update bandwidth rates
        bytes_out_rate, bytes_in_rate = self.calculate_bandwidth_rates(connection_id)
        resource.bytes_sent_rate = bytes_out_rate
        resource.bytes_received_rate = bytes_in_rate
        
        # Update memory estimate
        self.estimate_connection_memory(connection_id)
        
        return resource
    
    def get_aggregated_metrics(self) -> Dict[str, Any]:
        """Get aggregated operational metrics."""
        current_time = time.time()
        system_snapshot = self.get_system_snapshot()
        
        # Aggregate connection metrics
        total_memory = sum(self.estimate_connection_memory(cid) for cid in self.connection_resources.keys())
        total_messages = sum(r.message_count_sent + r.message_count_received for r in self.connection_resources.values())
        total_errors = sum(r.error_count for r in self.connection_resources.values())
        
        # Calculate averages
        active_connections = len(self.connection_resources)
        avg_memory_per_connection = total_memory / active_connections if active_connections > 0 else 0
        avg_messages_per_connection = total_messages / active_connections if active_connections > 0 else 0
        
        # Bandwidth rates
        total_bandwidth_out_rate = sum(r.bytes_sent_rate for r in self.connection_resources.values())
        total_bandwidth_in_rate = sum(r.bytes_received_rate for r in self.connection_resources.values())
        
        return {
            "timestamp": current_time,
            "system": asdict(system_snapshot),
            "connections": {
                "active_count": active_connections,
                "total_memory_bytes": total_memory,
                "avg_memory_per_connection": avg_memory_per_connection,
                "total_messages": total_messages,
                "avg_messages_per_connection": avg_messages_per_connection,
                "total_errors": total_errors,
                "bandwidth_out_rate": total_bandwidth_out_rate,
                "bandwidth_in_rate": total_bandwidth_in_rate
            },
            "alerts": self._check_alert_conditions(system_snapshot)
        }
    
    def _check_alert_conditions(self, snapshot: SystemResourceSnapshot) -> List[Dict[str, Any]]:
        """Check for alert conditions in metrics."""
        alerts = []
        
        # Memory alerts
        if snapshot.memory_percent > self.memory_warning_threshold:
            alerts.append({
                "type": "memory_high",
                "severity": "warning",
                "message": f"System memory usage at {snapshot.memory_percent:.1f}%",
                "value": snapshot.memory_percent,
                "threshold": self.memory_warning_threshold
            })
        
        # CPU alerts
        if snapshot.cpu_percent > self.cpu_warning_threshold:
            alerts.append({
                "type": "cpu_high",
                "severity": "warning", 
                "message": f"CPU usage at {snapshot.cpu_percent:.1f}%",
                "value": snapshot.cpu_percent,
                "threshold": self.cpu_warning_threshold
            })
        
        # Process memory alerts
        process_memory_mb = snapshot.process_memory_rss / (1024 * 1024)
        if process_memory_mb > 1000:  # 1GB
            alerts.append({
                "type": "process_memory_high",
                "severity": "warning",
                "message": f"Process memory usage at {process_memory_mb:.1f} MB",
                "value": process_memory_mb,
                "threshold": 1000
            })
        
        # Bandwidth alerts
        total_bandwidth = snapshot.total_bandwidth_in + snapshot.total_bandwidth_out
        if total_bandwidth > self.bandwidth_warning_threshold:
            alerts.append({
                "type": "bandwidth_high",
                "severity": "warning",
                "message": f"Total bandwidth usage at {total_bandwidth / (1024*1024):.1f} MB",
                "value": total_bandwidth,
                "threshold": self.bandwidth_warning_threshold
            })
        
        return alerts
    
    async def _system_collection_loop(self):
        """Background task for system metrics collection."""
        while True:
            try:
                await asyncio.sleep(self.collection_interval)
                
                snapshot = self.get_system_snapshot()
                self.system_snapshots.append(snapshot)
                
                # Log significant changes
                if len(self.system_snapshots) > 1:
                    prev_snapshot = self.system_snapshots[-2]
                    
                    # Log if CPU or memory changed significantly
                    if abs(snapshot.cpu_percent - prev_snapshot.cpu_percent) > 20:
                        logger.info("Significant CPU change",
                                  prev_cpu=prev_snapshot.cpu_percent,
                                  current_cpu=snapshot.cpu_percent)
                    
                    if abs(snapshot.memory_percent - prev_snapshot.memory_percent) > 10:
                        logger.info("Significant memory change",
                                  prev_memory=prev_snapshot.memory_percent,
                                  current_memory=snapshot.memory_percent)
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("System collection loop error", error=str(e))
    
    async def _connection_collection_loop(self):
        """Background task for connection-specific metrics collection."""
        while True:
            try:
                await asyncio.sleep(self.detailed_collection_interval)
                
                # Update connection metrics
                for connection_id in list(self.connection_resources.keys()):
                    self.get_connection_metrics(connection_id)
                
                # Force garbage collection periodically
                if time.time() % 300 < self.detailed_collection_interval:  # Every 5 minutes
                    collected = gc.collect()
                    if collected > 0:
                        logger.debug("Garbage collection", objects_collected=collected)
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Connection collection loop error", error=str(e))


# Global instance
_operational_metrics = None


async def get_operational_metrics() -> OperationalMetricsCollector:
    """Get global operational metrics collector instance."""
    global _operational_metrics
    if _operational_metrics is None:
        _operational_metrics = OperationalMetricsCollector()
        await _operational_metrics.start()
        logger.info("Operational metrics collector initialized")
    return _operational_metrics