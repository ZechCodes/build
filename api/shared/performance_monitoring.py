"""
Performance Monitoring and Metrics Collection Framework

Provides comprehensive monitoring for:
- Terminal latency and responsiveness
- VM operations (creation, start, stop)
- Session management performance
- WebSocket connection metrics
- Storage operation timing
- System resource utilization
- Business metrics and KPIs
"""

import asyncio
import time
import psutil
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Any, Callable, Union
from enum import Enum
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from collections import defaultdict, deque

from prometheus_client import (
    Counter, Histogram, Gauge, Summary, 
    CollectorRegistry, generate_latest,
    CONTENT_TYPE_LATEST
)
from pydantic import BaseModel, Field
import structlog


class MetricType(str, Enum):
    """Types of metrics."""
    COUNTER = "counter"
    HISTOGRAM = "histogram"
    GAUGE = "gauge"
    SUMMARY = "summary"


class MetricUnit(str, Enum):
    """Metric units."""
    SECONDS = "seconds"
    MILLISECONDS = "milliseconds"
    BYTES = "bytes"
    PERCENTAGE = "percentage"
    COUNT = "count"
    REQUESTS_PER_SECOND = "requests_per_second"


@dataclass
class MetricDefinition:
    """Definition of a metric."""
    name: str
    description: str
    metric_type: MetricType
    unit: MetricUnit
    labels: List[str] = field(default_factory=list)
    buckets: Optional[List[float]] = None  # For histograms


class PerformanceMetrics:
    """Core performance metrics definitions."""
    
    def __init__(self, registry: CollectorRegistry = None):
        self.registry = registry or CollectorRegistry()
        self.metrics = {}
        self._initialize_metrics()
    
    def _initialize_metrics(self):
        """Initialize all performance metrics."""
        
        # Terminal and WebSocket Metrics
        self.terminal_latency = Histogram(
            'terminal_response_time_seconds',
            'Terminal response time in seconds',
            ['session_id', 'operation'],
            buckets=[0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0],
            registry=self.registry
        )
        
        self.websocket_connections = Gauge(
            'websocket_connections_active',
            'Number of active WebSocket connections',
            ['session_type'],
            registry=self.registry
        )
        
        self.websocket_messages = Counter(
            'websocket_messages_total',
            'Total WebSocket messages sent/received',
            ['direction', 'message_type', 'session_id'],
            registry=self.registry
        )
        
        self.websocket_errors = Counter(
            'websocket_errors_total',
            'Total WebSocket errors',
            ['error_type'],
            registry=self.registry
        )
        
        # VM Management Metrics
        self.vm_operations = Histogram(
            'vm_operation_duration_seconds',
            'VM operation duration in seconds',
            ['operation', 'status'],
            buckets=[1, 5, 10, 30, 60, 120, 300, 600],
            registry=self.registry
        )
        
        self.vm_count = Gauge(
            'vm_instances_total',
            'Total number of VM instances',
            ['status', 'user_id'],
            registry=self.registry
        )
        
        self.vm_resource_usage = Gauge(
            'vm_resource_usage',
            'VM resource usage',
            ['vm_id', 'resource_type'],
            registry=self.registry
        )
        
        # Session Management Metrics
        self.session_creation_time = Histogram(
            'session_creation_duration_seconds',
            'Session creation time in seconds',
            ['session_type'],
            buckets=[0.1, 0.5, 1, 2, 5, 10, 30],
            registry=self.registry
        )
        
        self.session_recovery_time = Histogram(
            'session_recovery_duration_seconds',
            'Session recovery time in seconds',
            ['recovery_method'],
            buckets=[0.5, 1, 2, 5, 10, 20, 60],
            registry=self.registry
        )
        
        self.active_sessions = Gauge(
            'sessions_active_total',
            'Number of active sessions',
            ['session_type', 'user_id'],
            registry=self.registry
        )
        
        # Storage Metrics
        self.storage_operations = Histogram(
            'storage_operation_duration_seconds',
            'Storage operation duration',
            ['operation', 'backend', 'object_type'],
            buckets=[0.1, 0.5, 1, 5, 10, 30, 60, 120],
            registry=self.registry
        )
        
        self.storage_size = Gauge(
            'storage_size_bytes',
            'Storage size in bytes',
            ['user_id', 'object_type'],
            registry=self.registry
        )
        
        # Snapshot Metrics
        self.snapshot_operations = Histogram(
            'snapshot_operation_duration_seconds',
            'Snapshot operation duration',
            ['operation', 'vm_id'],
            buckets=[10, 30, 60, 120, 300, 600, 1200],
            registry=self.registry
        )
        
        self.snapshot_size = Histogram(
            'snapshot_size_bytes',
            'Snapshot size distribution',
            ['vm_type'],
            buckets=[1e6, 10e6, 100e6, 500e6, 1e9, 5e9, 10e9, 50e9],
            registry=self.registry
        )
        
        # System Resource Metrics
        self.system_cpu_usage = Gauge(
            'system_cpu_usage_percentage',
            'System CPU usage percentage',
            ['host'],
            registry=self.registry
        )
        
        self.system_memory_usage = Gauge(
            'system_memory_usage_bytes',
            'System memory usage in bytes',
            ['host', 'type'],
            registry=self.registry
        )
        
        self.system_disk_usage = Gauge(
            'system_disk_usage_bytes',
            'System disk usage in bytes',
            ['host', 'mount_point', 'type'],
            registry=self.registry
        )
        
        # Error and Health Metrics
        self.error_count = Counter(
            'errors_total',
            'Total number of errors',
            ['error_code', 'component', 'severity'],
            registry=self.registry
        )
        
        self.health_check_status = Gauge(
            'health_check_status',
            'Health check status (1=healthy, 0=unhealthy)',
            ['service', 'check_type'],
            registry=self.registry
        )
        
        # Business Metrics
        self.user_activity = Counter(
            'user_activity_total',
            'Total user activity events',
            ['user_id', 'activity_type'],
            registry=self.registry
        )
        
        self.concurrent_users = Gauge(
            'concurrent_users_total',
            'Number of concurrent users',
            [],
            registry=self.registry
        )
        
        # Rate Limiting Metrics
        self.rate_limit_hits = Counter(
            'rate_limit_hits_total',
            'Total rate limit hits',
            ['limit_type', 'user_id'],
            registry=self.registry
        )


