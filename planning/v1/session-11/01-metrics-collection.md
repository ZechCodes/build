# Session 11.1: Advanced Metrics Collection & Monitoring

## Objective
Implement comprehensive metrics collection system with advanced monitoring capabilities, providing real-time visibility into system performance, resource utilization, and application behavior across all platform components.

## Integration with Previous Sessions
- **Session 1**: Enhances existing Logfire integration with advanced metrics capabilities
- **Session 2**: Monitors authentication performance and security metrics
- **Session 3**: Tracks VM lifecycle and performance metrics
- **Session 6**: Monitors session management and WebSocket performance
- **Session 7**: Tracks storage performance and capacity metrics
- **Session 8**: Monitors frontend performance and user interaction metrics
- **Session 9**: Tracks Git operations and repository metrics
- **Session 10**: Monitors recording system performance and storage utilization

## Core Implementation

### Advanced Metrics Collection Engine
**Location**: `monitoring/metrics/collector.py`

```python
# monitoring/metrics/collector.py
import asyncio
import time
import psutil
import json
from typing import Dict, Any, List, Optional, Set, Callable, Union
from dataclasses import dataclass, field, asdict
from enum import Enum
from datetime import datetime, timedelta
import structlog
import logfire
from prometheus_client import Counter, Histogram, Gauge, CollectorRegistry, Info, Summary
from concurrent.futures import ThreadPoolExecutor
import threading

logger = structlog.get_logger()

class MetricType(Enum):
    COUNTER = "counter"
    GAUGE = "gauge"
    HISTOGRAM = "histogram"
    SUMMARY = "summary"
    INFO = "info"

class MetricScope(Enum):
    SYSTEM = "system"
    APPLICATION = "application"
    BUSINESS = "business"
    SECURITY = "security"
    CUSTOM = "custom"

@dataclass
class MetricDefinition:
    name: str
    type: MetricType
    scope: MetricScope
    description: str
    labels: List[str]
    unit: Optional[str] = None
    help_text: Optional[str] = None
    buckets: Optional[List[float]] = None  # For histograms
    quantiles: Optional[Dict[float, float]] = None  # For summaries

@dataclass
class MetricSample:
    name: str
    value: Union[int, float]
    labels: Dict[str, str]
    timestamp: float
    type: MetricType

@dataclass
class CollectionConfig:
    interval_seconds: int = 10
    enabled_scopes: Set[MetricScope] = field(default_factory=lambda: set(MetricScope))
    custom_collectors: List[Callable] = field(default_factory=list)
    retention_hours: int = 24
    max_samples_per_metric: int = 1000
    aggregation_window_seconds: int = 60

class AdvancedMetricsCollector:
    def __init__(self, config: CollectionConfig = None, registry: CollectorRegistry = None):
        self.config = config or CollectionConfig()
        self.registry = registry or CollectorRegistry()
        self.metrics: Dict[str, Any] = {}
        self.custom_metrics: Dict[str, MetricDefinition] = {}
        self.metric_samples: Dict[str, List[MetricSample]] = {}
        
        # Thread pool for CPU-intensive operations
        self.thread_pool = ThreadPoolExecutor(max_workers=4)
        
        # Collection state
        self.collection_task: Optional[asyncio.Task] = None
        self.is_collecting = False
        self.last_collection_time = 0
        
        # Performance tracking
        self.collection_stats = {
            "total_collections": 0,
            "total_metrics_collected": 0,
            "average_collection_time_ms": 0,
            "last_collection_duration_ms": 0,
            "errors_count": 0
        }
        
        # Initialize metrics
        self._initialize_platform_metrics()
        self._initialize_system_metrics()
        self._initialize_business_metrics()
        self._initialize_security_metrics()

    def _initialize_platform_metrics(self):
        """Initialize core platform metrics"""
        # HTTP/API metrics
        self.metrics['http_requests_total'] = Counter(
            'http_requests_total',
            'Total HTTP requests by method, endpoint, and status',
            ['method', 'endpoint', 'status_code', 'user_type'],
            registry=self.registry
        )
        
        self.metrics['http_request_duration_seconds'] = Histogram(
            'http_request_duration_seconds',
            'HTTP request duration in seconds',
            ['method', 'endpoint'],
            buckets=[0.01, 0.05, 0.1, 0.5, 1.0, 2.5, 5.0, 10.0],
            registry=self.registry
        )
        
        self.metrics['http_request_size_bytes'] = Histogram(
            'http_request_size_bytes',
            'HTTP request size in bytes',
            ['method', 'endpoint'],
            buckets=[100, 1000, 10000, 100000, 1000000],
            registry=self.registry
        )
        
        self.metrics['http_response_size_bytes'] = Histogram(
            'http_response_size_bytes',
            'HTTP response size in bytes',
            ['method', 'endpoint', 'status_code'],
            buckets=[100, 1000, 10000, 100000, 1000000],
            registry=self.registry
        )

        # Database metrics
        self.metrics['db_connections_active'] = Gauge(
            'db_connections_active',
            'Currently active database connections',
            ['database', 'pool'],
            registry=self.registry
        )
        
        self.metrics['db_connections_idle'] = Gauge(
            'db_connections_idle',
            'Currently idle database connections',
            ['database', 'pool'],
            registry=self.registry
        )
        
        self.metrics['db_query_duration_seconds'] = Histogram(
            'db_query_duration_seconds',
            'Database query execution time',
            ['query_type', 'database', 'table'],
            buckets=[0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0],
            registry=self.registry
        )
        
        self.metrics['db_transactions_total'] = Counter(
            'db_transactions_total',
            'Total database transactions',
            ['database', 'status'],
            registry=self.registry
        )

        # VM and container metrics
        self.metrics['vms_active_total'] = Gauge(
            'vms_active_total',
            'Number of active virtual machines',
            ['user_id', 'vm_type'],
            registry=self.registry
        )
        
        self.metrics['vm_operations_total'] = Counter(
            'vm_operations_total',
            'Total VM operations',
            ['operation', 'status', 'vm_type'],
            registry=self.registry
        )
        
        self.metrics['vm_cpu_usage_percent'] = Gauge(
            'vm_cpu_usage_percent',
            'VM CPU usage percentage',
            ['vm_id', 'user_id'],
            registry=self.registry
        )
        
        self.metrics['vm_memory_usage_bytes'] = Gauge(
            'vm_memory_usage_bytes',
            'VM memory usage in bytes',
            ['vm_id', 'user_id'],
            registry=self.registry
        )

        # Session and WebSocket metrics
        self.metrics['terminal_sessions_active'] = Gauge(
            'terminal_sessions_active',
            'Currently active terminal sessions',
            ['user_id', 'session_type'],
            registry=self.registry
        )
        
        self.metrics['websocket_connections_active'] = Gauge(
            'websocket_connections_active',
            'Currently active WebSocket connections',
            ['connection_type', 'user_type'],
            registry=self.registry
        )
        
        self.metrics['websocket_messages_total'] = Counter(
            'websocket_messages_total',
            'Total WebSocket messages',
            ['direction', 'message_type', 'connection_type'],
            registry=self.registry
        )
        
        self.metrics['session_duration_seconds'] = Histogram(
            'session_duration_seconds',
            'Terminal session duration',
            ['session_type', 'termination_reason'],
            buckets=[60, 300, 900, 1800, 3600, 7200, 14400],
            registry=self.registry
        )

    def _initialize_system_metrics(self):
        """Initialize system-level metrics"""
        # CPU metrics
        self.metrics['system_cpu_usage_percent'] = Gauge(
            'system_cpu_usage_percent',
            'System CPU usage percentage',
            ['cpu_core'],
            registry=self.registry
        )
        
        self.metrics['system_load_average'] = Gauge(
            'system_load_average',
            'System load average',
            ['period'],  # 1min, 5min, 15min
            registry=self.registry
        )

        # Memory metrics
        self.metrics['system_memory_usage_bytes'] = Gauge(
            'system_memory_usage_bytes',
            'System memory usage in bytes',
            ['memory_type'],  # total, available, used, free, cached, buffers
            registry=self.registry
        )
        
        self.metrics['system_swap_usage_bytes'] = Gauge(
            'system_swap_usage_bytes',
            'System swap usage in bytes',
            ['swap_type'],  # total, used, free
            registry=self.registry
        )

        # Disk metrics
        self.metrics['system_disk_usage_bytes'] = Gauge(
            'system_disk_usage_bytes',
            'Disk usage in bytes',
            ['device', 'mountpoint', 'usage_type'],  # total, used, free
            registry=self.registry
        )
        
        self.metrics['system_disk_io_operations_total'] = Counter(
            'system_disk_io_operations_total',
            'Total disk I/O operations',
            ['device', 'operation'],  # read, write
            registry=self.registry
        )
        
        self.metrics['system_disk_io_bytes_total'] = Counter(
            'system_disk_io_bytes_total',
            'Total disk I/O bytes',
            ['device', 'operation'],  # read, write
            registry=self.registry
        )

        # Network metrics
        self.metrics['system_network_bytes_total'] = Counter(
            'system_network_bytes_total',
            'Total network bytes',
            ['interface', 'direction'],  # sent, received
            registry=self.registry
        )
        
        self.metrics['system_network_packets_total'] = Counter(
            'system_network_packets_total',
            'Total network packets',
            ['interface', 'direction'],  # sent, received
            registry=self.registry
        )
        
        self.metrics['system_network_errors_total'] = Counter(
            'system_network_errors_total',
            'Total network errors',
            ['interface', 'error_type'],  # dropped, errors
            registry=self.registry
        )

    def _initialize_business_metrics(self):
        """Initialize business-specific metrics"""
        # User metrics
        self.metrics['users_active_total'] = Gauge(
            'users_active_total',
            'Number of active users',
            ['time_period'],  # 1h, 24h, 7d, 30d
            registry=self.registry
        )
        
        self.metrics['user_registrations_total'] = Counter(
            'user_registrations_total',
            'Total user registrations',
            ['registration_method', 'user_type'],
            registry=self.registry
        )
        
        self.metrics['user_sessions_total'] = Counter(
            'user_sessions_total',
            'Total user sessions',
            ['session_type', 'authentication_method'],
            registry=self.registry
        )

        # Repository and Git metrics
        self.metrics['git_repositories_total'] = Gauge(
            'git_repositories_total',
            'Total number of Git repositories',
            ['user_id', 'visibility'],  # public, private
            registry=self.registry
        )
        
        self.metrics['git_operations_total'] = Counter(
            'git_operations_total',
            'Total Git operations',
            ['operation', 'status', 'repository_type'],
            registry=self.registry
        )
        
        self.metrics['recording_sessions_total'] = Counter(
            'recording_sessions_total',
            'Total recording sessions',
            ['user_id', 'recording_type', 'status'],
            registry=self.registry
        )
        
        self.metrics['recording_storage_bytes'] = Gauge(
            'recording_storage_bytes',
            'Storage used by recordings',
            ['user_id', 'compression_type'],
            registry=self.registry
        )

    def _initialize_security_metrics(self):
        """Initialize security-related metrics"""
        # Authentication metrics
        self.metrics['auth_attempts_total'] = Counter(
            'auth_attempts_total',
            'Total authentication attempts',
            ['method', 'result', 'user_type'],  # success, failure, blocked
            registry=self.registry
        )
        
        self.metrics['auth_failures_consecutive'] = Gauge(
            'auth_failures_consecutive',
            'Consecutive authentication failures',
            ['user_id', 'ip_address'],
            registry=self.registry
        )

        # Security events
        self.metrics['security_events_total'] = Counter(
            'security_events_total',
            'Total security events',
            ['event_type', 'severity', 'source'],
            registry=self.registry
        )
        
        self.metrics['blocked_requests_total'] = Counter(
            'blocked_requests_total',
            'Total blocked requests',
            ['block_reason', 'source_type'],
            registry=self.registry
        )
        
        self.metrics['rate_limit_hits_total'] = Counter(
            'rate_limit_hits_total',
            'Total rate limit hits',
            ['endpoint', 'limit_type', 'user_type'],
            registry=self.registry
        )

        # Privacy and compliance
        self.metrics['privacy_filter_matches_total'] = Counter(
            'privacy_filter_matches_total',
            'Total privacy filter matches',
            ['filter_type', 'sensitivity_level'],
            registry=self.registry
        )
        
        self.metrics['data_access_requests_total'] = Counter(
            'data_access_requests_total',
            'Total data access requests',
            ['request_type', 'user_type', 'status'],
            registry=self.registry
        )

    async def start_collection(self):
        """Start metrics collection"""
        if self.is_collecting:
            logger.warning("Metrics collection already running")
            return
        
        self.is_collecting = True
        self.collection_task = asyncio.create_task(self._collection_loop())
        
        logfire.info("Advanced metrics collection started",
                   interval_seconds=self.config.interval_seconds,
                   enabled_scopes=[scope.value for scope in self.config.enabled_scopes])
        
        logger.info("Metrics collection started")

    async def stop_collection(self):
        """Stop metrics collection"""
        self.is_collecting = False
        
        if self.collection_task:
            self.collection_task.cancel()
            try:
                await self.collection_task
            except asyncio.CancelledError:
                pass
        
        logger.info("Metrics collection stopped")

    async def _collection_loop(self):
        """Main metrics collection loop"""
        while self.is_collecting:
            try:
                start_time = time.time()
                
                # Collect metrics from all enabled scopes
                metrics_collected = 0
                
                if MetricScope.SYSTEM in self.config.enabled_scopes:
                    metrics_collected += await self._collect_system_metrics()
                
                if MetricScope.APPLICATION in self.config.enabled_scopes:
                    metrics_collected += await self._collect_application_metrics()
                
                if MetricScope.BUSINESS in self.config.enabled_scopes:
                    metrics_collected += await self._collect_business_metrics()
                
                if MetricScope.SECURITY in self.config.enabled_scopes:
                    metrics_collected += await self._collect_security_metrics()
                
                if MetricScope.CUSTOM in self.config.enabled_scopes:
                    metrics_collected += await self._collect_custom_metrics()
                
                # Update collection statistics
                collection_duration = (time.time() - start_time) * 1000
                self.collection_stats["total_collections"] += 1
                self.collection_stats["total_metrics_collected"] += metrics_collected
                self.collection_stats["last_collection_duration_ms"] = collection_duration
                
                # Calculate running average
                total_time = (self.collection_stats["average_collection_time_ms"] * 
                            (self.collection_stats["total_collections"] - 1) + collection_duration)
                self.collection_stats["average_collection_time_ms"] = (
                    total_time / self.collection_stats["total_collections"]
                )
                
                self.last_collection_time = time.time()
                
                # Log collection summary
                logfire.debug("Metrics collection completed",
                            metrics_collected=metrics_collected,
                            collection_duration_ms=collection_duration,
                            total_collections=self.collection_stats["total_collections"])
                
                # Sleep until next collection
                await asyncio.sleep(self.config.interval_seconds)
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                self.collection_stats["errors_count"] += 1
                logger.error("Metrics collection error", error=str(e))
                
                # Wait before retrying
                await asyncio.sleep(min(self.config.interval_seconds, 30))

    async def _collect_system_metrics(self) -> int:
        """Collect system-level metrics"""
        try:
            metrics_count = 0
            
            # CPU metrics
            cpu_percent = psutil.cpu_percent(interval=None)
            self.metrics['system_cpu_usage_percent'].labels(cpu_core="total").set(cpu_percent)
            
            # Per-core CPU usage
            cpu_percents = psutil.cpu_percent(interval=None, percpu=True)
            for i, cpu_core_percent in enumerate(cpu_percents):
                self.metrics['system_cpu_usage_percent'].labels(cpu_core=f"core_{i}").set(cpu_core_percent)
            
            metrics_count += 1 + len(cpu_percents)
            
            # Load average
            load_avg = psutil.getloadavg()
            self.metrics['system_load_average'].labels(period="1min").set(load_avg[0])
            self.metrics['system_load_average'].labels(period="5min").set(load_avg[1])
            self.metrics['system_load_average'].labels(period="15min").set(load_avg[2])
            metrics_count += 3
            
            # Memory metrics
            memory = psutil.virtual_memory()
            self.metrics['system_memory_usage_bytes'].labels(memory_type="total").set(memory.total)
            self.metrics['system_memory_usage_bytes'].labels(memory_type="available").set(memory.available)
            self.metrics['system_memory_usage_bytes'].labels(memory_type="used").set(memory.used)
            self.metrics['system_memory_usage_bytes'].labels(memory_type="free").set(memory.free)
            metrics_count += 4
            
            # Swap metrics
            swap = psutil.swap_memory()
            self.metrics['system_swap_usage_bytes'].labels(swap_type="total").set(swap.total)
            self.metrics['system_swap_usage_bytes'].labels(swap_type="used").set(swap.used)
            self.metrics['system_swap_usage_bytes'].labels(swap_type="free").set(swap.free)
            metrics_count += 3
            
            # Disk metrics
            for disk in psutil.disk_partitions():
                try:
                    disk_usage = psutil.disk_usage(disk.mountpoint)
                    device = disk.device.replace('/', '_')
                    mountpoint = disk.mountpoint.replace('/', '_')
                    
                    self.metrics['system_disk_usage_bytes'].labels(
                        device=device, mountpoint=mountpoint, usage_type="total"
                    ).set(disk_usage.total)
                    
                    self.metrics['system_disk_usage_bytes'].labels(
                        device=device, mountpoint=mountpoint, usage_type="used"
                    ).set(disk_usage.used)
                    
                    self.metrics['system_disk_usage_bytes'].labels(
                        device=device, mountpoint=mountpoint, usage_type="free"
                    ).set(disk_usage.free)
                    
                    metrics_count += 3
                except PermissionError:
                    # Skip inaccessible mount points
                    continue
            
            # Network metrics
            network_io = psutil.net_io_counters(pernic=True)
            for interface, stats in network_io.items():
                interface_safe = interface.replace('.', '_')
                
                self.metrics['system_network_bytes_total'].labels(
                    interface=interface_safe, direction="sent"
                ).inc(stats.bytes_sent)
                
                self.metrics['system_network_bytes_total'].labels(
                    interface=interface_safe, direction="received"
                ).inc(stats.bytes_recv)
                
                self.metrics['system_network_packets_total'].labels(
                    interface=interface_safe, direction="sent"
                ).inc(stats.packets_sent)
                
                self.metrics['system_network_packets_total'].labels(
                    interface=interface_safe, direction="received"
                ).inc(stats.packets_recv)
                
                metrics_count += 4
            
            return metrics_count
            
        except Exception as e:
            logger.error("System metrics collection failed", error=str(e))
            return 0

    async def _collect_application_metrics(self) -> int:
        """Collect application-specific metrics"""
        try:
            # This would integrate with application components
            # to collect specific metrics like:
            # - Database connection pool status
            # - Cache hit rates
            # - Queue lengths
            # - Background job status
            return 0
        except Exception as e:
            logger.error("Application metrics collection failed", error=str(e))
            return 0

    async def _collect_business_metrics(self) -> int:
        """Collect business-specific metrics"""
        try:
            # This would collect metrics like:
            # - Active user counts
            # - Resource usage per user
            # - Feature usage statistics
            # - Business KPIs
            return 0
        except Exception as e:
            logger.error("Business metrics collection failed", error=str(e))
            return 0

    async def _collect_security_metrics(self) -> int:
        """Collect security-related metrics"""
        try:
            # This would collect metrics like:
            # - Authentication failure rates
            # - Security event counts
            # - Rate limiting hits
            # - Privacy filter activations
            return 0
        except Exception as e:
            logger.error("Security metrics collection failed", error=str(e))
            return 0

    async def _collect_custom_metrics(self) -> int:
        """Collect custom metrics from registered collectors"""
        try:
            metrics_count = 0
            
            for collector in self.config.custom_collectors:
                try:
                    custom_metrics = await collector()
                    metrics_count += len(custom_metrics) if custom_metrics else 0
                except Exception as e:
                    logger.error("Custom metrics collector failed", 
                               collector=collector.__name__, error=str(e))
            
            return metrics_count
        except Exception as e:
            logger.error("Custom metrics collection failed", error=str(e))
            return 0

    # Public API methods for recording metrics
    def record_http_request(self, method: str, endpoint: str, status_code: int, 
                          duration: float, request_size: int = 0, response_size: int = 0,
                          user_type: str = "anonymous"):
        """Record HTTP request metrics"""
        try:
            self.metrics['http_requests_total'].labels(
                method=method, endpoint=endpoint, status_code=str(status_code), user_type=user_type
            ).inc()
            
            self.metrics['http_request_duration_seconds'].labels(
                method=method, endpoint=endpoint
            ).observe(duration)
            
            if request_size > 0:
                self.metrics['http_request_size_bytes'].labels(
                    method=method, endpoint=endpoint
                ).observe(request_size)
            
            if response_size > 0:
                self.metrics['http_response_size_bytes'].labels(
                    method=method, endpoint=endpoint, status_code=str(status_code)
                ).observe(response_size)
        except Exception as e:
            logger.error("Failed to record HTTP request metrics", error=str(e))

    def record_db_operation(self, operation_type: str, database: str, table: str, 
                          duration: float, status: str = "success"):
        """Record database operation metrics"""
        try:
            self.metrics['db_query_duration_seconds'].labels(
                query_type=operation_type, database=database, table=table
            ).observe(duration)
            
            self.metrics['db_transactions_total'].labels(
                database=database, status=status
            ).inc()
        except Exception as e:
            logger.error("Failed to record database operation metrics", error=str(e))

    def record_vm_operation(self, operation: str, status: str, vm_type: str = "standard"):
        """Record VM operation metrics"""
        try:
            self.metrics['vm_operations_total'].labels(
                operation=operation, status=status, vm_type=vm_type
            ).inc()
        except Exception as e:
            logger.error("Failed to record VM operation metrics", error=str(e))

    def update_vm_metrics(self, vm_id: str, user_id: str, cpu_percent: float, memory_bytes: int):
        """Update VM resource metrics"""
        try:
            self.metrics['vm_cpu_usage_percent'].labels(vm_id=vm_id, user_id=user_id).set(cpu_percent)
            self.metrics['vm_memory_usage_bytes'].labels(vm_id=vm_id, user_id=user_id).set(memory_bytes)
        except Exception as e:
            logger.error("Failed to update VM metrics", error=str(e))

    def record_security_event(self, event_type: str, severity: str, source: str = "system"):
        """Record security event"""
        try:
            self.metrics['security_events_total'].labels(
                event_type=event_type, severity=severity, source=source
            ).inc()
        except Exception as e:
            logger.error("Failed to record security event metrics", error=str(e))

    def record_auth_attempt(self, method: str, result: str, user_type: str = "regular"):
        """Record authentication attempt"""
        try:
            self.metrics['auth_attempts_total'].labels(
                method=method, result=result, user_type=user_type
            ).inc()
        except Exception as e:
            logger.error("Failed to record authentication metrics", error=str(e))

    def get_collection_statistics(self) -> Dict[str, Any]:
        """Get metrics collection statistics"""
        return {
            **self.collection_stats,
            "is_collecting": self.is_collecting,
            "last_collection_time": self.last_collection_time,
            "collection_interval_seconds": self.config.interval_seconds,
            "enabled_scopes": [scope.value for scope in self.config.enabled_scopes],
            "total_metrics_defined": len(self.metrics),
            "custom_metrics_count": len(self.custom_metrics)
        }

    def register_custom_metric(self, definition: MetricDefinition) -> bool:
        """Register a custom metric definition"""
        try:
            if definition.name in self.metrics:
                logger.warning("Metric already exists", metric_name=definition.name)
                return False
            
            # Create Prometheus metric based on type
            if definition.type == MetricType.COUNTER:
                metric = Counter(
                    definition.name, definition.description, definition.labels, registry=self.registry
                )
            elif definition.type == MetricType.GAUGE:
                metric = Gauge(
                    definition.name, definition.description, definition.labels, registry=self.registry
                )
            elif definition.type == MetricType.HISTOGRAM:
                metric = Histogram(
                    definition.name, definition.description, definition.labels,
                    buckets=definition.buckets, registry=self.registry
                )
            elif definition.type == MetricType.SUMMARY:
                metric = Summary(
                    definition.name, definition.description, definition.labels, registry=self.registry
                )
            else:
                logger.error("Unsupported metric type", metric_type=definition.type.value)
                return False
            
            self.metrics[definition.name] = metric
            self.custom_metrics[definition.name] = definition
            
            logger.info("Custom metric registered", metric_name=definition.name, metric_type=definition.type.value)
            return True
            
        except Exception as e:
            logger.error("Failed to register custom metric", metric_name=definition.name, error=str(e))
            return False

    async def cleanup(self):
        """Cleanup metrics collection resources"""
        await self.stop_collection()
        self.thread_pool.shutdown(wait=True)
        logger.info("Metrics collector cleanup completed")
```

