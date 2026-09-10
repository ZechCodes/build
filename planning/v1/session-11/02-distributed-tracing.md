# Session 11.2: Distributed Tracing & Performance Analysis

## Objective
Implement comprehensive distributed tracing system to track requests across all platform components, enabling detailed performance analysis, debugging capabilities, and end-to-end observability for complex operations.

## Integration with Previous Sessions
- **Session 1**: Enhances Logfire integration with distributed tracing capabilities
- **Session 2**: Traces authentication flows and user session management
- **Session 3**: Traces VM lifecycle operations and resource management
- **Session 6**: Traces terminal session operations and WebSocket communications
- **Session 9**: Traces Git operations and repository management flows
- **Session 10**: Traces recording operations and playback sessions
- **Session 11.1**: Integrates with metrics collection for trace-based metrics

## Core Implementation

### Distributed Tracing Engine
**Location**: `monitoring/tracing/tracer.py`

```python
# monitoring/tracing/tracer.py
import asyncio
import time
import uuid
import json
import threading
from typing import Dict, Any, Optional, List, AsyncContextManager, Callable, Union
from dataclasses import dataclass, field, asdict
from contextlib import asynccontextmanager, contextmanager
from enum import Enum
import structlog
import logfire
from contextvars import ContextVar
from concurrent.futures import ThreadPoolExecutor

logger = structlog.get_logger()

class SpanKind(Enum):
    SERVER = "server"
    CLIENT = "client"
    PRODUCER = "producer"
    CONSUMER = "consumer"
    INTERNAL = "internal"

class SpanStatus(Enum):
    OK = "ok"
    ERROR = "error"
    TIMEOUT = "timeout"
    CANCELLED = "cancelled"

@dataclass
class TraceContext:
    trace_id: str
    span_id: str
    parent_span_id: Optional[str] = None
    operation_name: str = ""
    service_name: str = ""
    span_kind: SpanKind = SpanKind.INTERNAL
    start_time: float = field(default_factory=time.time)
    end_time: Optional[float] = None
    duration_ms: Optional[float] = None
    status: SpanStatus = SpanStatus.OK
    tags: Dict[str, Any] = field(default_factory=dict)
    logs: List[Dict[str, Any]] = field(default_factory=list)
    baggage: Dict[str, str] = field(default_factory=dict)
    error: Optional[str] = None
    error_details: Optional[Dict[str, Any]] = None
    resource_usage: Dict[str, Any] = field(default_factory=dict)

@dataclass
class TraceMetrics:
    total_spans: int = 0
    active_spans: int = 0
    completed_spans: int = 0
    error_spans: int = 0
    avg_duration_ms: float = 0.0
    p50_duration_ms: float = 0.0
    p95_duration_ms: float = 0.0
    p99_duration_ms: float = 0.0
    throughput_spans_per_second: float = 0.0

@dataclass
class SamplingRule:
    operation_pattern: str
    service_pattern: str
    sample_rate: float  # 0.0 to 1.0
    max_traces_per_second: Optional[int] = None
    priority: int = 0

class TraceExporter:
    """Base class for trace exporters"""
    async def export_trace(self, trace: TraceContext) -> bool:
        raise NotImplementedError
    
    async def export_batch(self, traces: List[TraceContext]) -> bool:
        raise NotImplementedError

class LogfireTraceExporter(TraceExporter):
    """Export traces to Logfire"""
    
    async def export_trace(self, trace: TraceContext) -> bool:
        try:
            # Convert trace to Logfire span format
            with logfire.span(
                trace.operation_name,
                trace_id=trace.trace_id,
                span_id=trace.span_id,
                parent_span_id=trace.parent_span_id,
                **trace.tags
            ) as span:
                span.set_attribute("service.name", trace.service_name)
                span.set_attribute("span.kind", trace.span_kind.value)
                span.set_attribute("duration_ms", trace.duration_ms or 0)
                
                if trace.status == SpanStatus.ERROR:
                    span.set_attribute("error", True)
                    if trace.error:
                        span.set_attribute("error.message", trace.error)
                    if trace.error_details:
                        span.set_attribute("error.details", json.dumps(trace.error_details))
                
                # Add logs as events
                for log_entry in trace.logs:
                    span.add_event(
                        log_entry.get("message", ""),
                        timestamp=log_entry.get("timestamp"),
                        attributes=log_entry.get("fields", {})
                    )
                
                # Add resource usage if available
                if trace.resource_usage:
                    for key, value in trace.resource_usage.items():
                        span.set_attribute(f"resource.{key}", value)
            
            return True
        except Exception as e:
            logger.error("Failed to export trace to Logfire", trace_id=trace.trace_id, error=str(e))
            return False
    
    async def export_batch(self, traces: List[TraceContext]) -> bool:
        success_count = 0
        for trace in traces:
            if await self.export_trace(trace):
                success_count += 1
        
        return success_count == len(traces)

# Context variable for current trace context
current_trace_context: ContextVar[Optional[TraceContext]] = ContextVar('current_trace_context', default=None)

class DistributedTracer:
    def __init__(self, service_name: str, sampling_rules: List[SamplingRule] = None):
        self.service_name = service_name
        self.sampling_rules = sampling_rules or []
        
        # Trace storage
        self.active_traces: Dict[str, TraceContext] = {}
        self.completed_traces: List[TraceContext] = []
        self.max_completed_traces = 10000
        
        # Exporters
        self.exporters: List[TraceExporter] = [LogfireTraceExporter()]
        
        # Metrics
        self.metrics = TraceMetrics()
        self.duration_samples: List[float] = []
        self.max_duration_samples = 1000
        
        # Configuration
        self.max_spans_per_trace = 1000
        self.max_trace_duration_seconds = 3600  # 1 hour
        self.export_batch_size = 100
        self.export_interval_seconds = 10
        
        # Background tasks
        self.export_task: Optional[asyncio.Task] = None
        self.cleanup_task: Optional[asyncio.Task] = None
        self.metrics_task: Optional[asyncio.Task] = None
        
        # Thread safety
        self.lock = threading.Lock()
        
        # Start background tasks
        self.start_background_tasks()

    def start_background_tasks(self):
        """Start background processing tasks"""
        self.export_task = asyncio.create_task(self._export_loop())
        self.cleanup_task = asyncio.create_task(self._cleanup_loop())
        self.metrics_task = asyncio.create_task(self._metrics_loop())

    async def stop_background_tasks(self):
        """Stop background processing tasks"""
        for task in [self.export_task, self.cleanup_task, self.metrics_task]:
            if task and not task.done():
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass

    @asynccontextmanager
    async def trace_operation(self, operation_name: str, 
                            span_kind: SpanKind = SpanKind.INTERNAL,
                            parent_trace_id: Optional[str] = None,
                            **tags) -> AsyncContextManager[TraceContext]:
        """Create a traced operation context"""
        
        # Get parent context
        parent_context = current_trace_context.get()
        if parent_trace_id:
            parent_span_id = parent_trace_id
            trace_id = parent_trace_id
        elif parent_context:
            parent_span_id = parent_context.span_id
            trace_id = parent_context.trace_id
        else:
            parent_span_id = None
            trace_id = self._generate_trace_id()
        
        # Check sampling
        if not self._should_sample(operation_name, self.service_name):
            # Create a no-op context for non-sampled traces
            trace_context = TraceContext(
                trace_id=trace_id,
                span_id=self._generate_span_id(),
                parent_span_id=parent_span_id,
                operation_name=operation_name,
                service_name=self.service_name,
                span_kind=span_kind,
                tags=tags
            )
            yield trace_context
            return
        
        span_id = self._generate_span_id()
        
        trace_context = TraceContext(
            trace_id=trace_id,
            span_id=span_id,
            parent_span_id=parent_span_id,
            operation_name=operation_name,
            service_name=self.service_name,
            span_kind=span_kind,
            tags=tags
        )
        
        # Store in active traces
        with self.lock:
            self.active_traces[span_id] = trace_context
            self.metrics.active_spans += 1
            self.metrics.total_spans += 1
        
        # Set context variable
        token = current_trace_context.set(trace_context)
        
        try:
            # Start resource monitoring if enabled
            await self._start_resource_monitoring(trace_context)
            
            yield trace_context
            
            # Mark as successful
            trace_context.end_time = time.time()
            trace_context.duration_ms = (trace_context.end_time - trace_context.start_time) * 1000
            trace_context.status = SpanStatus.OK
            
        except asyncio.CancelledError:
            trace_context.end_time = time.time()
            trace_context.duration_ms = (trace_context.end_time - trace_context.start_time) * 1000
            trace_context.status = SpanStatus.CANCELLED
            trace_context.error = "Operation cancelled"
            raise
            
        except Exception as e:
            # Mark as failed
            trace_context.end_time = time.time()
            trace_context.duration_ms = (trace_context.end_time - trace_context.start_time) * 1000
            trace_context.status = SpanStatus.ERROR
            trace_context.error = str(e)
            trace_context.error_details = {
                "exception_type": type(e).__name__,
                "exception_message": str(e),
                "traceback": self._get_traceback_summary()
            }
            raise
            
        finally:
            # Stop resource monitoring
            await self._stop_resource_monitoring(trace_context)
            
            # Reset context variable
            current_trace_context.reset(token)
            
            # Move to completed traces
            with self.lock:
                self.active_traces.pop(span_id, None)
                self.metrics.active_spans -= 1
                self.metrics.completed_spans += 1
                
                if trace_context.status == SpanStatus.ERROR:
                    self.metrics.error_spans += 1
                
                # Add to completed traces
                self.completed_traces.append(trace_context)
                
                # Add duration sample for metrics
                if trace_context.duration_ms:
                    self.duration_samples.append(trace_context.duration_ms)
                    if len(self.duration_samples) > self.max_duration_samples:
                        self.duration_samples = self.duration_samples[-self.max_duration_samples:]
                
                # Cleanup old traces
                if len(self.completed_traces) > self.max_completed_traces:
                    self.completed_traces = self.completed_traces[-self.max_completed_traces:]

    def add_trace_tag(self, key: str, value: Any):
        """Add tag to current trace"""
        context = current_trace_context.get()
        if context:
            context.tags[key] = value

    def add_trace_log(self, level: str, message: str, **fields):
        """Add log entry to current trace"""
        context = current_trace_context.get()
        if context:
            context.logs.append({
                "timestamp": time.time(),
                "level": level,
                "message": message,
                "fields": fields
            })

    def set_trace_baggage(self, key: str, value: str):
        """Set baggage item for current trace"""
        context = current_trace_context.get()
        if context:
            context.baggage[key] = value

    def get_trace_baggage(self, key: str) -> Optional[str]:
        """Get baggage item from current trace"""
        context = current_trace_context.get()
        if context:
            return context.baggage.get(key)
        return None

    def get_current_trace_id(self) -> Optional[str]:
        """Get current trace ID"""
        context = current_trace_context.get()
        return context.trace_id if context else None

    def get_current_span_id(self) -> Optional[str]:
        """Get current span ID"""
        context = current_trace_context.get()
        return context.span_id if context else None

    async def _start_resource_monitoring(self, trace_context: TraceContext):
        """Start monitoring resource usage for trace"""
        try:
            import psutil
            process = psutil.Process()
            trace_context.resource_usage["start_cpu_percent"] = process.cpu_percent()
            trace_context.resource_usage["start_memory_mb"] = process.memory_info().rss / 1024 / 1024
        except Exception:
            # Resource monitoring is optional
            pass

    async def _stop_resource_monitoring(self, trace_context: TraceContext):
        """Stop monitoring resource usage for trace"""
        try:
            import psutil
            process = psutil.Process()
            start_cpu = trace_context.resource_usage.get("start_cpu_percent", 0)
            start_memory = trace_context.resource_usage.get("start_memory_mb", 0)
            
            end_cpu = process.cpu_percent()
            end_memory = process.memory_info().rss / 1024 / 1024
            
            trace_context.resource_usage.update({
                "end_cpu_percent": end_cpu,
                "end_memory_mb": end_memory,
                "cpu_delta": end_cpu - start_cpu,
                "memory_delta_mb": end_memory - start_memory
            })
        except Exception:
            # Resource monitoring is optional
            pass

    def _should_sample(self, operation_name: str, service_name: str) -> bool:
        """Determine if trace should be sampled"""
        import re
        import random
        
        # Find matching sampling rule
        matching_rule = None
        for rule in sorted(self.sampling_rules, key=lambda r: r.priority, reverse=True):
            if (re.match(rule.operation_pattern, operation_name) and 
                re.match(rule.service_pattern, service_name)):
                matching_rule = rule
                break
        
        if not matching_rule:
            # Default sampling rate of 100% if no rules match
            return True
        
        # Apply sampling rate
        return random.random() < matching_rule.sample_rate

    async def _export_loop(self):
        """Background task to export completed traces"""
        while True:
            try:
                await asyncio.sleep(self.export_interval_seconds)
                
                # Get batch of traces to export
                with self.lock:
                    if not self.completed_traces:
                        continue
                    
                    batch_size = min(self.export_batch_size, len(self.completed_traces))
                    batch = self.completed_traces[:batch_size]
                    self.completed_traces = self.completed_traces[batch_size:]
                
                # Export batch to all exporters
                for exporter in self.exporters:
                    try:
                        await exporter.export_batch(batch)
                    except Exception as e:
                        logger.error("Trace export failed", exporter=type(exporter).__name__, error=str(e))
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Trace export loop error", error=str(e))

    async def _cleanup_loop(self):
        """Background task to cleanup old traces"""
        while True:
            try:
                await asyncio.sleep(300)  # Run every 5 minutes
                
                current_time = time.time()
                cutoff_time = current_time - self.max_trace_duration_seconds
                
                # Cleanup old active traces
                with self.lock:
                    expired_spans = [
                        span_id for span_id, trace in self.active_traces.items()
                        if trace.start_time < cutoff_time
                    ]
                    
                    for span_id in expired_spans:
                        trace = self.active_traces.pop(span_id)
                        trace.end_time = current_time
                        trace.duration_ms = (trace.end_time - trace.start_time) * 1000
                        trace.status = SpanStatus.TIMEOUT
                        trace.error = "Trace timeout - exceeded maximum duration"
                        
                        self.completed_traces.append(trace)
                        self.metrics.active_spans -= 1
                        self.metrics.completed_spans += 1
                        self.metrics.error_spans += 1
                
                if expired_spans:
                    logger.warning("Cleaned up expired traces", count=len(expired_spans))
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Trace cleanup loop error", error=str(e))

    async def _metrics_loop(self):
        """Background task to update trace metrics"""
        while True:
            try:
                await asyncio.sleep(60)  # Update every minute
                
                with self.lock:
                    if self.duration_samples:
                        # Calculate duration percentiles
                        sorted_durations = sorted(self.duration_samples)
                        count = len(sorted_durations)
                        
                        self.metrics.avg_duration_ms = sum(sorted_durations) / count
                        self.metrics.p50_duration_ms = sorted_durations[int(count * 0.5)]
                        self.metrics.p95_duration_ms = sorted_durations[int(count * 0.95)]
                        self.metrics.p99_duration_ms = sorted_durations[int(count * 0.99)]
                    
                    # Calculate throughput (spans per second over last minute)
                    self.metrics.throughput_spans_per_second = self.metrics.completed_spans / 60.0
                
                # Log metrics summary
                logfire.info("Trace metrics updated",
                           active_spans=self.metrics.active_spans,
                           completed_spans=self.metrics.completed_spans,
                           error_spans=self.metrics.error_spans,
                           avg_duration_ms=self.metrics.avg_duration_ms,
                           throughput_sps=self.metrics.throughput_spans_per_second)
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Trace metrics loop error", error=str(e))

    def get_trace_summary(self, hours: int = 1) -> Dict[str, Any]:
        """Get trace summary for the last N hours"""
        cutoff_time = time.time() - (hours * 3600)
        
        with self.lock:
            recent_traces = [
                trace for trace in self.completed_traces
                if trace.start_time >= cutoff_time
            ]
        
        if not recent_traces:
            return {
                "total_traces": 0,
                "successful_traces": 0,
                "error_traces": 0,
                "success_rate": 1.0,
                "average_duration_ms": 0,
                "operations": {}
            }
        
        total_traces = len(recent_traces)
        successful_traces = len([t for t in recent_traces if t.status == SpanStatus.OK])
        error_traces = total_traces - successful_traces
        
        avg_duration = sum(t.duration_ms or 0 for t in recent_traces) / total_traces
        
        # Group by operation
        operations = {}
        for trace in recent_traces:
            op_name = trace.operation_name
            if op_name not in operations:
                operations[op_name] = {
                    "count": 0,
                    "errors": 0,
                    "avg_duration_ms": 0,
                    "total_duration_ms": 0
                }
            
            operations[op_name]["count"] += 1
            operations[op_name]["total_duration_ms"] += trace.duration_ms or 0
            
            if trace.status == SpanStatus.ERROR:
                operations[op_name]["errors"] += 1
        
        # Calculate averages
        for op_stats in operations.values():
            if op_stats["count"] > 0:
                op_stats["avg_duration_ms"] = op_stats["total_duration_ms"] / op_stats["count"]
            del op_stats["total_duration_ms"]  # Remove intermediate field
        
        return {
            "total_traces": total_traces,
            "successful_traces": successful_traces,
            "error_traces": error_traces,
            "success_rate": successful_traces / total_traces,
            "average_duration_ms": avg_duration,
            "operations": operations,
            "metrics": asdict(self.metrics)
        }

    def add_exporter(self, exporter: TraceExporter):
        """Add a trace exporter"""
        self.exporters.append(exporter)

    def add_sampling_rule(self, rule: SamplingRule):
        """Add a sampling rule"""
        self.sampling_rules.append(rule)
        # Sort by priority
        self.sampling_rules.sort(key=lambda r: r.priority, reverse=True)

    def _generate_trace_id(self) -> str:
        """Generate a unique trace ID"""
        return str(uuid.uuid4())

    def _generate_span_id(self) -> str:
        """Generate a unique span ID"""
        return str(uuid.uuid4())

    def _get_traceback_summary(self) -> str:
        """Get a summary of the current traceback"""
        import traceback
        return ''.join(traceback.format_tb(traceback.exc_info()[2]))

    async def shutdown(self):
        """Shutdown the tracer"""
        await self.stop_background_tasks()
        
        # Export remaining traces
        with self.lock:
            if self.completed_traces:
                for exporter in self.exporters:
                    try:
                        await exporter.export_batch(self.completed_traces)
                    except Exception as e:
                        logger.error("Final trace export failed", error=str(e))
        
        logger.info("Distributed tracer shutdown complete")


# Global tracer instance
_global_tracer: Optional[DistributedTracer] = None

def get_tracer() -> Optional[DistributedTracer]:
    """Get the global tracer instance"""
    return _global_tracer

def initialize_tracer(service_name: str, sampling_rules: List[SamplingRule] = None):
    """Initialize the global tracer"""
    global _global_tracer
    _global_tracer = DistributedTracer(service_name, sampling_rules)
    return _global_tracer

# Convenience functions
async def trace_operation(operation_name: str, **kwargs):
    """Convenience function for tracing operations"""
    tracer = get_tracer()
    if tracer:
        return tracer.trace_operation(operation_name, **kwargs)
    else:
        # Return a no-op context manager if no tracer
        @asynccontextmanager
        async def noop_context():
            yield None
        return noop_context()

def add_trace_tag(key: str, value: Any):
    """Add tag to current trace"""
    tracer = get_tracer()
    if tracer:
        tracer.add_trace_tag(key, value)

def add_trace_log(level: str, message: str, **fields):
    """Add log to current trace"""
    tracer = get_tracer()
    if tracer:
        tracer.add_trace_log(level, message, **fields)

def get_current_trace_id() -> Optional[str]:
    """Get current trace ID"""
    tracer = get_tracer()
    return tracer.get_current_trace_id() if tracer else None
```