class PerformanceTimer:
    """Context manager for timing operations."""
    
    def __init__(self, metric: Union[Histogram, Summary], labels: Dict[str, str] = None):
        self.metric = metric
        self.labels = labels or {}
        self.start_time = None
    
    def __enter__(self):
        self.start_time = time.time()
        return self
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        duration = time.time() - self.start_time
        self.metric.labels(**self.labels).observe(duration)


@asynccontextmanager
async def async_timer(metric: Union[Histogram, Summary], labels: Dict[str, str] = None):
    """Async context manager for timing operations."""
    start_time = time.time()
    try:
        yield
    finally:
        duration = time.time() - start_time
        metric.labels(**(labels or {})).observe(duration)


class SystemResourceMonitor:
    """Monitor system resource usage."""
    
    def __init__(self, metrics: PerformanceMetrics, update_interval: int = 30):
        self.metrics = metrics
        self.update_interval = update_interval
        self.monitoring = False
        self.hostname = psutil.uname().node
        
    async def start_monitoring(self):
        """Start background monitoring."""
        self.monitoring = True
        while self.monitoring:
            try:
                await self._collect_system_metrics()
                await asyncio.sleep(self.update_interval)
            except Exception as e:
                structlog.get_logger().error("System monitoring error", error=str(e))
                await asyncio.sleep(self.update_interval)
    
    def stop_monitoring(self):
        """Stop background monitoring."""
        self.monitoring = False
    
    async def _collect_system_metrics(self):
        """Collect system metrics."""
        # CPU Usage
        cpu_percent = psutil.cpu_percent(interval=1)
        self.metrics.system_cpu_usage.labels(host=self.hostname).set(cpu_percent)
        
        # Memory Usage
        memory = psutil.virtual_memory()
        self.metrics.system_memory_usage.labels(
            host=self.hostname, type="used"
        ).set(memory.used)
        self.metrics.system_memory_usage.labels(
            host=self.hostname, type="available"
        ).set(memory.available)
        
        # Disk Usage
        for partition in psutil.disk_partitions():
            try:
                usage = psutil.disk_usage(partition.mountpoint)
                self.metrics.system_disk_usage.labels(
                    host=self.hostname,
                    mount_point=partition.mountpoint,
                    type="used"
                ).set(usage.used)
                self.metrics.system_disk_usage.labels(
                    host=self.hostname,
                    mount_point=partition.mountpoint,
                    type="total"
                ).set(usage.total)
            except PermissionError:
                # Skip inaccessible partitions
                pass


