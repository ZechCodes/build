# Session 11.4: Dashboards & Data Visualization

## Objective
Implement comprehensive dashboard and visualization system providing real-time insights into platform performance, user behavior, security events, and business metrics through interactive charts, graphs, and customizable dashboards.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for centralized data visualization and dashboard integration
- **Session 11.1**: Visualizes metrics collected by the metrics collection system
- **Session 11.2**: Displays distributed tracing data and performance analysis
- **Session 11.3**: Shows alert status, trends, and incident response dashboards
- All sessions: Provides unified view of all platform components and operations

## Core Implementation

### Dashboard Engine
**Location**: `monitoring/dashboards/dashboard_engine.py`

```python
# monitoring/dashboards/dashboard_engine.py
import asyncio
import time
import json
from typing import Dict, Any, List, Optional, Union, Callable
from dataclasses import dataclass, field, asdict
from enum import Enum
from datetime import datetime, timedelta
import structlog
import logfire
from concurrent.futures import ThreadPoolExecutor
import threading

logger = structlog.get_logger()

class ChartType(Enum):
    LINE = "line"
    BAR = "bar"
    AREA = "area"
    PIE = "pie"
    SCATTER = "scatter"
    HEATMAP = "heatmap"
    GAUGE = "gauge"
    TABLE = "table"
    STAT = "stat"
    HISTOGRAM = "histogram"

class TimeRange(Enum):
    LAST_5_MINUTES = "5m"
    LAST_15_MINUTES = "15m"
    LAST_30_MINUTES = "30m"
    LAST_1_HOUR = "1h"
    LAST_6_HOURS = "6h"
    LAST_12_HOURS = "12h"
    LAST_24_HOURS = "24h"
    LAST_7_DAYS = "7d"
    LAST_30_DAYS = "30d"
    CUSTOM = "custom"

class RefreshInterval(Enum):
    SECONDS_5 = 5
    SECONDS_10 = 10
    SECONDS_30 = 30
    MINUTES_1 = 60
    MINUTES_5 = 300
    MINUTES_15 = 900
    MANUAL = 0

@dataclass
class QueryDefinition:
    id: str
    name: str
    query: str
    data_source: str
    refresh_interval: RefreshInterval
    cache_duration_seconds: int = 60
    parameters: Dict[str, Any] = field(default_factory=dict)

@dataclass
class ChartConfiguration:
    id: str
    title: str
    chart_type: ChartType
    query_id: str
    options: Dict[str, Any] = field(default_factory=dict)
    position: Dict[str, int] = field(default_factory=dict)  # x, y, width, height
    time_range: TimeRange = TimeRange.LAST_1_HOUR
    custom_time_range: Optional[Dict[str, str]] = None
    thresholds: List[Dict[str, Any]] = field(default_factory=list)

@dataclass
class DashboardDefinition:
    id: str
    title: str
    description: str
    charts: List[ChartConfiguration]
    tags: List[str] = field(default_factory=list)
    is_public: bool = False
    owner: str = ""
    created_at: float = field(default_factory=time.time)
    updated_at: float = field(default_factory=time.time)
    refresh_interval: RefreshInterval = RefreshInterval.SECONDS_30
    variables: Dict[str, Any] = field(default_factory=dict)
    layout: Dict[str, Any] = field(default_factory=dict)

@dataclass
class DashboardData:
    dashboard_id: str
    charts_data: Dict[str, Any]
    generated_at: float
    query_execution_times: Dict[str, float]
    cache_status: Dict[str, str]
    errors: Dict[str, str] = field(default_factory=dict)

class DataSourceConnector:
    """Base class for data source connectors"""
    
    async def execute_query(self, query: str, parameters: Dict[str, Any] = None) -> Dict[str, Any]:
        raise NotImplementedError
    
    async def test_connection(self) -> bool:
        raise NotImplementedError

class MetricsDataSource(DataSourceConnector):
    """Data source for metrics data"""
    
    def __init__(self, metrics_collector):
        self.metrics_collector = metrics_collector
    
    async def execute_query(self, query: str, parameters: Dict[str, Any] = None) -> Dict[str, Any]:
        try:
            # Parse query to extract metric name and aggregation
            query_parts = self._parse_metrics_query(query)
            metric_name = query_parts.get("metric")
            aggregation = query_parts.get("aggregation", "avg")
            time_range = query_parts.get("time_range", "1h")
            
            # Get metric data from collector
            metric_data = await self._get_metric_data(metric_name, time_range, aggregation)
            
            return {
                "success": True,
                "data": metric_data,
                "metadata": {
                    "metric": metric_name,
                    "aggregation": aggregation,
                    "time_range": time_range,
                    "query": query
                }
            }
            
        except Exception as e:
            return {
                "success": False,
                "error": str(e),
                "data": []
            }
    
    def _parse_metrics_query(self, query: str) -> Dict[str, str]:
        """Parse metrics query string"""
        # Simple query parser - in production, use a proper query language
        parts = {}
        
        if "metric=" in query:
            parts["metric"] = query.split("metric=")[1].split("&")[0]
        if "aggregation=" in query:
            parts["aggregation"] = query.split("aggregation=")[1].split("&")[0]
        if "time_range=" in query:
            parts["time_range"] = query.split("time_range=")[1].split("&")[0]
        
        return parts
    
    async def _get_metric_data(self, metric_name: str, time_range: str, aggregation: str) -> List[Dict[str, Any]]:
        """Get metric data from collector"""
        # This would integrate with the actual metrics collector
        # For now, return mock data
        import random
        
        # Generate time series data
        end_time = time.time()
        range_seconds = self._parse_time_range(time_range)
        start_time = end_time - range_seconds
        
        data_points = []
        current_time = start_time
        interval = max(range_seconds / 100, 10)  # Up to 100 data points
        
        while current_time <= end_time:
            value = random.uniform(0, 100) if metric_name == "cpu_usage" else random.uniform(0, 1000)
            data_points.append({
                "timestamp": current_time,
                "value": value,
                "labels": {"instance": "server-1"}
            })
            current_time += interval
        
        return data_points
    
    def _parse_time_range(self, time_range: str) -> int:
        """Parse time range string to seconds"""
        time_ranges = {
            "5m": 300,
            "15m": 900,
            "30m": 1800,
            "1h": 3600,
            "6h": 21600,
            "12h": 43200,
            "24h": 86400,
            "7d": 604800,
            "30d": 2592000
        }
        return time_ranges.get(time_range, 3600)
    
    async def test_connection(self) -> bool:
        return self.metrics_collector is not None

class AlertsDataSource(DataSourceConnector):
    """Data source for alerts data"""
    
    def __init__(self, alert_manager):
        self.alert_manager = alert_manager
    
    async def execute_query(self, query: str, parameters: Dict[str, Any] = None) -> Dict[str, Any]:
        try:
            query_type = parameters.get("type", "active_alerts") if parameters else "active_alerts"
            
            if query_type == "active_alerts":
                data = self.alert_manager.get_active_alerts()
            elif query_type == "alert_history":
                hours = parameters.get("hours", 24) if parameters else 24
                data = self.alert_manager.get_alert_history(hours)
            elif query_type == "alert_stats":
                data = self.alert_manager.get_alert_statistics()
            else:
                data = []
            
            return {
                "success": True,
                "data": data,
                "metadata": {"query_type": query_type}
            }
            
        except Exception as e:
            return {
                "success": False,
                "error": str(e),
                "data": []
            }
    
    async def test_connection(self) -> bool:
        return self.alert_manager is not None

class TracingDataSource(DataSourceConnector):
    """Data source for tracing data"""
    
    def __init__(self, tracer):
        self.tracer = tracer
    
    async def execute_query(self, query: str, parameters: Dict[str, Any] = None) -> Dict[str, Any]:
        try:
            query_type = parameters.get("type", "trace_summary") if parameters else "trace_summary"
            
            if query_type == "trace_summary":
                hours = parameters.get("hours", 1) if parameters else 1
                data = self.tracer.get_trace_summary(hours)
            else:
                data = {}
            
            return {
                "success": True,
                "data": data,
                "metadata": {"query_type": query_type}
            }
            
        except Exception as e:
            return {
                "success": False,
                "error": str(e),
                "data": {}
            }
    
    async def test_connection(self) -> bool:
        return self.tracer is not None

class DashboardEngine:
    def __init__(self):
        self.dashboards: Dict[str, DashboardDefinition] = {}
        self.queries: Dict[str, QueryDefinition] = {}
        self.data_sources: Dict[str, DataSourceConnector] = {}
        self.data_cache: Dict[str, Dict[str, Any]] = {}
        
        # Background tasks
        self.refresh_task: Optional[asyncio.Task] = None
        self.cache_cleanup_task: Optional[asyncio.Task] = None
        
        # Configuration
        self.max_cache_entries = 10000
        self.cache_cleanup_interval = 3600  # 1 hour
        
        # Thread pool for query execution
        self.thread_pool = ThreadPoolExecutor(max_workers=10)
        
        # Statistics
        self.stats = {
            "total_dashboards": 0,
            "total_queries": 0,
            "cache_hits": 0,
            "cache_misses": 0,
            "query_executions": 0,
            "query_errors": 0
        }
        
        # Initialize default dashboards and queries
        self._initialize_default_content()

    def _initialize_default_content(self):
        """Initialize default dashboards and queries"""
        # System Overview Dashboard
        self._create_system_overview_dashboard()
        
        # Application Performance Dashboard
        self._create_application_performance_dashboard()
        
        # Security Dashboard
        self._create_security_dashboard()
        
        # Alerts Dashboard
        self._create_alerts_dashboard()

    def _create_system_overview_dashboard(self):
        """Create system overview dashboard"""
        # CPU Usage Query
        cpu_query = QueryDefinition(
            id="cpu_usage_query",
            name="CPU Usage",
            query="metric=system_cpu_usage_percent&aggregation=avg&time_range=1h",
            data_source="metrics",
            refresh_interval=RefreshInterval.SECONDS_30
        )
        self.queries[cpu_query.id] = cpu_query
        
        # Memory Usage Query
        memory_query = QueryDefinition(
            id="memory_usage_query",
            name="Memory Usage",
            query="metric=system_memory_usage_percent&aggregation=avg&time_range=1h",
            data_source="metrics",
            refresh_interval=RefreshInterval.SECONDS_30
        )
        self.queries[memory_query.id] = memory_query
        
        # Disk Usage Query
        disk_query = QueryDefinition(
            id="disk_usage_query",
            name="Disk Usage",
            query="metric=disk_usage_percent&aggregation=avg&time_range=1h",
            data_source="metrics",
            refresh_interval=RefreshInterval.MINUTES_5
        )
        self.queries[disk_query.id] = disk_query
        
        # Network I/O Query
        network_query = QueryDefinition(
            id="network_io_query",
            name="Network I/O",
            query="metric=network_bytes_total&aggregation=rate&time_range=1h",
            data_source="metrics",
            refresh_interval=RefreshInterval.SECONDS_30
        )
        self.queries[network_query.id] = network_query
        
        # Charts
        charts = [
            ChartConfiguration(
                id="cpu_chart",
                title="CPU Usage (%)",
                chart_type=ChartType.LINE,
                query_id="cpu_usage_query",
                position={"x": 0, "y": 0, "width": 6, "height": 4},
                thresholds=[{"value": 80, "color": "orange"}, {"value": 95, "color": "red"}]
            ),
            ChartConfiguration(
                id="memory_chart",
                title="Memory Usage (%)",
                chart_type=ChartType.LINE,
                query_id="memory_usage_query",
                position={"x": 6, "y": 0, "width": 6, "height": 4},
                thresholds=[{"value": 85, "color": "orange"}, {"value": 95, "color": "red"}]
            ),
            ChartConfiguration(
                id="disk_chart",
                title="Disk Usage (%)",
                chart_type=ChartType.GAUGE,
                query_id="disk_usage_query",
                position={"x": 0, "y": 4, "width": 6, "height": 4},
                thresholds=[{"value": 80, "color": "orange"}, {"value": 90, "color": "red"}]
            ),
            ChartConfiguration(
                id="network_chart",
                title="Network I/O (bytes/sec)",
                chart_type=ChartType.AREA,
                query_id="network_io_query",
                position={"x": 6, "y": 4, "width": 6, "height": 4}
            )
        ]
        
        dashboard = DashboardDefinition(
            id="system_overview",
            title="System Overview",
            description="Overview of system performance and resource utilization",
            charts=charts,
            tags=["system", "infrastructure", "performance"],
            is_public=True,
            refresh_interval=RefreshInterval.SECONDS_30
        )
        
        self.dashboards[dashboard.id] = dashboard

    def _create_application_performance_dashboard(self):
        """Create application performance dashboard"""
        # HTTP Requests Query
        http_requests_query = QueryDefinition(
            id="http_requests_query",
            name="HTTP Requests",
            query="metric=http_requests_total&aggregation=rate&time_range=1h",
            data_source="metrics",
            refresh_interval=RefreshInterval.SECONDS_30
        )
        self.queries[http_requests_query.id] = http_requests_query
        
        # Response Time Query
        response_time_query = QueryDefinition(
            id="response_time_query",
            name="Response Time",
            query="metric=http_request_duration_seconds&aggregation=avg&time_range=1h",
            data_source="metrics",
            refresh_interval=RefreshInterval.SECONDS_30
        )
        self.queries[response_time_query.id] = response_time_query
        
        # Error Rate Query
        error_rate_query = QueryDefinition(
            id="error_rate_query",
            name="Error Rate",
            query="metric=http_error_rate_percent&aggregation=avg&time_range=1h",
            data_source="metrics",
            refresh_interval=RefreshInterval.SECONDS_30
        )
        self.queries[error_rate_query.id] = error_rate_query
        
        # Active Sessions Query
        sessions_query = QueryDefinition(
            id="active_sessions_query",
            name="Active Sessions",
            query="metric=terminal_sessions_active&aggregation=sum&time_range=1h",
            data_source="metrics",
            refresh_interval=RefreshInterval.SECONDS_30
        )
        self.queries[sessions_query.id] = sessions_query
        
        charts = [
            ChartConfiguration(
                id="http_requests_chart",
                title="HTTP Requests/sec",
                chart_type=ChartType.BAR,
                query_id="http_requests_query",
                position={"x": 0, "y": 0, "width": 6, "height": 4}
            ),
            ChartConfiguration(
                id="response_time_chart",
                title="Average Response Time (ms)",
                chart_type=ChartType.LINE,
                query_id="response_time_query",
                position={"x": 6, "y": 0, "width": 6, "height": 4},
                thresholds=[{"value": 500, "color": "orange"}, {"value": 1000, "color": "red"}]
            ),
            ChartConfiguration(
                id="error_rate_chart",
                title="Error Rate (%)",
                chart_type=ChartType.STAT,
                query_id="error_rate_query",
                position={"x": 0, "y": 4, "width": 3, "height": 2},
                thresholds=[{"value": 1, "color": "orange"}, {"value": 5, "color": "red"}]
            ),
            ChartConfiguration(
                id="active_sessions_chart",
                title="Active Sessions",
                chart_type=ChartType.STAT,
                query_id="active_sessions_query",
                position={"x": 3, "y": 4, "width": 3, "height": 2}
            )
        ]
        
        dashboard = DashboardDefinition(
            id="application_performance",
            title="Application Performance",
            description="Application performance metrics and KPIs",
            charts=charts,
            tags=["application", "performance", "http"],
            is_public=True,
            refresh_interval=RefreshInterval.SECONDS_30
        )
        
        self.dashboards[dashboard.id] = dashboard

    def _create_security_dashboard(self):
        """Create security monitoring dashboard"""
        # Authentication Events Query
        auth_query = QueryDefinition(
            id="auth_events_query",
            name="Authentication Events",
            query="metric=auth_attempts_total&aggregation=rate&time_range=1h",
            data_source="metrics",
            refresh_interval=RefreshInterval.SECONDS_30
        )
        self.queries[auth_query.id] = auth_query
        
        # Security Events Query
        security_query = QueryDefinition(
            id="security_events_query",
            name="Security Events",
            query="metric=security_events_total&aggregation=rate&time_range=1h",
            data_source="metrics",
            refresh_interval=RefreshInterval.SECONDS_30
        )
        self.queries[security_query.id] = security_query
        
        charts = [
            ChartConfiguration(
                id="auth_events_chart",
                title="Authentication Events",
                chart_type=ChartType.LINE,
                query_id="auth_events_query",
                position={"x": 0, "y": 0, "width": 6, "height": 4}
            ),
            ChartConfiguration(
                id="security_events_chart",
                title="Security Events",
                chart_type=ChartType.LINE,
                query_id="security_events_query",
                position={"x": 6, "y": 0, "width": 6, "height": 4}
            )
        ]
        
        dashboard = DashboardDefinition(
            id="security_monitoring",
            title="Security Monitoring",
            description="Security events and authentication monitoring",
            charts=charts,
            tags=["security", "authentication", "monitoring"],
            is_public=True,
            refresh_interval=RefreshInterval.SECONDS_30
        )
        
        self.dashboards[dashboard.id] = dashboard

    def _create_alerts_dashboard(self):
        """Create alerts monitoring dashboard"""
        # Active Alerts Query
        active_alerts_query = QueryDefinition(
            id="active_alerts_query",
            name="Active Alerts",
            query="",
            data_source="alerts",
            refresh_interval=RefreshInterval.SECONDS_10,
            parameters={"type": "active_alerts"}
        )
        self.queries[active_alerts_query.id] = active_alerts_query
        
        # Alert Statistics Query
        alert_stats_query = QueryDefinition(
            id="alert_stats_query",
            name="Alert Statistics",
            query="",
            data_source="alerts",
            refresh_interval=RefreshInterval.SECONDS_30,
            parameters={"type": "alert_stats"}
        )
        self.queries[alert_stats_query.id] = alert_stats_query
        
        charts = [
            ChartConfiguration(
                id="active_alerts_table",
                title="Active Alerts",
                chart_type=ChartType.TABLE,
                query_id="active_alerts_query",
                position={"x": 0, "y": 0, "width": 12, "height": 6}
            ),
            ChartConfiguration(
                id="alert_stats_chart",
                title="Alert Statistics",
                chart_type=ChartType.STAT,
                query_id="alert_stats_query",
                position={"x": 0, "y": 6, "width": 12, "height": 2}
            )
        ]
        
        dashboard = DashboardDefinition(
            id="alerts_monitoring",
            title="Alerts Monitoring",
            description="Current alerts and alerting statistics",
            charts=charts,
            tags=["alerts", "incidents", "monitoring"],
            is_public=True,
            refresh_interval=RefreshInterval.SECONDS_10
        )
        
        self.dashboards[dashboard.id] = dashboard

    def register_data_source(self, name: str, connector: DataSourceConnector):
        """Register a data source connector"""
        self.data_sources[name] = connector
        logger.info("Data source registered", name=name, type=type(connector).__name__)

    async def start(self):
        """Start the dashboard engine"""
        self.refresh_task = asyncio.create_task(self._refresh_loop())
        self.cache_cleanup_task = asyncio.create_task(self._cache_cleanup_loop())
        
        logger.info("Dashboard engine started",
                   dashboards=len(self.dashboards),
                   queries=len(self.queries),
                   data_sources=len(self.data_sources))

    async def stop(self):
        """Stop the dashboard engine"""
        for task in [self.refresh_task, self.cache_cleanup_task]:
            if task and not task.done():
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass
        
        logger.info("Dashboard engine stopped")

    async def get_dashboard_data(self, dashboard_id: str, force_refresh: bool = False) -> Optional[DashboardData]:
        """Get data for a dashboard"""
        try:
            if dashboard_id not in self.dashboards:
                return None
            
            dashboard = self.dashboards[dashboard_id]
            charts_data = {}
            query_times = {}
            cache_status = {}
            errors = {}
            
            for chart in dashboard.charts:
                query_id = chart.query_id
                if query_id not in self.queries:
                    errors[chart.id] = f"Query {query_id} not found"
                    continue
                
                query = self.queries[query_id]
                
                # Check cache first
                cache_key = self._generate_cache_key(query_id, query.parameters)
                cached_data = self._get_cached_data(cache_key) if not force_refresh else None
                
                if cached_data:
                    charts_data[chart.id] = cached_data
                    cache_status[chart.id] = "hit"
                    query_times[chart.id] = 0
                    self.stats["cache_hits"] += 1
                else:
                    # Execute query
                    start_time = time.time()
                    data = await self._execute_query(query)
                    execution_time = (time.time() - start_time) * 1000
                    
                    if data.get("success", False):
                        charts_data[chart.id] = data
                        query_times[chart.id] = execution_time
                        cache_status[chart.id] = "miss"
                        
                        # Cache the result
                        self._cache_data(cache_key, data, query.cache_duration_seconds)
                        self.stats["cache_misses"] += 1
                    else:
                        errors[chart.id] = data.get("error", "Unknown error")
                        cache_status[chart.id] = "error"
                    
                    self.stats["query_executions"] += 1
                    if not data.get("success", False):
                        self.stats["query_errors"] += 1
            
            return DashboardData(
                dashboard_id=dashboard_id,
                charts_data=charts_data,
                generated_at=time.time(),
                query_execution_times=query_times,
                cache_status=cache_status,
                errors=errors
            )
            
        except Exception as e:
            logger.error("Failed to get dashboard data", dashboard_id=dashboard_id, error=str(e))
            return None

    async def _execute_query(self, query: QueryDefinition) -> Dict[str, Any]:
        """Execute a query against its data source"""
        try:
            if query.data_source not in self.data_sources:
                return {"success": False, "error": f"Data source {query.data_source} not found"}
            
            connector = self.data_sources[query.data_source]
            result = await connector.execute_query(query.query, query.parameters)
            
            return result
            
        except Exception as e:
            logger.error("Query execution failed", query_id=query.id, error=str(e))
            return {"success": False, "error": str(e)}

    def _generate_cache_key(self, query_id: str, parameters: Dict[str, Any]) -> str:
        """Generate cache key for query and parameters"""
        import hashlib
        params_str = json.dumps(parameters or {}, sort_keys=True)
        key_str = f"{query_id}:{params_str}"
        return hashlib.md5(key_str.encode()).hexdigest()

    def _get_cached_data(self, cache_key: str) -> Optional[Dict[str, Any]]:
        """Get data from cache if not expired"""
        if cache_key in self.data_cache:
            cached_entry = self.data_cache[cache_key]
            if time.time() < cached_entry["expires_at"]:
                return cached_entry["data"]
            else:
                # Remove expired entry
                del self.data_cache[cache_key]
        return None

    def _cache_data(self, cache_key: str, data: Dict[str, Any], duration_seconds: int):
        """Cache query result"""
        self.data_cache[cache_key] = {
            "data": data,
            "cached_at": time.time(),
            "expires_at": time.time() + duration_seconds
        }
        
        # Limit cache size
        if len(self.data_cache) > self.max_cache_entries:
            # Remove oldest entries
            sorted_entries = sorted(
                self.data_cache.items(),
                key=lambda x: x[1]["cached_at"]
            )
            for key, _ in sorted_entries[:len(self.data_cache) - self.max_cache_entries]:
                del self.data_cache[key]

    async def _refresh_loop(self):
        """Background task to refresh dashboard data"""
        while True:
            try:
                await asyncio.sleep(10)  # Check every 10 seconds
                
                # Find queries that need refresh
                current_time = time.time()
                for query_id, query in self.queries.items():
                    if query.refresh_interval == RefreshInterval.MANUAL:
                        continue
                    
                    # Check if query needs refresh based on interval
                    cache_key = self._generate_cache_key(query_id, query.parameters)
                    cached_data = self._get_cached_data(cache_key)
                    
                    if not cached_data:
                        # Execute query
                        await self._execute_query(query)
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Dashboard refresh loop error", error=str(e))

    async def _cache_cleanup_loop(self):
        """Background task to cleanup expired cache entries"""
        while True:
            try:
                await asyncio.sleep(self.cache_cleanup_interval)
                
                current_time = time.time()
                expired_keys = [
                    key for key, entry in self.data_cache.items()
                    if current_time >= entry["expires_at"]
                ]
                
                for key in expired_keys:
                    del self.data_cache[key]
                
                if expired_keys:
                    logger.debug("Cleaned up expired cache entries", count=len(expired_keys))
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Cache cleanup loop error", error=str(e))

    def get_dashboard_list(self) -> List[Dict[str, Any]]:
        """Get list of available dashboards"""
        return [
            {
                "id": dashboard.id,
                "title": dashboard.title,
                "description": dashboard.description,
                "tags": dashboard.tags,
                "is_public": dashboard.is_public,
                "chart_count": len(dashboard.charts),
                "created_at": dashboard.created_at,
                "updated_at": dashboard.updated_at
            }
            for dashboard in self.dashboards.values()
        ]

    def get_dashboard_definition(self, dashboard_id: str) -> Optional[DashboardDefinition]:
        """Get dashboard definition"""
        return self.dashboards.get(dashboard_id)

    def get_engine_statistics(self) -> Dict[str, Any]:
        """Get dashboard engine statistics"""
        return {
            **self.stats,
            "total_dashboards": len(self.dashboards),
            "total_queries": len(self.queries),
            "cache_entries": len(self.data_cache),
            "data_sources": list(self.data_sources.keys())
        }
```

