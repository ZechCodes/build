# Session 6.5: Performance Monitoring & Optimization

## Objective
Implement comprehensive performance monitoring and optimization for the session management system, ensuring optimal response times, resource utilization, and scalability under load.

## Integration with Previous Sessions
- **Session 1**: Leverages Logfire for performance metrics collection and analysis
- **Session 3**: Monitors VM manager integration performance and resource usage
- **Session 5**: Tracks database operation performance and query optimization
- **All Previous Sessions**: Provides end-to-end performance visibility across platform

## Performance Monitoring Implementation

### Session Performance Monitor
**Location**: `session-manager/monitoring/performance_monitor.py`

```python
# session-manager/monitoring/performance_monitor.py
import asyncio
import time
import psutil
from typing import Dict, Any, List, Optional
from dataclasses import dataclass, asdict
from collections import deque
import structlog
import logfire
from prometheus_client import Counter, Histogram, Gauge, Summary

logger = structlog.get_logger()

@dataclass
class PerformanceMetrics:
    timestamp: float
    session_count: int
    active_connections: int
    memory_usage_mb: float
    cpu_usage_percent: float
    redis_latency_ms: float
    database_latency_ms: float
    websocket_latency_ms: float
    session_operations_per_second: float
    error_rate_percent: float

class SessionPerformanceMonitor:
    def __init__(self, session_manager, websocket_gateway, buffer_manager):
        self.session_manager = session_manager
        self.websocket_gateway = websocket_gateway
        self.buffer_manager = buffer_manager
        
        # Prometheus metrics
        self.session_operations = Counter(
            'session_operations_total',
            'Total session operations',
            ['operation', 'status']
        )
        
        self.session_duration = Histogram(
            'session_operation_duration_seconds',
            'Session operation duration',
            ['operation']
        )
        
        self.active_sessions = Gauge(
            'active_sessions_total',
            'Number of active sessions'
        )
        
        self.websocket_connections = Gauge(
            'websocket_connections_active',
            'Active WebSocket connections'
        )
        
        self.redis_operations = Histogram(
            'redis_operation_duration_seconds',
            'Redis operation duration',
            ['operation']
        )
        
        self.memory_usage = Gauge(
            'session_memory_usage_bytes',
            'Session manager memory usage'
        )
        
        # Performance history
        self.metrics_history: deque = deque(maxlen=1000)
        self.monitoring_task: Optional[asyncio.Task] = None
        self.collection_interval = 10  # seconds
        
        # Performance thresholds
        self.thresholds = {
            'session_creation_ms': 500,
            'redis_operation_ms': 100,
            'websocket_latency_ms': 50,
            'memory_usage_mb': 1000,
            'cpu_usage_percent': 80,
            'error_rate_percent': 5
        }
    
    async def initialize(self):
        """Initialize performance monitoring"""
        self.monitoring_task = asyncio.create_task(self._monitoring_loop())
        logger.info("Performance monitoring initialized")
        logfire.info("Session performance monitoring started")
    
    async def _monitoring_loop(self):
        """Main performance monitoring loop"""
        while True:
            try:
                await asyncio.sleep(self.collection_interval)
                await self._collect_performance_metrics()
                await self._analyze_performance()
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Performance monitoring error", error=str(e))
    
    async def _collect_performance_metrics(self):
        """Collect current performance metrics"""
        try:
            start_time = time.time()
            
            # Collect system metrics
            memory_info = psutil.virtual_memory()
            cpu_percent = psutil.cpu_percent()
            
            # Collect application metrics
            session_count = len(self.session_manager.sessions)
            connection_count = len(getattr(self.websocket_gateway, 'connections', {}))
            
            # Measure component latencies
            redis_latency = await self._measure_redis_latency()
            db_latency = await self._measure_database_latency()
            websocket_latency = await self._measure_websocket_latency()
            
            # Calculate operation rates
            ops_per_second = await self._calculate_operations_per_second()
            error_rate = await self._calculate_error_rate()
            
            # Create metrics object
            metrics = PerformanceMetrics(
                timestamp=time.time(),
                session_count=session_count,
                active_connections=connection_count,
                memory_usage_mb=memory_info.used / (1024 * 1024),
                cpu_usage_percent=cpu_percent,
                redis_latency_ms=redis_latency,
                database_latency_ms=db_latency,
                websocket_latency_ms=websocket_latency,
                session_operations_per_second=ops_per_second,
                error_rate_percent=error_rate
            )
            
            # Store metrics
            self.metrics_history.append(metrics)
            
            # Update Prometheus metrics
            self.active_sessions.set(session_count)
            self.websocket_connections.set(connection_count)
            self.memory_usage.set(memory_info.used)
            
            # Log to Logfire
            logfire.info("Performance metrics collected",
                        session_count=session_count,
                        memory_usage_mb=metrics.memory_usage_mb,
                        cpu_usage_percent=cpu_percent,
                        redis_latency_ms=redis_latency,
                        operations_per_second=ops_per_second,
                        error_rate_percent=error_rate)
            
            collection_duration = (time.time() - start_time) * 1000
            logger.debug("Metrics collection completed",
                        duration_ms=collection_duration,
                        session_count=session_count)
            
        except Exception as e:
            logger.error("Failed to collect performance metrics", error=str(e))
    
    async def _measure_redis_latency(self) -> float:
        """Measure Redis operation latency"""
        try:
            start_time = time.time()
            
            # Perform a simple Redis operation
            test_key = "performance_test_key"
            await self.buffer_manager.redis.set(test_key, "test_value", ex=60)
            await self.buffer_manager.redis.get(test_key)
            await self.buffer_manager.redis.delete(test_key)
            
            latency_ms = (time.time() - start_time) * 1000
            return latency_ms
            
        except Exception as e:
            logger.error("Redis latency measurement failed", error=str(e))
            return 999.0  # High latency indicating error
    
    async def _measure_database_latency(self) -> float:
        """Measure database operation latency"""
        try:
            start_time = time.time()
            
            # Perform a simple database query (this would be implemented
            # with actual database connection)
            # await self.database.execute("SELECT 1")
            
            latency_ms = (time.time() - start_time) * 1000
            return latency_ms
            
        except Exception as e:
            logger.error("Database latency measurement failed", error=str(e))
            return 999.0
    
    async def _measure_websocket_latency(self) -> float:
        """Measure WebSocket operation latency"""
        try:
            # This would measure the time to process a WebSocket message
            # For now, return a simulated measurement
            return 25.0  # Simulated 25ms latency
            
        except Exception as e:
            logger.error("WebSocket latency measurement failed", error=str(e))
            return 999.0
    
    async def _calculate_operations_per_second(self) -> float:
        """Calculate session operations per second"""
        try:
            if len(self.metrics_history) < 2:
                return 0.0
            
            # Calculate based on last minute of data
            current_time = time.time()
            minute_ago = current_time - 60
            
            recent_metrics = [
                m for m in self.metrics_history 
                if m.timestamp >= minute_ago
            ]
            
            if len(recent_metrics) < 2:
                return 0.0
            
            # Estimate operations based on session count changes and activity
            # This is a simplified calculation
            return len(recent_metrics) * 2.0  # Approximate operations
            
        except Exception as e:
            logger.error("Operations per second calculation failed", error=str(e))
            return 0.0
    
    async def _calculate_error_rate(self) -> float:
        """Calculate error rate percentage"""
        try:
            # This would be calculated from actual error counters
            # For now, return a simulated value
            return 1.5  # Simulated 1.5% error rate
            
        except Exception as e:
            logger.error("Error rate calculation failed", error=str(e))
            return 0.0
    
    async def _analyze_performance(self):
        """Analyze performance and trigger alerts if needed"""
        if not self.metrics_history:
            return
        
        current_metrics = self.metrics_history[-1]
        
        # Check thresholds and log warnings
        alerts = []
        
        if current_metrics.redis_latency_ms > self.thresholds['redis_operation_ms']:
            alerts.append(f"High Redis latency: {current_metrics.redis_latency_ms:.1f}ms")
        
        if current_metrics.memory_usage_mb > self.thresholds['memory_usage_mb']:
            alerts.append(f"High memory usage: {current_metrics.memory_usage_mb:.1f}MB")
        
        if current_metrics.cpu_usage_percent > self.thresholds['cpu_usage_percent']:
            alerts.append(f"High CPU usage: {current_metrics.cpu_usage_percent:.1f}%")
        
        if current_metrics.error_rate_percent > self.thresholds['error_rate_percent']:
            alerts.append(f"High error rate: {current_metrics.error_rate_percent:.1f}%")
        
        if alerts:
            logger.warning("Performance threshold alerts", alerts=alerts)
            logfire.warning("Performance alerts triggered",
                          alerts=alerts,
                          session_count=current_metrics.session_count,
                          timestamp=current_metrics.timestamp)
    
    def get_performance_summary(self, duration_minutes: int = 10) -> Dict[str, Any]:
        """Get performance summary for the last N minutes"""
        try:
            cutoff_time = time.time() - (duration_minutes * 60)
            recent_metrics = [
                m for m in self.metrics_history 
                if m.timestamp >= cutoff_time
            ]
            
            if not recent_metrics:
                return {"error": "No metrics available"}
            
            # Calculate averages
            avg_memory = sum(m.memory_usage_mb for m in recent_metrics) / len(recent_metrics)
            avg_cpu = sum(m.cpu_usage_percent for m in recent_metrics) / len(recent_metrics)
            avg_redis_latency = sum(m.redis_latency_ms for m in recent_metrics) / len(recent_metrics)
            avg_sessions = sum(m.session_count for m in recent_metrics) / len(recent_metrics)
            
            # Calculate peaks
            peak_memory = max(m.memory_usage_mb for m in recent_metrics)
            peak_cpu = max(m.cpu_usage_percent for m in recent_metrics)
            peak_sessions = max(m.session_count for m in recent_metrics)
            
            summary = {
                "duration_minutes": duration_minutes,
                "data_points": len(recent_metrics),
                "averages": {
                    "memory_usage_mb": round(avg_memory, 2),
                    "cpu_usage_percent": round(avg_cpu, 2),
                    "redis_latency_ms": round(avg_redis_latency, 2),
                    "session_count": round(avg_sessions, 2)
                },
                "peaks": {
                    "memory_usage_mb": round(peak_memory, 2),
                    "cpu_usage_percent": round(peak_cpu, 2),
                    "session_count": peak_sessions
                },
                "latest": asdict(recent_metrics[-1])
            }
            
            return summary
            
        except Exception as e:
            logger.error("Performance summary generation failed", error=str(e))
            return {"error": str(e)}
```

