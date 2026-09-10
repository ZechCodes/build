# Session 11: Monitoring & Observability

## Objective
Implement comprehensive monitoring, logging, and observability infrastructure using Pydantic Logfire, providing real-time visibility into system performance, security events, and user activities across all platform components.

## Overview
This session enhances the existing Logfire integration from Session 1 to create a production-ready observability stack. It implements advanced metrics collection, distributed tracing, alerting systems, security monitoring, performance dashboards, and automated incident response capabilities.

## Prerequisites
- Session 1-10 completed successfully
- Basic Logfire integration operational
- All services instrumented with structured logging
- Database and Redis monitoring available
- Network monitoring capabilities

## Components to Implement

### 1. Enhanced Metrics Collection
**Location**: `monitoring/metrics/`

#### Advanced Metrics System
```python
# monitoring/metrics/collector.py
import asyncio
import time
from typing import Dict, Any, List, Optional
from dataclasses import dataclass
from enum import Enum
import psutil
import structlog
import logfire
from prometheus_client import Counter, Histogram, Gauge, CollectorRegistry

logger = structlog.get_logger()

class MetricType(Enum):
    COUNTER = "counter"
    GAUGE = "gauge"
    HISTOGRAM = "histogram"
    SUMMARY = "summary"

@dataclass
class MetricDefinition:
    name: str
    type: MetricType
    description: str
    labels: List[str]
    unit: Optional[str] = None

class AdvancedMetricsCollector:
    def __init__(self, registry: CollectorRegistry = None):
        self.registry = registry or CollectorRegistry()
        self.metrics: Dict[str, Any] = {}
        self.custom_metrics: Dict[str, MetricDefinition] = {}
        self.collection_interval = 10  # seconds
        self.collection_task: Optional[asyncio.Task] = None
        
        # Initialize core metrics
        self._initialize_core_metrics()
    
    def _initialize_core_metrics(self):
        """Initialize core platform metrics"""
        # Performance metrics
        self.metrics['http_requests_total'] = Counter(
            'http_requests_total',
            'Total HTTP requests',
            ['method', 'endpoint', 'status'],
            registry=self.registry
        )
        
        self.metrics['http_request_duration'] = Histogram(
            'http_request_duration_seconds',
            'HTTP request duration',
            ['method', 'endpoint'],
            registry=self.registry
        )
        
        # Database metrics
        self.metrics['db_connections_active'] = Gauge(
            'db_connections_active',
            'Active database connections',
            registry=self.registry
        )
        
        self.metrics['db_query_duration'] = Histogram(
            'db_query_duration_seconds',
            'Database query duration',
            ['query_type'],
            registry=self.registry
        )
        
        # VM metrics
        self.metrics['vms_active'] = Gauge(
            'vms_active_total',
            'Number of active VMs',
            ['user_id'],
            registry=self.registry
        )
        
        self.metrics['vm_operations_total'] = Counter(
            'vm_operations_total',
            'Total VM operations',
            ['operation', 'status'],
            registry=self.registry
        )
        
        # Session metrics
        self.metrics['terminal_sessions_active'] = Gauge(
            'terminal_sessions_active',
            'Active terminal sessions',
            registry=self.registry
        )
        
        self.metrics['websocket_connections'] = Gauge(
            'websocket_connections_active',
            'Active WebSocket connections',
            registry=self.registry
        )
        
        # Security metrics
        self.metrics['auth_attempts_total'] = Counter(
            'auth_attempts_total',
            'Authentication attempts',
            ['result', 'method'],
            registry=self.registry
        )
        
        self.metrics['security_events_total'] = Counter(
            'security_events_total',
            'Security events',
            ['event_type', 'severity'],
            registry=self.registry
        )
        
        # System metrics
        self.metrics['system_cpu_usage'] = Gauge(
            'system_cpu_usage_percent',
            'System CPU usage percentage',
            registry=self.registry
        )
        
        self.metrics['system_memory_usage'] = Gauge(
            'system_memory_usage_bytes',
            'System memory usage',
            registry=self.registry
        )
    
    async def start_collection(self):
        """Start metrics collection"""
        self.collection_task = asyncio.create_task(self._collection_loop())
        logger.info("Metrics collection started")
    
    async def stop_collection(self):
        """Stop metrics collection"""
        if self.collection_task:
            self.collection_task.cancel()
        logger.info("Metrics collection stopped")
    
    async def _collection_loop(self):
        """Main metrics collection loop"""
        while True:
            try:
                await self._collect_system_metrics()
                await self._collect_application_metrics()
                await asyncio.sleep(self.collection_interval)
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Metrics collection error", error=str(e))
                await asyncio.sleep(self.collection_interval)
    
    async def _collect_system_metrics(self):
        """Collect system-level metrics"""
        try:
            # CPU usage
            cpu_percent = psutil.cpu_percent()
            self.metrics['system_cpu_usage'].set(cpu_percent)
            
            # Memory usage
            memory = psutil.virtual_memory()
            self.metrics['system_memory_usage'].set(memory.used)
            
            # Send to Logfire
            logfire.info(
                "System metrics collected",
                cpu_percent=cpu_percent,
                memory_used_gb=memory.used / (1024**3),
                memory_percent=memory.percent
            )
            
        except Exception as e:
            logger.error("System metrics collection failed", error=str(e))
    
    async def _collect_application_metrics(self):
        """Collect application-specific metrics"""
        try:
            # This would be implemented to collect metrics from
            # various application components
            pass
        except Exception as e:
            logger.error("Application metrics collection failed", error=str(e))
    
    def record_http_request(self, method: str, endpoint: str, status_code: int, duration: float):
        """Record HTTP request metrics"""
        self.metrics['http_requests_total'].labels(
            method=method, endpoint=endpoint, status=str(status_code)
        ).inc()
        
        self.metrics['http_request_duration'].labels(
            method=method, endpoint=endpoint
        ).observe(duration)
    
    def record_db_query(self, query_type: str, duration: float):
        """Record database query metrics"""
        self.metrics['db_query_duration'].labels(query_type=query_type).observe(duration)
    
    def record_security_event(self, event_type: str, severity: str):
        """Record security event"""
        self.metrics['security_events_total'].labels(
            event_type=event_type, severity=severity
        ).inc()
```