### HTTP Request Tracing Middleware
**Location**: `monitoring/tracing/middleware.py`

```python
# monitoring/tracing/middleware.py
import time
from typing import Callable, Awaitable
from fastapi import Request, Response
from fastapi.middleware.base import BaseHTTPMiddleware
import structlog
from .tracer import get_tracer, SpanKind

logger = structlog.get_logger()

class TracingMiddleware(BaseHTTPMiddleware):
    def __init__(self, app, service_name: str = "api"):
        super().__init__(app)
        self.service_name = service_name

    async def dispatch(self, request: Request, call_next: Callable[[Request], Awaitable[Response]]) -> Response:
        tracer = get_tracer()
        if not tracer:
            return await call_next(request)
        
        # Extract trace information from headers
        trace_id = request.headers.get("X-Trace-Id")
        parent_span_id = request.headers.get("X-Parent-Span-Id")
        
        # Create operation name
        operation_name = f"{request.method} {request.url.path}"
        
        # Start trace
        async with tracer.trace_operation(
            operation_name=operation_name,
            span_kind=SpanKind.SERVER,
            parent_trace_id=trace_id,
            **{
                "http.method": request.method,
                "http.url": str(request.url),
                "http.scheme": request.url.scheme,
                "http.host": request.url.hostname,
                "http.user_agent": request.headers.get("user-agent", ""),
                "http.client_ip": request.client.host if request.client else "",
            }
        ) as trace_context:
            try:
                # Call next middleware/handler
                response = await call_next(request)
                
                # Add response information
                trace_context.tags.update({
                    "http.status_code": response.status_code,
                    "http.response_size": len(response.body) if hasattr(response, 'body') else 0
                })
                
                # Add trace headers to response
                response.headers["X-Trace-Id"] = trace_context.trace_id
                response.headers["X-Span-Id"] = trace_context.span_id
                
                return response
                
            except Exception as e:
                # Add error information
                trace_context.tags.update({
                    "error": True,
                    "error.type": type(e).__name__,
                    "error.message": str(e)
                })
                raise
```

