"""WebSocket-specific alert system for monitoring and notifications."""

import asyncio
import time
from typing import Dict, Any, List, Optional, Callable, Union
from enum import Enum
from dataclasses import dataclass, asdict
import structlog

from .metrics import get_websocket_metrics
from .redis_storage import get_redis_storage

logger = structlog.get_logger(__name__)


class AlertSeverity(Enum):
    """Alert severity levels."""
    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"
    CRITICAL = "critical"


class AlertCategory(Enum):
    """Alert categories for organization."""
    CONNECTION = "connection"
    AUTHENTICATION = "authentication"
    PERFORMANCE = "performance"
    SECURITY = "security"
    RESOURCE = "resource"
    ERROR = "error"


@dataclass
class AlertThreshold:
    """Configuration for an alert threshold."""
    name: str
    description: str
    category: AlertCategory
    severity: AlertSeverity
    metric_name: str
    operator: str  # >, <, >=, <=, ==, !=
    threshold_value: Union[int, float]
    window_seconds: int = 300  # 5 minutes default
    min_occurrences: int = 1
    cooldown_seconds: int = 900  # 15 minutes default


@dataclass
class Alert:
    """An active alert."""
    id: str
    threshold: AlertThreshold
    current_value: Union[int, float]
    first_triggered: float
    last_triggered: float
    occurrence_count: int
    acknowledged: bool = False
    acknowledged_at: Optional[float] = None
    acknowledged_by: Optional[str] = None
    resolved: bool = False
    resolved_at: Optional[float] = None
    metadata: Dict[str, Any] = None