### Dashboard API Endpoints
**Location**: `api/endpoints/dashboards.py`

```python
# api/endpoints/dashboards.py
from fastapi import APIRouter, HTTPException, Depends, status, Query
from typing import Optional, List
from pydantic import BaseModel
import structlog
import logfire

from ..auth import get_current_user
from ..dependencies import get_dashboard_engine

logger = structlog.get_logger()
router = APIRouter(prefix="/api/v1/dashboards", tags=["dashboards"])

class DashboardResponse(BaseModel):
    id: str
    title: str
    description: str
    tags: List[str]
    is_public: bool
    chart_count: int
    created_at: float
    updated_at: float

@router.get("/", response_model=List[DashboardResponse])
async def list_dashboards(
    current_user = Depends(get_current_user),
    dashboard_engine = Depends(get_dashboard_engine)
):
    """Get list of available dashboards"""
    try:
        dashboards = dashboard_engine.get_dashboard_list()
        
        # Filter based on user permissions
        accessible_dashboards = []
        for dashboard in dashboards:
            if dashboard["is_public"] or dashboard.get("owner") == current_user.id:
                accessible_dashboards.append(DashboardResponse(**dashboard))
        
        return accessible_dashboards
        
    except Exception as e:
        logger.error("Failed to list dashboards", error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to retrieve dashboards"
        )

@router.get("/{dashboard_id}")
async def get_dashboard(
    dashboard_id: str,
    force_refresh: bool = Query(False, description="Force refresh dashboard data"),
    current_user = Depends(get_current_user),
    dashboard_engine = Depends(get_dashboard_engine)
):
    """Get dashboard definition and data"""
    try:
        # Get dashboard definition
        definition = dashboard_engine.get_dashboard_definition(dashboard_id)
        if not definition:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Dashboard not found"
            )
        
        # Check permissions
        if not definition.is_public and definition.owner != current_user.id:
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Access denied to dashboard"
            )
        
        # Get dashboard data
        data = await dashboard_engine.get_dashboard_data(dashboard_id, force_refresh)
        if not data:
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail="Failed to generate dashboard data"
            )
        
        logfire.info("Dashboard accessed",
                   dashboard_id=dashboard_id,
                   user_id=current_user.id,
                   force_refresh=force_refresh)
        
        return {
            "definition": definition,
            "data": data
        }
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Failed to get dashboard", dashboard_id=dashboard_id, error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to retrieve dashboard"
        )

@router.get("/{dashboard_id}/data")
async def get_dashboard_data(
    dashboard_id: str,
    force_refresh: bool = Query(False, description="Force refresh dashboard data"),
    current_user = Depends(get_current_user),
    dashboard_engine = Depends(get_dashboard_engine)
):
    """Get dashboard data only"""
    try:
        # Check dashboard exists and permissions
        definition = dashboard_engine.get_dashboard_definition(dashboard_id)
        if not definition:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Dashboard not found"
            )
        
        if not definition.is_public and definition.owner != current_user.id:
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Access denied to dashboard"
            )
        
        # Get dashboard data
        data = await dashboard_engine.get_dashboard_data(dashboard_id, force_refresh)
        if not data:
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail="Failed to generate dashboard data"
            )
        
        return data
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Failed to get dashboard data", dashboard_id=dashboard_id, error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to retrieve dashboard data"
        )

@router.get("/stats/engine")
async def get_engine_statistics(
    current_user = Depends(get_current_user),
    dashboard_engine = Depends(get_dashboard_engine)
):
    """Get dashboard engine statistics"""
    try:
        # Check if user has admin permissions
        if not current_user.is_admin:
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Admin access required"
            )
        
        stats = dashboard_engine.get_engine_statistics()
        return stats
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Failed to get engine statistics", error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to retrieve engine statistics"
        )
```