## TDD Implementation Cycle

### Red Phase: Distributed Tracing Test Creation
```python
# monitoring/tests/test_distributed_tracer.py
import pytest
import asyncio
from monitoring.tracing.tracer import DistributedTracer, SpanKind, SpanStatus

@pytest.mark.asyncio
async def test_tracer_initialization():
    """Test tracer initializes with correct service name"""
    # This test should initially fail (Red phase)
    tracer = DistributedTracer("test-service")
    assert False, "Distributed tracer initialization not implemented yet"

@pytest.mark.asyncio
async def test_trace_operation_context():
    """Test trace operation context management"""
    # This test should initially fail (Red phase)
    assert False, "Trace operation context not implemented yet"

@pytest.mark.asyncio
async def test_trace_sampling():
    """Test trace sampling based on rules"""
    # This test should initially fail (Red phase)
    assert False, "Trace sampling not implemented yet"
```

### Green Phase: Distributed Tracing Implementation
```python
# Implement distributed tracing features to make tests pass
# This involves adding trace context management, span creation, and export logic
```

### Refactor Phase: Distributed Tracing Optimization
```python
# Optimize distributed tracing for performance and reliability
# Add advanced sampling strategies and export optimizations
# Enhance trace correlation and analysis capabilities
```

## Security Checklist ✅