## TDD Implementation Cycle

### Red Phase: Metrics Collection Test Creation
```python
# monitoring/tests/test_metrics_collector.py
import pytest
import asyncio
from monitoring.metrics.collector import AdvancedMetricsCollector, CollectionConfig, MetricScope

@pytest.mark.asyncio
async def test_metrics_collector_initialization():
    """Test metrics collector initializes with default configuration"""
    # This test should initially fail (Red phase)
    collector = AdvancedMetricsCollector()
    assert False, "Metrics collector initialization not implemented yet"

@pytest.mark.asyncio
async def test_system_metrics_collection():
    """Test system metrics collection accuracy"""
    # This test should initially fail (Red phase)
    assert False, "System metrics collection not implemented yet"

@pytest.mark.asyncio
async def test_custom_metric_registration():
    """Test custom metric registration and collection"""
    # This test should initially fail (Red phase)
    assert False, "Custom metric registration not implemented yet"
```

### Green Phase: Metrics Collection Implementation
```python
# Implement metrics collection features to make tests pass
# This involves adding metric definitions, collection loops, and data processing
```

### Refactor Phase: Metrics Collection Optimization
```python
# Optimize metrics collection for performance and accuracy
# Add advanced metric types and collection strategies
# Enhance error handling and resource management
```