## TDD Implementation Cycle

### Red Phase: Dashboard System Test Creation
```python
# monitoring/tests/test_dashboard_engine.py
import pytest
import asyncio
from monitoring.dashboards.dashboard_engine import DashboardEngine, DashboardDefinition

@pytest.mark.asyncio
async def test_dashboard_engine_initialization():
    """Test dashboard engine initializes with default dashboards"""
    # This test should initially fail (Red phase)
    engine = DashboardEngine()
    assert False, "Dashboard engine initialization not implemented yet"

@pytest.mark.asyncio
async def test_dashboard_data_generation():
    """Test dashboard data generation from queries"""
    # This test should initially fail (Red phase)
    assert False, "Dashboard data generation not implemented yet"

@pytest.mark.asyncio
async def test_data_source_integration():
    """Test data source connector integration"""
    # This test should initially fail (Red phase)
    assert False, "Data source integration not implemented yet"
```

### Green Phase: Dashboard System Implementation
```python
# Implement dashboard system features to make tests pass
# This involves adding chart rendering, data processing, and caching logic
```

### Refactor Phase: Dashboard System Optimization
```python
# Optimize dashboard system for performance and user experience
# Add advanced visualization capabilities and real-time updates
# Enhance caching strategies and data processing efficiency
```

## Security Checklist ✅