class HealthChecker:
    """Service health monitoring."""
    
    def __init__(self, metrics: PerformanceMetrics):
        self.metrics = metrics
        self.checks: Dict[str, Callable] = {}
        
    def register_check(self, name: str, check_func: Callable, check_type: str = "liveness"):
        """Register a health check function."""
        self.checks[f"{name}:{check_type}"] = check_func
    
    async def run_health_checks(self):
        """Run all registered health checks."""
        for check_name, check_func in self.checks.items():
            service, check_type = check_name.split(":", 1)
            try:
                if asyncio.iscoroutinefunction(check_func):
                    is_healthy = await check_func()
                else:
                    is_healthy = check_func()
                
                status = 1 if is_healthy else 0
                self.metrics.health_check_status.labels(
                    service=service,
                    check_type=check_type
                ).set(status)
                
            except Exception as e:
                structlog.get_logger().error(
                    "Health check failed",
                    service=service,
                    check_type=check_type,
                    error=str(e)
                )
                self.metrics.health_check_status.labels(
                    service=service,
                    check_type=check_type
                ).set(0)


class MetricsCollector:
    """Central metrics collection service."""
    
    def __init__(self):
        self.metrics = PerformanceMetrics()
        self.resource_monitor = SystemResourceMonitor(self.metrics)
        self.health_checker = HealthChecker(self.metrics)
        self.logger = structlog.get_logger()
        
        # Metric aggregation
        self.metric_buffer = defaultdict(lambda: deque(maxlen=1000))
        
    async def start(self):
        """Start metrics collection."""
        # Start system resource monitoring
        asyncio.create_task(self.resource_monitor.start_monitoring())
        
        # Start periodic health checks
        asyncio.create_task(self._periodic_health_checks())
        
        self.logger.info("Metrics collection started")
    
    async def stop(self):
        """Stop metrics collection."""
        self.resource_monitor.stop_monitoring()
        self.logger.info("Metrics collection stopped")
    
    async def _periodic_health_checks(self):
        """Run health checks periodically."""
        while True:
            try:
                await self.health_checker.run_health_checks()
                await asyncio.sleep(60)  # Check every minute
            except Exception as e:
                self.logger.error("Health check error", error=str(e))
                await asyncio.sleep(60)
    
    # WebSocket Metrics
    def record_websocket_connection(self, session_type: str, connected: bool = True):
        """Record WebSocket connection."""
        if connected:
            self.metrics.websocket_connections.labels(session_type=session_type).inc()
        else:
            self.metrics.websocket_connections.labels(session_type=session_type).dec()
    
    def record_websocket_message(self, direction: str, message_type: str, session_id: str):
        """Record WebSocket message."""
        self.metrics.websocket_messages.labels(
            direction=direction,
            message_type=message_type,
            session_id=session_id
        ).inc()
    
    def record_websocket_error(self, error_type: str):
        """Record WebSocket error."""
        self.metrics.websocket_errors.labels(error_type=error_type).inc()
    
    # Terminal Metrics
    @asynccontextmanager
    async def measure_terminal_operation(self, session_id: str, operation: str):
        """Measure terminal operation latency."""
        async with async_timer(
            self.metrics.terminal_latency,
            {"session_id": session_id, "operation": operation}
        ):
            yield
    
    # VM Metrics
    @asynccontextmanager
    async def measure_vm_operation(self, operation: str, vm_id: str = None):
        """Measure VM operation duration."""
        start_time = time.time()
        status = "success"
        try:
            yield
        except Exception:
            status = "error"
            raise
        finally:
            duration = time.time() - start_time
            self.metrics.vm_operations.labels(
                operation=operation,
                status=status
            ).observe(duration)
    
    def update_vm_count(self, status: str, user_id: str, count: int):
        """Update VM count metric."""
        self.metrics.vm_count.labels(status=status, user_id=user_id).set(count)
    
    def record_vm_resource_usage(self, vm_id: str, resource_type: str, value: float):
        """Record VM resource usage."""
        self.metrics.vm_resource_usage.labels(
            vm_id=vm_id,
            resource_type=resource_type
        ).set(value)
    
    # Session Metrics
    @asynccontextmanager
    async def measure_session_creation(self, session_type: str):
        """Measure session creation time."""
        async with async_timer(
            self.metrics.session_creation_time,
            {"session_type": session_type}
        ):
            yield
    
    @asynccontextmanager
    async def measure_session_recovery(self, recovery_method: str):
        """Measure session recovery time."""
        async with async_timer(
            self.metrics.session_recovery_time,
            {"recovery_method": recovery_method}
        ):
            yield
    
    def update_active_sessions(self, session_type: str, user_id: str, count: int):
        """Update active sessions count."""
        self.metrics.active_sessions.labels(
            session_type=session_type,
            user_id=user_id
        ).set(count)
    
    # Storage Metrics
    @asynccontextmanager
    async def measure_storage_operation(self, operation: str, backend: str, object_type: str):
        """Measure storage operation duration."""
        async with async_timer(
            self.metrics.storage_operations,
            {"operation": operation, "backend": backend, "object_type": object_type}
        ):
            yield
    
    def update_storage_size(self, user_id: str, object_type: str, size_bytes: int):
        """Update storage size metric."""
        self.metrics.storage_size.labels(
            user_id=user_id,
            object_type=object_type
        ).set(size_bytes)
    
    # Snapshot Metrics
    @asynccontextmanager
    async def measure_snapshot_operation(self, operation: str, vm_id: str):
        """Measure snapshot operation duration."""
        async with async_timer(
            self.metrics.snapshot_operations,
            {"operation": operation, "vm_id": vm_id}
        ):
            yield
    
    def record_snapshot_size(self, vm_type: str, size_bytes: int):
        """Record snapshot size."""
        self.metrics.snapshot_size.labels(vm_type=vm_type).observe(size_bytes)
    
    # Error Metrics
    def record_error(self, error_code: str, component: str, severity: str):
        """Record error occurrence."""
        self.metrics.error_count.labels(
            error_code=error_code,
            component=component,
            severity=severity
        ).inc()
    
    # Business Metrics
    def record_user_activity(self, user_id: str, activity_type: str):
        """Record user activity."""
        self.metrics.user_activity.labels(
            user_id=user_id,
            activity_type=activity_type
        ).inc()
    
    def update_concurrent_users(self, count: int):
        """Update concurrent users count."""
        self.metrics.concurrent_users.set(count)
    
    # Rate Limiting Metrics
    def record_rate_limit_hit(self, limit_type: str, user_id: str):
        """Record rate limit hit."""
        self.metrics.rate_limit_hits.labels(
            limit_type=limit_type,
            user_id=user_id
        ).inc()
    
    # Metrics Export
    def get_metrics(self) -> str:
        """Get Prometheus metrics as string."""
        return generate_latest(self.metrics.registry)
    
    def get_content_type(self) -> str:
        """Get content type for metrics."""
        return CONTENT_TYPE_LATEST


