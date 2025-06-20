# Session 7.5: Performance Optimization & Monitoring

## Objective
Implement comprehensive performance optimization and monitoring for the VM snapshot system, ensuring optimal throughput, storage efficiency, and scalability under high load conditions.

## Integration with Previous Sessions
- **Session 1**: Leverages Logfire for performance metrics collection and analysis
- **Session 3**: Optimizes VM manager integration for snapshot operations
- **Session 5**: Optimizes database queries for snapshot metadata operations
- **Session 6**: Coordinates with session management for efficient resource utilization

## Performance Monitoring Implementation

### Snapshot Performance Monitor
**Location**: `snapshot-manager/monitoring/performance_monitor.py`

```python
# snapshot-manager/monitoring/performance_monitor.py
import asyncio
import time
import psutil
from typing import Dict, Any, List, Optional, Tuple
from dataclasses import dataclass, asdict
from collections import deque
import structlog
import logfire
from prometheus_client import Counter, Histogram, Gauge, Summary

logger = structlog.get_logger()

@dataclass
class SnapshotPerformanceMetrics:
    timestamp: float
    active_snapshots: int
    snapshot_creation_rate: float
    storage_throughput_mbps: float
    compression_ratio: float
    deduplication_ratio: float
    average_snapshot_size_mb: float
    storage_usage_gb: float
    api_response_time_ms: float
    error_rate_percent: float
    concurrent_operations: int

class SnapshotPerformanceMonitor:
    def __init__(self, snapshot_manager, storage_backend, api_metrics):
        self.snapshot_manager = snapshot_manager
        self.storage_backend = storage_backend
        self.api_metrics = api_metrics
        
        # Prometheus metrics
        self.snapshot_operations = Counter(
            'snapshot_operations_total',
            'Total snapshot operations',
            ['operation', 'status', 'snapshot_type']
        )
        
        self.snapshot_duration = Histogram(
            'snapshot_operation_duration_seconds',
            'Snapshot operation duration',
            ['operation'],
            buckets=[1, 5, 10, 30, 60, 120, 300, 600]
        )
        
        self.active_snapshots_gauge = Gauge(
            'active_snapshots_total',
            'Number of active snapshots'
        )
        
        self.storage_throughput = Histogram(
            'storage_throughput_mbps',
            'Storage throughput in MB/s',
            ['operation'],
            buckets=[1, 5, 10, 25, 50, 100, 250, 500]
        )
        
        self.compression_ratio_gauge = Gauge(
            'compression_ratio',
            'Snapshot compression ratio'
        )
        
        self.deduplication_savings = Gauge(
            'deduplication_savings_percent',
            'Storage savings from deduplication'
        )
        
        self.api_latency = Histogram(
            'snapshot_api_latency_seconds',
            'API endpoint latency',
            ['endpoint', 'method'],
            buckets=[0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0]
        )
        
        # Performance history
        self.metrics_history: deque = deque(maxlen=1000)
        self.monitoring_task: Optional[asyncio.Task] = None
        self.collection_interval = 30  # seconds
        
        # Performance thresholds
        self.thresholds = {
            'snapshot_creation_time_seconds': 120,
            'storage_throughput_mbps': 25,
            'api_response_time_ms': 500,
            'compression_ratio_minimum': 0.3,
            'error_rate_percent_maximum': 2,
            'concurrent_operations_maximum': 20
        }
        
        # Optimization state
        self.optimization_active = False
        self.performance_degradation_detected = False
    
    async def initialize(self):
        """Initialize performance monitoring"""
        self.monitoring_task = asyncio.create_task(self._monitoring_loop())
        logger.info("Snapshot performance monitoring initialized")
        logfire.info("Snapshot performance monitoring started",
                    service="snapshot-performance")
    
    async def _monitoring_loop(self):
        """Main performance monitoring loop"""
        while True:
            try:
                await asyncio.sleep(self.collection_interval)
                await self._collect_performance_metrics()
                await self._analyze_performance_trends()
                await self._trigger_optimization_if_needed()
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Performance monitoring error", error=str(e))
    
    async def _collect_performance_metrics(self):
        """Collect comprehensive performance metrics"""
        try:
            start_time = time.time()
            
            # Collect snapshot manager metrics
            active_snapshots = len([
                s for s in self.snapshot_manager.snapshots.values()
                if s.state in ['creating', 'available', 'restoring']
            ])
            
            # Calculate snapshot creation rate (last 5 minutes)
            creation_rate = await self._calculate_snapshot_creation_rate()
            
            # Collect storage metrics
            storage_metrics = await self._collect_storage_metrics()
            
            # Collect API metrics
            api_metrics = await self._collect_api_metrics()
            
            # Calculate compression and deduplication ratios
            compression_ratio = await self._calculate_compression_ratio()
            deduplication_ratio = await self._calculate_deduplication_ratio()
            
            # Calculate average snapshot size
            avg_size_mb = await self._calculate_average_snapshot_size()
            
            # Calculate storage usage
            storage_usage_gb = await self._calculate_storage_usage()
            
            # Count concurrent operations
            concurrent_ops = len(self.snapshot_manager.active_operations)
            
            # Calculate error rate
            error_rate = await self._calculate_error_rate()
            
            # Create metrics object
            metrics = SnapshotPerformanceMetrics(
                timestamp=time.time(),
                active_snapshots=active_snapshots,
                snapshot_creation_rate=creation_rate,
                storage_throughput_mbps=storage_metrics.get('throughput_mbps', 0),
                compression_ratio=compression_ratio,
                deduplication_ratio=deduplication_ratio,
                average_snapshot_size_mb=avg_size_mb,
                storage_usage_gb=storage_usage_gb,
                api_response_time_ms=api_metrics.get('avg_response_time_ms', 0),
                error_rate_percent=error_rate,
                concurrent_operations=concurrent_ops
            )
            
            # Store metrics
            self.metrics_history.append(metrics)
            
            # Update Prometheus metrics
            self.active_snapshots_gauge.set(active_snapshots)
            self.compression_ratio_gauge.set(compression_ratio)
            self.deduplication_savings.set(deduplication_ratio * 100)
            
            # Log to Logfire with comprehensive metrics
            logfire.info("Snapshot performance metrics collected",
                        active_snapshots=active_snapshots,
                        creation_rate_per_hour=creation_rate * 60,
                        storage_throughput_mbps=storage_metrics.get('throughput_mbps', 0),
                        compression_ratio=compression_ratio,
                        deduplication_ratio=deduplication_ratio,
                        storage_usage_gb=storage_usage_gb,
                        api_response_time_ms=api_metrics.get('avg_response_time_ms', 0),
                        error_rate_percent=error_rate,
                        concurrent_operations=concurrent_ops)
            
            collection_duration = (time.time() - start_time) * 1000
            logger.debug("Performance metrics collection completed",
                        duration_ms=collection_duration,
                        metrics_count=len(self.metrics_history))
            
        except Exception as e:
            logger.error("Failed to collect performance metrics", error=str(e))
            logfire.error("Performance metrics collection failed", error=str(e))
    
    async def _calculate_snapshot_creation_rate(self) -> float:
        """Calculate snapshot creation rate per minute"""
        try:
            current_time = time.time()
            five_minutes_ago = current_time - 300
            
            recent_snapshots = [
                s for s in self.snapshot_manager.snapshots.values()
                if s.created_at >= five_minutes_ago
            ]
            
            return len(recent_snapshots) / 5.0  # per minute
            
        except Exception as e:
            logger.error("Failed to calculate creation rate", error=str(e))
            return 0.0
    
    async def _collect_storage_metrics(self) -> Dict[str, Any]:
        """Collect storage performance metrics"""
        try:
            if hasattr(self.storage_backend, 'metrics') and self.storage_backend.metrics:
                recent_metrics = [
                    m for m in self.storage_backend.metrics
                    if time.time() - m.timestamp < 300  # Last 5 minutes
                ]
                
                if recent_metrics:
                    # Calculate average throughput
                    total_bytes = sum(m.size_bytes for m in recent_metrics if m.success)
                    total_duration = sum(m.duration_ms for m in recent_metrics if m.success)
                    
                    if total_duration > 0:
                        throughput_mbps = (total_bytes / (1024 * 1024)) / (total_duration / 1000)
                    else:
                        throughput_mbps = 0
                    
                    return {
                        'throughput_mbps': throughput_mbps,
                        'operations_count': len(recent_metrics),
                        'success_rate': len([m for m in recent_metrics if m.success]) / len(recent_metrics)
                    }
            
            return {'throughput_mbps': 0, 'operations_count': 0, 'success_rate': 1.0}
            
        except Exception as e:
            logger.error("Failed to collect storage metrics", error=str(e))
            return {'throughput_mbps': 0, 'operations_count': 0, 'success_rate': 0.0}
    
    async def _calculate_compression_ratio(self) -> float:
        """Calculate average compression ratio"""
        try:
            snapshots_with_compression = [
                s for s in self.snapshot_manager.snapshots.values()
                if s.size_bytes > 0 and s.compressed_size_bytes > 0
            ]
            
            if not snapshots_with_compression:
                return 0.0
            
            total_original = sum(s.size_bytes for s in snapshots_with_compression)
            total_compressed = sum(s.compressed_size_bytes for s in snapshots_with_compression)
            
            if total_original > 0:
                return total_compressed / total_original
            
            return 0.0
            
        except Exception as e:
            logger.error("Failed to calculate compression ratio", error=str(e))
            return 0.0
    
    async def _calculate_deduplication_ratio(self) -> float:
        """Calculate deduplication storage savings ratio"""
        try:
            # This would integrate with the deduplication engine
            # For now, return a simulated value
            return 0.35  # 35% deduplication savings
            
        except Exception as e:
            logger.error("Failed to calculate deduplication ratio", error=str(e))
            return 0.0
    
    async def _analyze_performance_trends(self):
        """Analyze performance trends and detect degradation"""
        if len(self.metrics_history) < 10:
            return
        
        current_metrics = self.metrics_history[-1]
        
        # Check for performance threshold violations
        alerts = []
        
        if current_metrics.storage_throughput_mbps < self.thresholds['storage_throughput_mbps']:
            alerts.append(f"Low storage throughput: {current_metrics.storage_throughput_mbps:.1f} MB/s")
        
        if current_metrics.api_response_time_ms > self.thresholds['api_response_time_ms']:
            alerts.append(f"High API latency: {current_metrics.api_response_time_ms:.1f}ms")
        
        if current_metrics.compression_ratio < self.thresholds['compression_ratio_minimum']:
            alerts.append(f"Low compression ratio: {current_metrics.compression_ratio:.2f}")
        
        if current_metrics.error_rate_percent > self.thresholds['error_rate_percent_maximum']:
            alerts.append(f"High error rate: {current_metrics.error_rate_percent:.1f}%")
        
        if current_metrics.concurrent_operations > self.thresholds['concurrent_operations_maximum']:
            alerts.append(f"High concurrent operations: {current_metrics.concurrent_operations}")
        
        if alerts:
            self.performance_degradation_detected = True
            logger.warning("Performance degradation detected", alerts=alerts)
            logfire.warning("Snapshot performance alerts",
                          alerts=alerts,
                          metrics=asdict(current_metrics))
        else:
            self.performance_degradation_detected = False
    
    async def _trigger_optimization_if_needed(self):
        """Trigger performance optimization if degradation detected"""
        if self.performance_degradation_detected and not self.optimization_active:
            await self._perform_performance_optimization()
    
    async def _perform_performance_optimization(self):
        """Perform automatic performance optimization"""
        try:
            self.optimization_active = True
            logger.info("Starting automatic performance optimization")
            
            # Optimization strategies
            optimizations_applied = []
            
            # 1. Cleanup old operations
            cleaned_operations = await self._cleanup_stale_operations()
            if cleaned_operations > 0:
                optimizations_applied.append(f"Cleaned {cleaned_operations} stale operations")
            
            # 2. Optimize storage connections
            if hasattr(self.storage_backend, 'optimize_connections'):
                await self.storage_backend.optimize_connections()
                optimizations_applied.append("Optimized storage connections")
            
            # 3. Trigger garbage collection for memory optimization
            import gc
            collected = gc.collect()
            if collected > 0:
                optimizations_applied.append(f"Freed {collected} memory objects")
            
            # 4. Optimize compression settings based on recent performance
            await self._optimize_compression_settings()
            optimizations_applied.append("Optimized compression settings")
            
            logger.info("Performance optimization completed", 
                       optimizations=optimizations_applied)
            logfire.info("Snapshot performance optimization completed",
                        optimizations_applied=optimizations_applied)
            
        except Exception as e:
            logger.error("Performance optimization failed", error=str(e))
            logfire.error("Performance optimization failed", error=str(e))
        finally:
            self.optimization_active = False
    
    def get_performance_summary(self, duration_minutes: int = 30) -> Dict[str, Any]:
        """Get comprehensive performance summary"""
        try:
            cutoff_time = time.time() - (duration_minutes * 60)
            recent_metrics = [
                m for m in self.metrics_history
                if m.timestamp >= cutoff_time
            ]
            
            if not recent_metrics:
                return {"error": "No metrics available"}
            
            # Calculate statistics
            avg_creation_rate = sum(m.snapshot_creation_rate for m in recent_metrics) / len(recent_metrics)
            avg_throughput = sum(m.storage_throughput_mbps for m in recent_metrics) / len(recent_metrics)
            avg_compression = sum(m.compression_ratio for m in recent_metrics) / len(recent_metrics)
            avg_api_latency = sum(m.api_response_time_ms for m in recent_metrics) / len(recent_metrics)
            avg_error_rate = sum(m.error_rate_percent for m in recent_metrics) / len(recent_metrics)
            
            # Calculate peaks
            peak_active_snapshots = max(m.active_snapshots for m in recent_metrics)
            peak_throughput = max(m.storage_throughput_mbps for m in recent_metrics)
            peak_api_latency = max(m.api_response_time_ms for m in recent_metrics)
            peak_concurrent_ops = max(m.concurrent_operations for m in recent_metrics)
            
            # Current values
            current = recent_metrics[-1]
            
            summary = {
                "duration_minutes": duration_minutes,
                "data_points": len(recent_metrics),
                "averages": {
                    "creation_rate_per_minute": round(avg_creation_rate, 2),
                    "storage_throughput_mbps": round(avg_throughput, 2),
                    "compression_ratio": round(avg_compression, 3),
                    "api_latency_ms": round(avg_api_latency, 2),
                    "error_rate_percent": round(avg_error_rate, 2)
                },
                "peaks": {
                    "active_snapshots": peak_active_snapshots,
                    "storage_throughput_mbps": round(peak_throughput, 2),
                    "api_latency_ms": round(peak_api_latency, 2),
                    "concurrent_operations": peak_concurrent_ops
                },
                "current": {
                    "active_snapshots": current.active_snapshots,
                    "storage_usage_gb": round(current.storage_usage_gb, 2),
                    "deduplication_ratio": round(current.deduplication_ratio, 3),
                    "performance_status": "degraded" if self.performance_degradation_detected else "optimal"
                },
                "thresholds_status": {
                    "storage_throughput": "OK" if avg_throughput >= self.thresholds['storage_throughput_mbps'] else "BELOW_THRESHOLD",
                    "api_latency": "OK" if avg_api_latency <= self.thresholds['api_response_time_ms'] else "ABOVE_THRESHOLD",
                    "error_rate": "OK" if avg_error_rate <= self.thresholds['error_rate_percent_maximum'] else "ABOVE_THRESHOLD"
                }
            }
            
            return summary
            
        except Exception as e:
            logger.error("Performance summary generation failed", error=str(e))
            return {"error": str(e)}
```

