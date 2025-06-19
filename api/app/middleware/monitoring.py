"""Monitoring and metrics middleware."""

import time
import asyncio
import structlog
from typing import Dict, Any, Optional
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import Response

try:
    import psutil
    PSUTIL_AVAILABLE = True
except ImportError:
    PSUTIL_AVAILABLE = False

try:
    from prometheus_client import Counter, Histogram, Gauge, generate_latest, CONTENT_TYPE_LATEST
    PROMETHEUS_AVAILABLE = True
except ImportError:
    PROMETHEUS_AVAILABLE = False

from ..core.config import get_settings

logger = structlog.get_logger(__name__)

# Prometheus metrics (only if available)
if PROMETHEUS_AVAILABLE:
    REQUEST_COUNT = Counter(
        'http_requests_total',
        'Total HTTP requests',
        ['method', 'endpoint', 'status_code']
    )

    REQUEST_DURATION = Histogram(
        'http_request_duration_seconds',
        'HTTP request duration in seconds',
        ['method', 'endpoint']
    )

    ACTIVE_CONNECTIONS = Gauge(
        'http_active_connections',
        'Number of active HTTP connections'
    )

    SYSTEM_CPU_USAGE = Gauge(
        'system_cpu_usage_percent',
        'System CPU usage percentage'
    )

    SYSTEM_MEMORY_USAGE = Gauge(
        'system_memory_usage_bytes',
        'System memory usage in bytes'
    )

    SYSTEM_MEMORY_AVAILABLE = Gauge(
        'system_memory_available_bytes',
        'System available memory in bytes'
    )

    REDIS_OPERATIONS = Counter(
        'redis_operations_total',
        'Total Redis operations',
        ['operation', 'status']
    )

    DATABASE_CONNECTIONS = Gauge(
        'database_connections_active',
        'Active database connections'
    )
else:
    # Create dummy objects if Prometheus is not available
    class DummyMetric:
        def inc(self, *args, **kwargs): pass
        def observe(self, *args, **kwargs): pass
        def set(self, *args, **kwargs): pass
        def labels(self, *args, **kwargs): return self
        def dec(self, *args, **kwargs): pass
    
    REQUEST_COUNT = DummyMetric()
    REQUEST_DURATION = DummyMetric()
    ACTIVE_CONNECTIONS = DummyMetric()
    SYSTEM_CPU_USAGE = DummyMetric()
    SYSTEM_MEMORY_USAGE = DummyMetric()
    SYSTEM_MEMORY_AVAILABLE = DummyMetric()
    REDIS_OPERATIONS = DummyMetric()
    DATABASE_CONNECTIONS = DummyMetric()


class PrometheusMiddleware(BaseHTTPMiddleware):
    """Collect Prometheus metrics for HTTP requests."""
    
    def __init__(self, app, **kwargs):
        super().__init__(app)
        self.settings = get_settings()
        
    async def dispatch(self, request: Request, call_next):
        # Skip metrics collection for the metrics endpoint itself
        if request.url.path == "/metrics":
            return await call_next(request)
        
        # Increment active connections
        ACTIVE_CONNECTIONS.inc()
        
        # Start timer
        start_time = time.time()
        
        try:
            response = await call_next(request)
            
            # Calculate duration
            duration = time.time() - start_time
            
            # Get endpoint pattern (remove IDs and query params for grouping)
            endpoint = self.normalize_endpoint(request.url.path)
            
            # Record metrics
            REQUEST_COUNT.labels(
                method=request.method,
                endpoint=endpoint,
                status_code=response.status_code
            ).inc()
            
            REQUEST_DURATION.labels(
                method=request.method,
                endpoint=endpoint
            ).observe(duration)
            
            return response
            
        except Exception as e:
            # Record error metrics
            endpoint = self.normalize_endpoint(request.url.path)
            REQUEST_COUNT.labels(
                method=request.method,
                endpoint=endpoint,
                status_code=500
            ).inc()
            
            raise
            
        finally:
            # Decrement active connections
            ACTIVE_CONNECTIONS.dec()
    
    def normalize_endpoint(self, path: str) -> str:
        """Normalize endpoint path for metrics grouping."""
        # Replace UUIDs and numeric IDs with placeholders
        import re
        
        # Replace UUIDs
        path = re.sub(
            r'/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}',
            '/{uuid}',
            path,
            flags=re.IGNORECASE
        )
        
        # Replace numeric IDs
        path = re.sub(r'/\d+', '/{id}', path)
        
        return path


