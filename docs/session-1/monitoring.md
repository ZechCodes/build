# Monitoring & Observability

This document details the comprehensive monitoring and observability implementation using Pydantic Logfire in Session 1 of the Build Platform.

## Monitoring Overview

The Build Platform implements comprehensive observability through:

1. **Distributed Tracing** - Request flow across components
2. **Structured Logging** - JSON-formatted logs with context
3. **Metrics Collection** - Performance and health metrics
4. **Error Tracking** - Exception monitoring and reporting
5. **Security Monitoring** - Security event tracking
6. **Performance Monitoring** - Response time and resource usage

## Pydantic Logfire Integration

### Logfire Configuration

```python
# api/monitoring/logfire_setup.py
import logfire
from fastapi import FastAPI
import structlog
from typing import Any, Dict
import os

def setup_monitoring(app: FastAPI) -> None:
    """Configure Pydantic Logfire for comprehensive observability."""
    
    # Configure Logfire
    logfire.configure(
        service_name="build-api",
        service_version="1.0.0",
        environment=os.getenv("ENVIRONMENT", "development"),
        send_to_logfire=os.getenv("LOGFIRE_ENABLED", "true").lower() == "true",
        console=True,
        token=os.getenv("LOGFIRE_TOKEN"),
        project_name="build-platform"
    )
    
    # Instrument FastAPI automatically
    logfire.instrument_fastapi(app)
    
    # Instrument SQLAlchemy
    logfire.instrument_sqlalchemy(echo=False)
    
    # Instrument Redis
    logfire.instrument_redis()
    
    # Configure structured logging
    configure_structured_logging()
    
    logfire.info("Monitoring system initialized", service="build-api")

def configure_structured_logging():
    """Configure structured logging with Logfire integration."""
    structlog.configure(
        processors=[
            structlog.stdlib.filter_by_level,
            structlog.stdlib.add_logger_name,
            structlog.stdlib.add_log_level,
            structlog.stdlib.PositionalArgumentsFormatter(),
            structlog.processors.TimeStamper(fmt="iso"),
            structlog.processors.StackInfoRenderer(),
            structlog.processors.format_exc_info,
            structlog.processors.UnicodeDecoder(),
            structlog.processors.JSONRenderer()
        ],
        context_class=dict,
        logger_factory=structlog.stdlib.LoggerFactory(),
        wrapper_class=structlog.stdlib.BoundLogger,
        cache_logger_on_first_use=True,
    )
```

### Logfire Middleware