## Performance Optimization Strategies

### Storage Optimization
```python
# Storage performance optimization techniques
class StorageOptimizer:
    def __init__(self, storage_backend):
        self.storage = storage_backend
        
    async def optimize_multipart_uploads(self):
        """Optimize multipart upload parameters based on performance data"""
        # Analyze recent upload performance
        recent_uploads = self._get_recent_upload_metrics()
        
        if recent_uploads:
            avg_throughput = self._calculate_average_throughput(recent_uploads)
            
            # Adjust chunk size based on throughput
            if avg_throughput < 25:  # MB/s
                # Increase chunk size for better throughput
                self.storage.multipart_chunksize = min(
                    self.storage.multipart_chunksize * 2,
                    50 * 1024 * 1024  # Max 50MB
                )
            elif avg_throughput > 100:  # MB/s
                # Decrease chunk size for better parallelism
                self.storage.multipart_chunksize = max(
                    self.storage.multipart_chunksize // 2,
                    5 * 1024 * 1024  # Min 5MB
                )
    
    async def optimize_compression_settings(self):
        """Optimize compression settings based on performance trade-offs"""
        # Analyze compression performance vs savings
        compression_metrics = self._get_compression_metrics()
        
        # Adjust compression level based on CPU usage and savings
        if compression_metrics['cpu_usage_high'] and compression_metrics['savings_low']:
            # Reduce compression level
            self._adjust_compression_level(-1)
        elif compression_metrics['cpu_usage_low'] and compression_metrics['savings_potential']:
            # Increase compression level
            self._adjust_compression_level(1)
```