## Security Checklist ✅

### Metrics Collection Security
- [ ] Metrics collection system access controls and authentication
- [ ] Sensitive data exclusion from metrics (no PII, credentials, or secrets)
- [ ] Metrics collection authorization and user-based filtering
- [ ] Protection against metrics enumeration and unauthorized access
- [ ] Secure transmission of metrics data to collection endpoints
- [ ] Metrics collection rate limiting to prevent DoS attacks
- [ ] Audit logging for metrics collection system access and changes
- [ ] Protection against metrics injection and manipulation attacks
- [ ] Secure storage of metrics collection configuration
- [ ] Monitoring system network isolation and hardening

### Metrics Data Security
- [ ] Metrics data encryption at rest and in transit
- [ ] Access controls for metrics queries and dashboards
- [ ] Metrics data retention policy enforcement and secure deletion
- [ ] Protection against unauthorized metrics data export
- [ ] Secure aggregation and anonymization of user metrics
- [ ] Compliance with data protection regulations (GDPR, CCPA)
- [ ] Metrics data integrity verification and tamper detection
- [ ] Secure backup and recovery of metrics data
- [ ] Protection against metrics data correlation attacks
- [ ] User consent management for metrics collection

### Collection Infrastructure Security
- [ ] Monitoring infrastructure hardening and security patching
- [ ] Secure configuration management for metrics collection
- [ ] Protection against monitoring system compromise
- [ ] Secure communication between metrics collectors and storage
- [ ] Monitoring system service account security and rotation
- [ ] Protection against resource exhaustion attacks on collection
- [ ] Secure deployment and update procedures for monitoring
- [ ] Network security controls for monitoring traffic
- [ ] Incident response procedures for monitoring security events
- [ ] Regular security assessments of monitoring infrastructure