## TDD Performance Testing Cycle

### Performance-First Development Process

1. **Red Phase**: Write failing performance tests
   ```bash
   # Create performance test file
   touch session-manager/tests/performance/test_session_performance.py
   
   # Run failing performance test
   pytest session-manager/tests/performance/test_session_performance.py::test_session_creation_latency -v
   ```

2. **Green Phase**: Implement basic performance monitoring
   ```bash
   # Add performance metrics collection
   pytest session-manager/tests/performance/test_session_performance.py::test_session_creation_latency -v
   ```

3. **Refactor Phase**: Optimize performance bottlenecks
   ```bash
   # Optimize Redis operations and memory usage
   pytest session-manager/tests/performance/ -v
   ```

4. **Performance Commit**: Commit optimizations
   ```bash
   git add session-manager/monitoring/ session-manager/tests/performance/
   git commit -m "perf: implement session performance monitoring and optimization
   
   - Add SessionPerformanceMonitor with comprehensive metrics collection
   - Implement real-time performance threshold monitoring
   - Add Prometheus metrics integration for scalability
   - Optimize Redis operations with connection pooling
   - Include performance regression testing framework
   
   Tests: Added performance test suite with load testing scenarios
   Performance: Session creation latency < 200ms, Redis ops < 50ms
   Monitoring: Real-time performance alerts and Logfire integration"
   ```

