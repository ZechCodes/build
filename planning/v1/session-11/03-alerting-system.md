# Session 11.3: Intelligent Alerting System & Incident Response

## Objective
Implement comprehensive alerting system with intelligent rule evaluation, multi-channel notifications, escalation procedures, and automated incident response capabilities to ensure proactive monitoring and rapid issue resolution.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for alert correlation and centralized logging
- **Session 2**: Monitors authentication failures and security events
- **Session 11.1**: Processes metrics data to trigger alerts based on thresholds
- **Session 11.2**: Uses distributed tracing data for performance-based alerts
- All sessions: Monitors all platform components for health and performance

## Core Implementation

### Alert Management Engine
**Location**: `monitoring/alerts/alert_manager.py`

```python
# monitoring/alerts/alert_manager.py
import asyncio
import time
import json
import hashlib
from typing import Dict, Any, List, Optional, Set, Callable, Union
from dataclasses import dataclass, field, asdict
from enum import Enum
from datetime import datetime, timedelta
import structlog
import logfire
from concurrent.futures import ThreadPoolExecutor
import threading

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
    EXPIRED = "expired"

class AlertChannel(Enum):
    EMAIL = "email"
    SMS = "sms"
    SLACK = "slack"
    WEBHOOK = "webhook"
    PUSH = "push"
    PAGERDUTY = "pagerduty"

class ConditionOperator(Enum):
    GREATER_THAN = "gt"
    LESS_THAN = "lt"
    EQUALS = "eq"
    NOT_EQUALS = "ne"
    CONTAINS = "contains"
    NOT_CONTAINS = "not_contains"
    REGEX_MATCH = "regex_match"

@dataclass
class AlertCondition:
    metric_name: str
    operator: ConditionOperator
    threshold: Union[float, int, str]
    window_minutes: int = 5
    consecutive_periods: int = 1

@dataclass
class AlertRule:
    id: str
    name: str
    description: str
    severity: AlertSeverity
    conditions: List[AlertCondition]
    is_enabled: bool = True
    cooldown_minutes: int = 5
    auto_resolve_minutes: Optional[int] = None
    notification_channels: List[AlertChannel] = field(default_factory=list)
    escalation_rules: List[Dict[str, Any]] = field(default_factory=list)
    tags: Dict[str, str] = field(default_factory=dict)
    created_at: float = field(default_factory=time.time)
    updated_at: float = field(default_factory=time.time)
    last_triggered: float = 0
    trigger_count: int = 0

@dataclass
class Alert:
    id: str
    rule_id: str
    rule_name: str
    description: str
    severity: AlertSeverity
    status: AlertStatus
    created_at: float
    updated_at: float
    resolved_at: Optional[float] = None
    acknowledged_at: Optional[float] = None
    source: str = "alert_manager"
    tags: Dict[str, str] = field(default_factory=dict)
    metrics: Dict[str, Any] = field(default_factory=dict)
    context: Dict[str, Any] = field(default_factory=dict)
    acknowledged_by: Optional[str] = None
    resolved_by: Optional[str] = None
    resolution_notes: Optional[str] = None
    notification_history: List[Dict[str, Any]] = field(default_factory=list)
    escalation_level: int = 0
    fingerprint: Optional[str] = None

@dataclass
class NotificationChannel:
    type: AlertChannel
    name: str
    config: Dict[str, Any]
    is_enabled: bool = True
    rate_limit_per_hour: int = 100
    severity_filter: Set[AlertSeverity] = field(default_factory=lambda: set(AlertSeverity))

@dataclass
class EscalationRule:
    id: str
    name: str
    delay_minutes: int
    notification_channels: List[str]
    conditions: List[Dict[str, Any]] = field(default_factory=list)
    is_final: bool = False

class AlertEvaluator:
    """Evaluates alert conditions against metrics"""
    
    def __init__(self):
        self.metric_history: Dict[str, List[Dict[str, Any]]] = {}
        self.max_history_points = 1000
    
    def add_metric_sample(self, metric_name: str, value: Union[float, int], timestamp: float, labels: Dict[str, str] = None):
        """Add metric sample for evaluation"""
        if metric_name not in self.metric_history:
            self.metric_history[metric_name] = []
        
        sample = {
            "value": value,
            "timestamp": timestamp,
            "labels": labels or {}
        }
        
        self.metric_history[metric_name].append(sample)
        
        # Keep only recent samples
        if len(self.metric_history[metric_name]) > self.max_history_points:
            self.metric_history[metric_name] = self.metric_history[metric_name][-self.max_history_points:]
    
    def evaluate_condition(self, condition: AlertCondition) -> Dict[str, Any]:
        """Evaluate a single alert condition"""
        if condition.metric_name not in self.metric_history:
            return {"triggered": False, "reason": "No metric data available"}
        
        # Get samples within the time window
        current_time = time.time()
        window_start = current_time - (condition.window_minutes * 60)
        
        recent_samples = [
            sample for sample in self.metric_history[condition.metric_name]
            if sample["timestamp"] >= window_start
        ]
        
        if not recent_samples:
            return {"triggered": False, "reason": "No recent metric data"}
        
        # Group samples into periods for consecutive checking
        period_duration = condition.window_minutes * 60 / max(condition.consecutive_periods, 1)
        triggered_periods = 0
        
        for i in range(condition.consecutive_periods):
            period_start = window_start + (i * period_duration)
            period_end = period_start + period_duration
            
            period_samples = [
                sample for sample in recent_samples
                if period_start <= sample["timestamp"] < period_end
            ]
            
            if not period_samples:
                break
            
            # Evaluate condition for this period
            period_value = self._aggregate_samples(period_samples)
            if self._check_threshold(period_value, condition.operator, condition.threshold):
                triggered_periods += 1
            else:
                break
        
        triggered = triggered_periods >= condition.consecutive_periods
        
        return {
            "triggered": triggered,
            "triggered_periods": triggered_periods,
            "required_periods": condition.consecutive_periods,
            "current_value": recent_samples[-1]["value"] if recent_samples else None,
            "threshold": condition.threshold,
            "operator": condition.operator.value
        }
    
    def _aggregate_samples(self, samples: List[Dict[str, Any]]) -> Union[float, int]:
        """Aggregate samples within a period (currently uses average)"""
        if not samples:
            return 0
        return sum(sample["value"] for sample in samples) / len(samples)
    
    def _check_threshold(self, value: Union[float, int], operator: ConditionOperator, threshold: Union[float, int, str]) -> bool:
        """Check if value meets threshold condition"""
        try:
            if operator == ConditionOperator.GREATER_THAN:
                return value > float(threshold)
            elif operator == ConditionOperator.LESS_THAN:
                return value < float(threshold)
            elif operator == ConditionOperator.EQUALS:
                return value == float(threshold)
            elif operator == ConditionOperator.NOT_EQUALS:
                return value != float(threshold)
            elif operator == ConditionOperator.CONTAINS:
                return str(threshold) in str(value)
            elif operator == ConditionOperator.NOT_CONTAINS:
                return str(threshold) not in str(value)
            elif operator == ConditionOperator.REGEX_MATCH:
                import re
                return bool(re.match(str(threshold), str(value)))
            else:
                return False
        except (ValueError, TypeError):
            return False

class AlertManager:
    def __init__(self, notification_service, metrics_collector=None):
        self.notification_service = notification_service
        self.metrics_collector = metrics_collector
        self.evaluator = AlertEvaluator()
        
        # Storage
        self.alert_rules: Dict[str, AlertRule] = {}
        self.active_alerts: Dict[str, Alert] = {}
        self.alert_history: List[Alert] = []
        self.notification_channels: Dict[str, NotificationChannel] = {}
        self.escalation_rules: Dict[str, EscalationRule] = {}
        
        # Configuration
        self.max_history = 50000
        self.evaluation_interval_seconds = 30
        self.cleanup_interval_seconds = 3600
        self.auto_resolve_check_interval_seconds = 300
        
        # State
        self.is_running = False
        self.evaluation_task: Optional[asyncio.Task] = None
        self.cleanup_task: Optional[asyncio.Task] = None
        self.escalation_task: Optional[asyncio.Task] = None
        
        # Thread safety
        self.lock = threading.Lock()
        
        # Statistics
        self.stats = {
            "total_alerts_created": 0,
            "total_alerts_resolved": 0,
            "total_notifications_sent": 0,
            "evaluation_cycles": 0,
            "last_evaluation_duration_ms": 0
        }
        
        # Initialize default alert rules
        self._initialize_default_rules()
        self._initialize_default_channels()

    def _initialize_default_rules(self):
        """Initialize default alerting rules"""
        default_rules = [
            AlertRule(
                id="high_cpu_usage",
                name="High CPU Usage",
                description="System CPU usage is above 80%",
                severity=AlertSeverity.WARNING,
                conditions=[
                    AlertCondition(
                        metric_name="system_cpu_usage_percent",
                        operator=ConditionOperator.GREATER_THAN,
                        threshold=80.0,
                        window_minutes=5,
                        consecutive_periods=2
                    )
                ],
                cooldown_minutes=10,
                auto_resolve_minutes=15,
                notification_channels=[AlertChannel.EMAIL, AlertChannel.SLACK]
            ),
            AlertRule(
                id="critical_cpu_usage",
                name="Critical CPU Usage",
                description="System CPU usage is above 95%",
                severity=AlertSeverity.CRITICAL,
                conditions=[
                    AlertCondition(
                        metric_name="system_cpu_usage_percent",
                        operator=ConditionOperator.GREATER_THAN,
                        threshold=95.0,
                        window_minutes=2,
                        consecutive_periods=1
                    )
                ],
                cooldown_minutes=5,
                notification_channels=[AlertChannel.EMAIL, AlertChannel.SMS, AlertChannel.PAGERDUTY]
            ),
            AlertRule(
                id="high_memory_usage",
                name="High Memory Usage",
                description="System memory usage is above 90%",
                severity=AlertSeverity.CRITICAL,
                conditions=[
                    AlertCondition(
                        metric_name="system_memory_usage_percent",
                        operator=ConditionOperator.GREATER_THAN,
                        threshold=90.0,
                        window_minutes=5,
                        consecutive_periods=2
                    )
                ],
                cooldown_minutes=10,
                notification_channels=[AlertChannel.EMAIL, AlertChannel.SLACK]
            ),
            AlertRule(
                id="high_error_rate",
                name="High HTTP Error Rate",
                description="HTTP error rate is above 5%",
                severity=AlertSeverity.CRITICAL,
                conditions=[
                    AlertCondition(
                        metric_name="http_error_rate_percent",
                        operator=ConditionOperator.GREATER_THAN,
                        threshold=5.0,
                        window_minutes=3,
                        consecutive_periods=2
                    )
                ],
                cooldown_minutes=5,
                notification_channels=[AlertChannel.EMAIL, AlertChannel.SLACK, AlertChannel.PAGERDUTY]
            ),
            AlertRule(
                id="database_connection_low",
                name="Low Database Connections",
                description="Available database connections below threshold",
                severity=AlertSeverity.WARNING,
                conditions=[
                    AlertCondition(
                        metric_name="db_connections_available",
                        operator=ConditionOperator.LESS_THAN,
                        threshold=5,
                        window_minutes=2,
                        consecutive_periods=1
                    )
                ],
                cooldown_minutes=15,
                notification_channels=[AlertChannel.EMAIL, AlertChannel.SLACK]
            ),
            AlertRule(
                id="auth_failure_spike",
                name="Authentication Failure Spike",
                description="High rate of authentication failures detected",
                severity=AlertSeverity.WARNING,
                conditions=[
                    AlertCondition(
                        metric_name="auth_failures_per_minute",
                        operator=ConditionOperator.GREATER_THAN,
                        threshold=20,
                        window_minutes=5,
                        consecutive_periods=1
                    )
                ],
                cooldown_minutes=10,
                notification_channels=[AlertChannel.EMAIL, AlertChannel.SLACK]
            ),
            AlertRule(
                id="disk_space_low",
                name="Low Disk Space",
                description="Disk space usage above 85%",
                severity=AlertSeverity.WARNING,
                conditions=[
                    AlertCondition(
                        metric_name="disk_usage_percent",
                        operator=ConditionOperator.GREATER_THAN,
                        threshold=85.0,
                        window_minutes=10,
                        consecutive_periods=1
                    )
                ],
                cooldown_minutes=60,
                notification_channels=[AlertChannel.EMAIL]
            ),
            AlertRule(
                id="vm_creation_failures",
                name="VM Creation Failures",
                description="High rate of VM creation failures",
                severity=AlertSeverity.CRITICAL,
                conditions=[
                    AlertCondition(
                        metric_name="vm_creation_failure_rate",
                        operator=ConditionOperator.GREATER_THAN,
                        threshold=10.0,
                        window_minutes=5,
                        consecutive_periods=1
                    )
                ],
                cooldown_minutes=5,
                notification_channels=[AlertChannel.EMAIL, AlertChannel.SLACK, AlertChannel.PAGERDUTY]
            )
        ]
        
        for rule in default_rules:
            self.alert_rules[rule.id] = rule

    def _initialize_default_channels(self):
        """Initialize default notification channels"""
        # These would be configured with actual credentials/endpoints
        self.notification_channels = {
            "email_ops": NotificationChannel(
                type=AlertChannel.EMAIL,
                name="Operations Email",
                config={"recipients": ["ops@company.com"], "smtp_server": "smtp.company.com"},
                severity_filter={AlertSeverity.WARNING, AlertSeverity.CRITICAL, AlertSeverity.EMERGENCY}
            ),
            "slack_alerts": NotificationChannel(
                type=AlertChannel.SLACK,
                name="Alerts Slack Channel",
                config={"webhook_url": "https://hooks.slack.com/...", "channel": "#alerts"},
                severity_filter={AlertSeverity.CRITICAL, AlertSeverity.EMERGENCY}
            ),
            "pagerduty_critical": NotificationChannel(
                type=AlertChannel.PAGERDUTY,
                name="PagerDuty Critical",
                config={"service_key": "...", "routing_key": "..."},
                severity_filter={AlertSeverity.CRITICAL, AlertSeverity.EMERGENCY}
            )
        }

    async def start(self):
        """Start the alert manager"""
        if self.is_running:
            logger.warning("Alert manager already running")
            return
        
        self.is_running = True
        
        # Start background tasks
        self.evaluation_task = asyncio.create_task(self._evaluation_loop())
        self.cleanup_task = asyncio.create_task(self._cleanup_loop())
        self.escalation_task = asyncio.create_task(self._escalation_loop())
        
        logfire.info("Alert manager started",
                   total_rules=len(self.alert_rules),
                   evaluation_interval=self.evaluation_interval_seconds)
        
        logger.info("Alert manager started")

    async def stop(self):
        """Stop the alert manager"""
        self.is_running = False
        
        # Cancel background tasks
        for task in [self.evaluation_task, self.cleanup_task, self.escalation_task]:
            if task and not task.done():
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass
        
        logger.info("Alert manager stopped")

    async def _evaluation_loop(self):
        """Main alert evaluation loop"""
        while self.is_running:
            try:
                start_time = time.time()
                
                # Collect current metrics if collector is available
                if self.metrics_collector:
                    await self._collect_metrics_for_evaluation()
                
                # Evaluate all enabled alert rules
                for rule_id, rule in self.alert_rules.items():
                    if rule.is_enabled:
                        await self._evaluate_rule(rule)
                
                # Update statistics
                evaluation_duration = (time.time() - start_time) * 1000
                self.stats["evaluation_cycles"] += 1
                self.stats["last_evaluation_duration_ms"] = evaluation_duration
                
                # Check for auto-resolutions
                await self._check_auto_resolutions()
                
                logfire.debug("Alert evaluation cycle completed",
                            duration_ms=evaluation_duration,
                            active_alerts=len(self.active_alerts),
                            rules_evaluated=len([r for r in self.alert_rules.values() if r.is_enabled]))
                
                await asyncio.sleep(self.evaluation_interval_seconds)
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Alert evaluation loop error", error=str(e))
                await asyncio.sleep(min(self.evaluation_interval_seconds, 60))

    async def _collect_metrics_for_evaluation(self):
        """Collect current metrics for alert evaluation"""
        try:
            # This would integrate with the metrics collector to get current values
            # For now, we'll simulate with some basic system metrics
            import psutil
            
            # CPU usage
            cpu_percent = psutil.cpu_percent(interval=None)
            self.evaluator.add_metric_sample("system_cpu_usage_percent", cpu_percent, time.time())
            
            # Memory usage
            memory = psutil.virtual_memory()
            memory_percent = memory.percent
            self.evaluator.add_metric_sample("system_memory_usage_percent", memory_percent, time.time())
            
            # Disk usage (for root partition)
            disk = psutil.disk_usage('/')
            disk_percent = (disk.used / disk.total) * 100
            self.evaluator.add_metric_sample("disk_usage_percent", disk_percent, time.time())
            
        except Exception as e:
            logger.error("Failed to collect metrics for evaluation", error=str(e))

    async def _evaluate_rule(self, rule: AlertRule):
        """Evaluate a single alert rule"""
        try:
            # Check cooldown period
            current_time = time.time()
            if current_time - rule.last_triggered < (rule.cooldown_minutes * 60):
                return
            
            # Evaluate all conditions (AND logic)
            all_conditions_met = True
            condition_results = []
            
            for condition in rule.conditions:
                result = self.evaluator.evaluate_condition(condition)
                condition_results.append(result)
                
                if not result["triggered"]:
                    all_conditions_met = False
                    break
            
            if all_conditions_met:
                await self._trigger_alert(rule, condition_results)
                rule.last_triggered = current_time
                rule.trigger_count += 1
                rule.updated_at = current_time
                
        except Exception as e:
            logger.error("Failed to evaluate alert rule", rule_id=rule.id, error=str(e))

    async def _trigger_alert(self, rule: AlertRule, condition_results: List[Dict[str, Any]]):
        """Trigger an alert"""
        try:
            # Generate alert fingerprint for deduplication
            fingerprint = self._generate_alert_fingerprint(rule, condition_results)
            
            # Check if alert already exists
            existing_alert = None
            for alert in self.active_alerts.values():
                if alert.fingerprint == fingerprint and alert.status == AlertStatus.ACTIVE:
                    existing_alert = alert
                    break
            
            if existing_alert:
                # Update existing alert
                existing_alert.updated_at = time.time()
                existing_alert.context["condition_results"] = condition_results
                existing_alert.context["trigger_count"] = existing_alert.context.get("trigger_count", 0) + 1
                
                logfire.debug("Updated existing alert",
                            alert_id=existing_alert.id,
                            rule_id=rule.id,
                            trigger_count=existing_alert.context["trigger_count"])
                return
            
            # Create new alert
            alert_id = self._generate_alert_id(rule.id)
            
            alert = Alert(
                id=alert_id,
                rule_id=rule.id,
                rule_name=rule.name,
                description=rule.description,
                severity=rule.severity,
                status=AlertStatus.ACTIVE,
                created_at=time.time(),
                updated_at=time.time(),
                tags=rule.tags.copy(),
                context={
                    "condition_results": condition_results,
                    "trigger_count": 1,
                    "rule_cooldown_minutes": rule.cooldown_minutes
                },
                fingerprint=fingerprint
            )
            
            # Store alert
            self.active_alerts[alert_id] = alert
            self.stats["total_alerts_created"] += 1
            
            # Send notifications
            await self._send_alert_notifications(alert, rule)
            
            # Log alert creation
            logfire.warning(f"Alert triggered: {rule.name}",
                          alert_id=alert_id,
                          rule_id=rule.id,
                          severity=rule.severity.value,
                          description=rule.description,
                          condition_results=condition_results)
            
            logger.warning("Alert triggered",
                         alert_id=alert_id,
                         rule_id=rule.id,
                         rule_name=rule.name,
                         severity=rule.severity.value)
            
        except Exception as e:
            logger.error("Failed to trigger alert", rule_id=rule.id, error=str(e))

    async def _send_alert_notifications(self, alert: Alert, rule: AlertRule):
        """Send alert notifications to configured channels"""
        try:
            for channel_type in rule.notification_channels:
                # Find configured channel of this type
                channel = None
                for ch in self.notification_channels.values():
                    if ch.type == channel_type and ch.is_enabled and alert.severity in ch.severity_filter:
                        channel = ch
                        break
                
                if not channel:
                    continue
                
                # Check rate limits
                if not self._check_notification_rate_limit(channel, alert):
                    continue
                
                # Send notification
                success = await self.notification_service.send_alert_notification(alert, channel)
                
                # Record notification
                notification_record = {
                    "channel_type": channel_type.value,
                    "channel_name": channel.name,
                    "sent_at": time.time(),
                    "success": success
                }
                alert.notification_history.append(notification_record)
                
                if success:
                    self.stats["total_notifications_sent"] += 1
                    logfire.info("Alert notification sent",
                               alert_id=alert.id,
                               channel_type=channel_type.value,
                               channel_name=channel.name)
                else:
                    logger.error("Failed to send alert notification",
                               alert_id=alert.id,
                               channel_type=channel_type.value)
                
        except Exception as e:
            logger.error("Failed to send alert notifications", alert_id=alert.id, error=str(e))

    def _check_notification_rate_limit(self, channel: NotificationChannel, alert: Alert) -> bool:
        """Check if notification is within rate limits"""
        # Simple rate limiting based on channel configuration
        current_time = time.time()
        hour_ago = current_time - 3600
        
        # Count notifications sent in the last hour for this channel
        notifications_this_hour = sum(
            1 for a in self.active_alerts.values()
            for notification in a.notification_history
            if (notification.get("channel_name") == channel.name and
                notification.get("sent_at", 0) > hour_ago and
                notification.get("success", False))
        )
        
        return notifications_this_hour < channel.rate_limit_per_hour

    async def _escalation_loop(self):
        """Handle alert escalations"""
        while self.is_running:
            try:
                await asyncio.sleep(60)  # Check every minute
                
                current_time = time.time()
                
                for alert in list(self.active_alerts.values()):
                    if alert.status == AlertStatus.ACTIVE:
                        await self._check_alert_escalation(alert, current_time)
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Alert escalation loop error", error=str(e))

    async def _check_alert_escalation(self, alert: Alert, current_time: float):
        """Check if alert needs escalation"""
        try:
            rule = self.alert_rules.get(alert.rule_id)
            if not rule or not rule.escalation_rules:
                return
            
            alert_age_minutes = (current_time - alert.created_at) / 60
            
            for escalation in rule.escalation_rules:
                if alert.escalation_level >= len(rule.escalation_rules):
                    break
                
                escalation_rule = rule.escalation_rules[alert.escalation_level]
                delay_minutes = escalation_rule.get("delay_minutes", 30)
                
                if alert_age_minutes >= delay_minutes:
                    await self._escalate_alert(alert, escalation_rule)
                    break
                
        except Exception as e:
            logger.error("Failed to check alert escalation", alert_id=alert.id, error=str(e))

    async def _escalate_alert(self, alert: Alert, escalation_rule: Dict[str, Any]):
        """Escalate an alert"""
        try:
            alert.escalation_level += 1
            alert.updated_at = time.time()
            
            # Send escalation notifications
            channels = escalation_rule.get("notification_channels", [])
            for channel_name in channels:
                if channel_name in self.notification_channels:
                    channel = self.notification_channels[channel_name]
                    await self.notification_service.send_escalation_notification(alert, channel, escalation_rule)
            
            logfire.warning("Alert escalated",
                          alert_id=alert.id,
                          escalation_level=alert.escalation_level,
                          escalation_channels=channels)
            
            logger.warning("Alert escalated",
                         alert_id=alert.id,
                         escalation_level=alert.escalation_level)
            
        except Exception as e:
            logger.error("Failed to escalate alert", alert_id=alert.id, error=str(e))

    async def acknowledge_alert(self, alert_id: str, acknowledged_by: str, notes: str = "") -> bool:
        """Acknowledge an alert"""
        try:
            if alert_id not in self.active_alerts:
                return False
            
            alert = self.active_alerts[alert_id]
            if alert.status != AlertStatus.ACTIVE:
                return False
            
            alert.status = AlertStatus.ACKNOWLEDGED
            alert.acknowledged_by = acknowledged_by
            alert.acknowledged_at = time.time()
            alert.updated_at = time.time()
            
            if notes:
                alert.context["acknowledgment_notes"] = notes
            
            logfire.info("Alert acknowledged",
                       alert_id=alert_id,
                       acknowledged_by=acknowledged_by,
                       notes=notes)
            
            logger.info("Alert acknowledged",
                      alert_id=alert_id,
                      acknowledged_by=acknowledged_by)
            
            return True
            
        except Exception as e:
            logger.error("Failed to acknowledge alert", alert_id=alert_id, error=str(e))
            return False

    async def resolve_alert(self, alert_id: str, resolved_by: str = "", resolution_notes: str = "") -> bool:
        """Resolve an alert"""
        try:
            if alert_id not in self.active_alerts:
                return False
            
            alert = self.active_alerts[alert_id]
            alert.status = AlertStatus.RESOLVED
            alert.resolved_by = resolved_by
            alert.resolved_at = time.time()
            alert.updated_at = time.time()
            alert.resolution_notes = resolution_notes
            
            # Move to history
            self.alert_history.append(alert)
            del self.active_alerts[alert_id]
            self.stats["total_alerts_resolved"] += 1
            
            # Cleanup old history
            if len(self.alert_history) > self.max_history:
                self.alert_history = self.alert_history[-self.max_history:]
            
            logfire.info("Alert resolved",
                       alert_id=alert_id,
                       resolved_by=resolved_by,
                       resolution_notes=resolution_notes,
                       duration_minutes=(alert.resolved_at - alert.created_at) / 60)
            
            logger.info("Alert resolved",
                      alert_id=alert_id,
                      resolved_by=resolved_by)
            
            return True
            
        except Exception as e:
            logger.error("Failed to resolve alert", alert_id=alert_id, error=str(e))
            return False

    async def _check_auto_resolutions(self):
        """Check for alerts that should be auto-resolved"""
        try:
            current_time = time.time()
            
            for alert_id, alert in list(self.active_alerts.items()):
                rule = self.alert_rules.get(alert.rule_id)
                if not rule or not rule.auto_resolve_minutes:
                    continue
                
                # Check if conditions are no longer met
                conditions_still_met = True
                for condition in rule.conditions:
                    result = self.evaluator.evaluate_condition(condition)
                    if not result["triggered"]:
                        conditions_still_met = False
                        break
                
                if not conditions_still_met:
                    # Check if enough time has passed
                    time_since_created = (current_time - alert.created_at) / 60
                    if time_since_created >= rule.auto_resolve_minutes:
                        await self.resolve_alert(alert_id, "system", "Auto-resolved: conditions no longer met")
                
        except Exception as e:
            logger.error("Failed to check auto-resolutions", error=str(e))

    async def _cleanup_loop(self):
        """Cleanup old data and perform maintenance"""
        while self.is_running:
            try:
                await asyncio.sleep(self.cleanup_interval_seconds)
                
                # Cleanup old metric history
                self._cleanup_metric_history()
                
                # Cleanup old alert history
                self._cleanup_alert_history()
                
                logger.debug("Alert manager cleanup completed")
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Alert cleanup loop error", error=str(e))

    def _cleanup_metric_history(self):
        """Cleanup old metric samples"""
        cutoff_time = time.time() - (24 * 3600)  # Keep 24 hours
        
        for metric_name, samples in self.evaluator.metric_history.items():
            self.evaluator.metric_history[metric_name] = [
                sample for sample in samples
                if sample["timestamp"] > cutoff_time
            ]

    def _cleanup_alert_history(self):
        """Cleanup old alert history"""
        cutoff_time = time.time() - (30 * 24 * 3600)  # Keep 30 days
        
        self.alert_history = [
            alert for alert in self.alert_history
            if alert.created_at > cutoff_time
        ]

    def _generate_alert_fingerprint(self, rule: AlertRule, condition_results: List[Dict[str, Any]]) -> str:
        """Generate unique fingerprint for alert deduplication"""
        fingerprint_data = {
            "rule_id": rule.id,
            "conditions": [
                {
                    "metric": condition.metric_name,
                    "operator": condition.operator.value,
                    "threshold": condition.threshold
                }
                for condition in rule.conditions
            ]
        }
        
        fingerprint_str = json.dumps(fingerprint_data, sort_keys=True)
        return hashlib.md5(fingerprint_str.encode()).hexdigest()

    def _generate_alert_id(self, rule_id: str) -> str:
        """Generate unique alert ID"""
        timestamp = str(int(time.time() * 1000))
        return f"alert_{rule_id}_{timestamp}"

    def get_alert_statistics(self) -> Dict[str, Any]:
        """Get alert manager statistics"""
        return {
            **self.stats,
            "active_alerts": len(self.active_alerts),
            "total_rules": len(self.alert_rules),
            "enabled_rules": len([r for r in self.alert_rules.values() if r.is_enabled]),
            "alert_history_size": len(self.alert_history),
            "notification_channels": len(self.notification_channels),
            "is_running": self.is_running
        }

    def get_active_alerts(self) -> List[Dict[str, Any]]:
        """Get all active alerts"""
        return [asdict(alert) for alert in self.active_alerts.values()]

    def get_alert_history(self, hours: int = 24) -> List[Dict[str, Any]]:
        """Get alert history for specified hours"""
        cutoff_time = time.time() - (hours * 3600)
        return [
            asdict(alert) for alert in self.alert_history
            if alert.created_at >= cutoff_time
        ]
```