### Memory Optimization
```python
# Memory usage optimization for snapshot operations
class MemoryOptimizer:
    def __init__(self, snapshot_manager):
        self.snapshot_manager = snapshot_manager
        
    async def optimize_memory_usage(self):
        """Optimize memory usage for snapshot operations"""
        # Clear completed operation references
        completed_operations = [
            op_id for op_id, task in self.snapshot_manager.active_operations.items()
            if task.done()
        ]
        
        for op_id in completed_operations:
            del self.snapshot_manager.active_operations[op_id]
        
        # Implement LRU cache for snapshot metadata
        await self._implement_metadata_caching()
        
        # Optimize buffer sizes based on available memory
        await self._optimize_buffer_sizes()
```

## TDD Performance Testing Cycle

### Performance-First Development Process

1. **Red Phase**: Write failing performance tests
   ```bash
   # Create performance test file
   touch snapshot-manager/tests/performance/test_snapshot_performance.py
   
   # Run failing performance test
   pytest snapshot-manager/tests/performance/test_snapshot_performance.py::test_snapshot_creation_performance -v
   ```

2. **Green Phase**: Implement basic performance monitoring
   ```bash
   # Add performance monitoring
   pytest snapshot-manager/tests/performance/test_snapshot_performance.py::test_snapshot_creation_performance -v
   ```

