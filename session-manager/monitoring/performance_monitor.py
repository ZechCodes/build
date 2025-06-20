"""
Session Performance Monitor

Real-time performance monitoring with metrics collection, alerting,
and Prometheus integration for session management operations.
"""
import asyncio
import time
import psutil
from typing import Dict, Any, List, Optional
from dataclasses import dataclass, asdict
from collections import deque
import structlog
import logfire

try:
    from prometheus_client import Counter, Histogram, Gauge, Summary
    PROMETHEUS_AVAILABLE = True
except ImportError:
    PROMETHEUS_AVAILABLE = False
    # Mock classes for when prometheus is not available
    class Counter:
        def __init__(self, *args, **kwargs): pass
        def inc(self, *args, **kwargs): pass
    class Histogram:
        def __init__(self, *args, **kwargs): pass
        def observe(self, *args, **kwargs): pass
    class Gauge:
        def __init__(self, *args, **kwargs): pass
        def set(self, *args, **kwargs): pass
    class Summary:
        def __init__(self, *args, **kwargs): pass
        def observe(self, *args, **kwargs): pass

logger = structlog.get_logger()


@dataclass
class PerformanceMetrics:
    """Performance metrics snapshot"""
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
    """Performance monitoring with metrics and alerting"""
    
    def __init__(self, session_manager, websocket_gateway, buffer_manager):
        self.session_manager = session_manager
        self.websocket_gateway = websocket_gateway
        self.buffer_manager = buffer_manager
        
        # Prometheus metrics (if available)
        if PROMETHEUS_AVAILABLE:
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
        else:
            # Mock metrics when Prometheus is not available
            self.session_operations = Counter()
            self.session_duration = Histogram()
            self.active_sessions = Gauge()
            self.websocket_connections = Gauge()
            self.redis_operations = Histogram()
            self.memory_usage = Gauge()
        
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
        
        # Operation tracking
        self.operation_counts = {
            'session_created': 0,
            'session_deleted': 0,
            'buffer_stored': 0,
            'buffer_retrieved': 0,
            'recovery_initiated': 0,
            'websocket_connected': 0,
            'errors': 0
        }
        
        self.operation_timings: Dict[str, List[float]] = {}
        
    async def initialize(self):
        """Initialize performance monitoring"""
        self.monitoring_task = asyncio.create_task(self._monitoring_loop())
        logger.info("Performance monitoring initialized")
        logfire.info("Session performance monitoring started")
    
    async def stop(self):
        """Stop performance monitoring"""
        if self.monitoring_task:
            self.monitoring_task.cancel()
            try:
                await self.monitoring_task
            except asyncio.CancelledError:
                pass
        
        logger.info("Performance monitoring stopped")
    
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
            try:
                memory_info = psutil.virtual_memory()
                cpu_percent = psutil.cpu_percent()
            except Exception:
                # Fallback if psutil is not available
                memory_info = type('MemInfo', (), {'used': 0})()
                cpu_percent = 0
            
            # Collect application metrics
            session_count = len(getattr(self.session_manager, 'sessions', {}))
            connection_count = getattr(self.websocket_gateway, 'get_connection_count', lambda: 0)()
            
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
                memory_usage_mb=getattr(memory_info, 'used', 0) / (1024 * 1024),
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
            self.memory_usage.set(getattr(memory_info, 'used', 0))
            
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
            
            # Perform a simple Redis operation if available
            if hasattr(self.buffer_manager, 'redis'):
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
            
            # Perform a simple database query (mock for now)
            await asyncio.sleep(0.001)  # Simulate DB query
            
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
            
            # Use operation counts to calculate rate
            total_operations = sum(self.operation_counts.values())
            return total_operations / 60.0  # Operations per second over last minute
            
        except Exception as e:
            logger.error("Operations per second calculation failed", error=str(e))
            return 0.0
    
    async def _calculate_error_rate(self) -> float:
        """Calculate error rate percentage"""
        try:
            total_operations = sum(self.operation_counts.values())
            error_count = self.operation_counts.get('errors', 0)
            
            if total_operations == 0:
                return 0.0
            
            return (error_count / total_operations) * 100.0
            
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
    
    def record_operation(self, operation: str, duration_ms: Optional[float] = None, 
                        success: bool = True):
        """Record an operation for performance tracking"""
        try:
            # Update operation counts
            if success:
                self.operation_counts[operation] = self.operation_counts.get(operation, 0) + 1
            else:
                self.operation_counts['errors'] = self.operation_counts.get('errors', 0) + 1
            
            # Record timing if provided
            if duration_ms is not None:
                if operation not in self.operation_timings:
                    self.operation_timings[operation] = []
                self.operation_timings[operation].append(duration_ms)
                
                # Keep only recent timings
                if len(self.operation_timings[operation]) > 1000:
                    self.operation_timings[operation] = self.operation_timings[operation][-500:]
            
            # Update Prometheus metrics
            status = 'success' if success else 'error'
            self.session_operations.inc()
            
            if duration_ms is not None:
                self.session_duration.observe(duration_ms / 1000.0)
            
        except Exception as e:
            logger.error("Failed to record operation", operation=operation, error=str(e))
    
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
            
            # Calculate operation statistics
            operation_stats = {}
            for operation, timings in self.operation_timings.items():
                if timings:
                    operation_stats[operation] = {
                        'count': len(timings),
                        'avg_ms': sum(timings) / len(timings),
                        'min_ms': min(timings),
                        'max_ms': max(timings)
                    }
            
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
                "operation_counts": self.operation_counts.copy(),
                "operation_stats": operation_stats,
                "latest": asdict(recent_metrics[-1]) if recent_metrics else None
            }
            
            return summary
            
        except Exception as e:
            logger.error("Performance summary generation failed", error=str(e))
            return {"error": str(e)}
    
    def get_current_metrics(self) -> Optional[PerformanceMetrics]:
        """Get the most recent performance metrics"""
        return self.metrics_history[-1] if self.metrics_history else None
    
    def reset_operation_counts(self):
        """Reset operation counters (useful for testing)"""
        self.operation_counts.clear()
        self.operation_timings.clear()
    
    def set_threshold(self, metric: str, value: float):
        """Update performance threshold"""
        if metric in self.thresholds:
            self.thresholds[metric] = value
            logger.info("Performance threshold updated", metric=metric, value=value)
        else:
            logger.warning("Unknown performance metric", metric=metric)
    
    def get_alerts(self) -> List[Dict[str, Any]]:
        """Get current performance alerts"""
        try:
            alerts = []
            
            if not self.metrics_history:
                return alerts
            
            current_metrics = self.metrics_history[-1]
            
            # Check each threshold
            if current_metrics.redis_latency_ms > self.thresholds['redis_operation_ms']:
                alerts.append({
                    'metric': 'redis_latency',
                    'value': current_metrics.redis_latency_ms,
                    'threshold': self.thresholds['redis_operation_ms'],
                    'severity': 'warning',
                    'message': f"Redis latency {current_metrics.redis_latency_ms:.1f}ms exceeds threshold"
                })
            
            if current_metrics.memory_usage_mb > self.thresholds['memory_usage_mb']:
                alerts.append({
                    'metric': 'memory_usage',
                    'value': current_metrics.memory_usage_mb,
                    'threshold': self.thresholds['memory_usage_mb'],
                    'severity': 'critical',
                    'message': f"Memory usage {current_metrics.memory_usage_mb:.1f}MB exceeds threshold"
                })
            
            if current_metrics.cpu_usage_percent > self.thresholds['cpu_usage_percent']:
                alerts.append({
                    'metric': 'cpu_usage',
                    'value': current_metrics.cpu_usage_percent,
                    'threshold': self.thresholds['cpu_usage_percent'],
                    'severity': 'warning',
                    'message': f"CPU usage {current_metrics.cpu_usage_percent:.1f}% exceeds threshold"
                })
            
            if current_metrics.error_rate_percent > self.thresholds['error_rate_percent']:
                alerts.append({
                    'metric': 'error_rate',
                    'value': current_metrics.error_rate_percent,
                    'threshold': self.thresholds['error_rate_percent'],
                    'severity': 'critical',
                    'message': f"Error rate {current_metrics.error_rate_percent:.1f}% exceeds threshold"
                })
            
            return alerts
            
        except Exception as e:
            logger.error("Failed to get performance alerts", error=str(e))
            return []