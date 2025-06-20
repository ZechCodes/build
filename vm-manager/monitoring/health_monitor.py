"""VM health monitoring system with comprehensive metrics and alerting."""

import asyncio
import time
import psutil
import json
from typing import Dict, List, Optional, Any
from datetime import datetime, timezone
from dataclasses import dataclass
from enum import Enum

import structlog

from config.settings import VMManagerSettings

logger = structlog.get_logger(__name__)


class HealthStatus(Enum):
    """VM health status levels."""
    HEALTHY = "healthy"
    WARNING = "warning"
    CRITICAL = "critical"
    UNKNOWN = "unknown"
    
    def __lt__(self, other):
        """Enable comparison for status priority."""
        order = {
            HealthStatus.HEALTHY: 0,
            HealthStatus.WARNING: 1,
            HealthStatus.CRITICAL: 2,
            HealthStatus.UNKNOWN: 3
        }
        return order[self] < order[other]


@dataclass
class HealthMetric:
    """Individual health metric data."""
    name: str
    value: float
    unit: str
    status: HealthStatus
    threshold_warning: Optional[float] = None
    threshold_critical: Optional[float] = None
    timestamp: Optional[datetime] = None
    
    def __post_init__(self):
        if self.timestamp is None:
            self.timestamp = datetime.now(timezone.utc)


@dataclass
class VMHealthReport:
    """Comprehensive VM health report."""
    vm_id: str
    overall_status: HealthStatus
    metrics: List[HealthMetric]
    uptime: float
    last_check: datetime
    error_count: int = 0
    warnings: List[str] = None
    
    def __post_init__(self):
        if self.warnings is None:
            self.warnings = []


class HealthMonitorError(Exception):
    """Exception raised when health monitoring operations fail."""
    pass


