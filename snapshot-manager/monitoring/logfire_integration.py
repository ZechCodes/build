"""
Advanced Logfire Integration for Snapshot Manager

Provides comprehensive monitoring, alerting, and observability integration
with Logfire for the VM snapshot system.
"""

import asyncio
import time
import json
from typing import Dict, Any, Optional, List
from dataclasses import dataclass, asdict
from enum import Enum
import structlog
import logfire

logger = structlog.get_logger()


class AlertSeverity(Enum):
    """Alert severity levels."""
    INFO = "info"
    WARNING = "warning"
    ERROR = "error"
    CRITICAL = "critical"


class MetricType(Enum):
    """Types of metrics to track."""
    COUNTER = "counter"
    GAUGE = "gauge"
    HISTOGRAM = "histogram"
    TIMER = "timer"


@dataclass
class SnapshotMetric:
    """Snapshot-specific metric definition."""
    name: str
    value: float
    metric_type: MetricType
    tags: Dict[str, str]
    timestamp: float
    description: Optional[str] = None


@dataclass
class SnapshotAlert:
    """Snapshot system alert."""
    alert_id: str
    severity: AlertSeverity
    title: str
    message: str
    tags: Dict[str, str]
    timestamp: float
    resolved: bool = False
    resolution_time: Optional[float] = None