### Performance and Resource Security
- [ ] Resource consumption limits for metrics collection operations
- [ ] Memory usage monitoring and protection during collection
- [ ] CPU usage limits and monitoring for collection processes
- [ ] Disk I/O monitoring and rate limiting for metrics storage
- [ ] Network bandwidth protection for metrics transmission
- [ ] Collection interval optimization to prevent resource exhaustion
- [ ] Metrics collection job scheduling and prioritization
- [ ] Resource cleanup and garbage collection for metrics data
- [ ] Performance monitoring for the monitoring system itself
- [ ] Automated scaling and resource management for high load

### API and Integration Security
- [ ] Metrics API authentication and authorization
- [ ] API rate limiting for metrics queries and updates
- [ ] Input validation for custom metrics registration
- [ ] Protection against API abuse and unauthorized access
- [ ] Secure integration with external monitoring systems
- [ ] API versioning and backward compatibility security
- [ ] Metrics API audit logging and access monitoring
- [ ] Protection against API enumeration and discovery attacks
- [ ] Secure webhook and notification integrations
- [ ] API security testing and vulnerability scanning

## Performance Requirements

### Collection Performance
- Metrics collection interval accuracy ± 5%
- System metrics collection latency < 50ms
- Application metrics collection latency < 100ms
- Custom metrics registration < 10ms
- Collection loop overhead < 2% CPU usage
- Memory usage < 100MB for metrics collection