class VMHealthMonitor:
    """Monitors VM health with metrics collection and alerting."""
    
    def __init__(
        self,
        settings: Optional[VMManagerSettings] = None,
        check_interval: int = 30,
        max_error_count: int = 3
    ):
        from config.settings import settings as default_settings
        self.settings = settings or default_settings
        
        self.check_interval = check_interval
        self.max_error_count = max_error_count
        
        # Health monitoring state
        self.vm_health_reports: Dict[str, VMHealthReport] = {}
        self.monitoring_tasks: Dict[str, asyncio.Task] = {}
        self.is_monitoring = False
        self.global_monitor_task: Optional[asyncio.Task] = None
        
        # Metrics thresholds
        self.thresholds = {
            "cpu_usage": {"warning": 80.0, "critical": 95.0},
            "memory_usage": {"warning": 85.0, "critical": 95.0},
            "disk_usage": {"warning": 90.0, "critical": 98.0},
            "response_time": {"warning": 5.0, "critical": 10.0},
            "error_rate": {"warning": 5.0, "critical": 10.0}
        }
        
        logger.info("VM health monitor initialized", interval=check_interval)
    
    async def start_monitoring(self) -> None:
        """Start global health monitoring service."""
        if self.is_monitoring:
            logger.warning("Health monitoring already running")
            return
        
        self.is_monitoring = True
        self.global_monitor_task = asyncio.create_task(self._global_monitor_loop())
        
        logger.info("VM health monitoring started")
    
    async def stop_monitoring(self) -> None:
        """Stop all health monitoring."""
        self.is_monitoring = False
        
        # Cancel global monitor
        if self.global_monitor_task:
            self.global_monitor_task.cancel()
            try:
                await self.global_monitor_task
            except asyncio.CancelledError:
                pass
        
        # Cancel individual VM monitors
        for task in self.monitoring_tasks.values():
            task.cancel()
        
        if self.monitoring_tasks:
            try:
                await asyncio.gather(*self.monitoring_tasks.values(), return_exceptions=True)
            except Exception as e:
                logger.error("Error stopping monitoring tasks", error=str(e))
        
        self.monitoring_tasks.clear()
        
        logger.info("VM health monitoring stopped")
    
    async def add_vm_monitoring(self, vm_id: str, process_info: Dict[str, Any]) -> None:
        """Add a VM to health monitoring."""
        try:
            self._validate_vm_id(vm_id)
            
            if vm_id in self.monitoring_tasks:
                logger.warning("VM already being monitored", vm_id=vm_id)
                return
            
            # Initialize health report
            self.vm_health_reports[vm_id] = VMHealthReport(
                vm_id=vm_id,
                overall_status=HealthStatus.UNKNOWN,
                metrics=[],
                uptime=0.0,
                last_check=datetime.now(timezone.utc)
            )
            
            # Start monitoring task
            monitor_task = asyncio.create_task(
                self._monitor_vm_loop(vm_id, process_info)
            )
            self.monitoring_tasks[vm_id] = monitor_task
            
            logger.info("VM health monitoring added", vm_id=vm_id)
            
        except Exception as e:
            logger.error("Failed to add VM monitoring", vm_id=vm_id, error=str(e))
            raise HealthMonitorError(f"Failed to add VM monitoring: {e}")
    
    async def remove_vm_monitoring(self, vm_id: str) -> None:
        """Remove a VM from health monitoring."""
        try:
            self._validate_vm_id(vm_id)
            
            # Cancel monitoring task
            if vm_id in self.monitoring_tasks:
                task = self.monitoring_tasks[vm_id]
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass
                del self.monitoring_tasks[vm_id]
            
            # Remove health report
            if vm_id in self.vm_health_reports:
                del self.vm_health_reports[vm_id]
            
            logger.info("VM health monitoring removed", vm_id=vm_id)
            
        except Exception as e:
            logger.error("Failed to remove VM monitoring", vm_id=vm_id, error=str(e))
    
    async def get_vm_health(self, vm_id: str) -> Optional[VMHealthReport]:
        """Get current health report for a VM."""
        try:
            self._validate_vm_id(vm_id)
            return self.vm_health_reports.get(vm_id)
        except Exception as e:
            logger.error("Failed to get VM health", vm_id=vm_id, error=str(e))
            return None
    
    async def get_all_vm_health(self) -> Dict[str, VMHealthReport]:
        """Get health reports for all monitored VMs."""
        return self.vm_health_reports.copy()
    
    async def check_vm_health(self, vm_id: str, process_info: Dict[str, Any]) -> VMHealthReport:
        """Perform immediate health check for a VM."""
        try:
            self._validate_vm_id(vm_id)
            
            metrics = []
            warnings = []
            overall_status = HealthStatus.HEALTHY
            
            # Check process status
            process = process_info.get("process")
            if process:
                # CPU usage metric
                try:
                    if hasattr(process, 'pid') and process.pid:
                        proc = psutil.Process(process.pid)
                        
                        # CPU usage
                        cpu_usage = proc.cpu_percent(interval=0.1)
                        cpu_metric = self._create_metric(
                            "cpu_usage", cpu_usage, "%", "cpu_usage"
                        )
                        metrics.append(cpu_metric)
                        if cpu_metric.status != HealthStatus.HEALTHY:
                            overall_status = max(overall_status, cpu_metric.status)
                        
                        # Memory usage as percentage of system memory
                        memory_info = proc.memory_info()
                        memory_usage_mb = (memory_info.rss / (1024 * 1024))  # MB
                        
                        # For testing, use a fixed threshold approach
                        # In production, this would be percentage of available memory
                        memory_usage_percent = min(memory_usage_mb / 10, 100)  # Assume 1GB = 100%
                        memory_metric = self._create_metric(
                            "memory_usage", memory_usage_percent, "%", "memory_usage"
                        )
                        metrics.append(memory_metric)
                        if memory_metric.status != HealthStatus.HEALTHY:
                            overall_status = max(overall_status, memory_metric.status)
                        
                        # Check if process is still running
                        if not proc.is_running():
                            warnings.append("Process is no longer running")
                            overall_status = HealthStatus.CRITICAL
                
                except (psutil.NoSuchProcess, psutil.AccessDenied) as e:
                    warnings.append(f"Cannot access process metrics: {e}")
                    overall_status = HealthStatus.WARNING
            else:
                warnings.append("No process information available")
                overall_status = HealthStatus.CRITICAL
            
            # Calculate uptime
            started_at = process_info.get("started_at")
            uptime = 0.0
            if started_at:
                try:
                    start_time = datetime.fromisoformat(started_at)
                    uptime = (datetime.now(timezone.utc) - start_time).total_seconds()
                    
                    uptime_metric = HealthMetric(
                        name="uptime",
                        value=uptime,
                        unit="seconds",
                        status=HealthStatus.HEALTHY,
                        timestamp=datetime.now(timezone.utc)
                    )
                    metrics.append(uptime_metric)
                except Exception as e:
                    warnings.append(f"Cannot calculate uptime: {e}")
            
            # Response time check (mock implementation)
            response_time = await self._check_response_time(vm_id, process_info)
            if response_time is not None:
                response_metric = self._create_metric(
                    "response_time", response_time, "ms", "response_time"
                )
                metrics.append(response_metric)
                if response_metric.status != HealthStatus.HEALTHY:
                    overall_status = max(overall_status, response_metric.status)
            
            # Create health report
            report = VMHealthReport(
                vm_id=vm_id,
                overall_status=overall_status,
                metrics=metrics,
                uptime=uptime,
                last_check=datetime.now(timezone.utc),
                warnings=warnings
            )
            
            # Update stored report
            if vm_id in self.vm_health_reports:
                old_report = self.vm_health_reports[vm_id]
                
                # Increment error count if status is critical
                if overall_status == HealthStatus.CRITICAL:
                    report.error_count = old_report.error_count + 1
                else:
                    report.error_count = 0  # Reset on healthy check
            else:
                # First report - set error count based on status
                if overall_status == HealthStatus.CRITICAL:
                    report.error_count = 1
                else:
                    report.error_count = 0
            
            self.vm_health_reports[vm_id] = report
            
            # Log health status changes
            if overall_status != HealthStatus.HEALTHY:
                logger.warning(
                    "VM health issue detected",
                    vm_id=vm_id,
                    status=overall_status.value,
                    warnings=warnings
                )
            
            return report
            
        except Exception as e:
            logger.error("Health check failed", vm_id=vm_id, error=str(e))
            
            # Create error report
            error_report = VMHealthReport(
                vm_id=vm_id,
                overall_status=HealthStatus.UNKNOWN,
                metrics=[],
                uptime=0.0,
                last_check=datetime.now(timezone.utc),
                warnings=[f"Health check failed: {e}"]
            )
            
            if vm_id in self.vm_health_reports:
                error_report.error_count = self.vm_health_reports[vm_id].error_count + 1
            
            self.vm_health_reports[vm_id] = error_report
            return error_report
    
    async def _global_monitor_loop(self) -> None:
        """Global monitoring loop for system-wide checks."""
        logger.info("Global health monitor loop started")
        
        try:
            while self.is_monitoring:
                await asyncio.sleep(self.check_interval * 2)  # Less frequent global checks
                
                if not self.is_monitoring:
                    break
                
                # Perform global health checks
                await self._perform_global_health_checks()
                
        except asyncio.CancelledError:
            logger.info("Global monitor loop cancelled")
        except Exception as e:
            logger.error("Global monitor loop error", error=str(e))
    
    async def _monitor_vm_loop(self, vm_id: str, process_info: Dict[str, Any]) -> None:
        """Individual VM monitoring loop."""
        logger.debug("VM monitor loop started", vm_id=vm_id)
        
        try:
            while self.is_monitoring and vm_id in self.monitoring_tasks:
                await asyncio.sleep(self.check_interval)
                
                if not self.is_monitoring:
                    break
                
                # Perform health check
                await self.check_vm_health(vm_id, process_info)
                
                # Check if VM has too many errors
                report = self.vm_health_reports.get(vm_id)
                if report and report.error_count >= self.max_error_count:
                    logger.error(
                        "VM has exceeded maximum error count",
                        vm_id=vm_id,
                        error_count=report.error_count,
                        max_errors=self.max_error_count
                    )
                    # Could trigger alerts or auto-recovery here
                
        except asyncio.CancelledError:
            logger.debug("VM monitor loop cancelled", vm_id=vm_id)
        except Exception as e:
            logger.error("VM monitor loop error", vm_id=vm_id, error=str(e))
    
    async def _perform_global_health_checks(self) -> None:
        """Perform system-wide health checks."""
        try:
            # System resource checks
            cpu_usage = psutil.cpu_percent(interval=1)
            memory = psutil.virtual_memory()
            disk = psutil.disk_usage('/')
            
            # Log system metrics
            logger.info(
                "System health metrics",
                cpu_percent=cpu_usage,
                memory_percent=memory.percent,
                disk_percent=disk.percent,
                active_vms=len(self.monitoring_tasks)
            )
            
            # Check for system-wide issues
            if cpu_usage > 90:
                logger.warning("High system CPU usage", cpu_percent=cpu_usage)
            
            if memory.percent > 90:
                logger.warning("High system memory usage", memory_percent=memory.percent)
            
            if disk.percent > 95:
                logger.error("Critical disk usage", disk_percent=disk.percent)
            
        except Exception as e:
            logger.error("Global health check failed", error=str(e))
    
    async def _check_response_time(self, vm_id: str, process_info: Dict[str, Any]) -> Optional[float]:
        """Check VM response time (mock implementation for now)."""
        try:
            # Mock response time based on VM ID hash for consistent testing
            # In real implementation, this would ping the VM or check API endpoint
            hash_value = hash(vm_id) % 100
            
            # Make it more predictable for testing - most VMs get healthy response times
            if hash_value < 70:  # 70% get healthy response time
                mock_response_time = 2.0 + (hash_value % 30) / 10.0  # 2.0-4.9ms range
            elif hash_value < 85:  # 15% get warning response time  
                mock_response_time = 5.5 + (hash_value % 30) / 10.0  # 5.5-8.5ms range
            else:  # 15% get critical response time
                mock_response_time = 10.5 + (hash_value % 30) / 10.0  # 10.5-13.5ms range
            
            return mock_response_time
            
        except Exception as e:
            logger.error("Response time check failed", vm_id=vm_id, error=str(e))
            return None
    
    def _create_metric(
        self, 
        name: str, 
        value: float, 
        unit: str, 
        threshold_key: str
    ) -> HealthMetric:
        """Create a health metric with status evaluation."""
        thresholds = self.thresholds.get(threshold_key, {})
        warning_threshold = thresholds.get("warning")
        critical_threshold = thresholds.get("critical")
        
        # Determine status
        status = HealthStatus.HEALTHY
        if critical_threshold and value >= critical_threshold:
            status = HealthStatus.CRITICAL
        elif warning_threshold and value >= warning_threshold:
            status = HealthStatus.WARNING
        
        return HealthMetric(
            name=name,
            value=value,
            unit=unit,
            status=status,
            threshold_warning=warning_threshold,
            threshold_critical=critical_threshold,
            timestamp=datetime.now(timezone.utc)
        )
    
    def _validate_vm_id(self, vm_id: str) -> None:
        """Validate VM ID format for security."""
        if not vm_id:
            raise HealthMonitorError("VM ID cannot be empty")
        
        if not isinstance(vm_id, str):
            raise HealthMonitorError("VM ID must be a string")
        
        if len(vm_id) > 100:
            raise HealthMonitorError("VM ID too long")
        
        # Basic UUID format check
        import uuid
        try:
            uuid.UUID(vm_id)
        except ValueError:
            raise HealthMonitorError(f"Invalid VM ID format: {vm_id}")
    
    def get_monitoring_status(self) -> Dict[str, Any]:
        """Get overall monitoring service status."""
        return {
            "is_monitoring": self.is_monitoring,
            "monitored_vms": len(self.monitoring_tasks),
            "check_interval": self.check_interval,
            "max_error_count": self.max_error_count,
            "active_tasks": len([t for t in self.monitoring_tasks.values() if not t.done()]),
            "thresholds": self.thresholds
        }
    
    async def export_health_data(self, vm_id: Optional[str] = None) -> Dict[str, Any]:
        """Export health data for external monitoring systems."""
        try:
            if vm_id:
                self._validate_vm_id(vm_id)
                report = self.vm_health_reports.get(vm_id)
                if not report:
                    return {}
                
                return {
                    "vm_id": report.vm_id,
                    "status": report.overall_status.value,
                    "uptime": report.uptime,
                    "last_check": report.last_check.isoformat(),
                    "error_count": report.error_count,
                    "metrics": [
                        {
                            "name": m.name,
                            "value": m.value,
                            "unit": m.unit,
                            "status": m.status.value,
                            "timestamp": m.timestamp.isoformat()
                        }
                        for m in report.metrics
                    ],
                    "warnings": report.warnings
                }
            else:
                # Export all VMs
                return {
                    "monitoring_status": self.get_monitoring_status(),
                    "vms": {
                        vm_id: await self.export_health_data(vm_id)
                        for vm_id in self.vm_health_reports.keys()
                    }
                }
                
        except Exception as e:
            logger.error("Failed to export health data", vm_id=vm_id, error=str(e))
            return {"error": str(e)}