3. **Refactor Phase**: Optimize for performance targets
   ```bash
   # Implement performance optimizations
   pytest snapshot-manager/tests/performance/ -v
   ```

4. **Performance Commit**: Commit optimizations
   ```bash
   git add snapshot-manager/monitoring/ snapshot-manager/tests/performance/
   git commit -m "perf: implement snapshot performance monitoring and optimization
   
   - Add comprehensive performance monitoring with Prometheus metrics
   - Implement automatic performance optimization with degradation detection
   - Add storage throughput optimization with adaptive multipart uploads
   - Include memory optimization with efficient resource management
   - Integrate with Logfire for performance analytics and alerting
   
   Tests: Added performance test suite with load testing scenarios
   Performance: Snapshot creation <60s, storage throughput >50MB/s
   Monitoring: Real-time performance tracking with automated optimization"
   ```

## Performance Requirements & Targets

### Snapshot Operations Performance
- **Snapshot creation time**: < 60 seconds for 1GB VM (95th percentile)
- **Snapshot restoration time**: < 30 seconds for 1GB VM (95th percentile)
- **Snapshot deletion time**: < 10 seconds (average)
- **Concurrent snapshots**: Support 20+ simultaneous operations per instance
- **Snapshot throughput**: > 10 snapshots/hour per VM
- **Memory usage**: < 500MB for 100 active snapshot operations