```python
# api/monitoring/middleware.py
from starlette.middleware.base import BaseHTTPMiddleware
from fastapi import Request, Response
import logfire
import time
import uuid
from typing import Optional

class LogfireTrackingMiddleware(BaseHTTPMiddleware):
    """Request tracking and distributed tracing with Logfire."""
    
    async def dispatch(self, request: Request, call_next):
        # Generate unique request ID
        request_id = str(uuid.uuid4())
        request.state.request_id = request_id
        
        # Extract user context
        user_id = getattr(request.state, 'user_id', None)
        
        start_time = time.time()
        
        with logfire.span(
            "HTTP Request",
            method=request.method,
            url=str(request.url),
            user_agent=request.headers.get("user-agent"),
            request_id=request_id,
            user_id=str(user_id) if user_id else None,
            ip_address=request.client.host
        ) as span:
            try:
                response = await call_next(request)
                
                process_time = time.time() - start_time
                
                # Set response attributes
                span.set_attribute("response.status_code", response.status_code)
                span.set_attribute("response.process_time", process_time)
                span.set_attribute("response.headers.content_length", 
                                 response.headers.get("content-length"))
                
                # Add request ID to response headers
                response.headers["X-Request-ID"] = request_id
                
                # Log request completion
                log_level = "warn" if response.status_code >= 400 else "info"
                getattr(logfire, log_level)(
                    f"Request completed: {request.method} {request.url.path}",
                    method=request.method,
                    path=request.url.path,
                    status_code=response.status_code,
                    process_time_ms=round(process_time * 1000, 2),
                    request_id=request_id,
                    user_id=str(user_id) if user_id else None,
                    ip_address=request.client.host
                )
                
                return response
                
            except Exception as e:
                process_time = time.time() - start_time
                
                # Set error attributes
                span.set_attribute("error", True)
                span.set_attribute("error.message", str(e))
                span.set_attribute("error.type", type(e).__name__)
                span.set_attribute("response.process_time", process_time)
                
                # Log error
                logfire.error(
                    f"Request failed: {request.method} {request.url.path}",
                    method=request.method,
                    path=request.url.path,
                    error_type=type(e).__name__,
                    error_message=str(e),
                    process_time_ms=round(process_time * 1000, 2),
                    request_id=request_id,
                    user_id=str(user_id) if user_id else None,
                    ip_address=request.client.host,
                    exc_info=e
                )
                
                raise

class LogfirePerformanceMiddleware(BaseHTTPMiddleware):
    """Performance monitoring and slow request detection."""
    
    def __init__(self, app, slow_request_threshold: float = 1.0):
        super().__init__(app)
        self.slow_request_threshold = slow_request_threshold
    
    async def dispatch(self, request: Request, call_next):
        start_time = time.time()
        
        response = await call_next(request)
        
        process_time = time.time() - start_time
        
        # Check for slow requests
        if process_time > self.slow_request_threshold:
            logfire.warn(
                "Slow request detected",
                method=request.method,
                path=request.url.path,
                process_time_ms=round(process_time * 1000, 2),
                threshold_ms=round(self.slow_request_threshold * 1000, 2),
                request_id=getattr(request.state, 'request_id', None)
            )
        
        # Add performance metrics
        response.headers["X-Response-Time"] = str(round(process_time * 1000, 2))
        
        return response

class LogfireUserActivityMiddleware(BaseHTTPMiddleware):
    """User activity tracking and behavior analysis."""
    
    async def dispatch(self, request: Request, call_next):
        response = await call_next(request)
        
        # Track user activity if user is authenticated
        user_id = getattr(request.state, 'user_id', None)
        if user_id and request.method in ["POST", "PUT", "PATCH", "DELETE"]:
            logfire.info(
                "User activity",
                user_id=str(user_id),
                action=f"{request.method} {request.url.path}",
                ip_address=request.client.host,
                user_agent=request.headers.get("user-agent"),
                timestamp=time.time()
            )
        
        return response
```

## Database Monitoring

### Database Query Tracking