## TDD Implementation Cycle

### Red Phase: Alerting System Test Creation
```python
# monitoring/tests/test_alert_manager.py
import pytest
import asyncio
from monitoring.alerts.alert_manager import AlertManager, AlertRule, AlertSeverity, AlertCondition

@pytest.mark.asyncio
async def test_alert_manager_initialization():
    """Test alert manager initializes with default rules"""
    # This test should initially fail (Red phase)
    manager = AlertManager(None)
    assert False, "Alert manager initialization not implemented yet"

@pytest.mark.asyncio
async def test_alert_condition_evaluation():
    """Test alert condition evaluation logic"""
    # This test should initially fail (Red phase)
    assert False, "Alert condition evaluation not implemented yet"

@pytest.mark.asyncio
async def test_alert_escalation():
    """Test alert escalation workflow"""
    # This test should initially fail (Red phase)
    assert False, "Alert escalation not implemented yet"
```

### Green Phase: Alerting System Implementation
```python
# Implement alerting system features to make tests pass
# This involves adding rule evaluation, notification, and escalation logic
```

### Refactor Phase: Alerting System Optimization
```python
# Optimize alerting system for performance and reliability
# Add intelligent deduplication and noise reduction
# Enhance notification delivery and escalation procedures
```

## Security Checklist ✅