### 2. Distributed Tracing
**Location**: `monitoring/tracing/`

#### Enhanced Tracing System
```python
# monitoring/tracing/tracer.py
import asyncio
import time
import uuid
from typing import Dict, Any, Optional, List, AsyncContextManager
from dataclasses import dataclass, field
from contextlib import asynccontextmanager
import structlog
import logfire

logger = structlog.get_logger()

@dataclass
class TraceContext:
    trace_id: str
    span_id: str
    parent_span_id: Optional[str] = None
    operation_name: str = ""
    start_time: float = field(default_factory=time.time)
    end_time: Optional[float] = None
    tags: Dict[str, Any] = field(default_factory=dict)
    logs: List[Dict[str, Any]] = field(default_factory=list)
    error: Optional[str] = None

class DistributedTracer:
    def __init__(self):
        self.active_traces: Dict[str, TraceContext] = {}
        self.completed_traces: List[TraceContext] = []
        self.max_completed_traces = 1000
    
    @asynccontextmanager
    async def trace_operation(self, operation_name: str, 
                            parent_trace_id: Optional[str] = None,
                            **tags) -> AsyncContextManager[TraceContext]:
        """Create a traced operation context"""
        trace_id = str(uuid.uuid4())
        span_id = str(uuid.uuid4())
        
        trace_context = TraceContext(
            trace_id=trace_id,
            span_id=span_id,
            parent_span_id=parent_trace_id,
            operation_name=operation_name,
            tags=tags
        )
        
        self.active_traces[trace_id] = trace_context
        
        # Start Logfire span
        with logfire.span(
            operation_name,
            trace_id=trace_id,
            span_id=span_id,
            parent_span_id=parent_trace_id,
            **tags
        ) as span:
            try:
                yield trace_context
                
                # Mark as successful
                trace_context.end_time = time.time()
                span.set_attribute("success", True)
                span.set_attribute("duration_ms", 
                                 (trace_context.end_time - trace_context.start_time) * 1000)
                
            except Exception as e:
                # Mark as failed
                trace_context.end_time = time.time()
                trace_context.error = str(e)
                span.set_attribute("success", False)
                span.set_attribute("error", str(e))
                span.record_exception(e)
                raise
                
            finally:
                # Move to completed traces
                self.active_traces.pop(trace_id, None)
                self.completed_traces.append(trace_context)
                
                # Cleanup old traces
                if len(self.completed_traces) > self.max_completed_traces:
                    self.completed_traces = self.completed_traces[-self.max_completed_traces:]
    
    def add_trace_log(self, trace_id: str, level: str, message: str, **fields):
        """Add log entry to trace"""
        if trace_id in self.active_traces:
            self.active_traces[trace_id].logs.append({
                "timestamp": time.time(),
                "level": level,
                "message": message,
                **fields
            })
    
    def get_trace_summary(self, hours: int = 1) -> Dict[str, Any]:
        """Get trace summary for the last N hours"""
        cutoff_time = time.time() - (hours * 3600)
        
        recent_traces = [
            trace for trace in self.completed_traces
            if trace.start_time >= cutoff_time
        ]
        
        total_traces = len(recent_traces)
        successful_traces = len([t for t in recent_traces if not t.error])
        failed_traces = total_traces - successful_traces
        
        if total_traces > 0:
            avg_duration = sum(
                (t.end_time or t.start_time) - t.start_time 
                for t in recent_traces
            ) / total_traces
        else:
            avg_duration = 0
        
        return {
            "total_traces": total_traces,
            "successful_traces": successful_traces,
            "failed_traces": failed_traces,
            "success_rate": successful_traces / total_traces if total_traces > 0 else 1.0,
            "average_duration_ms": avg_duration * 1000
        }
```