```python
# api/monitoring/database_tracking.py
from sqlalchemy import event, text
from sqlalchemy.engine import Engine
from typing import Dict, Any, Optional
import logfire
import time
import uuid

class DatabaseTracker:
    """Comprehensive database operation tracking."""
    
    def __init__(self):
        self.active_queries: Dict[str, Dict[str, Any]] = {}
        self.slow_query_threshold = 1.0  # 1 second
    
    def setup_tracking(self, engine: Engine):
        """Set up database event listeners."""
        event.listen(engine, "before_cursor_execute", self.before_cursor_execute)
        event.listen(engine, "after_cursor_execute", self.after_cursor_execute)
        event.listen(engine, "handle_error", self.handle_error)
    
    def before_cursor_execute(self, conn, cursor, statement, parameters, context, executemany):
        """Track query start."""
        query_id = str(uuid.uuid4())
        context._logfire_query_id = query_id
        
        # Store query details
        self.active_queries[query_id] = {
            "statement": statement,
            "parameters": parameters,
            "start_time": time.time(),
            "connection_id": id(conn)
        }
        
        # Start Logfire span
        context._logfire_span = logfire.span(
            "Database Query",
            query_id=query_id,
            sql_operation=self.extract_operation(statement),
            connection_id=id(conn)
        ).__enter__()
    
    def after_cursor_execute(self, conn, cursor, statement, parameters, context, executemany):
        """Track query completion."""
        query_id = getattr(context, '_logfire_query_id', None)
        span = getattr(context, '_logfire_span', None)
        
        if query_id and query_id in self.active_queries:
            query_info = self.active_queries[query_id]
            duration = time.time() - query_info["start_time"]
            
            # Set span attributes
            if span:
                span.set_attribute("query.duration_ms", round(duration * 1000, 2))
                span.set_attribute("query.rows_affected", cursor.rowcount)
                span.__exit__(None, None, None)
            
            # Log query completion
            log_level = "warn" if duration > self.slow_query_threshold else "debug"
            getattr(logfire, log_level)(
                "Database query completed",
                query_id=query_id,
                operation=self.extract_operation(statement),
                duration_ms=round(duration * 1000, 2),
                rows_affected=cursor.rowcount,
                is_slow=duration > self.slow_query_threshold
            )
            
            # Clean up
            del self.active_queries[query_id]
    
    def handle_error(self, exception_context):
        """Handle database errors."""
        query_id = getattr(exception_context.context, '_logfire_query_id', None)
        span = getattr(exception_context.context, '_logfire_span', None)
        
        if span:
            span.set_attribute("error", True)
            span.set_attribute("error.message", str(exception_context.original_exception))
            span.__exit__(type(exception_context.original_exception), 
                         exception_context.original_exception, None)
        
        # Log database error
        logfire.error(
            "Database query failed",
            query_id=query_id,
            error_type=type(exception_context.original_exception).__name__,
            error_message=str(exception_context.original_exception),
            statement=exception_context.statement
        )
        
        # Clean up if query exists
        if query_id and query_id in self.active_queries:
            del self.active_queries[query_id]
    
    def extract_operation(self, statement: str) -> str:
        """Extract SQL operation type from statement."""
        statement = statement.strip().upper()
        if statement.startswith('SELECT'):
            return 'SELECT'
        elif statement.startswith('INSERT'):
            return 'INSERT'
        elif statement.startswith('UPDATE'):
            return 'UPDATE'
        elif statement.startswith('DELETE'):
            return 'DELETE'
        elif statement.startswith('CREATE'):
            return 'CREATE'
        elif statement.startswith('ALTER'):
            return 'ALTER'
        elif statement.startswith('DROP'):
            return 'DROP'
        else:
            return 'OTHER'

# Global tracker instance
db_tracker = DatabaseTracker()
```

### Database Health Monitoring

```python
# api/monitoring/database_health.py
import asyncio
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import text
import logfire
import time

class DatabaseHealthMonitor:
    """Monitor database health and performance."""
    
    def __init__(self, db_session: AsyncSession):
        self.db = db_session
        self.health_check_interval = 60  # 1 minute
        self.is_monitoring = False
    
    async def start_monitoring(self):
        """Start continuous database health monitoring."""
        self.is_monitoring = True
        while self.is_monitoring:
            await self.check_database_health()
            await asyncio.sleep(self.health_check_interval)
    
    def stop_monitoring(self):
        """Stop database health monitoring."""
        self.is_monitoring = False
    
    async def check_database_health(self) -> dict:
        """Perform comprehensive database health check."""
        health_data = {
            "timestamp": time.time(),
            "status": "unknown",
            "metrics": {}
        }
        
        try:
            # Basic connectivity test
            start_time = time.time()
            await self.db.execute(text("SELECT 1"))
            connectivity_time = time.time() - start_time
            
            # Connection pool metrics
            pool = self.db.bind.pool
            pool_metrics = {
                "size": pool.size(),
                "checked_out": pool.checkedout(),
                "checked_in": pool.checkedin(),
                "invalid": pool.invalid(),
                "overflow": pool.overflow()
            }
            
            # Query performance test
            start_time = time.time()
            result = await self.db.execute(text("SELECT COUNT(*) FROM pg_stat_activity"))
            active_connections = result.scalar()
            query_time = time.time() - start_time
            
            # Database size
            result = await self.db.execute(text(
                "SELECT pg_size_pretty(pg_database_size(current_database()))"
            ))
            db_size = result.scalar()
            
            health_data.update({
                "status": "healthy",
                "metrics": {
                    "connectivity_time_ms": round(connectivity_time * 1000, 2),
                    "query_time_ms": round(query_time * 1000, 2),
                    "active_connections": active_connections,
                    "database_size": db_size,
                    "pool": pool_metrics
                }
            })
            
            # Log health status
            logfire.info(
                "Database health check completed",
                status="healthy",
                connectivity_time_ms=round(connectivity_time * 1000, 2),
                query_time_ms=round(query_time * 1000, 2),
                active_connections=active_connections,
                pool_utilization=pool_metrics["checked_out"] / pool_metrics["size"] * 100
            )
            
        except Exception as e:
            health_data.update({
                "status": "unhealthy",
                "error": str(e)
            })
            
            logfire.error(
                "Database health check failed",
                error_type=type(e).__name__,
                error_message=str(e)
            )
        
        return health_data
```