### Storage Performance
- **Upload throughput**: > 50 MB/s to storage backend (average)
- **Download throughput**: > 100 MB/s from storage backend (average)
- **Storage latency**: < 100ms for metadata operations
- **Compression performance**: > 25 MB/s compression throughput
- **Deduplication efficiency**: > 30% storage savings for typical workloads
- **Storage space utilization**: > 80% efficiency with compression + deduplication

### API Performance
- **API response time**: < 200ms for metadata operations (95th percentile)
- **API response time**: < 500ms for snapshot initiation (95th percentile)
- **Concurrent API requests**: Support 100+ requests/second per instance
- **Database query time**: < 50ms for snapshot listing (95th percentile)
- **Authentication overhead**: < 20ms per API request
- **Rate limiting overhead**: < 5ms per request

### System Resource Utilization
- **CPU usage**: < 60% under normal load, < 80% under peak load
- **Memory usage**: < 2GB per 1000 active snapshots
- **Disk I/O**: Optimized with sequential writes and reads
- **Network bandwidth**: Efficient usage with compression and batching
- **File descriptors**: < 1000 for maximum concurrent operations
- **Database connections**: Efficient pooling with < 50 active connections

## Load Testing Framework

### Snapshot Load Testing
```python
# snapshot-manager/tests/performance/load_test.py
import asyncio
import aiohttp
import time
from typing import List, Dict
from dataclasses import dataclass

@dataclass
class LoadTestResult:
    operation: str
    success_count: int
    failure_count: int
    average_duration_ms: float
    p95_duration_ms: float
    throughput_ops_per_second: float

class SnapshotLoadTester:
    def __init__(self, base_url: str, auth_token: str):
        self.base_url = base_url
        self.auth_token = auth_token
        self.results: List[LoadTestResult] = []
    
    async def run_comprehensive_load_test(self, duration_minutes: int = 10):
        """Run comprehensive load test for snapshot operations"""
        
        # Test scenarios
        test_scenarios = [
            ("create_snapshots", self._test_snapshot_creation_load),
            ("list_snapshots", self._test_snapshot_listing_load),
            ("restore_snapshots", self._test_snapshot_restoration_load),
            ("delete_snapshots", self._test_snapshot_deletion_load)
        ]
        
        for scenario_name, test_func in test_scenarios:
            logger.info(f"Starting load test: {scenario_name}")
            result = await test_func(duration_minutes)
            self.results.append(result)
            logger.info(f"Completed load test: {scenario_name}", result=result)
        
        return self.results
    
    async def _test_snapshot_creation_load(self, duration_minutes: int) -> LoadTestResult:
        """Test snapshot creation under load"""
        end_time = time.time() + (duration_minutes * 60)
        success_count = 0
        failure_count = 0
        durations = []
        
        while time.time() < end_time:
            try:
                start_time = time.time()
                
                # Create snapshot via API
                async with aiohttp.ClientSession() as session:
                    async with session.post(
                        f"{self.base_url}/snapshots",
                        headers={"Authorization": f"Bearer {self.auth_token}"},
                        json={
                            "vm_id": f"load_test_vm_{int(time.time())}",
                            "name": f"load_test_snapshot_{int(time.time())}",
                            "description": "Load test snapshot"
                        }
                    ) as response:
                        if response.status == 201:
                            success_count += 1
                        else:
                            failure_count += 1
                        
                        duration_ms = (time.time() - start_time) * 1000
                        durations.append(duration_ms)
                
                # Rate limiting to prevent overwhelming
                await asyncio.sleep(1)
                
            except Exception:
                failure_count += 1
        
        # Calculate statistics
        avg_duration = sum(durations) / len(durations) if durations else 0
        p95_duration = sorted(durations)[int(len(durations) * 0.95)] if durations else 0
        total_ops = success_count + failure_count
        throughput = total_ops / (duration_minutes * 60) if duration_minutes > 0 else 0
        
        return LoadTestResult(
            operation="create_snapshots",
            success_count=success_count,
            failure_count=failure_count,
            average_duration_ms=avg_duration,
            p95_duration_ms=p95_duration,
            throughput_ops_per_second=throughput
        )
```