class HealthMetricsMiddleware(BaseHTTPMiddleware):
    """Collect system health metrics."""
    
    def __init__(self, app, **kwargs):
        super().__init__(app)
        self.settings = get_settings()
        self.last_update = 0
        self.update_interval = 30  # Update every 30 seconds
        
    async def dispatch(self, request: Request, call_next):
        # Update system metrics periodically
        current_time = time.time()
        if current_time - self.last_update > self.update_interval:
            await self.update_system_metrics()
            self.last_update = current_time
        
        return await call_next(request)
    
    async def update_system_metrics(self):
        """Update system resource metrics."""
        if not PSUTIL_AVAILABLE:
            return
            
        try:
            # CPU usage
            cpu_percent = psutil.cpu_percent(interval=None)
            SYSTEM_CPU_USAGE.set(cpu_percent)
            
            # Memory usage
            memory = psutil.virtual_memory()
            SYSTEM_MEMORY_USAGE.set(memory.used)
            SYSTEM_MEMORY_AVAILABLE.set(memory.available)
            
        except Exception as e:
            logger.error("Failed to update system metrics", error=str(e))


class ErrorTrackingMiddleware(BaseHTTPMiddleware):
    """Track and log application errors."""
    
    def __init__(self, app, **kwargs):
        super().__init__(app)
        self.settings = get_settings()
        
    async def dispatch(self, request: Request, call_next):
        try:
            response = await call_next(request)
            
            # Log 4xx and 5xx responses
            if response.status_code >= 400:
                logger.warning(
                    "HTTP error response",
                    method=request.method,
                    path=request.url.path,
                    status_code=response.status_code,
                    client_ip=self.get_client_ip(request),
                    user_agent=request.headers.get("user-agent", "")
                )
            
            return response
            
        except Exception as e:
            logger.error(
                "Unhandled exception in request",
                method=request.method,
                path=request.url.path,
                error=str(e),
                error_type=type(e).__name__,
                client_ip=self.get_client_ip(request)
            )
            
            raise
    
    def get_client_ip(self, request: Request) -> str:
        """Get client IP address."""
        forwarded_for = request.headers.get("X-Forwarded-For")
        if forwarded_for:
            return forwarded_for.split(",")[0].strip()
        
        real_ip = request.headers.get("X-Real-IP")
        if real_ip:
            return real_ip
        
        return request.client.host if request.client else "unknown"


class PerformanceMonitoringMiddleware(BaseHTTPMiddleware):
    """Monitor application performance and detect anomalies."""
    
    def __init__(self, app, **kwargs):
        super().__init__(app)
        self.settings = get_settings()
        self.slow_request_threshold = 1.0  # 1 second
        self.very_slow_request_threshold = 5.0  # 5 seconds
        
    async def dispatch(self, request: Request, call_next):
        start_time = time.time()
        
        response = await call_next(request)
        
        duration = time.time() - start_time
        
        # Log slow requests
        if duration > self.slow_request_threshold:
            level = "warning" if duration < self.very_slow_request_threshold else "error"
            
            log_data = {
                "message": "Slow request detected",
                "method": request.method,
                "path": request.url.path,
                "duration_seconds": round(duration, 3),
                "client_ip": self.get_client_ip(request),
                "status_code": response.status_code
            }
            
            if level == "error":
                logger.error(**log_data)
            else:
                logger.warning(**log_data)
        
        return response
    
    def get_client_ip(self, request: Request) -> str:
        """Get client IP address."""
        forwarded_for = request.headers.get("X-Forwarded-For")
        if forwarded_for:
            return forwarded_for.split(",")[0].strip()
        
        return request.client.host if request.client else "unknown"


async def get_system_metrics() -> Dict[str, Any]:
    """Get current system metrics."""
    if not PSUTIL_AVAILABLE:
        return {"error": "psutil not available", "status": "degraded"}
    
    try:
        # CPU metrics
        cpu_percent = psutil.cpu_percent(interval=0.1)
        cpu_count = psutil.cpu_count()
        
        # Memory metrics
        memory = psutil.virtual_memory()
        
        # Disk metrics
        disk = psutil.disk_usage('/')
        
        # Network metrics (if needed)
        # network = psutil.net_io_counters()
        
        return {
            "cpu": {
                "usage_percent": cpu_percent,
                "count": cpu_count
            },
            "memory": {
                "total": memory.total,
                "available": memory.available,
                "used": memory.used,
                "percent": memory.percent
            },
            "disk": {
                "total": disk.total,
                "used": disk.used,
                "free": disk.free,
                "percent": (disk.used / disk.total) * 100
            },
            "status": "healthy"
        }
    except Exception as e:
        logger.error("Failed to get system metrics", error=str(e))
        return {"error": str(e), "status": "unhealthy"}


def record_redis_operation(operation: str, success: bool = True):
    """Record Redis operation metrics."""
    status = "success" if success else "error"
    REDIS_OPERATIONS.labels(operation=operation, status=status).inc()


def set_database_connections(count: int):
    """Set current database connection count."""
    DATABASE_CONNECTIONS.set(count)