### 3. Alerting System
**Location**: `monitoring/alerts/`

#### Alert Management
```python
# monitoring/alerts/alert_manager.py
import asyncio
import json
from typing import Dict, Any, List, Optional, Callable
from dataclasses import dataclass, asdict
from enum import Enum
import structlog
import logfire

logger = structlog.get_logger()

class AlertSeverity(Enum):
    INFO = "info"
    WARNING = "warning"
    CRITICAL = "critical"
    EMERGENCY = "emergency"

class AlertStatus(Enum):
    ACTIVE = "active"
    ACKNOWLEDGED = "acknowledged"
    RESOLVED = "resolved"
    SUPPRESSED = "suppressed"

@dataclass
class Alert:
    id: str
    name: str
    description: str
    severity: AlertSeverity
    status: AlertStatus
    created_at: float
    updated_at: float
    resolved_at: Optional[float]
    source: str
    tags: Dict[str, str]
    metrics: Dict[str, Any]
    acknowledged_by: Optional[str] = None
    resolution_notes: Optional[str] = None

class AlertRule:
    def __init__(self, name: str, condition: Callable[[Dict[str, Any]], bool],
                 severity: AlertSeverity, description: str,
                 cooldown_seconds: int = 300):
        self.name = name
        self.condition = condition
        self.severity = severity
        self.description = description
        self.cooldown_seconds = cooldown_seconds
        self.last_triggered = 0

class AlertManager:
    def __init__(self, notification_service):
        self.notification_service = notification_service
        self.active_alerts: Dict[str, Alert] = {}
        self.alert_rules: List[AlertRule] = []
        self.alert_history: List[Alert] = []
        self.max_history = 10000
        
        # Initialize default alert rules
        self._initialize_default_rules()
    
    def _initialize_default_rules(self):
        """Initialize default alerting rules"""
        # High CPU usage
        self.alert_rules.append(AlertRule(
            name="high_cpu_usage",
            condition=lambda m: m.get("cpu_percent", 0) > 80,
            severity=AlertSeverity.WARNING,
            description="CPU usage above 80%",
            cooldown_seconds=300
        ))
        
        # High memory usage
        self.alert_rules.append(AlertRule(
            name="high_memory_usage",
            condition=lambda m: m.get("memory_percent", 0) > 90,
            severity=AlertSeverity.CRITICAL,
            description="Memory usage above 90%",
            cooldown_seconds=180
        ))
        
        # High error rate
        self.alert_rules.append(AlertRule(
            name="high_error_rate",
            condition=lambda m: m.get("error_rate", 0) > 0.05,
            severity=AlertSeverity.CRITICAL,
            description="Error rate above 5%",
            cooldown_seconds=120
        ))
        
        # Database connection issues
        self.alert_rules.append(AlertRule(
            name="database_connection_low",
            condition=lambda m: m.get("db_connections_available", 100) < 5,
            severity=AlertSeverity.WARNING,
            description="Low database connections available",
            cooldown_seconds=60
        ))
        
        # Failed authentication attempts
        self.alert_rules.append(AlertRule(
            name="auth_failure_spike",
            condition=lambda m: m.get("auth_failures_per_minute", 0) > 20,
            severity=AlertSeverity.WARNING,
            description="High authentication failure rate",
            cooldown_seconds=300
        ))
    
    async def evaluate_rules(self, metrics: Dict[str, Any]):
        """Evaluate alert rules against current metrics"""
        current_time = time.time()
        
        for rule in self.alert_rules:
            try:
                # Check cooldown
                if current_time - rule.last_triggered < rule.cooldown_seconds:
                    continue
                
                # Evaluate condition
                if rule.condition(metrics):
                    await self._trigger_alert(rule, metrics)
                    rule.last_triggered = current_time
                    
            except Exception as e:
                logger.error("Alert rule evaluation failed", 
                           rule_name=rule.name, error=str(e))
    
    async def _trigger_alert(self, rule: AlertRule, metrics: Dict[str, Any]):
        """Trigger an alert"""
        alert_id = f"{rule.name}_{int(time.time())}"
        
        alert = Alert(
            id=alert_id,
            name=rule.name,
            description=rule.description,
            severity=rule.severity,
            status=AlertStatus.ACTIVE,
            created_at=time.time(),
            updated_at=time.time(),
            resolved_at=None,
            source="alert_manager",
            tags={"rule": rule.name},
            metrics=metrics.copy()
        )
        
        self.active_alerts[alert_id] = alert
        
        # Send notification
        await self.notification_service.send_alert(alert)
        
        # Log to Logfire
        logfire.warning(
            f"Alert triggered: {rule.name}",
            alert_id=alert_id,
            severity=rule.severity.value,
            description=rule.description,
            metrics=metrics
        )
        
        logger.warning("Alert triggered", alert_id=alert_id, 
                      rule_name=rule.name, severity=rule.severity.value)
    
    async def acknowledge_alert(self, alert_id: str, acknowledged_by: str) -> bool:
        """Acknowledge an alert"""
        if alert_id not in self.active_alerts:
            return False
        
        alert = self.active_alerts[alert_id]
        alert.status = AlertStatus.ACKNOWLEDGED
        alert.acknowledged_by = acknowledged_by
        alert.updated_at = time.time()
        
        logger.info("Alert acknowledged", alert_id=alert_id, 
                   acknowledged_by=acknowledged_by)
        return True
    
    async def resolve_alert(self, alert_id: str, resolution_notes: str = "") -> bool:
        """Resolve an alert"""
        if alert_id not in self.active_alerts:
            return False
        
        alert = self.active_alerts[alert_id]
        alert.status = AlertStatus.RESOLVED
        alert.resolved_at = time.time()
        alert.updated_at = time.time()
        alert.resolution_notes = resolution_notes
        
        # Move to history
        self.alert_history.append(alert)
        del self.active_alerts[alert_id]
        
        # Cleanup old history
        if len(self.alert_history) > self.max_history:
            self.alert_history = self.alert_history[-self.max_history:]
        
        logger.info("Alert resolved", alert_id=alert_id, 
                   resolution_notes=resolution_notes)
        return True
```