class PerformanceAnalyzer:
    """Analyze performance trends and anomalies."""
    
    def __init__(self, metrics_collector: MetricsCollector):
        self.metrics_collector = metrics_collector
        self.thresholds = {
            "terminal_latency_p95": 0.1,  # 100ms
            "vm_creation_time_p95": 30.0,  # 30 seconds
            "session_recovery_time_p95": 5.0,  # 5 seconds
            "cpu_usage_avg": 80.0,  # 80%
            "memory_usage_percentage": 85.0,  # 85%
            "error_rate": 0.01  # 1%
        }
    
    async def analyze_performance(self) -> Dict[str, Any]:
        """Analyze current performance metrics."""
        analysis = {
            "timestamp": datetime.utcnow().isoformat(),
            "alerts": [],
            "summary": {},
            "recommendations": []
        }
        
        # This would implement actual performance analysis
        # For now, returning structure
        return analysis
    
    def check_thresholds(self) -> List[Dict[str, Any]]:
        """Check if any metrics exceed thresholds."""
        alerts = []
        
        # This would implement threshold checking logic
        # against the actual metrics data
        
        return alerts


# Global metrics collector instance
metrics_collector = MetricsCollector()


# Decorator for automatic performance monitoring
def monitor_performance(
    operation: str = None,
    component: str = None,
    record_errors: bool = True
):
    """Decorator for automatic performance monitoring."""
    def decorator(func: Callable) -> Callable:
        operation_name = operation or func.__name__
        component_name = component or func.__module__
        
        async def async_wrapper(*args, **kwargs):
            start_time = time.time()
            status = "success"
            
            try:
                result = await func(*args, **kwargs)
                return result
            except Exception as e:
                status = "error"
                if record_errors:
                    metrics_collector.record_error(
                        error_code=type(e).__name__,
                        component=component_name,
                        severity="high"
                    )
                raise
            finally:
                duration = time.time() - start_time
                # Record operation duration
                # This would use appropriate metric based on operation type
        
        def sync_wrapper(*args, **kwargs):
            start_time = time.time()
            status = "success"
            
            try:
                result = func(*args, **kwargs)
                return result
            except Exception as e:
                status = "error"
                if record_errors:
                    metrics_collector.record_error(
                        error_code=type(e).__name__,
                        component=component_name,
                        severity="high"
                    )
                raise
            finally:
                duration = time.time() - start_time
                # Record operation duration
        
        if asyncio.iscoroutinefunction(func):
            return async_wrapper
        else:
            return sync_wrapper
    
    return decorator