## Redis Monitoring

### Redis Health and Metrics

```python
# api/monitoring/redis_monitoring.py
import redis.asyncio as redis
import logfire
import time
import asyncio

class RedisMonitor:
    """Monitor Redis health and performance."""
    
    def __init__(self, redis_client: redis.Redis):
        self.redis = redis_client
        self.health_check_interval = 60
        self.is_monitoring = False
    
    async def start_monitoring(self):
        """Start Redis health monitoring."""
        self.is_monitoring = True
        while self.is_monitoring:
            await self.check_redis_health()
            await asyncio.sleep(self.health_check_interval)
    
    def stop_monitoring(self):
        """Stop Redis monitoring."""
        self.is_monitoring = False
    
    async def check_redis_health(self) -> dict:
        """Comprehensive Redis health check."""
        health_data = {
            "timestamp": time.time(),
            "status": "unknown",
            "metrics": {}
        }
        
        try:
            # Basic connectivity test
            start_time = time.time()
            await self.redis.ping()
            ping_time = time.time() - start_time
            
            # Get Redis info
            info = await self.redis.info()
            
            # Memory metrics
            memory_metrics = {
                "used_memory": info.get("used_memory", 0),
                "used_memory_human": info.get("used_memory_human", "0B"),
                "used_memory_peak": info.get("used_memory_peak", 0),
                "maxmemory": info.get("maxmemory", 0),
                "memory_usage_percent": round(
                    (info.get("used_memory", 0) / max(info.get("maxmemory", 1), 1)) * 100, 2
                ) if info.get("maxmemory", 0) > 0 else 0
            }
            
            # Performance metrics
            stats = await self.redis.info("stats")
            performance_metrics = {
                "total_commands_processed": stats.get("total_commands_processed", 0),
                "keyspace_hits": stats.get("keyspace_hits", 0),
                "keyspace_misses": stats.get("keyspace_misses", 0),
                "hit_rate_percent": round(
                    (stats.get("keyspace_hits", 0) / 
                     max(stats.get("keyspace_hits", 0) + stats.get("keyspace_misses", 0), 1)) * 100, 2
                ),
                "connected_clients": info.get("connected_clients", 0)
            }
            
            health_data.update({
                "status": "healthy",
                "metrics": {
                    "ping_time_ms": round(ping_time * 1000, 2),
                    "memory": memory_metrics,
                    "performance": performance_metrics,
                    "uptime_seconds": info.get("uptime_in_seconds", 0)
                }
            })
            
            # Log health status
            logfire.info(
                "Redis health check completed",
                status="healthy",
                ping_time_ms=round(ping_time * 1000, 2),
                memory_usage_percent=memory_metrics["memory_usage_percent"],
                hit_rate_percent=performance_metrics["hit_rate_percent"],
                connected_clients=performance_metrics["connected_clients"]
            )
            
        except Exception as e:
            health_data.update({
                "status": "unhealthy",
                "error": str(e)
            })
            
            logfire.error(
                "Redis health check failed",
                error_type=type(e).__name__,
                error_message=str(e)
            )
        
        return health_data
```