## Critical Decisions

### Observability Stack
- **Decision**: Pydantic Logfire as primary observability platform
- **Rationale**: Native Python integration, comprehensive features
- **Integration**: Enhanced with Prometheus metrics for ecosystem compatibility

### Metrics Strategy
- **Decision**: Hybrid approach with Prometheus metrics and Logfire events
- **Rationale**: Industry standard metrics with modern observability
- **Collection**: Real-time collection with configurable intervals

### Alerting Philosophy
- **Decision**: Proactive alerting with intelligent noise reduction
- **Rationale**: Early problem detection without alert fatigue
- **Implementation**: Rule-based with cooldown periods and severity levels

### Data Retention
- **Decision**: Tiered retention with hot/warm/cold storage
- **Rationale**: Balance between cost and accessibility
- **Strategy**: 30 days hot, 6 months warm, 2 years cold storage

## Security Checklist ✅

### Monitoring Security
- [ ] Monitoring system access controls and authentication
- [ ] Secure transmission of monitoring data
- [ ] Monitoring data encryption at rest and in transit
- [ ] Protection against monitoring system compromise
- [ ] Audit logging for monitoring system access
- [ ] Rate limiting on monitoring API endpoints
- [ ] Monitoring system network isolation
- [ ] Secure backup of monitoring data
- [ ] Monitoring system hardening and patching
- [ ] Protection against monitoring data tampering

