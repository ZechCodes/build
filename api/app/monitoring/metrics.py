"""Comprehensive metrics collection with Logfire integration."""

import time
import asyncio
import structlog
from typing import Dict, Any, List, Optional
from dataclasses import dataclass, asdict
from datetime import datetime, timedelta

try:
    import logfire
    LOGFIRE_AVAILABLE = True
except ImportError:
    LOGFIRE_AVAILABLE = False

from .logfire_setup import log_performance_metric, create_span

logger = structlog.get_logger(__name__)


@dataclass
class MetricPoint:
    """Single metric data point."""
    name: str
    value: float
    timestamp: float
    tags: Dict[str, str]
    unit: str = "count"


@dataclass
class HealthMetrics:
    """System health metrics."""
    cpu_usage: float
    memory_usage: float
    memory_available: float
    disk_usage: float
    active_connections: int
    response_time_avg: float
    error_rate: float
    timestamp: float


class MetricsCollector:
    """Centralized metrics collection and reporting."""
    
    def __init__(self, retention_hours: int = 24):
        self.retention_hours = retention_hours
        self.metrics: List[MetricPoint] = []
        self.health_history: List[HealthMetrics] = []
        self.counters: Dict[str, float] = {}
        self.gauges: Dict[str, float] = {}
        self.histograms: Dict[str, List[float]] = {}
        self._last_cleanup = time.time()
    
    def record_counter(self, name: str, value: float = 1.0, tags: Optional[Dict[str, str]] = None):
        """Record a counter metric."""
        tags = tags or {}
        
        # Update internal counter
        counter_key = f"{name}:{','.join(f'{k}={v}' for k, v in sorted(tags.items()))}"
        self.counters[counter_key] = self.counters.get(counter_key, 0) + value
        
        # Create metric point
        metric = MetricPoint(
            name=name,
            value=value,
            timestamp=time.time(),
            tags=tags,
            unit="count"
        )
        
        self._add_metric(metric)
        
        # Log to Logfire
        log_performance_metric(name, value, "count", tags)
    
    def record_gauge(self, name: str, value: float, tags: Optional[Dict[str, str]] = None, unit: str = "value"):
        """Record a gauge metric."""
        tags = tags or {}
        
        # Update internal gauge
        gauge_key = f"{name}:{','.join(f'{k}={v}' for k, v in sorted(tags.items()))}"
        self.gauges[gauge_key] = value
        
        # Create metric point
        metric = MetricPoint(
            name=name,
            value=value,
            timestamp=time.time(),
            tags=tags,
            unit=unit
        )
        
        self._add_metric(metric)
        
        # Log to Logfire
        log_performance_metric(name, value, unit, tags)
    
    def record_histogram(self, name: str, value: float, tags: Optional[Dict[str, str]] = None, unit: str = "ms"):
        """Record a histogram metric."""
        tags = tags or {}
        
        # Update internal histogram
        hist_key = f"{name}:{','.join(f'{k}={v}' for k, v in sorted(tags.items()))}"
        if hist_key not in self.histograms:
            self.histograms[hist_key] = []
        self.histograms[hist_key].append(value)
        
        # Keep only recent values (last 1000)
        if len(self.histograms[hist_key]) > 1000:
            self.histograms[hist_key] = self.histograms[hist_key][-1000:]
        
        # Create metric point
        metric = MetricPoint(
            name=name,
            value=value,
            timestamp=time.time(),
            tags=tags,
            unit=unit
        )
        
        self._add_metric(metric)
        
        # Log to Logfire
        log_performance_metric(name, value, unit, tags)
    
    def record_timing(self, name: str, duration_ms: float, tags: Optional[Dict[str, str]] = None):
        """Record a timing metric."""
        self.record_histogram(name, duration_ms, tags, "ms")
    
    def record_health_metrics(self, health: HealthMetrics):
        """Record system health metrics."""
        self.health_history.append(health)
        
        # Keep only recent health data
        cutoff_time = time.time() - (self.retention_hours * 3600)
        self.health_history = [h for h in self.health_history if h.timestamp > cutoff_time]
        
        # Record individual metrics
        self.record_gauge("system.cpu_usage", health.cpu_usage, unit="percent")
        self.record_gauge("system.memory_usage", health.memory_usage, unit="bytes")
        self.record_gauge("system.memory_available", health.memory_available, unit="bytes")
        self.record_gauge("system.disk_usage", health.disk_usage, unit="percent")
        self.record_gauge("http.active_connections", health.active_connections, unit="count")
        self.record_gauge("http.response_time_avg", health.response_time_avg, unit="ms")
        self.record_gauge("http.error_rate", health.error_rate, unit="percent")
    
    def get_counter_value(self, name: str, tags: Optional[Dict[str, str]] = None) -> float:
        """Get current counter value."""
        tags = tags or {}
        counter_key = f"{name}:{','.join(f'{k}={v}' for k, v in sorted(tags.items()))}"
        return self.counters.get(counter_key, 0.0)
    
    def get_gauge_value(self, name: str, tags: Optional[Dict[str, str]] = None) -> Optional[float]:
        """Get current gauge value."""
        tags = tags or {}
        gauge_key = f"{name}:{','.join(f'{k}={v}' for k, v in sorted(tags.items()))}"
        return self.gauges.get(gauge_key)
    
    def get_histogram_stats(self, name: str, tags: Optional[Dict[str, str]] = None) -> Dict[str, float]:
        """Get histogram statistics."""
        tags = tags or {}
        hist_key = f"{name}:{','.join(f'{k}={v}' for k, v in sorted(tags.items()))}"
        values = self.histograms.get(hist_key, [])
        
        if not values:
            return {}
        
        sorted_values = sorted(values)
        count = len(sorted_values)
        
        return {
            "count": count,
            "min": sorted_values[0],
            "max": sorted_values[-1],
            "mean": sum(sorted_values) / count,
            "median": sorted_values[count // 2],
            "p95": sorted_values[int(count * 0.95)] if count > 0 else 0,
            "p99": sorted_values[int(count * 0.99)] if count > 0 else 0
        }
    
    def get_recent_metrics(self, name: str, minutes: int = 10) -> List[MetricPoint]:
        """Get recent metrics for a specific name."""
        cutoff_time = time.time() - (minutes * 60)
        return [m for m in self.metrics if m.name == name and m.timestamp > cutoff_time]
    
    def get_health_summary(self) -> Dict[str, Any]:
        """Get health metrics summary."""
        if not self.health_history:
            return {}
        
        recent_health = self.health_history[-1]
        
        # Calculate trends (last 10 vs previous 10)
        if len(self.health_history) >= 20:
            recent_10 = self.health_history[-10:]
            previous_10 = self.health_history[-20:-10]
            
            trends = {
                "cpu_trend": self._calculate_trend([h.cpu_usage for h in recent_10], 
                                                 [h.cpu_usage for h in previous_10]),
                "memory_trend": self._calculate_trend([h.memory_usage for h in recent_10], 
                                                    [h.memory_usage for h in previous_10]),
                "response_time_trend": self._calculate_trend([h.response_time_avg for h in recent_10], 
                                                           [h.response_time_avg for h in previous_10])
            }
        else:
            trends = {}
        
        return {
            "current": asdict(recent_health),
            "trends": trends,
            "data_points": len(self.health_history)
        }
    
    def get_all_metrics_summary(self) -> Dict[str, Any]:
        """Get comprehensive metrics summary."""
        # Cleanup old metrics first
        self._cleanup_old_metrics()
        
        return {
            "counters": dict(self.counters),
            "gauges": dict(self.gauges),
            "histogram_stats": {
                name: self.get_histogram_stats(name.split(":")[0], 
                                             dict(tag.split("=") for tag in name.split(":")[1].split(",") if "=" in tag) if ":" in name else {})
                for name in self.histograms.keys()
            },
            "health": self.get_health_summary(),
            "total_metrics": len(self.metrics),
            "retention_hours": self.retention_hours
        }
    
    def _add_metric(self, metric: MetricPoint):
        """Add metric to storage."""
        self.metrics.append(metric)
        
        # Cleanup old metrics periodically
        if time.time() - self._last_cleanup > 300:  # Every 5 minutes
            self._cleanup_old_metrics()
            self._last_cleanup = time.time()
    
    def _cleanup_old_metrics(self):
        """Remove old metrics beyond retention period."""
        cutoff_time = time.time() - (self.retention_hours * 3600)
        self.metrics = [m for m in self.metrics if m.timestamp > cutoff_time]
    
    def _calculate_trend(self, recent: List[float], previous: List[float]) -> str:
        """Calculate trend direction."""
        if not recent or not previous:
            return "unknown"
        
        recent_avg = sum(recent) / len(recent)
        previous_avg = sum(previous) / len(previous)
        
        change_percent = ((recent_avg - previous_avg) / previous_avg) * 100 if previous_avg > 0 else 0
        
        if change_percent > 5:
            return "increasing"
        elif change_percent < -5:
            return "decreasing"
        else:
            return "stable"


# Global metrics collector
metrics_collector = MetricsCollector()


class TimingContext:
    """Context manager for timing operations."""
    
    def __init__(self, metric_name: str, tags: Optional[Dict[str, str]] = None):
        self.metric_name = metric_name
        self.tags = tags or {}
        self.start_time = None
        self.span = None
    
    def __enter__(self):
        self.start_time = time.time()
        
        if LOGFIRE_AVAILABLE:
            try:
                self.span = create_span(f"Timing: {self.metric_name}", **self.tags)
                self.span.__enter__()
            except Exception as e:
                logger.debug("Failed to create timing span", error=str(e))
        
        return self
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        if self.start_time:
            duration_ms = (time.time() - self.start_time) * 1000
            metrics_collector.record_timing(self.metric_name, duration_ms, self.tags)
        
        if self.span:
            try:
                if self.start_time:
                    self.span.set_attribute("duration_ms", duration_ms)
                self.span.__exit__(exc_type, exc_val, exc_tb)
            except Exception as e:
                logger.debug("Failed to close timing span", error=str(e))


def time_operation(metric_name: str, tags: Optional[Dict[str, str]] = None) -> TimingContext:
    """Create a timing context manager."""
    return TimingContext(metric_name, tags)


def increment_counter(name: str, value: float = 1.0, tags: Optional[Dict[str, str]] = None):
    """Increment a counter metric."""
    metrics_collector.record_counter(name, value, tags)


def set_gauge(name: str, value: float, tags: Optional[Dict[str, str]] = None, unit: str = "value"):
    """Set a gauge metric."""
    metrics_collector.record_gauge(name, value, tags, unit)


def record_histogram(name: str, value: float, tags: Optional[Dict[str, str]] = None, unit: str = "ms"):
    """Record a histogram value."""
    metrics_collector.record_histogram(name, value, tags, unit)


async def collect_system_health() -> HealthMetrics:
    """Collect current system health metrics."""
    try:
        # Import here to avoid circular imports
        from ..middleware.monitoring import get_system_metrics
        
        system_metrics = await get_system_metrics()
        
        return HealthMetrics(
            cpu_usage=system_metrics.get("cpu", {}).get("usage_percent", 0),
            memory_usage=system_metrics.get("memory", {}).get("used", 0),
            memory_available=system_metrics.get("memory", {}).get("available", 0),
            disk_usage=system_metrics.get("disk", {}).get("percent", 0),
            active_connections=metrics_collector.get_gauge_value("http.active_connections") or 0,
            response_time_avg=metrics_collector.get_gauge_value("http.response_time_avg") or 0,
            error_rate=metrics_collector.get_gauge_value("http.error_rate") or 0,
            timestamp=time.time()
        )
    except Exception as e:
        logger.error("Failed to collect system health", error=str(e))
        return HealthMetrics(
            cpu_usage=0, memory_usage=0, memory_available=0, disk_usage=0,
            active_connections=0, response_time_avg=0, error_rate=0,
            timestamp=time.time()
        )


async def start_metrics_collection_task():
    """Start background task for metrics collection."""
    async def collect_metrics():
        while True:
            try:
                health = await collect_system_health()
                metrics_collector.record_health_metrics(health)
                
                # Log health summary periodically
                if int(time.time()) % 300 == 0:  # Every 5 minutes
                    summary = metrics_collector.get_health_summary()
                    logger.info("System health summary", **summary.get("current", {}))
                
            except Exception as e:
                logger.error("Failed to collect metrics", error=str(e))
            
            await asyncio.sleep(30)  # Collect every 30 seconds
    
    # Start the task
    asyncio.create_task(collect_metrics())
    logger.info("Metrics collection task started")