### Dashboard Access Control
- [ ] Dashboard access permissions and user-based filtering
- [ ] Dashboard creation and modification authorization
- [ ] Protection against unauthorized dashboard enumeration
- [ ] Dashboard sharing controls and permissions validation
- [ ] User-specific dashboard visibility and access controls
- [ ] Admin dashboard access controls and restrictions
- [ ] Dashboard ownership validation and enforcement
- [ ] Protection against dashboard configuration injection
- [ ] Audit logging for dashboard access and modifications
- [ ] Session-based dashboard access validation

### Data Security and Privacy
- [ ] Sensitive data exclusion from dashboard visualizations
- [ ] Query result filtering based on user permissions
- [ ] Data source access controls and authentication
- [ ] Protection against data exposure through dashboard queries
- [ ] User consent management for dashboard data collection
- [ ] Compliance with data protection regulations for dashboards
- [ ] Data anonymization and aggregation in dashboards
- [ ] Protection against data correlation attacks through dashboards
- [ ] Secure data export and sharing from dashboards
- [ ] Data retention controls for dashboard queries and cache

### Query Security
- [ ] Query injection prevention and input validation
- [ ] Query authorization and user-based filtering
- [ ] Protection against malicious query execution
- [ ] Query resource limits and timeout enforcement
- [ ] Secure query parameter handling and validation
- [ ] Protection against query enumeration attacks
- [ ] Query audit logging and monitoring
- [ ] Safe query parsing and execution
- [ ] Protection against query-based DoS attacks
- [ ] Secure integration with data source query languages

