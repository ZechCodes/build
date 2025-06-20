"""Monitoring alerts and notification system."""

import asyncio
import time
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Any, Callable
from enum import Enum
import structlog

from ..core.redis import get_redis
from ..core.config import get_settings

logger = structlog.get_logger(__name__)

class AlertSeverity(Enum):
    """Alert severity levels."""
    CRITICAL = "critical"
    HIGH = "high"
    MEDIUM = "medium"
    LOW = "low"
    INFO = "info"

class AlertStatus(Enum):
    """Alert status."""
    FIRING = "firing"
    RESOLVED = "resolved"
    ACKNOWLEDGED = "acknowledged"
    MUTED = "muted"

class Alert:
    """Alert model."""
    
    def __init__(
        self,
        name: str,
        severity: AlertSeverity,
        message: str,
        labels: Optional[Dict[str, str]] = None,
        annotations: Optional[Dict[str, str]] = None,
        firing_time: Optional[datetime] = None
    ):
        self.name = name
        self.severity = severity
        self.message = message
        self.labels = labels or {}
        self.annotations = annotations or {}
        self.firing_time = firing_time or datetime.utcnow()
        self.status = AlertStatus.FIRING
        self.acknowledged_by: Optional[str] = None
        self.resolved_time: Optional[datetime] = None
        self.id = f"{name}_{int(time.time())}"
    
    def to_dict(self) -> Dict[str, Any]:
        """Convert alert to dictionary."""
        return {
            "id": self.id,
            "name": self.name,
            "severity": self.severity.value,
            "message": self.message,
            "labels": self.labels,
            "annotations": self.annotations,
            "firing_time": self.firing_time.isoformat(),
            "status": self.status.value,
            "acknowledged_by": self.acknowledged_by,
            "resolved_time": self.resolved_time.isoformat() if self.resolved_time else None
        }
    
    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> 'Alert':
        """Create alert from dictionary."""
        alert = cls(
            name=data["name"],
            severity=AlertSeverity(data["severity"]),
            message=data["message"],
            labels=data.get("labels", {}),
            annotations=data.get("annotations", {}),
            firing_time=datetime.fromisoformat(data["firing_time"])
        )
        alert.id = data["id"]
        alert.status = AlertStatus(data["status"])
        alert.acknowledged_by = data.get("acknowledged_by")
        if data.get("resolved_time"):
            alert.resolved_time = datetime.fromisoformat(data["resolved_time"])
        return alert

class AlertRule:
    """Alert rule configuration."""
    
    def __init__(
        self,
        name: str,
        condition: Callable[[], bool],
        severity: AlertSeverity,
        message: str,
        check_interval: int = 60,
        labels: Optional[Dict[str, str]] = None,
        annotations: Optional[Dict[str, str]] = None,
        cooldown_minutes: int = 5
    ):
        self.name = name
        self.condition = condition
        self.severity = severity
        self.message = message
        self.check_interval = check_interval
        self.labels = labels or {}
        self.annotations = annotations or {}
        self.cooldown_minutes = cooldown_minutes
        self.last_check = 0
        self.last_fired = 0
        self.is_enabled = True