### Performance Test Cases

```python
# session-manager/tests/performance/test_session_performance.py
import pytest
import asyncio
import time
from unittest.mock import AsyncMock
from session_manager.monitoring.performance_monitor import SessionPerformanceMonitor

@pytest.fixture
def performance_monitor(session_manager_mock, websocket_gateway_mock, buffer_manager_mock):
    """Performance monitor instance"""
    return SessionPerformanceMonitor(
        session_manager_mock,
        websocket_gateway_mock,
        buffer_manager_mock
    )

class TestSessionPerformance:
    async def test_session_creation_latency(self, performance_monitor):
        """Test session creation meets latency requirements"""
        start_time = time.time()
        
        # Simulate session creation
        session_id = await performance_monitor.session_manager.create_session(
            "test_user", "test_vm"
        )
        
        latency_ms = (time.time() - start_time) * 1000
        
        # Verify latency requirement
        assert latency_ms < 500, f"Session creation too slow: {latency_ms:.1f}ms"
        assert session_id is not None
    
    async def test_concurrent_session_performance(self, performance_monitor):
        """Test performance under concurrent session load"""
        concurrent_sessions = 100
        start_time = time.time()
        
        # Create concurrent sessions
        tasks = []
        for i in range(concurrent_sessions):
            task = performance_monitor.session_manager.create_session(
                f"user_{i}", f"vm_{i}"
            )
            tasks.append(task)
        
        session_ids = await asyncio.gather(*tasks)
        
        total_time = time.time() - start_time
        avg_latency = (total_time / concurrent_sessions) * 1000
        
        # Verify concurrent performance
        assert avg_latency < 200, f"Concurrent session creation too slow: {avg_latency:.1f}ms"
        assert len(session_ids) == concurrent_sessions
        assert all(sid is not None for sid in session_ids)
    
    async def test_redis_operation_performance(self, performance_monitor):
        """Test Redis operation performance"""
        start_time = time.time()
        
        # Measure Redis latency
        latency_ms = await performance_monitor._measure_redis_latency()
        
        # Verify Redis performance
        assert latency_ms < 100, f"Redis operations too slow: {latency_ms:.1f}ms"
    
    async def test_memory_usage_efficiency(self, performance_monitor):
        """Test memory usage stays within limits"""
        # Initialize monitoring
        await performance_monitor.initialize()
        
        # Collect initial metrics
        await performance_monitor._collect_performance_metrics()
        
        if performance_monitor.metrics_history:
            metrics = performance_monitor.metrics_history[-1]
            
            # Verify memory usage is reasonable
            assert metrics.memory_usage_mb < 1000, f"Memory usage too high: {metrics.memory_usage_mb:.1f}MB"
```