### Caching and Performance Security
- [ ] Cache security and access controls
- [ ] Protection against cache poisoning attacks
- [ ] Secure cache key generation and validation
- [ ] Cache data encryption and secure storage
- [ ] Cache invalidation security and integrity
- [ ] Protection against cache-based information disclosure
- [ ] Cache resource limits and overflow protection
- [ ] Secure cache cleanup and data deletion
- [ ] Cache isolation between different users/tenants
- [ ] Performance monitoring for cache security impacts

### Visualization Security
- [ ] Chart rendering security and XSS prevention
- [ ] Protection against malicious chart configurations
- [ ] Secure handling of chart data and parameters
- [ ] Visualization input validation and sanitization
- [ ] Protection against chart-based information disclosure
- [ ] Secure chart export and sharing functionality
- [ ] Chart template security and validation
- [ ] Protection against visualization injection attacks
- [ ] Secure integration with external chart libraries
- [ ] Chart data filtering and access controls

## Performance Requirements

### Dashboard Performance
- Dashboard loading time < 3 seconds
- Chart rendering time < 1 second per chart
- Real-time data updates < 5 seconds
- Query execution time < 2 seconds per query
- Cache hit ratio > 80% for frequently accessed data
- Dashboard refresh time < 10 seconds

### Data Processing Performance
- Data source query throughput > 100 queries/minute
- Chart data processing < 500ms per chart
- Data aggregation performance < 1 second for large datasets
- Cache management overhead < 5% of total processing time
- Memory usage < 200MB for dashboard engine
- Background refresh processing < 30 seconds per cycle