### Alert Security and Access Control
- [ ] Alert system access controls and authentication
- [ ] Alert rule management authorization and validation
- [ ] Protection against unauthorized alert creation or modification
- [ ] Alert viewing permissions and user-based filtering
- [ ] Secure alert acknowledgment and resolution authorization
- [ ] Alert rule injection prevention and validation
- [ ] Protection against alert system enumeration
- [ ] Audit logging for all alert system operations
- [ ] Alert system configuration security and validation
- [ ] Protection against alert system privilege escalation

### Notification Security
- [ ] Notification channel configuration security and encryption
- [ ] Protection against notification spoofing and tampering
- [ ] Secure notification delivery authentication and validation
- [ ] Notification content filtering to prevent information disclosure
- [ ] Rate limiting and abuse prevention for notifications
- [ ] Secure integration with external notification services
- [ ] Protection against notification replay and injection attacks
- [ ] Notification audit logging and delivery confirmation
- [ ] Secure handling of notification credentials and tokens
- [ ] Protection against notification channel enumeration

### Data Protection and Privacy
- [ ] Sensitive data exclusion from alerts and notifications
- [ ] Alert data encryption at rest and in transit
- [ ] User consent management for alert notifications
- [ ] Compliance with data protection regulations for alert data
- [ ] Secure alert data retention and deletion policies
- [ ] Protection against alert data correlation attacks
- [ ] Alert data anonymization and pseudonymization capabilities
- [ ] Secure alert data export and sharing functionality
- [ ] Protection against unauthorized alert data access
- [ ] Alert data integrity verification and tamper detection