## Performance Regression Testing

### Automated Performance Validation
```bash
# scripts/performance_regression_test.sh
#!/bin/bash
echo "Running snapshot performance regression tests..."

# Set performance baseline
export PERFORMANCE_BASELINE_FILE="benchmarks/snapshot_performance_baseline.json"

# Run performance tests
python -m pytest snapshot-manager/tests/performance/ -v \
  --benchmark-json=current_performance.json \
  --benchmark-compare=$PERFORMANCE_BASELINE_FILE \
  --benchmark-compare-fail=mean:5%

# Validate performance regression
python scripts/validate_performance_regression.py \
  --baseline=$PERFORMANCE_BASELINE_FILE \
  --current=current_performance.json \
  --threshold=10  # 10% regression threshold

echo "Performance regression testing completed"
```

### Continuous Performance Monitoring
```yaml
# .github/workflows/performance-monitoring.yml
name: Performance Monitoring
on:
  schedule:
    - cron: '0 */6 * * *'  # Every 6 hours
  workflow_dispatch:

jobs:
  performance-monitoring:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Setup Python
        uses: actions/setup-python@v4
        with:
          python-version: '3.11'
      - name: Run performance tests
        run: |
          pip install -r requirements-dev.txt
          pytest snapshot-manager/tests/performance/ --benchmark-json=performance.json
      - name: Update performance baseline
        run: python scripts/update_performance_baseline.py performance.json
      - name: Send performance alerts
        if: failure()
        run: python scripts/send_performance_alert.py
```