## Error Tracking

### Exception Monitoring

```python
# api/monitoring/error_tracking.py
from fastapi import Request, HTTPException
from starlette.middleware.base import BaseHTTPMiddleware
import logfire
import traceback
from typing import Optional
import sys

class ErrorTrackingMiddleware(BaseHTTPMiddleware):
    """Comprehensive error tracking and reporting."""
    
    async def dispatch(self, request: Request, call_next):
        try:
            response = await call_next(request)
            return response
        except HTTPException as e:
            # Log HTTP exceptions
            self.log_http_exception(request, e)
            raise
        except Exception as e:
            # Log unexpected exceptions
            self.log_unexpected_exception(request, e)
            raise
    
    def log_http_exception(self, request: Request, exception: HTTPException):
        """Log HTTP exceptions with context."""
        logfire.warn(
            f"HTTP exception: {exception.status_code}",
            status_code=exception.status_code,
            detail=exception.detail,
            method=request.method,
            path=request.url.path,
            request_id=getattr(request.state, 'request_id', None),
            user_id=getattr(request.state, 'user_id', None),
            ip_address=request.client.host
        )
    
    def log_unexpected_exception(self, request: Request, exception: Exception):
        """Log unexpected exceptions with full context."""
        # Get traceback information
        exc_type, exc_value, exc_traceback = sys.exc_info()
        tb_lines = traceback.format_exception(exc_type, exc_value, exc_traceback)
        
        logfire.error(
            f"Unexpected exception: {type(exception).__name__}",
            error_type=type(exception).__name__,
            error_message=str(exception),
            method=request.method,
            path=request.url.path,
            request_id=getattr(request.state, 'request_id', None),
            user_id=getattr(request.state, 'user_id', None),
            ip_address=request.client.host,
            traceback=tb_lines,
            exc_info=exception
        )

class ErrorAnalyzer:
    """Analyze and categorize errors for insights."""
    
    def __init__(self, redis_client):
        self.redis = redis_client
    
    async def track_error(self, error_type: str, details: dict):
        """Track error for analysis."""
        timestamp = int(time.time())
        
        # Increment error counter
        counter_key = f"errors:{error_type}:hour:{timestamp // 3600}"
        await self.redis.incr(counter_key)
        await self.redis.expire(counter_key, 86400)  # 24 hours
        
        # Store error details
        error_key = f"error_details:{error_type}:{timestamp}"
        await self.redis.setex(error_key, 3600, json.dumps(details))  # 1 hour
    
    async def get_error_summary(self, hours: int = 24) -> dict:
        """Get error summary for the specified period."""
        current_hour = int(time.time()) // 3600
        error_counts = {}
        
        for hour_offset in range(hours):
            hour = current_hour - hour_offset
            pattern = f"errors:*:hour:{hour}"
            
            for key in await self.redis.keys(pattern):
                error_type = key.split(':')[1]
                count = await self.redis.get(key)
                
                if error_type not in error_counts:
                    error_counts[error_type] = 0
                error_counts[error_type] += int(count) if count else 0
        
        return error_counts
```

## Metrics Collection

### Application Metrics