### Metrics Security
- [ ] Sensitive data exclusion from metrics
- [ ] Metrics access authorization
- [ ] Metrics endpoint authentication
- [ ] Protection against metrics enumeration
- [ ] Secure metrics aggregation and storage
- [ ] Metrics data retention policy enforcement
- [ ] Monitoring of metrics collection integrity
- [ ] Protection against metrics injection attacks
- [ ] Secure metrics export functionality
- [ ] Compliance with data protection regulations

### Alerting Security
- [ ] Alert notification authentication and encryption
- [ ] Protection against alert spoofing
- [ ] Secure alert escalation procedures
- [ ] Alert access controls and authorization
- [ ] Protection against alert fatigue attacks
- [ ] Secure alert acknowledgment and resolution
- [ ] Alert history protection and integrity
- [ ] Monitoring of alerting system health
- [ ] Protection against alert suppression attacks
- [ ] Secure integration with external alert systems

### Logging Security
- [ ] Log data encryption and secure transmission
- [ ] Protection against log injection attacks
- [ ] Log access controls and audit trails
- [ ] Secure log aggregation and storage
- [ ] Log integrity verification and tamper detection
- [ ] Protection of sensitive data in logs
- [ ] Secure log retention and disposal
- [ ] Monitoring of logging system health
- [ ] Protection against log flooding attacks
- [ ] Compliance with logging regulations and standards

## Testing Requirements

### Monitoring System Testing
- [ ] Metrics collection accuracy and completeness
- [ ] Alert rule evaluation and triggering
- [ ] Dashboard functionality and performance
- [ ] Monitoring system scalability and reliability
- [ ] Integration with external monitoring tools
- [ ] Monitoring data retention and cleanup
- [ ] Alert notification delivery and reliability
- [ ] Monitoring system disaster recovery