## Monitoring Integration

### Prometheus Metrics Configuration
```yaml
# prometheus/snapshot_metrics.yml
- job_name: 'snapshot-manager'
  static_configs:
    - targets: ['snapshot-manager:8000']
  metrics_path: /metrics
  scrape_interval: 30s
  
- job_name: 'snapshot-storage'
  static_configs:
    - targets: ['snapshot-storage:8001']
  metrics_path: /metrics
  scrape_interval: 60s
```

### Grafana Dashboard Configuration
```json
{
  "dashboard": {
    "title": "Snapshot Performance Dashboard",
    "panels": [
      {
        "title": "Snapshot Creation Rate",
        "type": "graph",
        "targets": [
          {
            "expr": "rate(snapshot_operations_total{operation=\"create\"}[5m])",
            "legendFormat": "Creation Rate"
          }
        ]
      },
      {
        "title": "Storage Throughput",
        "type": "graph", 
        "targets": [
          {
            "expr": "histogram_quantile(0.95, rate(storage_throughput_mbps_bucket[5m]))",
            "legendFormat": "95th Percentile Throughput"
          }
        ]
      },
      {
        "title": "Compression Efficiency",
        "type": "singlestat",
        "targets": [
          {
            "expr": "compression_ratio",
            "legendFormat": "Compression Ratio"
          }
        ]
      }
    ]
  }
}
```

## Next Performance Implementation Steps

1. **Complete performance monitoring system** with all metrics collection
2. **Implement storage optimization** with adaptive parameters
3. **Add memory usage optimization** with efficient caching
4. **Create comprehensive load testing** with realistic scenarios
5. **Add performance regression testing** in CI/CD pipeline
6. **Implement auto-scaling triggers** based on performance metrics
7. **Create performance documentation** with optimization guides

## Performance Commit Guidelines

Performance commits should include:
- **Benchmark results** demonstrating performance improvements
- **Resource utilization analysis** with before/after measurements
- **Performance test coverage** for optimized components
- **Scalability validation** with load testing results
- **Monitoring integration** for performance tracking
- **Documentation updates** with performance characteristics and tuning guides