### Trace Data Security
- [ ] Sensitive data exclusion from traces (no PII, passwords, or secrets)
- [ ] Trace data encryption in transit and at rest
- [ ] Access controls for trace data viewing and analysis
- [ ] Trace data retention policy enforcement and secure deletion
- [ ] Protection against trace data correlation attacks
- [ ] User consent management for trace data collection
- [ ] Compliance with data protection regulations for trace data
- [ ] Secure trace data export and sharing functionality
- [ ] Trace data anonymization and pseudonymization capabilities
- [ ] Protection against unauthorized trace data access

### Tracing Infrastructure Security
- [ ] Tracing system access controls and authentication
- [ ] Secure communication between tracing components
- [ ] Protection against tracing system compromise
- [ ] Tracing system network isolation and hardening
- [ ] Secure configuration management for tracing infrastructure
- [ ] Audit logging for tracing system access and modifications
- [ ] Protection against trace injection and manipulation attacks
- [ ] Secure deployment and update procedures for tracing
- [ ] Incident response procedures for tracing security events
- [ ] Regular security assessments of tracing infrastructure

### Performance and Resource Security
- [ ] Resource consumption limits for tracing operations
- [ ] Memory usage monitoring and protection during tracing
- [ ] CPU usage limits for trace processing and export
- [ ] Network bandwidth protection for trace transmission
- [ ] Disk I/O monitoring and rate limiting for trace storage
- [ ] Trace collection rate limiting to prevent DoS attacks
- [ ] Protection against trace-based resource exhaustion
- [ ] Tracing overhead monitoring and optimization
- [ ] Automatic scaling and resource management for tracing
- [ ] Performance impact assessment for tracing operations