```python
# api/monitoring/metrics.py
from prometheus_client import Counter, Histogram, Gauge, generate_latest
import time
from typing import Dict, Any
import logfire

# Prometheus metrics
REQUEST_COUNT = Counter('http_requests_total', 'Total HTTP requests', ['method', 'endpoint', 'status'])
REQUEST_DURATION = Histogram('http_request_duration_seconds', 'HTTP request duration')
ACTIVE_CONNECTIONS = Gauge('active_connections', 'Active database connections')
REDIS_OPERATIONS = Counter('redis_operations_total', 'Total Redis operations', ['operation', 'status'])

class MetricsCollector:
    """Collect and expose application metrics."""
    
    def __init__(self):
        self.custom_metrics: Dict[str, Any] = {}
        self.start_time = time.time()
    
    def record_request(self, method: str, endpoint: str, status_code: int, duration: float):
        """Record HTTP request metrics."""
        REQUEST_COUNT.labels(method=method, endpoint=endpoint, status=str(status_code)).inc()
        REQUEST_DURATION.observe(duration)
        
        # Also log to Logfire
        logfire.info(
            "Request metrics",
            method=method,
            endpoint=endpoint,
            status_code=status_code,
            duration_ms=round(duration * 1000, 2)
        )
    
    def update_connection_count(self, count: int):
        """Update active database connections metric."""
        ACTIVE_CONNECTIONS.set(count)
    
    def record_redis_operation(self, operation: str, success: bool):
        """Record Redis operation metrics."""
        status = "success" if success else "error"
        REDIS_OPERATIONS.labels(operation=operation, status=status).inc()
    
    def get_system_metrics(self) -> dict:
        """Get current system metrics."""
        uptime = time.time() - self.start_time
        
        return {
            "uptime_seconds": uptime,
            "uptime_human": self.format_uptime(uptime),
            "custom_metrics": self.custom_metrics
        }
    
    def format_uptime(self, seconds: float) -> str:
        """Format uptime in human-readable format."""
        days = int(seconds // 86400)
        hours = int((seconds % 86400) // 3600)
        minutes = int((seconds % 3600) // 60)
        
        parts = []
        if days > 0:
            parts.append(f"{days}d")
        if hours > 0:
            parts.append(f"{hours}h")
        if minutes > 0:
            parts.append(f"{minutes}m")
        
        return " ".join(parts) if parts else "< 1m"
    
    def export_prometheus_metrics(self) -> str:
        """Export metrics in Prometheus format."""
        return generate_latest()

# Global metrics collector
metrics_collector = MetricsCollector()

class MetricsMiddleware(BaseHTTPMiddleware):
    """Middleware to collect request metrics."""
    
    async def dispatch(self, request: Request, call_next):
        start_time = time.time()
        
        response = await call_next(request)
        
        duration = time.time() - start_time
        
        # Record metrics
        metrics_collector.record_request(
            method=request.method,
            endpoint=request.url.path,
            status_code=response.status_code,
            duration=duration
        )
        
        return response
```

## Health Check Monitoring

### Comprehensive Health Checks

```python
# api/monitoring/health.py
from typing import Dict, Any, List
import asyncio
import logfire
import time

class HealthChecker:
    """Comprehensive system health monitoring."""
    
    def __init__(self, db_monitor, redis_monitor):
        self.db_monitor = db_monitor
        self.redis_monitor = redis_monitor
        self.health_status = {}
        self.last_check = None
    
    async def check_all_components(self) -> dict:
        """Check health of all system components."""
        start_time = time.time()
        
        health_checks = {
            "api": self.check_api_health(),
            "database": self.db_monitor.check_database_health(),
            "redis": self.redis_monitor.check_redis_health()
        }
        
        # Run all health checks concurrently
        results = await asyncio.gather(*health_checks.values(), return_exceptions=True)
        
        # Process results
        health_status = {}
        for component, result in zip(health_checks.keys(), results):
            if isinstance(result, Exception):
                health_status[component] = {
                    "status": "error",
                    "error": str(result)
                }
            else:
                health_status[component] = result
        
        # Calculate overall status
        overall_status = "healthy"
        unhealthy_components = []
        
        for component, status in health_status.items():
            if status.get("status") != "healthy":
                overall_status = "unhealthy"
                unhealthy_components.append(component)
        
        check_duration = time.time() - start_time
        
        response = {
            "status": overall_status,
            "timestamp": time.time(),
            "check_duration_ms": round(check_duration * 1000, 2),
            "components": health_status,
            "unhealthy_components": unhealthy_components
        }
        
        # Cache results
        self.health_status = response
        self.last_check = time.time()
        
        # Log health status
        log_level = "error" if overall_status != "healthy" else "info"
        getattr(logfire, log_level)(
            f"Health check completed: {overall_status}",
            overall_status=overall_status,
            check_duration_ms=round(check_duration * 1000, 2),
            unhealthy_components=unhealthy_components
        )
        
        return response
    
    async def check_api_health(self) -> dict:
        """Check API service health."""
        try:
            # Check basic API functionality
            health_data = {
                "status": "healthy",
                "uptime": metrics_collector.get_system_metrics()["uptime_seconds"],
                "version": "1.0.0"
            }
            
            return health_data
            
        except Exception as e:
            return {
                "status": "unhealthy",
                "error": str(e)
            }
    
    def get_cached_health(self) -> dict:
        """Get cached health status."""
        if self.last_check and time.time() - self.last_check < 30:  # 30 second cache
            return self.health_status
        
        # Return basic status if no recent check
        return {
            "status": "unknown",
            "message": "Health check not recently performed"
        }
```