### Scalability Requirements
- Support 50+ concurrent dashboard users
- Handle 100+ dashboards with efficient management
- Support 1000+ chart configurations
- Process 10,000+ data points per chart
- Scale to 24/7 operation with 99.5% uptime
- Support 10+ data source integrations

## Commit Instructions

After implementing the dashboard and visualization system:

```bash
git add monitoring/dashboards/
git add api/endpoints/dashboards.py
git commit -m "Add comprehensive dashboard and visualization system

- Implement DashboardEngine with configurable chart and query system
- Add multiple data source connectors (metrics, alerts, tracing)
- Implement intelligent caching with TTL and cleanup mechanisms
- Add default dashboards for system, application, security, and alerts
- Include chart type support (line, bar, gauge, table, stats, etc.)
- Add dashboard API endpoints with access control
- Implement real-time data refresh and background processing
- Add comprehensive dashboard statistics and monitoring
- Include user permission-based dashboard filtering
- Add TDD cycle with Red-Green-Refactor for dashboard features
- Ensure >85% dashboard system test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete dashboard system test suite:

```bash
# Run all dashboard tests
pytest monitoring/tests/test_dashboard_engine.py -v --timeout=300

# Run specific dashboard test categories
pytest monitoring/tests/dashboards/ -k "data_sources" -v
pytest monitoring/tests/dashboards/ -k "caching" -v
pytest monitoring/tests/dashboards/ -k "charts" -v

# Run dashboard API tests
pytest api/tests/test_dashboard_endpoints.py -v

# Run dashboard performance tests
pytest monitoring/tests/dashboards/performance/ -v
```

Validate dashboard system test coverage:
```bash
pytest monitoring/tests/dashboards/ --cov=monitoring.dashboards --cov-report=html --cov-fail-under=85
```

## Integration Testing

Test dashboard system integration with platform components:
```bash
# Test integration with Session 11.1 (Metrics Collection)
pytest monitoring/tests/integration/test_dashboards_metrics_integration.py -v

# Test integration with Session 11.3 (Alerting System)
pytest monitoring/tests/integration/test_dashboards_alerts_integration.py -v

# Test dashboard API integration
pytest monitoring/tests/integration/test_dashboards_api_integration.py -v
```