class WebSocketAlertManager:
    """Manage WebSocket-specific alerts and notifications."""
    
    def __init__(self):
        self.redis_storage = None
        self.metrics_collector = None
        
        # Alert configuration
        self.thresholds = self._initialize_thresholds()
        
        # Active alerts tracking
        self.active_alerts: Dict[str, Alert] = {}
        self.alert_history: List[Alert] = []
        
        # Notification handlers
        self.notification_handlers: List[Callable] = []
        
        # Alert checking interval
        self.check_interval = 60  # Check every minute
        self.check_task: Optional[asyncio.Task] = None
        
    def _initialize_thresholds(self) -> List[AlertThreshold]:
        """Initialize default alert thresholds based on Session 5 requirements."""
        return [
            # Connection failure alerts
            AlertThreshold(
                name="high_connection_failure_rate",
                description="Connection failure rate exceeds 5%",
                category=AlertCategory.CONNECTION,
                severity=AlertSeverity.HIGH,
                metric_name="connection_failure_percentage",
                operator=">",
                threshold_value=5.0,
                window_seconds=300,
                min_occurrences=3
            ),
            
            # Authentication failure alerts
            AlertThreshold(
                name="high_auth_failure_rate",
                description="Authentication failure rate exceeds 10%",
                category=AlertCategory.AUTHENTICATION,
                severity=AlertSeverity.MEDIUM,
                metric_name="auth_failure_percentage",
                operator=">",
                threshold_value=10.0,
                window_seconds=300,
                min_occurrences=2
            ),
            
            # Rate limiting alerts
            AlertThreshold(
                name="high_rate_limiting_violations",
                description="Rate limiting violations affect >10% of connections",
                category=AlertCategory.SECURITY,
                severity=AlertSeverity.MEDIUM,
                metric_name="rate_limit_violation_percentage",
                operator=">",
                threshold_value=10.0,
                window_seconds=300,
                min_occurrences=2
            ),
            
            # Message processing delays
            AlertThreshold(
                name="message_processing_delays",
                description="Message processing latency exceeds 1 second",
                category=AlertCategory.PERFORMANCE,
                severity=AlertSeverity.MEDIUM,
                metric_name="message_processing_p95",
                operator=">",
                threshold_value=1000,  # milliseconds
                window_seconds=300,
                min_occurrences=3
            ),
            
            # High error rate
            AlertThreshold(
                name="high_websocket_error_rate",
                description="WebSocket error rate exceeds 1%",
                category=AlertCategory.ERROR,
                severity=AlertSeverity.HIGH,
                metric_name="websocket_error_percentage",
                operator=">",
                threshold_value=1.0,
                window_seconds=300,
                min_occurrences=2
            ),
            
            # Resource usage alerts
            AlertThreshold(
                name="high_memory_usage",
                description="Memory usage per connection exceeds 80%",
                category=AlertCategory.RESOURCE,
                severity=AlertSeverity.MEDIUM,
                metric_name="memory_usage_percentage",
                operator=">",
                threshold_value=80.0,
                window_seconds=300,
                min_occurrences=3
            ),
            
            # Security alerts
            AlertThreshold(
                name="security_violations_detected",
                description="Multiple security violations detected",
                category=AlertCategory.SECURITY,
                severity=AlertSeverity.CRITICAL,
                metric_name="security_violations_count",
                operator=">",
                threshold_value=5,
                window_seconds=300,
                min_occurrences=1
            ),
            
            # Connection surge alerts
            AlertThreshold(
                name="connection_surge",
                description="Unusual spike in connection attempts",
                category=AlertCategory.CONNECTION,
                severity=AlertSeverity.MEDIUM,
                metric_name="connection_attempt_rate",
                operator=">",
                threshold_value=100,  # connections per minute
                window_seconds=60,
                min_occurrences=2
            ),
            
            # Low connection success rate
            AlertThreshold(
                name="low_connection_success_rate",
                description="Connection success rate below 90%",
                category=AlertCategory.CONNECTION,
                severity=AlertSeverity.MEDIUM,
                metric_name="connection_success_percentage",
                operator="<",
                threshold_value=90.0,
                window_seconds=300,
                min_occurrences=3
            ),
            
            # Message queue backup
            AlertThreshold(
                name="message_queue_backup",
                description="Message queues backing up",
                category=AlertCategory.PERFORMANCE,
                severity=AlertSeverity.HIGH,
                metric_name="average_queue_depth",
                operator=">",
                threshold_value=500,
                window_seconds=300,
                min_occurrences=2
            )
        ]
    
    async def initialize(self):
        """Initialize the alert manager."""
        try:
            # Try to initialize Redis storage, continue without it if it fails
            try:
                self.redis_storage = await get_redis_storage()
            except Exception as e:
                logger.warning("Redis not available for alert manager", error=str(e))
                self.redis_storage = None
            
            self.metrics_collector = await get_websocket_metrics()
            
            # Start alert checking task
            self.check_task = asyncio.create_task(self._alert_check_loop())
            
            logger.info("WebSocket alert manager initialized", 
                       thresholds_count=len(self.thresholds),
                       redis_available=self.redis_storage is not None)
            
        except Exception as e:
            logger.error("Failed to initialize alert manager", error=str(e))
            # Continue without Redis if needed
            self.redis_storage = None
            if self.metrics_collector is None:
                raise
    
    async def shutdown(self):
        """Shutdown the alert manager."""
        if self.check_task:
            self.check_task.cancel()
        
        logger.info("WebSocket alert manager shutdown")
    
    def add_notification_handler(self, handler: Callable[[Alert], None]):
        """Add a notification handler for alerts."""
        self.notification_handlers.append(handler)
        logger.info("Notification handler added", 
                   total_handlers=len(self.notification_handlers))
    
    async def _alert_check_loop(self):
        """Main alert checking loop."""
        while True:
            try:
                await asyncio.sleep(self.check_interval)
                await self._check_all_thresholds()
                await self._cleanup_resolved_alerts()
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Error in alert check loop", error=str(e))
    
    async def _check_all_thresholds(self):
        """Check all configured thresholds."""
        current_time = time.time()
        
        # Get current metrics
        metrics = self.metrics_collector.get_comprehensive_metrics()
        
        # Calculate additional derived metrics
        derived_metrics = await self._calculate_derived_metrics(metrics)
        all_metrics = {**metrics, **derived_metrics}
        
        for threshold in self.thresholds:
            try:
                await self._check_threshold(threshold, all_metrics, current_time)
            except Exception as e:
                logger.error("Error checking threshold", 
                           threshold=threshold.name, error=str(e))
    
    async def _calculate_derived_metrics(self, base_metrics: Dict[str, Any]) -> Dict[str, Any]:
        """Calculate derived metrics needed for alerts."""
        derived = {}
        
        # Connection metrics
        conn_data = base_metrics.get("connections", {})
        total_attempts = conn_data.get("total_attempted", 0)
        total_failed = conn_data.get("total_failed", 0)
        
        if total_attempts > 0:
            derived["connection_failure_percentage"] = (total_failed / total_attempts) * 100
            derived["connection_success_percentage"] = ((total_attempts - total_failed) / total_attempts) * 100
        else:
            derived["connection_failure_percentage"] = 0
            derived["connection_success_percentage"] = 100
        
        # Authentication metrics
        auth_data = base_metrics.get("authentication", {})
        total_auth_attempts = auth_data.get("total_attempts", 0)
        total_auth_failures = auth_data.get("total_failures", 0)
        
        if total_auth_attempts > 0:
            derived["auth_failure_percentage"] = (total_auth_failures / total_auth_attempts) * 100
        else:
            derived["auth_failure_percentage"] = 0
        
        # Performance metrics
        perf_data = base_metrics.get("performance", {})
        response_stats = perf_data.get("response_time_stats", {})
        derived["message_processing_p95"] = response_stats.get("p95", 0) * 1000  # Convert to ms
        
        # Error rate calculation
        total_messages = base_metrics.get("messages", {}).get("total_sent", 0) + \
                        base_metrics.get("messages", {}).get("total_received", 0)
        total_errors = perf_data.get("total_errors", 0)
        
        if total_messages > 0:
            derived["websocket_error_percentage"] = (total_errors / total_messages) * 100
        else:
            derived["websocket_error_percentage"] = 0
        
        # Resource metrics (simplified - would need actual system monitoring)
        derived["memory_usage_percentage"] = 50.0  # Placeholder
        derived["average_queue_depth"] = 10  # Placeholder
        
        # Security metrics
        derived["security_violations_count"] = 0  # Would be calculated from security logs
        
        # Rate limiting violations (would need rate limiter integration)
        derived["rate_limit_violation_percentage"] = 2.0  # Placeholder
        
        # Connection rate
        conn_rates = conn_data.get("rates", {})
        derived["connection_attempt_rate"] = conn_rates.get("attempt_rate", 0) * 60  # per minute
        
        return derived
    
    async def _check_threshold(self, threshold: AlertThreshold, metrics: Dict[str, Any], current_time: float):
        """Check a specific threshold against current metrics."""
        # Extract metric value
        metric_value = self._extract_metric_value(threshold.metric_name, metrics)
        
        if metric_value is None:
            logger.warning("Metric not found", metric_name=threshold.metric_name)
            return
        
        # Check if threshold is violated
        violation = self._evaluate_threshold(metric_value, threshold.operator, threshold.threshold_value)
        
        alert_id = f"{threshold.name}_{int(current_time / threshold.window_seconds)}"
        
        if violation:
            await self._handle_threshold_violation(alert_id, threshold, metric_value, current_time)
        else:
            await self._handle_threshold_recovery(alert_id, threshold, current_time)
    
    def _extract_metric_value(self, metric_path: str, metrics: Dict[str, Any]) -> Optional[Union[int, float]]:
        """Extract a metric value from nested metrics dictionary."""
        try:
            # Handle nested paths like "connections.rates.failure_rate"
            keys = metric_path.split('.')
            value = metrics
            
            for key in keys:
                if isinstance(value, dict) and key in value:
                    value = value[key]
                else:
                    # Try direct key access
                    if metric_path in metrics:
                        return metrics[metric_path]
                    return None
            
            return value if isinstance(value, (int, float)) else None
            
        except Exception as e:
            logger.error("Error extracting metric value", 
                        metric_path=metric_path, error=str(e))
            return None
    
    def _evaluate_threshold(self, value: Union[int, float], operator: str, threshold: Union[int, float]) -> bool:
        """Evaluate if a value violates a threshold."""
        operators = {
            ">": lambda v, t: v > t,
            "<": lambda v, t: v < t,
            ">=": lambda v, t: v >= t,
            "<=": lambda v, t: v <= t,
            "==": lambda v, t: v == t,
            "!=": lambda v, t: v != t
        }
        
        if operator not in operators:
            logger.error("Unknown operator", operator=operator)
            return False
        
        return operators[operator](value, threshold)
    
    async def _handle_threshold_violation(self, alert_id: str, threshold: AlertThreshold, 
                                        current_value: Union[int, float], current_time: float):
        """Handle a threshold violation."""
        if alert_id in self.active_alerts:
            # Update existing alert
            alert = self.active_alerts[alert_id]
            alert.last_triggered = current_time
            alert.occurrence_count += 1
            alert.current_value = current_value
        else:
            # Create new alert
            alert = Alert(
                id=alert_id,
                threshold=threshold,
                current_value=current_value,
                first_triggered=current_time,
                last_triggered=current_time,
                occurrence_count=1,
                metadata={"metric_name": threshold.metric_name}
            )
            self.active_alerts[alert_id] = alert
        
        # Check if we should fire the alert (min occurrences met)
        if alert.occurrence_count >= threshold.min_occurrences:
            await self._fire_alert(alert)
    
    async def _handle_threshold_recovery(self, alert_id: str, threshold: AlertThreshold, current_time: float):
        """Handle threshold recovery (no longer violated)."""
        if alert_id in self.active_alerts:
            alert = self.active_alerts[alert_id]
            
            # Check cooldown period
            if current_time - alert.last_triggered > threshold.cooldown_seconds:
                await self._resolve_alert(alert, current_time)
    
    async def _fire_alert(self, alert: Alert):
        """Fire an alert to all notification handlers."""
        logger.warning("Alert triggered", 
                      alert_id=alert.id,
                      severity=alert.threshold.severity.value,
                      description=alert.threshold.description,
                      current_value=alert.current_value,
                      threshold_value=alert.threshold.threshold_value,
                      occurrence_count=alert.occurrence_count)
        
        # Store alert in Redis
        if self.redis_storage:
            await self._store_alert_in_redis(alert)
        
        # Notify handlers
        for handler in self.notification_handlers:
            try:
                if asyncio.iscoroutinefunction(handler):
                    await handler(alert)
                else:
                    handler(alert)
            except Exception as e:
                logger.error("Error in notification handler", error=str(e))
    
    async def _resolve_alert(self, alert: Alert, current_time: float):
        """Resolve an alert."""
        alert.resolved = True
        alert.resolved_at = current_time
        
        logger.info("Alert resolved", 
                   alert_id=alert.id,
                   duration=current_time - alert.first_triggered)
        
        # Move to history
        self.alert_history.append(alert)
        del self.active_alerts[alert.id]
        
        # Store resolution in Redis
        if self.redis_storage:
            await self._store_alert_in_redis(alert)
    
    async def _cleanup_resolved_alerts(self):
        """Clean up old resolved alerts from history."""
        current_time = time.time()
        cutoff_time = current_time - 86400  # Keep 24 hours of history
        
        self.alert_history = [
            alert for alert in self.alert_history 
            if alert.resolved_at and alert.resolved_at > cutoff_time
        ]
    
    async def _store_alert_in_redis(self, alert: Alert):
        """Store alert in Redis for persistence."""
        try:
            if not self.redis_storage or not self.redis_storage.redis_client:
                return
            
            alert_data = asdict(alert)
            # Convert enum to string for serialization
            alert_data["threshold"]["category"] = alert.threshold.category.value
            alert_data["threshold"]["severity"] = alert.threshold.severity.value
            
            key = f"ws:alert:{alert.id}"
            encrypted_data = self.redis_storage.encryption.encrypt_connection_state(alert_data)
            
            # Store with 7 day TTL
            await self.redis_storage.redis_client.setex(key, 604800, encrypted_data)
            
        except Exception as e:
            logger.error("Failed to store alert in Redis", error=str(e))
    
    async def acknowledge_alert(self, alert_id: str, acknowledged_by: str):
        """Acknowledge an active alert."""
        if alert_id in self.active_alerts:
            alert = self.active_alerts[alert_id]
            alert.acknowledged = True
            alert.acknowledged_at = time.time()
            alert.acknowledged_by = acknowledged_by
            
            logger.info("Alert acknowledged", 
                       alert_id=alert_id, 
                       acknowledged_by=acknowledged_by)
            
            if self.redis_storage:
                await self._store_alert_in_redis(alert)
            
            return True
        return False
    
    def get_active_alerts(self, category: Optional[AlertCategory] = None, 
                         severity: Optional[AlertSeverity] = None) -> List[Alert]:
        """Get active alerts, optionally filtered."""
        alerts = list(self.active_alerts.values())
        
        if category:
            alerts = [a for a in alerts if a.threshold.category == category]
        
        if severity:
            alerts = [a for a in alerts if a.threshold.severity == severity]
        
        return alerts
    
    def get_alert_summary(self) -> Dict[str, Any]:
        """Get a summary of alert status."""
        active_alerts = list(self.active_alerts.values())
        
        summary = {
            "timestamp": time.time(),
            "total_active": len(active_alerts),
            "total_resolved_24h": len(self.alert_history),
            "by_severity": {
                "critical": len([a for a in active_alerts if a.threshold.severity == AlertSeverity.CRITICAL]),
                "high": len([a for a in active_alerts if a.threshold.severity == AlertSeverity.HIGH]),
                "medium": len([a for a in active_alerts if a.threshold.severity == AlertSeverity.MEDIUM]),
                "low": len([a for a in active_alerts if a.threshold.severity == AlertSeverity.LOW])
            },
            "by_category": {
                "connection": len([a for a in active_alerts if a.threshold.category == AlertCategory.CONNECTION]),
                "authentication": len([a for a in active_alerts if a.threshold.category == AlertCategory.AUTHENTICATION]),
                "performance": len([a for a in active_alerts if a.threshold.category == AlertCategory.PERFORMANCE]),
                "security": len([a for a in active_alerts if a.threshold.category == AlertCategory.SECURITY]),
                "resource": len([a for a in active_alerts if a.threshold.category == AlertCategory.RESOURCE]),
                "error": len([a for a in active_alerts if a.threshold.category == AlertCategory.ERROR])
            },
            "acknowledged": len([a for a in active_alerts if a.acknowledged]),
            "unacknowledged": len([a for a in active_alerts if not a.acknowledged])
        }
        
        return summary