## Monitoring Dashboards

### Logfire Dashboard Configuration

```python
# api/monitoring/dashboard_config.py
import logfire

def setup_dashboards():
    """Configure monitoring dashboards in Logfire."""
    
    # Request Performance Dashboard
    logfire.info(
        "Dashboard configuration: Request Performance",
        dashboard_type="request_performance",
        metrics=[
            "http_request_duration_seconds",
            "http_requests_total",
            "response_time_percentiles"
        ]
    )
    
    # Database Performance Dashboard
    logfire.info(
        "Dashboard configuration: Database Performance",
        dashboard_type="database_performance",
        metrics=[
            "database_query_duration",
            "database_connection_pool_usage",
            "slow_query_count"
        ]
    )
    
    # Security Monitoring Dashboard
    logfire.info(
        "Dashboard configuration: Security Monitoring",
        dashboard_type="security_monitoring",
        metrics=[
            "security_events_total",
            "rate_limit_violations",
            "authentication_failures"
        ]
    )
    
    # System Health Dashboard
    logfire.info(
        "Dashboard configuration: System Health",
        dashboard_type="system_health",
        metrics=[
            "component_health_status",
            "uptime_percentage",
            "error_rate"
        ]
    )
```

## Alerting Configuration

### Alert Rules

```python
# api/monitoring/alerts.py
import logfire
from typing import Dict, Any, Callable
import asyncio

class AlertManager:
    """Manage monitoring alerts and notifications."""
    
    def __init__(self):
        self.alert_rules = {
            "high_error_rate": {
                "condition": lambda metrics: metrics.get("error_rate", 0) > 5.0,
                "severity": "high",
                "message": "Error rate exceeds 5%"
            },
            "slow_database_queries": {
                "condition": lambda metrics: metrics.get("avg_query_time", 0) > 1000,
                "severity": "medium",
                "message": "Average database query time exceeds 1 second"
            },
            "high_memory_usage": {
                "condition": lambda metrics: metrics.get("memory_usage_percent", 0) > 90,
                "severity": "high",
                "message": "Memory usage exceeds 90%"
            },
            "redis_connection_issues": {
                "condition": lambda metrics: metrics.get("redis_status", "") != "healthy",
                "severity": "high",
                "message": "Redis connection issues detected"
            }
        }
        self.active_alerts = set()
    
    async def check_alerts(self, metrics: Dict[str, Any]):
        """Check all alert conditions."""
        for alert_name, rule in self.alert_rules.items():
            condition_met = rule["condition"](metrics)
            
            if condition_met and alert_name not in self.active_alerts:
                # New alert triggered
                await self.trigger_alert(alert_name, rule, metrics)
                self.active_alerts.add(alert_name)
            elif not condition_met and alert_name in self.active_alerts:
                # Alert resolved
                await self.resolve_alert(alert_name, rule)
                self.active_alerts.remove(alert_name)
    
    async def trigger_alert(self, alert_name: str, rule: Dict[str, Any], metrics: Dict[str, Any]):
        """Trigger an alert."""
        logfire.error(
            f"ALERT TRIGGERED: {alert_name}",
            alert_name=alert_name,
            severity=rule["severity"],
            message=rule["message"],
            metrics=metrics,
            timestamp=time.time()
        )
        
        # Send notifications (email, Slack, etc.)
        await self.send_notifications(alert_name, rule, metrics)
    
    async def resolve_alert(self, alert_name: str, rule: Dict[str, Any]):
        """Resolve an alert."""
        logfire.info(
            f"ALERT RESOLVED: {alert_name}",
            alert_name=alert_name,
            severity=rule["severity"],
            message=f"Alert {alert_name} has been resolved",
            timestamp=time.time()
        )
    
    async def send_notifications(self, alert_name: str, rule: Dict[str, Any], metrics: Dict[str, Any]):
        """Send alert notifications."""
        # Implement notification channels
        # - Email notifications
        # - Slack/Discord webhooks
        # - PagerDuty integration
        # - SMS notifications
        pass
```