### System Security and Resilience
- [ ] Alert system hardening and security patching
- [ ] Protection against alert system compromise and attacks
- [ ] Secure communication between alerting components
- [ ] Alert system network isolation and access controls
- [ ] Protection against DoS attacks on alerting infrastructure
- [ ] Secure backup and recovery of alert configuration and data
- [ ] Alert system high availability and failover procedures
- [ ] Incident response procedures for alerting system security events
- [ ] Regular security assessments of alerting infrastructure
- [ ] Alert system disaster recovery and business continuity

### Performance and Resource Security
- [ ] Resource consumption limits for alerting operations
- [ ] Memory usage monitoring and protection during alert processing
- [ ] CPU usage limits for alert evaluation and notification
- [ ] Network bandwidth protection for alert notifications
- [ ] Alert processing rate limiting to prevent resource exhaustion
- [ ] Protection against alert-based resource exhaustion attacks
- [ ] Alert system performance monitoring and optimization
- [ ] Automatic scaling and resource management for alerting
- [ ] Alert queue management and overflow protection
- [ ] Performance impact assessment for alerting operations

## Performance Requirements

### Alert Evaluation Performance
- Alert rule evaluation latency < 5 seconds per cycle
- Condition evaluation processing < 100ms per condition
- Alert creation and storage < 500ms
- Notification delivery < 30 seconds
- Escalation processing < 60 seconds
- Auto-resolution checking < 10 seconds