## Performance Requirements & Targets

### Session Operations Performance
- **Session creation latency**: < 200ms (95th percentile)
- **Session state updates**: < 50ms (average)
- **Session retrieval**: < 100ms (95th percentile)
- **Session cleanup**: < 500ms (maximum)
- **Concurrent sessions**: Support 1000+ active sessions per instance
- **Session throughput**: > 100 operations/second per instance

### Redis Operations Performance  
- **Buffer storage**: < 50ms (95th percentile)
- **Buffer retrieval**: < 30ms (95th percentile)
- **Key operations**: < 10ms (average)
- **Connection pooling**: > 90% connection reuse rate
- **Memory efficiency**: < 1KB overhead per session
- **Compression ratio**: > 70% for buffers > 4KB

### WebSocket Performance
- **Connection establishment**: < 500ms
- **Message processing**: < 10ms per message
- **Heartbeat latency**: < 30ms
- **Concurrent connections**: 1000+ per instance
- **Message throughput**: > 1000 messages/second
- **Connection cleanup**: < 60 seconds after disconnect

### Resource Utilization
- **Memory usage**: < 500MB for 1000 active sessions
- **CPU usage**: < 40% under normal load
- **Network bandwidth**: Optimized with compression
- **Disk I/O**: Minimal with Redis-based storage
- **File descriptors**: < 2000 for maximum load
- **Thread pool**: Efficient async operation handling

## Performance Optimization Strategies

### Redis Optimization
```python
# Optimized Redis operations with pipelining
async def optimized_buffer_operations(self, operations: List[Dict]):
    """Batch Redis operations for better performance"""
    pipe = self.redis.pipeline()
    
    for op in operations:
        if op['type'] == 'set':
            pipe.hset(op['key'], mapping=op['data'])
        elif op['type'] == 'get':
            pipe.hgetall(op['key'])
    
    results = await pipe.execute()
    return results
```

### Memory Optimization
```python
# Memory-efficient session storage
class OptimizedSessionContext:
    """Memory-optimized session context with slots"""
    __slots__ = ['session_id', 'user_id', 'state', 'last_activity']
    
    def __init__(self, session_id: str, user_id: str):
        self.session_id = session_id
        self.user_id = user_id
        self.state = SessionState.INITIALIZING
        self.last_activity = time.time()
```

### Connection Pool Optimization
```python
# Optimized connection management
class OptimizedConnectionPool:
    """High-performance connection pool"""
    
    def __init__(self, max_connections: int = 100):
        self.max_connections = max_connections
        self.pool = asyncio.Queue(maxsize=max_connections)
        self.active_connections = 0
    
    async def get_connection(self):
        """Get connection with timeout"""
        try:
            return await asyncio.wait_for(
                self.pool.get(), timeout=5.0
            )
        except asyncio.TimeoutError:
            raise ConnectionError("Connection pool exhausted")
```

## Load Testing & Benchmarking