### Performance Testing
- [ ] High-volume metrics ingestion
- [ ] Real-time dashboard updates
- [ ] Alert processing under load
- [ ] Query performance optimization
- [ ] Storage performance and scalability
- [ ] Network bandwidth utilization
- [ ] Memory usage optimization
- [ ] CPU usage efficiency

### Security Testing
- [ ] Monitoring system access controls
- [ ] Data encryption verification
- [ ] Alert security validation
- [ ] Audit trail completeness
- [ ] Vulnerability assessment
- [ ] Penetration testing
- [ ] Compliance validation
- [ ] Incident response procedures

### Integration Testing
- [ ] Service monitoring integration
- [ ] Database monitoring integration
- [ ] Infrastructure monitoring integration
- [ ] Security monitoring integration
- [ ] Third-party tool integration
- [ ] API monitoring integration
- [ ] Error tracking integration
- [ ] Performance monitoring integration

## Performance Targets

### Metrics Collection
- Metrics ingestion rate > 10,000 metrics/second
- Collection latency < 100ms
- Storage efficiency > 90%
- Query response time < 500ms
- Dashboard load time < 2 seconds
- Real-time update latency < 1 second

### Alerting Performance
- Alert evaluation latency < 30 seconds
- Alert notification delivery < 60 seconds
- Alert processing capacity > 1,000 alerts/minute
- Alert resolution tracking < 5 seconds
- False positive rate < 5%
- Alert suppression effectiveness > 95%

### System Performance
- Monitoring overhead < 5% of system resources
- Log processing rate > 100,000 logs/second
- Trace processing latency < 100ms
- Data retention efficiency > 80%
- Backup completion < 4 hours
- Recovery time < 1 hour

## Documentation Deliverables

### Technical Documentation
- [ ] Monitoring architecture documentation
- [ ] Metrics and alerting configuration guide
- [ ] Dashboard creation and customization guide
- [ ] Integration guide for new services
- [ ] Troubleshooting and debugging guide
- [ ] Performance tuning guide

### Operational Documentation
- [ ] Monitoring system runbook
- [ ] Alert response procedures
- [ ] Escalation procedures and contacts
- [ ] Incident response playbook
- [ ] Capacity planning guide
- [ ] Maintenance procedures

## Next Steps

Upon successful completion of Session 11:
1. Comprehensive monitoring and observability operational
2. Advanced metrics collection providing system visibility
3. Proactive alerting preventing issues before user impact
4. Distributed tracing enabling performance optimization
5. Security monitoring detecting and responding to threats
6. Performance targets met for all monitoring operations
7. Integration with all platform services validated
8. Proceed to Session 12: API Rate Limiting & DDoS Protection

## Risk Mitigation

### Technical Risks
1. **Monitoring system failures**: Redundancy, backup systems
2. **Data loss**: Replication, backup verification
3. **Performance impact**: Resource limits, optimization
4. **Storage exhaustion**: Retention policies, cleanup automation
5. **Alert fatigue**: Intelligent filtering, prioritization

### Security Risks
1. **Monitoring blind spots**: Comprehensive coverage validation
2. **False alerts**: Tuning, machine learning improvements
3. **Data exposure**: Encryption, access controls
4. **System compromise**: Hardening, network isolation
5. **Compliance violations**: Regular audits, policy enforcement

---

**Session 11 Success Criteria:**
- Comprehensive monitoring system providing full platform visibility
- Advanced metrics collection with real-time dashboards
- Proactive alerting system preventing issues before user impact
- Distributed tracing enabling performance analysis and optimization
- Security monitoring detecting and responding to threats
- Security checklist 100% complete with comprehensive protection
- Performance targets achieved for all monitoring operations
- Integration with Sessions 1-10 providing end-to-end observability
- All tests passing with >80% coverage including security tests
- Documentation complete with operational runbooks and procedures
- Ready for Session 12 rate limiting and DDoS protection implementation