### System Performance
- Support 1000+ alert rules with efficient evaluation
- Handle 10,000+ metric samples per minute for evaluation
- Process 100+ simultaneous alerts without degradation
- Support 50+ notification channels with rate limiting
- Maintain 100,000+ historical alerts with fast querying
- Scale to 24/7 operation with 99.9% uptime

### Notification Performance
- Email notification delivery < 60 seconds
- Slack notification delivery < 10 seconds
- SMS notification delivery < 30 seconds
- Webhook notification delivery < 5 seconds
- PagerDuty integration response < 15 seconds
- Notification failure retry with exponential backoff

## Commit Instructions

After implementing the alerting system:

```bash
git add monitoring/alerts/
git commit -m "Add intelligent alerting system with comprehensive incident response

- Implement AlertManager with rule-based evaluation engine
- Add AlertEvaluator with metric-based condition checking
- Implement multi-channel notification system (email, Slack, SMS, PagerDuty)
- Add intelligent alert deduplication and fingerprinting
- Include escalation procedures with configurable rules
- Add alert acknowledgment and resolution workflows
- Implement auto-resolution based on condition clearing
- Add comprehensive alert statistics and monitoring
- Include rate limiting and notification management
- Add TDD cycle with Red-Green-Refactor for alerting features
- Ensure >85% alerting system test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete alerting system test suite:

```bash
# Run all alerting tests
pytest monitoring/tests/test_alert_manager.py -v --timeout=300

# Run specific alerting test categories
pytest monitoring/tests/alerts/ -k "evaluation" -v
pytest monitoring/tests/alerts/ -k "notification" -v
pytest monitoring/tests/alerts/ -k "escalation" -v

# Run alerting performance tests
pytest monitoring/tests/alerts/performance/ -v

# Run alerting integration tests
pytest monitoring/tests/alerts/integration/ -v
```

Validate alerting system test coverage:
```bash
pytest monitoring/tests/alerts/ --cov=monitoring.alerts --cov-report=html --cov-fail-under=85
```

## Integration Testing

Test alerting system integration with platform components:
```bash
# Test integration with Session 11.1 (Metrics Collection)
pytest monitoring/tests/integration/test_alerts_metrics_integration.py -v

# Test integration with notification services
pytest monitoring/tests/integration/test_alerts_notification_integration.py -v

# Test alert escalation workflows
pytest monitoring/tests/integration/test_alerts_escalation_integration.py -v
```