### Data Processing Performance
- Metrics processing throughput > 10,000 metrics/second
- Real-time metric updates latency < 200ms
- Prometheus exposition endpoint response < 500ms
- Metric aggregation processing < 1 second
- Historical metrics query response < 2 seconds
- Metrics export throughput > 1MB/second

### Scalability Requirements
- Support 1000+ custom metrics definitions
- Handle 100+ concurrent metrics collectors
- Scale to 1M+ metric samples per hour
- Support 100+ metrics collection endpoints
- Manage 10GB+ metrics data storage
- Support 50+ simultaneous metrics queries

## Commit Instructions

After implementing the metrics collection system:

```bash
git add monitoring/metrics/
git commit -m "Add advanced metrics collection system with comprehensive monitoring

- Implement AdvancedMetricsCollector with multi-scope metrics support
- Add comprehensive platform metrics (HTTP, DB, VM, sessions, WebSocket)
- Implement system metrics collection (CPU, memory, disk, network)
- Add business and security metrics with custom metric registration
- Include Prometheus integration with configurable collection intervals
- Add performance monitoring for metrics collection itself
- Implement thread-safe collection with error handling and recovery
- Add comprehensive metric labeling and categorization
- Include collection statistics and health monitoring
- Add TDD cycle with Red-Green-Refactor for metrics features
- Ensure >85% metrics collection test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete metrics collection test suite:

```bash
# Run all metrics collection tests
pytest monitoring/tests/test_metrics_collector.py -v --timeout=300

# Run specific metrics test categories
pytest monitoring/tests/metrics/ -k "system_metrics" -v
pytest monitoring/tests/metrics/ -k "custom_metrics" -v
pytest monitoring/tests/metrics/ -k "collection_performance" -v

# Run metrics collection performance tests
pytest monitoring/tests/metrics/performance/ -v

# Run metrics collection integration tests
pytest monitoring/tests/metrics/integration/ -v
```

Validate metrics collection test coverage:
```bash
pytest monitoring/tests/metrics/ --cov=monitoring.metrics --cov-report=html --cov-fail-under=85
```

## Integration Testing

Test metrics collection integration with platform components:
```bash
# Test integration with Session 1 (Logfire)
pytest monitoring/tests/integration/test_metrics_logfire_integration.py -v

# Test integration with application components
pytest monitoring/tests/integration/test_metrics_application_integration.py -v

# Test Prometheus metrics exposition
pytest monitoring/tests/integration/test_metrics_prometheus_integration.py -v
```