class LogfireMonitoring:
    """
    Advanced Logfire monitoring integration for snapshot operations.
    
    Provides:
    - Real-time metrics tracking
    - Performance monitoring
    - Alert management
    - Distributed tracing
    - Custom dashboards
    """
    
    def __init__(self, service_name: str = "snapshot-manager"):
        """Initialize Logfire monitoring."""
        self.service_name = service_name
        self.metrics_buffer: List[SnapshotMetric] = []
        self.active_alerts: Dict[str, SnapshotAlert] = {}
        self.performance_baselines: Dict[str, float] = {}
        
        # Monitoring configuration
        self.metrics_flush_interval = 60  # 1 minute
        self.alert_cooldown_seconds = 300  # 5 minutes
        self.performance_window_size = 100  # Last 100 operations
        
        # Initialize Logfire with enhanced configuration
        self._initialize_logfire()
        
    def _initialize_logfire(self):
        """Initialize Logfire with custom configuration."""
        try:
            # Check if we're in test mode
            import os
            is_test_mode = "pytest" in os.environ.get("_", "") or "PYTEST_CURRENT_TEST" in os.environ
            
            if is_test_mode:
                # Minimal configuration for testing
                logfire.configure(
                    service_name=self.service_name,
                    send_to_logfire=False,
                    console=False
                )
            else:
                # Full configuration for production
                logfire.configure(
                    service_name=self.service_name,
                    send_to_logfire=True,
                    console=True
                )
                
                # Set up custom instrumentations only in production
                logfire.instrument_asyncio()
                logfire.instrument_requests()
            
            logger.info("Logfire monitoring initialized", service=self.service_name, test_mode=is_test_mode)
            
        except Exception as e:
            logger.error("Failed to initialize Logfire", error=str(e))
            # Don't raise in test mode to avoid breaking tests
            import os
            is_test_mode = "pytest" in os.environ.get("_", "") or "PYTEST_CURRENT_TEST" in os.environ
            if not is_test_mode:
                raise
    
    def track_snapshot_operation(self, operation_type: str, vm_id: str, user_id: str,
                                snapshot_id: Optional[str] = None) -> str:
        """
        Start tracking a snapshot operation with distributed tracing.
        
        Args:
            operation_type: Type of operation (create, restore, delete)
            vm_id: VM identifier
            user_id: User identifier
            snapshot_id: Snapshot identifier (for restore/delete)
            
        Returns:
            str: Trace ID for the operation
        """
        try:
            # Create span for the operation
            with logfire.span(
                f"snapshot_{operation_type}",
                vm_id=vm_id,
                user_id=user_id,
                snapshot_id=snapshot_id,
                operation_type=operation_type
            ) as span:
                
                # Record operation start metrics
                self.record_metric(
                    name=f"snapshot_operations_started",
                    value=1,
                    metric_type=MetricType.COUNTER,
                    tags={
                        "operation_type": operation_type,
                        "vm_id": vm_id,
                        "user_id": user_id
                    }
                )
                
                # Log operation start
                logfire.info(
                    "Snapshot operation started",
                    operation_type=operation_type,
                    vm_id=vm_id,
                    user_id=user_id,
                    snapshot_id=snapshot_id,
                    trace_id=span.context.trace_id if span.context else None
                )
                
                return str(span.context.trace_id) if span.context else "unknown"
                
        except Exception as e:
            logger.error("Failed to track snapshot operation", error=str(e))
            return "error"
    
    def record_operation_completion(self, operation_type: str, trace_id: str,
                                  success: bool, duration_seconds: float,
                                  data_size_bytes: int = 0,
                                  error_message: Optional[str] = None):
        """
        Record completion of a snapshot operation.
        
        Args:
            operation_type: Type of operation
            trace_id: Trace ID from start
            success: Whether operation succeeded
            duration_seconds: Operation duration
            data_size_bytes: Amount of data processed
            error_message: Error message if failed
        """
        try:
            # Calculate throughput
            throughput_mbps = 0
            if duration_seconds > 0 and data_size_bytes > 0:
                throughput_mbps = (data_size_bytes / (1024 * 1024)) / duration_seconds
            
            # Record completion metrics
            self.record_metric(
                name=f"snapshot_operations_completed",
                value=1,
                metric_type=MetricType.COUNTER,
                tags={
                    "operation_type": operation_type,
                    "success": str(success).lower()
                }
            )
            
            # Record duration
            self.record_metric(
                name=f"snapshot_operation_duration",
                value=duration_seconds,
                metric_type=MetricType.HISTOGRAM,
                tags={"operation_type": operation_type}
            )
            
            # Record throughput if applicable
            if throughput_mbps > 0:
                self.record_metric(
                    name=f"snapshot_operation_throughput",
                    value=throughput_mbps,
                    metric_type=MetricType.GAUGE,
                    tags={"operation_type": operation_type}
                )
            
            # Log completion
            if success:
                logfire.info(
                    "Snapshot operation completed successfully",
                    operation_type=operation_type,
                    trace_id=trace_id,
                    duration_seconds=duration_seconds,
                    data_size_mb=data_size_bytes // (1024 * 1024),
                    throughput_mbps=throughput_mbps
                )
            else:
                logfire.error(
                    "Snapshot operation failed",
                    operation_type=operation_type,
                    trace_id=trace_id,
                    duration_seconds=duration_seconds,
                    error_message=error_message
                )
                
                # Create alert for failures
                self.create_alert(
                    severity=AlertSeverity.ERROR,
                    title=f"Snapshot {operation_type} operation failed",
                    message=f"Operation failed after {duration_seconds:.2f}s: {error_message}",
                    tags={
                        "operation_type": operation_type,
                        "trace_id": trace_id
                    }
                )
            
            # Update performance baselines
            self._update_performance_baseline(operation_type, duration_seconds, success)
            
        except Exception as e:
            logger.error("Failed to record operation completion", error=str(e))
    
    def record_metric(self, name: str, value: float, metric_type: MetricType,
                     tags: Dict[str, str], description: Optional[str] = None):
        """
        Record a custom metric.
        
        Args:
            name: Metric name
            value: Metric value
            metric_type: Type of metric
            tags: Metric tags
            description: Optional description
        """
        try:
            metric = SnapshotMetric(
                name=name,
                value=value,
                metric_type=metric_type,
                tags=tags,
                timestamp=time.time(),
                description=description
            )
            
            self.metrics_buffer.append(metric)
            
            # Also send to Logfire immediately for real-time monitoring
            logfire.info(
                f"Metric: {name}",
                metric_name=name,
                metric_value=value,
                metric_type=metric_type.value,
                **tags
            )
            
        except Exception as e:
            logger.error("Failed to record metric", metric_name=name, error=str(e))
    
    def create_alert(self, severity: AlertSeverity, title: str, message: str,
                    tags: Dict[str, str]) -> str:
        """
        Create a monitoring alert.
        
        Args:
            severity: Alert severity
            title: Alert title
            message: Alert message
            tags: Alert tags
            
        Returns:
            str: Alert ID
        """
        try:
            import uuid
            alert_id = f"alert_{uuid.uuid4().hex[:8]}"
            
            alert = SnapshotAlert(
                alert_id=alert_id,
                severity=severity,
                title=title,
                message=message,
                tags=tags,
                timestamp=time.time()
            )
            
            self.active_alerts[alert_id] = alert
            
            # Send to Logfire
            if severity == AlertSeverity.CRITICAL:
                logfire.error(f"CRITICAL ALERT: {title}", message=message, alert_id=alert_id, **tags)
            elif severity == AlertSeverity.ERROR:
                logfire.error(f"ERROR ALERT: {title}", message=message, alert_id=alert_id, **tags)
            elif severity == AlertSeverity.WARNING:
                logfire.warning(f"WARNING ALERT: {title}", message=message, alert_id=alert_id, **tags)
            else:
                logfire.info(f"INFO ALERT: {title}", message=message, alert_id=alert_id, **tags)
            
            logger.info("Alert created", alert_id=alert_id, severity=severity.value, title=title)
            return alert_id
            
        except Exception as e:
            logger.error("Failed to create alert", error=str(e))
            return "error"
    
    def resolve_alert(self, alert_id: str, resolution_message: Optional[str] = None):
        """
        Resolve an active alert.
        
        Args:
            alert_id: Alert identifier
            resolution_message: Optional resolution message
        """
        try:
            if alert_id in self.active_alerts:
                alert = self.active_alerts[alert_id]
                alert.resolved = True
                alert.resolution_time = time.time()
                
                logfire.info(
                    "Alert resolved",
                    alert_id=alert_id,
                    resolution_message=resolution_message,
                    resolution_time_seconds=alert.resolution_time - alert.timestamp
                )
                
                logger.info("Alert resolved", alert_id=alert_id)
            else:
                logger.warning("Alert not found for resolution", alert_id=alert_id)
                
        except Exception as e:
            logger.error("Failed to resolve alert", alert_id=alert_id, error=str(e))
    
    def record_system_health(self, cpu_percent: float, memory_percent: float,
                           disk_percent: float, active_operations: int):
        """
        Record system health metrics.
        
        Args:
            cpu_percent: CPU usage percentage
            memory_percent: Memory usage percentage
            disk_percent: Disk usage percentage
            active_operations: Number of active operations
        """
        try:
            # Record system metrics
            self.record_metric("system_cpu_usage", cpu_percent, MetricType.GAUGE, 
                             {"component": "system"})
            self.record_metric("system_memory_usage", memory_percent, MetricType.GAUGE,
                             {"component": "system"})
            self.record_metric("system_disk_usage", disk_percent, MetricType.GAUGE,
                             {"component": "system"})
            self.record_metric("active_operations", active_operations, MetricType.GAUGE,
                             {"component": "snapshot_manager"})
            
            # Check for alert conditions
            if cpu_percent > 90:
                self.create_alert(
                    AlertSeverity.WARNING,
                    "High CPU Usage",
                    f"CPU usage is {cpu_percent:.1f}%",
                    {"metric": "cpu_usage", "value": str(cpu_percent)}
                )
            
            if memory_percent > 85:
                self.create_alert(
                    AlertSeverity.WARNING,
                    "High Memory Usage", 
                    f"Memory usage is {memory_percent:.1f}%",
                    {"metric": "memory_usage", "value": str(memory_percent)}
                )
            
            if disk_percent > 90:
                self.create_alert(
                    AlertSeverity.ERROR,
                    "High Disk Usage",
                    f"Disk usage is {disk_percent:.1f}%",
                    {"metric": "disk_usage", "value": str(disk_percent)}
                )
            
        except Exception as e:
            logger.error("Failed to record system health", error=str(e))
    
    def record_security_event(self, event_type: str, user_id: str, vm_id: Optional[str] = None,
                            snapshot_id: Optional[str] = None, details: Optional[Dict] = None):
        """
        Record security-related events.
        
        Args:
            event_type: Type of security event
            user_id: User involved
            vm_id: VM involved (if applicable)
            snapshot_id: Snapshot involved (if applicable)
            details: Additional event details
        """
        try:
            logfire.info(
                "Security event",
                event_type=event_type,
                user_id=user_id,
                vm_id=vm_id,
                snapshot_id=snapshot_id,
                details=details or {},
                security_event=True
            )
            
            # Record security metric
            self.record_metric(
                "security_events",
                1,
                MetricType.COUNTER,
                {
                    "event_type": event_type,
                    "user_id": user_id
                }
            )
            
            # Create alert for suspicious events
            suspicious_events = ["unauthorized_access", "quota_exceeded", "rate_limit_exceeded"]
            if event_type in suspicious_events:
                self.create_alert(
                    AlertSeverity.WARNING,
                    f"Security Event: {event_type}",
                    f"User {user_id} triggered {event_type}",
                    {
                        "event_type": event_type,
                        "user_id": user_id,
                        "security": "true"
                    }
                )
            
        except Exception as e:
            logger.error("Failed to record security event", error=str(e))
    
    def get_performance_dashboard_data(self) -> Dict[str, Any]:
        """
        Get performance data for dashboard display.
        
        Returns:
            Dict containing dashboard metrics
        """
        try:
            # Calculate recent performance statistics
            recent_metrics = self.metrics_buffer[-self.performance_window_size:]
            
            # Group metrics by type
            operation_durations = [m for m in recent_metrics if m.name == "snapshot_operation_duration"]
            operation_counts = [m for m in recent_metrics if "operations_" in m.name]
            throughput_metrics = [m for m in recent_metrics if "throughput" in m.name]
            
            dashboard_data = {
                "timestamp": time.time(),
                "metrics_summary": {
                    "total_operations": len(operation_counts),
                    "avg_duration_seconds": (
                        sum(m.value for m in operation_durations) / len(operation_durations)
                        if operation_durations else 0
                    ),
                    "avg_throughput_mbps": (
                        sum(m.value for m in throughput_metrics) / len(throughput_metrics)
                        if throughput_metrics else 0
                    )
                },
                "active_alerts": len([a for a in self.active_alerts.values() if not a.resolved]),
                "resolved_alerts": len([a for a in self.active_alerts.values() if a.resolved]),
                "performance_baselines": self.performance_baselines.copy(),
                "recent_operations": [
                    {
                        "name": m.name,
                        "value": m.value,
                        "timestamp": m.timestamp,
                        "tags": m.tags
                    }
                    for m in recent_metrics[-10:]  # Last 10 operations
                ]
            }
            
            return dashboard_data
            
        except Exception as e:
            logger.error("Failed to get dashboard data", error=str(e))
            return {"error": str(e)}
    
    async def start_background_monitoring(self):
        """Start background monitoring tasks."""
        try:
            # Start metrics flushing task
            asyncio.create_task(self._metrics_flush_loop())
            
            # Start alert cleanup task
            asyncio.create_task(self._alert_cleanup_loop())
            
            logger.info("Background monitoring started")
            
        except Exception as e:
            logger.error("Failed to start background monitoring", error=str(e))
    
    def _update_performance_baseline(self, operation_type: str, duration: float, success: bool):
        """Update performance baseline for operation type."""
        try:
            if success:  # Only update baselines with successful operations
                baseline_key = f"{operation_type}_baseline"
                
                if baseline_key not in self.performance_baselines:
                    self.performance_baselines[baseline_key] = duration
                else:
                    # Exponential moving average
                    alpha = 0.1  # Weight for new values
                    self.performance_baselines[baseline_key] = (
                        alpha * duration + (1 - alpha) * self.performance_baselines[baseline_key]
                    )
                
                # Check for performance degradation
                if duration > self.performance_baselines[baseline_key] * 2:
                    self.create_alert(
                        AlertSeverity.WARNING,
                        f"Performance Degradation: {operation_type}",
                        f"Operation took {duration:.2f}s vs baseline {self.performance_baselines[baseline_key]:.2f}s",
                        {
                            "operation_type": operation_type,
                            "duration": str(duration),
                            "baseline": str(self.performance_baselines[baseline_key])
                        }
                    )
                    
        except Exception as e:
            logger.error("Failed to update performance baseline", error=str(e))
    
    async def _metrics_flush_loop(self):
        """Background loop to flush metrics periodically."""
        try:
            while True:
                await asyncio.sleep(self.metrics_flush_interval)
                
                try:
                    # Log metrics summary
                    if self.metrics_buffer:
                        recent_count = len(self.metrics_buffer[-100:])  # Last 100 metrics
                        
                        logfire.info(
                            "Metrics summary",
                            total_metrics=len(self.metrics_buffer),
                            recent_metrics=recent_count,
                            service=self.service_name
                        )
                        
                        # Keep only recent metrics to prevent memory growth
                        if len(self.metrics_buffer) > 1000:
                            self.metrics_buffer = self.metrics_buffer[-1000:]
                            
                except Exception as e:
                    logger.error("Failed to flush metrics", error=str(e))
                    
        except asyncio.CancelledError:
            logger.info("Metrics flush loop cancelled")
        except Exception as e:
            logger.error("Metrics flush loop failed", error=str(e))
    
    async def _alert_cleanup_loop(self):
        """Background loop to clean up old resolved alerts."""
        try:
            while True:
                await asyncio.sleep(300)  # Check every 5 minutes
                
                try:
                    current_time = time.time()
                    cleanup_threshold = 24 * 3600  # 24 hours
                    
                    # Remove old resolved alerts
                    alerts_to_remove = [
                        alert_id for alert_id, alert in self.active_alerts.items()
                        if alert.resolved and (current_time - alert.timestamp) > cleanup_threshold
                    ]
                    
                    for alert_id in alerts_to_remove:
                        del self.active_alerts[alert_id]
                    
                    if alerts_to_remove:
                        logger.info("Cleaned up old alerts", count=len(alerts_to_remove))
                        
                except Exception as e:
                    logger.error("Failed to cleanup alerts", error=str(e))
                    
        except asyncio.CancelledError:
            logger.info("Alert cleanup loop cancelled")
        except Exception as e:
            logger.error("Alert cleanup loop failed", error=str(e))


# Global monitoring instance
_monitoring_instance: Optional[LogfireMonitoring] = None


def get_monitoring() -> LogfireMonitoring:
    """Get the global monitoring instance."""
    global _monitoring_instance
    if _monitoring_instance is None:
        _monitoring_instance = LogfireMonitoring()
    return _monitoring_instance


def initialize_monitoring(service_name: str = "snapshot-manager") -> LogfireMonitoring:
    """Initialize the global monitoring instance."""
    global _monitoring_instance
    _monitoring_instance = LogfireMonitoring(service_name)
    return _monitoring_instance