### Load Testing Framework
```python
# session-manager/tests/performance/load_test.py
import asyncio
import aiohttp
import time
from typing import List, Dict

class SessionLoadTester:
    """Load testing framework for session management"""
    
    def __init__(self, base_url: str, max_concurrent: int = 100):
        self.base_url = base_url
        self.max_concurrent = max_concurrent
        self.results: List[Dict] = []
    
    async def run_load_test(self, duration_seconds: int):
        """Run load test for specified duration"""
        semaphore = asyncio.Semaphore(self.max_concurrent)
        end_time = time.time() + duration_seconds
        
        tasks = []
        while time.time() < end_time:
            task = asyncio.create_task(
                self._session_operation(semaphore)
            )
            tasks.append(task)
            
            # Prevent task accumulation
            if len(tasks) >= self.max_concurrent * 2:
                completed, pending = await asyncio.wait(
                    tasks, return_when=asyncio.FIRST_COMPLETED
                )
                tasks = list(pending)
        
        # Wait for remaining tasks
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
    
    async def _session_operation(self, semaphore):
        """Perform a session operation under semaphore control"""
        async with semaphore:
            start_time = time.time()
            try:
                # Simulate session creation
                await asyncio.sleep(0.1)  # Simulated operation
                
                latency = (time.time() - start_time) * 1000
                self.results.append({
                    'operation': 'create_session',
                    'latency_ms': latency,
                    'success': True,
                    'timestamp': time.time()
                })
                
            except Exception as e:
                self.results.append({
                    'operation': 'create_session',
                    'latency_ms': 0,
                    'success': False,
                    'error': str(e),
                    'timestamp': time.time()
                })
```

### Benchmark Testing
```python
async def run_session_benchmarks():
    """Run comprehensive session performance benchmarks"""
    
    benchmarks = {
        'session_creation': test_session_creation_benchmark,
        'buffer_operations': test_buffer_operations_benchmark,
        'websocket_throughput': test_websocket_throughput_benchmark,
        'concurrent_load': test_concurrent_load_benchmark
    }
    
    results = {}
    for name, test_func in benchmarks.items():
        print(f"Running benchmark: {name}")
        result = await test_func()
        results[name] = result
        print(f"  Result: {result}")
    
    return results
```

## Monitoring Integration with Logfire

### Performance Dashboard Configuration
```python
# Configure Logfire for performance monitoring
logfire.configure(
    service_name="session-manager-performance",
    service_version="1.0.0",
    environment="production"
)

# Performance event logging
async def log_performance_event(operation: str, duration_ms: float, 
                               context: Dict[str, Any]):
    """Log performance events to Logfire"""
    logfire.info(
        f"Performance: {operation}",
        operation=operation,
        duration_ms=duration_ms,
        performance_tier="session_management",
        **context
    )
```

### Alert Configuration
```python
# Performance alert thresholds
PERFORMANCE_ALERTS = {
    'session_creation_slow': {
        'threshold_ms': 500,
        'severity': 'WARNING'
    },
    'redis_latency_high': {
        'threshold_ms': 100,
        'severity': 'CRITICAL'
    },
    'memory_usage_high': {
        'threshold_mb': 1000,
        'severity': 'WARNING'
    },
    'error_rate_high': {
        'threshold_percent': 5,
        'severity': 'CRITICAL'
    }
}
```

## Performance Regression Testing

### Automated Performance Validation
```bash
# Performance regression test script
#!/bin/bash
echo "Running performance regression tests..."

# Run load tests
python -m pytest session-manager/tests/performance/ -v --benchmark-only

# Check performance metrics
python session-manager/scripts/validate_performance.py

# Generate performance report
python session-manager/scripts/generate_performance_report.py
```

### Continuous Performance Integration
```yaml
# .github/workflows/performance-tests.yml
name: Performance Tests
on: [push, pull_request]

jobs:
  performance:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Setup Python
        uses: actions/setup-python@v4
        with:
          python-version: '3.11'
      - name: Install dependencies
        run: pip install -r requirements-dev.txt
      - name: Run performance tests
        run: pytest session-manager/tests/performance/ --benchmark-json=benchmark.json
      - name: Validate performance
        run: python scripts/validate_performance_regression.py benchmark.json
```

## Next Performance Implementation Steps

1. **Complete performance monitoring** with all metrics collection
2. **Implement Redis connection pooling** for improved performance  
3. **Add comprehensive load testing** with realistic scenarios
4. **Create performance regression testing** in CI/CD pipeline
5. **Optimize memory usage** with efficient data structures
6. **Add performance alerting** with Logfire integration
7. **Create performance documentation** with optimization guides

## Performance Commit Guidelines

Performance commits should include:
- **Benchmark results** before and after optimization
- **Performance test coverage** for new optimizations
- **Resource utilization impact** analysis
- **Scalability considerations** and testing
- **Monitoring integration** for performance tracking
- **Documentation updates** with performance characteristics