class AlertManager:
    """Alert management system."""
    
    def __init__(self):
        self.rules: Dict[str, AlertRule] = {}
        self.active_alerts: Dict[str, Alert] = {}
        self.notification_handlers: List[Callable[[Alert], None]] = []
        self.settings = get_settings()
        self._monitoring_task: Optional[asyncio.Task] = None
    
    def add_rule(self, rule: AlertRule) -> None:
        """Add an alert rule."""
        self.rules[rule.name] = rule
        logger.info("Alert rule added", rule_name=rule.name, severity=rule.severity.value)
    
    def remove_rule(self, rule_name: str) -> None:
        """Remove an alert rule."""
        if rule_name in self.rules:
            del self.rules[rule_name]
            logger.info("Alert rule removed", rule_name=rule_name)
    
    def enable_rule(self, rule_name: str) -> None:
        """Enable an alert rule."""
        if rule_name in self.rules:
            self.rules[rule_name].is_enabled = True
            logger.info("Alert rule enabled", rule_name=rule_name)
    
    def disable_rule(self, rule_name: str) -> None:
        """Disable an alert rule."""
        if rule_name in self.rules:
            self.rules[rule_name].is_enabled = False
            logger.info("Alert rule disabled", rule_name=rule_name)
    
    def add_notification_handler(self, handler: Callable[[Alert], None]) -> None:
        """Add a notification handler."""
        self.notification_handlers.append(handler)
    
    async def fire_alert(
        self,
        name: str,
        severity: AlertSeverity,
        message: str,
        labels: Optional[Dict[str, str]] = None,
        annotations: Optional[Dict[str, str]] = None
    ) -> Alert:
        """Fire an alert."""
        alert = Alert(
            name=name,
            severity=severity,
            message=message,
            labels=labels,
            annotations=annotations
        )
        
        # Store alert
        self.active_alerts[alert.id] = alert
        await self._store_alert(alert)
        
        # Send notifications
        await self._send_notifications(alert)
        
        logger.warning(
            "Alert fired",
            alert_id=alert.id,
            name=name,
            severity=severity.value,
            message=message
        )
        
        return alert
    
    async def resolve_alert(self, alert_id: str, resolved_by: Optional[str] = None) -> bool:
        """Resolve an alert."""
        if alert_id in self.active_alerts:
            alert = self.active_alerts[alert_id]
            alert.status = AlertStatus.RESOLVED
            alert.resolved_time = datetime.utcnow()
            
            await self._store_alert(alert)
            
            logger.info(
                "Alert resolved",
                alert_id=alert_id,
                resolved_by=resolved_by
            )
            
            # Remove from active alerts
            del self.active_alerts[alert_id]
            return True
        
        return False
    
    async def acknowledge_alert(self, alert_id: str, acknowledged_by: str) -> bool:
        """Acknowledge an alert."""
        if alert_id in self.active_alerts:
            alert = self.active_alerts[alert_id]
            alert.status = AlertStatus.ACKNOWLEDGED
            alert.acknowledged_by = acknowledged_by
            
            await self._store_alert(alert)
            
            logger.info(
                "Alert acknowledged",
                alert_id=alert_id,
                acknowledged_by=acknowledged_by
            )
            
            return True
        
        return False\n    \n    async def get_alerts(\n        self,\n        status: Optional[AlertStatus] = None,\n        severity: Optional[AlertSeverity] = None,\n        limit: int = 100\n    ) -> List[Alert]:\n        \"\"\"Get alerts with filtering.\"\"\"\n        alerts = list(self.active_alerts.values())\n        \n        if status:\n            alerts = [a for a in alerts if a.status == status]\n        \n        if severity:\n            alerts = [a for a in alerts if a.severity == severity]\n        \n        # Sort by firing time (newest first)\n        alerts.sort(key=lambda a: a.firing_time, reverse=True)\n        \n        return alerts[:limit]\n    \n    async def start_monitoring(self) -> None:\n        \"\"\"Start the alert monitoring task.\"\"\"\n        if self._monitoring_task is None or self._monitoring_task.done():\n            self._monitoring_task = asyncio.create_task(self._monitoring_loop())\n            logger.info(\"Alert monitoring started\")\n    \n    async def stop_monitoring(self) -> None:\n        \"\"\"Stop the alert monitoring task.\"\"\"\n        if self._monitoring_task and not self._monitoring_task.done():\n            self._monitoring_task.cancel()\n            try:\n                await self._monitoring_task\n            except asyncio.CancelledError:\n                pass\n            logger.info(\"Alert monitoring stopped\")\n    \n    async def _monitoring_loop(self) -> None:\n        \"\"\"Main monitoring loop.\"\"\"\n        while True:\n            try:\n                current_time = time.time()\n                \n                for rule in self.rules.values():\n                    if not rule.is_enabled:\n                        continue\n                    \n                    # Check if it's time to evaluate this rule\n                    if current_time - rule.last_check < rule.check_interval:\n                        continue\n                    \n                    rule.last_check = current_time\n                    \n                    try:\n                        # Evaluate the condition\n                        if await self._evaluate_condition(rule.condition):\n                            # Check cooldown\n                            if current_time - rule.last_fired < (rule.cooldown_minutes * 60):\n                                continue\n                            \n                            # Fire alert\n                            await self.fire_alert(\n                                name=rule.name,\n                                severity=rule.severity,\n                                message=rule.message,\n                                labels=rule.labels,\n                                annotations=rule.annotations\n                            )\n                            \n                            rule.last_fired = current_time\n                    \n                    except Exception as e:\n                        logger.error(\n                            \"Error evaluating alert rule\",\n                            rule_name=rule.name,\n                            error=str(e)\n                        )\n                \n                # Sleep for a short interval\n                await asyncio.sleep(10)\n            \n            except asyncio.CancelledError:\n                break\n            except Exception as e:\n                logger.error(\"Error in monitoring loop\", error=str(e))\n                await asyncio.sleep(60)  # Wait longer on error\n    \n    async def _evaluate_condition(self, condition: Callable[[], bool]) -> bool:\n        \"\"\"Evaluate alert condition safely.\"\"\"\n        try:\n            if asyncio.iscoroutinefunction(condition):\n                return await condition()\n            else:\n                return condition()\n        except Exception as e:\n            logger.error(\"Error evaluating condition\", error=str(e))\n            return False\n    \n    async def _store_alert(self, alert: Alert) -> None:\n        \"\"\"Store alert in Redis.\"\"\"\n        try:\n            redis = await get_redis()\n            alert_key = f\"alert:{alert.id}\"\n            await redis.setex(alert_key, 86400 * 7, str(alert.to_dict()))  # Store for 7 days\n        except Exception as e:\n            logger.error(\"Failed to store alert\", alert_id=alert.id, error=str(e))\n    \n    async def _send_notifications(self, alert: Alert) -> None:\n        \"\"\"Send alert notifications.\"\"\"\n        for handler in self.notification_handlers:\n            try:\n                if asyncio.iscoroutinefunction(handler):\n                    await handler(alert)\n                else:\n                    handler(alert)\n            except Exception as e:\n                logger.error(\n                    \"Failed to send notification\",\n                    alert_id=alert.id,\n                    handler=str(handler),\n                    error=str(e)\n                )\n\n\n# Global alert manager instance\n_alert_manager: Optional[AlertManager] = None\n\n\ndef get_alert_manager() -> AlertManager:\n    \"\"\"Get global alert manager instance.\"\"\"\n    global _alert_manager\n    if _alert_manager is None:\n        _alert_manager = AlertManager()\n    return _alert_manager\n\n\n# Notification handlers\n\nasync def log_notification_handler(alert: Alert) -> None:\n    \"\"\"Log notification handler.\"\"\"\n    logger.warning(\n        \"ALERT NOTIFICATION\",\n        alert_id=alert.id,\n        name=alert.name,\n        severity=alert.severity.value,\n        message=alert.message,\n        labels=alert.labels\n    )\n\n\nasync def webhook_notification_handler(alert: Alert) -> None:\n    \"\"\"Webhook notification handler.\"\"\"\n    import httpx\n    \n    webhook_url = get_settings().webhook_url if hasattr(get_settings(), 'webhook_url') else None\n    if not webhook_url:\n        return\n    \n    try:\n        payload = {\n            \"alert\": alert.to_dict(),\n            \"timestamp\": datetime.utcnow().isoformat()\n        }\n        \n        async with httpx.AsyncClient() as client:\n            response = await client.post(\n                webhook_url,\n                json=payload,\n                timeout=10\n            )\n            response.raise_for_status()\n            \n        logger.info(\"Webhook notification sent\", alert_id=alert.id, webhook_url=webhook_url)\n    \n    except Exception as e:\n        logger.error(\n            \"Failed to send webhook notification\",\n            alert_id=alert.id,\n            webhook_url=webhook_url,\n            error=str(e)\n        )\n\n\n# Built-in alert rules\n\ndef setup_default_alert_rules() -> None:\n    \"\"\"Setup default alert rules for the Build platform.\"\"\"\n    alert_manager = get_alert_manager()\n    \n    # Database connection pool alert\n    async def check_database_pool() -> bool:\n        try:\n            from ..core.deps import check_database_health\n            health = await check_database_health()\n            pool_usage = health.get(\"metrics\", {}).get(\"pool_usage_percent\", 0)\n            return pool_usage > 80\n        except Exception:\n            return False\n    \n    alert_manager.add_rule(AlertRule(\n        name=\"database_pool_high\",\n        condition=check_database_pool,\n        severity=AlertSeverity.HIGH,\n        message=\"Database connection pool usage is above 80%\",\n        check_interval=60,\n        labels={\"service\": \"database\", \"component\": \"connection_pool\"},\n        annotations={\"runbook\": \"Check database connection leaks and scale pool size\"}\n    ))\n    \n    # Redis memory alert\n    async def check_redis_memory() -> bool:\n        try:\n            from ..core.deps import check_redis_health\n            health = await check_redis_health()\n            memory_usage = health.get(\"metrics\", {}).get(\"memory_usage_percent\", 0)\n            return memory_usage > 80\n        except Exception:\n            return False\n    \n    alert_manager.add_rule(AlertRule(\n        name=\"redis_memory_high\",\n        condition=check_redis_memory,\n        severity=AlertSeverity.HIGH,\n        message=\"Redis memory usage is above 80%\",\n        check_interval=60,\n        labels={\"service\": \"redis\", \"component\": \"memory\"},\n        annotations={\"runbook\": \"Check Redis memory usage and clean up expired keys\"}\n    ))\n    \n    # API error rate alert\n    async def check_api_error_rate() -> bool:\n        try:\n            from ..monitoring.metrics import metrics_collector\n            error_rate = metrics_collector.get_error_rate(window_minutes=5)\n            return error_rate > 0.01  # 1% error rate\n        except Exception:\n            return False\n    \n    alert_manager.add_rule(AlertRule(\n        name=\"api_error_rate_high\",\n        condition=check_api_error_rate,\n        severity=AlertSeverity.MEDIUM,\n        message=\"API error rate is above 1% in the last 5 minutes\",\n        check_interval=30,\n        labels={\"service\": \"api\", \"component\": \"errors\"},\n        annotations={\"runbook\": \"Check API logs and investigate error causes\"}\n    ))\n    \n    # Failed authentication attempts alert\n    async def check_failed_auth_attempts() -> bool:\n        try:\n            redis = await get_redis()\n            failed_attempts = 0\n            \n            # Count recent failed auth attempts\n            pattern = \"auth_attempts:*\"\n            keys = await redis.keys(pattern)\n            \n            for key in keys:\n                attempts = await redis.get(key)\n                if attempts and int(attempts) >= 5:\n                    failed_attempts += 1\n            \n            return failed_attempts > 10  # More than 10 accounts with 5+ failed attempts\n        except Exception:\n            return False\n    \n    alert_manager.add_rule(AlertRule(\n        name=\"auth_attacks_detected\",\n        condition=check_failed_auth_attempts,\n        severity=AlertSeverity.CRITICAL,\n        message=\"Potential brute force attack detected - multiple accounts with failed login attempts\",\n        check_interval=30,\n        labels={\"service\": \"auth\", \"component\": \"security\"},\n        annotations={\"runbook\": \"Investigate IP addresses and consider temporary IP blocking\"}\n    ))\n    \n    # System disk space alert\n    async def check_disk_space() -> bool:\n        try:\n            import shutil\n            total, used, free = shutil.disk_usage(\"/\")\n            usage_percent = (used / total) * 100\n            return usage_percent > 85\n        except Exception:\n            return False\n    \n    alert_manager.add_rule(AlertRule(\n        name=\"disk_space_low\",\n        condition=check_disk_space,\n        severity=AlertSeverity.HIGH,\n        message=\"System disk usage is above 85%\",\n        check_interval=300,  # Check every 5 minutes\n        labels={\"service\": \"system\", \"component\": \"disk\"},\n        annotations={\"runbook\": \"Clean up logs and temporary files, consider disk expansion\"}\n    ))\n    \n    # Add notification handlers\n    alert_manager.add_notification_handler(log_notification_handler)\n    \n    # Add webhook handler if configured\n    settings = get_settings()\n    if hasattr(settings, 'webhook_url') and settings.webhook_url:\n        alert_manager.add_notification_handler(webhook_notification_handler)\n    \n    logger.info(\"Default alert rules configured\", rule_count=len(alert_manager.rules))