# Health check functions
async def check_database_health() -> bool:
    """Check database connectivity."""
    # Would implement actual database health check
    return True


async def check_redis_health() -> bool:
    """Check Redis connectivity."""
    # Would implement actual Redis health check
    return True


async def check_storage_health() -> bool:
    """Check storage service health."""
    # Would implement actual storage health check
    return True


# Configuration
class MonitoringConfig:
    """Monitoring configuration."""
    
    # Collection intervals
    SYSTEM_METRICS_INTERVAL = 30  # seconds
    HEALTH_CHECK_INTERVAL = 60   # seconds
    
    # Retention
    METRIC_BUFFER_SIZE = 1000
    
    # Thresholds
    TERMINAL_LATENCY_THRESHOLD = 0.1  # 100ms
    VM_OPERATION_THRESHOLD = 60.0     # 60 seconds
    SESSION_RECOVERY_THRESHOLD = 5.0  # 5 seconds
    
    # Alerting
    ENABLE_ALERTING = True
    ALERT_WEBHOOK_URL = None
    
    @classmethod
    def from_env(cls):
        """Load configuration from environment."""
        import os
        return cls(
            SYSTEM_METRICS_INTERVAL=int(os.getenv("METRICS_INTERVAL", "30")),
            HEALTH_CHECK_INTERVAL=int(os.getenv("HEALTH_CHECK_INTERVAL", "60")),
            TERMINAL_LATENCY_THRESHOLD=float(os.getenv("TERMINAL_LATENCY_THRESHOLD", "0.1")),
            ENABLE_ALERTING=os.getenv("ENABLE_ALERTING", "true").lower() == "true"
        )


# Initialize monitoring
async def initialize_monitoring():
    """Initialize performance monitoring."""
    await metrics_collector.start()
    
    # Register health checks
    metrics_collector.health_checker.register_check(
        "database", check_database_health, "liveness"
    )
    metrics_collector.health_checker.register_check(
        "redis", check_redis_health, "liveness"
    )
    metrics_collector.health_checker.register_check(
        "storage", check_storage_health, "readiness"
    )
    
    structlog.get_logger().info("Performance monitoring initialized")


if __name__ == "__main__":
    # Example usage
    async def example_monitoring():
        await initialize_monitoring()
        
        # Example VM operation monitoring
        async with metrics_collector.measure_vm_operation("create", "vm-123"):
            await asyncio.sleep(0.1)  # Simulate work
        
        # Example terminal operation monitoring
        async with metrics_collector.measure_terminal_operation("session-456", "command"):
            await asyncio.sleep(0.01)  # Simulate work
        
        # Get metrics
        metrics_output = metrics_collector.get_metrics()
        print("Metrics collected successfully")
    
    asyncio.run(example_monitoring())