## Monitoring Best Practices

### Performance Optimization

1. **Sampling**: Use sampling for high-volume traces
2. **Async Processing**: Process monitoring data asynchronously
3. **Batching**: Batch metrics for efficient transmission
4. **Caching**: Cache health check results

### Data Retention

```python
# api/monitoring/retention.py
import asyncio
from datetime import datetime, timedelta

class DataRetentionManager:
    """Manage monitoring data retention."""
    
    def __init__(self, redis_client):
        self.redis = redis_client
        self.retention_policies = {
            "metrics": timedelta(days=30),
            "logs": timedelta(days=90),
            "traces": timedelta(days=7),
            "alerts": timedelta(days=365)
        }
    
    async def cleanup_expired_data(self):
        """Clean up expired monitoring data."""
        current_time = datetime.utcnow()
        
        for data_type, retention_period in self.retention_policies.items():
            cutoff_time = current_time - retention_period
            cutoff_timestamp = int(cutoff_time.timestamp())
            
            # Clean up Redis keys
            pattern = f"{data_type}:*"
            keys = await self.redis.keys(pattern)
            
            for key in keys:
                # Extract timestamp from key
                try:
                    key_timestamp = int(key.split(':')[-1])
                    if key_timestamp < cutoff_timestamp:
                        await self.redis.delete(key)
                except (ValueError, IndexError):
                    # Skip keys without timestamp
                    continue
            
            logfire.info(
                f"Data cleanup completed for {data_type}",
                data_type=data_type,
                retention_days=retention_period.days,
                cleaned_keys=len([k for k in keys if self._is_expired(k, cutoff_timestamp)])
            )
    
    def _is_expired(self, key: str, cutoff_timestamp: int) -> bool:
        """Check if a key is expired."""
        try:
            key_timestamp = int(key.split(':')[-1])
            return key_timestamp < cutoff_timestamp
        except (ValueError, IndexError):
            return False
```

## Troubleshooting

### Common Monitoring Issues

1. **High Memory Usage**: Implement sampling and data retention
2. **Slow Queries**: Enable query optimization recommendations
3. **Missing Traces**: Check instrumentation configuration
4. **Alert Fatigue**: Tune alert thresholds and implement alert routing

### Debug Mode

```python
# Enable verbose monitoring for debugging
if settings.debug:
    logfire.configure(
        service_name="build-api-debug",
        console=True,
        send_to_logfire=False,
        pydantic_plugin_config={"record": "all"}
    )
```

This comprehensive monitoring setup provides complete observability for the Build Platform, enabling proactive issue detection, performance optimization, and security monitoring.