### Sampling and Export Security
- [ ] Secure sampling rule configuration and management
- [ ] Protection against sampling rule manipulation
- [ ] Secure trace export authentication and authorization
- [ ] Export endpoint security and rate limiting
- [ ] Protection against malicious trace export requests
- [ ] Secure batch processing and export scheduling
- [ ] Export failure handling and retry security
- [ ] Protection against export data corruption
- [ ] Secure integration with external tracing systems
- [ ] Export audit logging and monitoring

### Context and Correlation Security
- [ ] Secure trace context propagation and validation
- [ ] Protection against trace context spoofing
- [ ] Secure baggage handling and validation
- [ ] Cross-service trace correlation security
- [ ] Protection against trace correlation attacks
- [ ] Secure parent-child span relationship validation
- [ ] Context isolation between different users/tenants
- [ ] Protection against context injection attacks
- [ ] Secure trace ID and span ID generation
- [ ] Context cleanup and resource management

## Performance Requirements

### Tracing Performance
- Trace creation overhead < 1ms per span
- Trace context propagation latency < 0.1ms
- Span export batching efficiency > 95%
- Memory usage < 10MB per 1000 active spans
- Export throughput > 10,000 spans/second
- Sampling decision latency < 0.01ms

### Processing Performance
- Trace processing latency < 100ms
- Background export processing < 5 seconds per batch
- Metrics calculation from traces < 1 second
- Trace storage efficiency > 80%
- Query response time for traces < 2 seconds
- Trace correlation processing < 500ms