# Global instance
_websocket_alert_manager = None


async def get_websocket_alert_manager() -> WebSocketAlertManager:
    """Get global WebSocket alert manager instance."""
    global _websocket_alert_manager
    if _websocket_alert_manager is None:
        _websocket_alert_manager = WebSocketAlertManager()
        await _websocket_alert_manager.initialize()
    return _websocket_alert_manager


# Default notification handlers

def log_alert_handler(alert: Alert):
    """Default log-based alert handler."""
    severity_level = {
        AlertSeverity.LOW: "info",
        AlertSeverity.MEDIUM: "warning", 
        AlertSeverity.HIGH: "error",
        AlertSeverity.CRITICAL: "critical"
    }
    
    level = severity_level.get(alert.threshold.severity, "warning")
    logger.log(level, "WebSocket Alert", 
               alert_id=alert.id,
               severity=alert.threshold.severity.value,
               category=alert.threshold.category.value,
               description=alert.threshold.description,
               current_value=alert.current_value,
               threshold_value=alert.threshold.threshold_value,
               occurrence_count=alert.occurrence_count)


async def webhook_alert_handler(alert: Alert, webhook_url: str):
    """Webhook-based alert handler."""
    import httpx
    
    payload = {
        "alert_id": alert.id,
        "severity": alert.threshold.severity.value,
        "category": alert.threshold.category.value,
        "description": alert.threshold.description,
        "current_value": alert.current_value,
        "threshold_value": alert.threshold.threshold_value,
        "occurrence_count": alert.occurrence_count,
        "first_triggered": alert.first_triggered,
        "last_triggered": alert.last_triggered
    }
    
    try:
        async with httpx.AsyncClient() as client:
            response = await client.post(webhook_url, json=payload, timeout=10)
            response.raise_for_status()
            logger.info("Alert sent to webhook", alert_id=alert.id, webhook_url=webhook_url)
    except Exception as e:
        logger.error("Failed to send alert to webhook", 
                    alert_id=alert.id, webhook_url=webhook_url, error=str(e))