### Scalability Requirements
- Support 100,000+ spans per minute
- Handle 1,000+ concurrent traced operations
- Scale to 100+ services with distributed tracing
- Support 10GB+ trace data storage
- Manage 1M+ completed traces in memory
- Support 50+ concurrent trace exporters

## Commit Instructions

After implementing the distributed tracing system:

```bash
git add monitoring/tracing/
git commit -m "Add comprehensive distributed tracing with performance analysis

- Implement DistributedTracer with async context management
- Add TraceContext with comprehensive span lifecycle tracking
- Implement multiple span kinds and status tracking
- Add LogfireTraceExporter with structured trace export
- Include sampling rules with configurable rates and patterns
- Add HTTP tracing middleware for automatic request tracing
- Implement resource monitoring and performance analysis
- Add background processing for export and cleanup
- Include trace metrics and analytics with percentile calculations
- Add TDD cycle with Red-Green-Refactor for tracing features
- Ensure >85% distributed tracing test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete distributed tracing test suite:

```bash
# Run all distributed tracing tests
pytest monitoring/tests/test_distributed_tracer.py -v --timeout=300

# Run specific tracing test categories
pytest monitoring/tests/tracing/ -k "trace_context" -v
pytest monitoring/tests/tracing/ -k "sampling" -v
pytest monitoring/tests/tracing/ -k "export" -v

# Run tracing performance tests
pytest monitoring/tests/tracing/performance/ -v

# Run tracing middleware tests
pytest monitoring/tests/tracing/test_middleware.py -v
```

Validate distributed tracing test coverage:
```bash
pytest monitoring/tests/tracing/ --cov=monitoring.tracing --cov-report=html --cov-fail-under=85
```

## Integration Testing

Test distributed tracing integration with platform components:
```bash
# Test integration with Session 1 (Logfire)
pytest monitoring/tests/integration/test_tracing_logfire_integration.py -v

# Test integration with HTTP middleware
pytest monitoring/tests/integration/test_tracing_http_integration.py -v

# Test cross-service trace propagation
pytest monitoring/tests/integration/test_tracing_propagation_integration.py -v
```