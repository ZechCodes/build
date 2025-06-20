"""Tests for VM health monitoring system."""

import pytest
import asyncio
import time
from unittest.mock import MagicMock, patch, AsyncMock
from datetime import datetime, timezone

from monitoring.health_monitor import (
    VMHealthMonitor, HealthStatus, HealthMetric, VMHealthReport, 
    HealthMonitorError
)


class TestHealthMetric:
    """Test health metric functionality."""
    
    def test_health_metric_creation(self):
        """Test creating a health metric."""
        metric = HealthMetric(
            name="cpu_usage",
            value=75.5,
            unit="%",
            status=HealthStatus.WARNING,
            threshold_warning=80.0,
            threshold_critical=95.0
        )
        
        assert metric.name == "cpu_usage"
        assert metric.value == 75.5
        assert metric.unit == "%"
        assert metric.status == HealthStatus.WARNING
        assert metric.threshold_warning == 80.0
        assert metric.threshold_critical == 95.0
        assert isinstance(metric.timestamp, datetime)
    
    def test_health_metric_auto_timestamp(self):
        """Test automatic timestamp assignment."""
        before = datetime.now(timezone.utc)
        metric = HealthMetric("test", 1.0, "unit", HealthStatus.HEALTHY)
        after = datetime.now(timezone.utc)
        
        assert before <= metric.timestamp <= after


class TestVMHealthReport:
    """Test VM health report functionality."""
    
    def test_health_report_creation(self):
        """Test creating a health report."""
        metrics = [
            HealthMetric("cpu", 50.0, "%", HealthStatus.HEALTHY),
            HealthMetric("memory", 60.0, "%", HealthStatus.HEALTHY)
        ]
        
        report = VMHealthReport(
            vm_id="12345678-1234-1234-1234-123456789012",
            overall_status=HealthStatus.HEALTHY,
            metrics=metrics,
            uptime=3600.0,
            last_check=datetime.now(timezone.utc)
        )
        
        assert report.vm_id == "12345678-1234-1234-1234-123456789012"
        assert report.overall_status == HealthStatus.HEALTHY
        assert len(report.metrics) == 2
        assert report.uptime == 3600.0
        assert report.error_count == 0
        assert report.warnings == []
    
    def test_health_report_with_warnings(self):
        """Test health report with warnings."""
        warnings = ["High CPU usage", "Network latency"]
        
        report = VMHealthReport(
            vm_id="12345678-1234-1234-1234-123456789012",
            overall_status=HealthStatus.WARNING,
            metrics=[],
            uptime=1800.0,
            last_check=datetime.now(timezone.utc),
            error_count=2,
            warnings=warnings
        )
        
        assert report.error_count == 2
        assert report.warnings == warnings


@patch('monitoring.health_monitor.psutil')
class TestVMHealthMonitor:
    """Test VM health monitoring functionality."""
    
    def test_init_default_config(self, mock_psutil, test_settings):
        """Test initialization with default configuration."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        assert monitor.check_interval == 30
        assert monitor.max_error_count == 3
        assert monitor.is_monitoring is False
        assert len(monitor.vm_health_reports) == 0
        assert len(monitor.monitoring_tasks) == 0
        assert "cpu_usage" in monitor.thresholds
        assert "memory_usage" in monitor.thresholds
    
    def test_init_custom_config(self, mock_psutil, test_settings):
        """Test initialization with custom configuration."""
        monitor = VMHealthMonitor(
            settings=test_settings,
            check_interval=60,
            max_error_count=5
        )
        
        assert monitor.check_interval == 60
        assert monitor.max_error_count == 5
    
    @pytest.mark.asyncio
    async def test_start_stop_monitoring(self, mock_psutil, test_settings):
        """Test starting and stopping monitoring service."""
        monitor = VMHealthMonitor(settings=test_settings, check_interval=1)
        
        # Start monitoring
        await monitor.start_monitoring()
        assert monitor.is_monitoring is True
        assert monitor.global_monitor_task is not None
        
        # Stop monitoring
        await monitor.stop_monitoring()
        assert monitor.is_monitoring is False
        assert len(monitor.monitoring_tasks) == 0
    
    @pytest.mark.asyncio
    async def test_start_monitoring_already_running(self, mock_psutil, test_settings):
        """Test starting monitoring when already running."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        await monitor.start_monitoring()
        assert monitor.is_monitoring is True
        
        # Try to start again
        await monitor.start_monitoring()
        assert monitor.is_monitoring is True  # Should still be running
        
        await monitor.stop_monitoring()
    
    @pytest.mark.asyncio
    async def test_add_vm_monitoring(self, mock_psutil, test_settings):
        """Test adding VM to monitoring."""
        monitor = VMHealthMonitor(settings=test_settings, check_interval=1)
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        process_info = {
            "process": MagicMock(),
            "pid": 12345,
            "started_at": datetime.now(timezone.utc).isoformat()
        }
        
        await monitor.add_vm_monitoring(vm_id, process_info)
        
        assert vm_id in monitor.vm_health_reports
        assert vm_id in monitor.monitoring_tasks
        assert monitor.vm_health_reports[vm_id].vm_id == vm_id
        
        # Cleanup
        await monitor.remove_vm_monitoring(vm_id)
    
    @pytest.mark.asyncio
    async def test_add_vm_monitoring_duplicate(self, mock_psutil, test_settings):
        """Test adding duplicate VM to monitoring."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        process_info = {"process": MagicMock(), "pid": 12345}
        
        await monitor.add_vm_monitoring(vm_id, process_info)
        
        # Try to add same VM again
        await monitor.add_vm_monitoring(vm_id, process_info)
        
        # Should still only have one entry
        assert len(monitor.monitoring_tasks) == 1
        
        await monitor.remove_vm_monitoring(vm_id)
    
    @pytest.mark.asyncio
    async def test_remove_vm_monitoring(self, mock_psutil, test_settings):
        """Test removing VM from monitoring."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        process_info = {"process": MagicMock(), "pid": 12345}
        
        await monitor.add_vm_monitoring(vm_id, process_info)
        assert vm_id in monitor.monitoring_tasks
        
        await monitor.remove_vm_monitoring(vm_id)
        assert vm_id not in monitor.monitoring_tasks
        assert vm_id not in monitor.vm_health_reports
    
    @pytest.mark.asyncio
    async def test_remove_vm_monitoring_not_found(self, mock_psutil, test_settings):
        """Test removing non-existent VM from monitoring."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        vm_id = "nonexistent-vm-id"
        
        # Should not raise an error
        await monitor.remove_vm_monitoring(vm_id)
    
    @pytest.mark.asyncio
    async def test_get_vm_health(self, mock_psutil, test_settings):
        """Test getting VM health report."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        
        # No health report initially
        health = await monitor.get_vm_health(vm_id)
        assert health is None
        
        # Add VM and check health
        process_info = {"process": MagicMock(), "pid": 12345}
        await monitor.add_vm_monitoring(vm_id, process_info)
        
        health = await monitor.get_vm_health(vm_id)
        assert health is not None
        assert health.vm_id == vm_id
        
        await monitor.remove_vm_monitoring(vm_id)
    
    @pytest.mark.asyncio
    async def test_get_all_vm_health(self, mock_psutil, test_settings):
        """Test getting all VM health reports."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        # Initially empty
        all_health = await monitor.get_all_vm_health()
        assert len(all_health) == 0
        
        # Add multiple VMs
        vm_ids = [f"{i:08d}-1234-1234-1234-123456789012" for i in range(3)]
        for vm_id in vm_ids:
            process_info = {"process": MagicMock(), "pid": 12345 + hash(vm_id) % 1000}
            await monitor.add_vm_monitoring(vm_id, process_info)
        
        all_health = await monitor.get_all_vm_health()
        assert len(all_health) == 3
        
        for vm_id in vm_ids:
            assert vm_id in all_health
            await monitor.remove_vm_monitoring(vm_id)
    
    @pytest.mark.asyncio
    async def test_check_vm_health_healthy(self, mock_psutil, test_settings):
        """Test VM health check for healthy VM."""
        # Setup mock process
        mock_process = MagicMock()
        mock_process.pid = 12345
        mock_process.is_running.return_value = True
        mock_process.cpu_percent.return_value = 50.0  # Healthy CPU
        mock_process.memory_info.return_value = MagicMock(rss=512 * 1024 * 1024)  # 512MB
        
        mock_psutil.Process.return_value = mock_process
        
        monitor = VMHealthMonitor(settings=test_settings)
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        process_info = {
            "process": mock_process,
            "pid": 12345,
            "started_at": datetime.now(timezone.utc).isoformat()
        }
        
        report = await monitor.check_vm_health(vm_id, process_info)
        
        assert report.vm_id == vm_id
        assert report.overall_status == HealthStatus.HEALTHY
        assert len(report.metrics) >= 3  # CPU, memory, uptime, response_time
        assert report.uptime > 0
        assert len(report.warnings) == 0
    
    @pytest.mark.asyncio
    async def test_check_vm_health_warning(self, mock_psutil, test_settings):
        """Test VM health check for VM with warnings."""
        # Setup mock process with high CPU
        mock_process = MagicMock()
        mock_process.pid = 12345
        mock_process.is_running.return_value = True
        mock_process.cpu_percent.return_value = 85.0  # Warning level CPU
        mock_process.memory_info.return_value = MagicMock(rss=256 * 1024 * 1024)  # 256MB
        
        mock_psutil.Process.return_value = mock_process
        
        monitor = VMHealthMonitor(settings=test_settings)
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        process_info = {
            "process": mock_process,
            "pid": 12345,
            "started_at": datetime.now(timezone.utc).isoformat()
        }
        
        report = await monitor.check_vm_health(vm_id, process_info)
        
        assert report.vm_id == vm_id
        assert report.overall_status == HealthStatus.WARNING
        
        # Find CPU metric
        cpu_metrics = [m for m in report.metrics if m.name == "cpu_usage"]
        assert len(cpu_metrics) == 1
        assert cpu_metrics[0].status == HealthStatus.WARNING
    
    @pytest.mark.asyncio
    async def test_check_vm_health_critical(self, mock_psutil, test_settings):
        """Test VM health check for critical VM."""
        # Setup mock process with critical CPU
        mock_process = MagicMock()
        mock_process.pid = 12345
        mock_process.is_running.return_value = True
        mock_process.cpu_percent.return_value = 98.0  # Critical level CPU
        mock_process.memory_info.return_value = MagicMock(rss=256 * 1024 * 1024)
        
        mock_psutil.Process.return_value = mock_process
        
        monitor = VMHealthMonitor(settings=test_settings)
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        process_info = {
            "process": mock_process,
            "pid": 12345,
            "started_at": datetime.now(timezone.utc).isoformat()
        }
        
        report = await monitor.check_vm_health(vm_id, process_info)
        
        assert report.vm_id == vm_id
        assert report.overall_status == HealthStatus.CRITICAL
        
        # Find CPU metric
        cpu_metrics = [m for m in report.metrics if m.name == "cpu_usage"]
        assert len(cpu_metrics) == 1
        assert cpu_metrics[0].status == HealthStatus.CRITICAL
    
    @pytest.mark.asyncio
    async def test_check_vm_health_no_process(self, mock_psutil, test_settings):
        """Test VM health check with no process."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        process_info = {"process": None, "pid": None}
        
        report = await monitor.check_vm_health(vm_id, process_info)
        
        assert report.vm_id == vm_id
        assert report.overall_status == HealthStatus.CRITICAL
        assert len(report.warnings) > 0
        assert "No process information available" in report.warnings
    
    @pytest.mark.asyncio
    async def test_check_vm_health_process_not_running(self, mock_psutil, test_settings):
        """Test VM health check for stopped process."""
        # Setup mock process that is not running
        mock_process = MagicMock()
        mock_process.pid = 12345
        mock_process.is_running.return_value = False
        mock_process.cpu_percent.return_value = 0.0
        mock_process.memory_info.return_value = MagicMock(rss=0)
        
        mock_psutil.Process.return_value = mock_process
        
        monitor = VMHealthMonitor(settings=test_settings)
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        process_info = {
            "process": mock_process,
            "pid": 12345,
            "started_at": datetime.now(timezone.utc).isoformat()
        }
        
        report = await monitor.check_vm_health(vm_id, process_info)
        
        assert report.vm_id == vm_id
        assert report.overall_status == HealthStatus.CRITICAL
        assert any("Process is no longer running" in w for w in report.warnings)
    
    @pytest.mark.asyncio
    async def test_check_vm_health_process_access_denied(self, mock_psutil, test_settings):
        """Test VM health check with process access denied."""
        # Create proper exception classes
        class MockAccessDenied(Exception):
            pass
        
        class MockNoSuchProcess(Exception):
            pass
        
        # Setup mock psutil exceptions
        mock_psutil.AccessDenied = MockAccessDenied
        mock_psutil.NoSuchProcess = MockNoSuchProcess
        mock_psutil.Process.side_effect = MockAccessDenied("Access denied")
        
        monitor = VMHealthMonitor(settings=test_settings)
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        process_info = {
            "process": MagicMock(pid=12345),
            "pid": 12345,
            "started_at": datetime.now(timezone.utc).isoformat()
        }
        
        report = await monitor.check_vm_health(vm_id, process_info)
        
        assert report.vm_id == vm_id
        assert report.overall_status == HealthStatus.WARNING
        assert any("Cannot access process metrics" in w for w in report.warnings)
    
    @pytest.mark.asyncio
    async def test_check_vm_health_error_count_increment(self, mock_psutil, test_settings):
        """Test error count increment on critical status."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        process_info = {"process": None, "pid": None}  # Will cause critical status
        
        # First check
        report1 = await monitor.check_vm_health(vm_id, process_info)
        assert report1.error_count == 1
        
        # Second check
        report2 = await monitor.check_vm_health(vm_id, process_info)
        assert report2.error_count == 2
        
        # Third check
        report3 = await monitor.check_vm_health(vm_id, process_info)
        assert report3.error_count == 3
    
    @pytest.mark.asyncio
    async def test_check_vm_health_error_count_reset(self, mock_psutil, test_settings):
        """Test error count reset on healthy status."""
        # Setup healthy mock process
        mock_process = MagicMock()
        mock_process.pid = 12345
        mock_process.is_running.return_value = True
        mock_process.cpu_percent.return_value = 50.0
        mock_process.memory_info.return_value = MagicMock(rss=256 * 1024 * 1024)
        
        mock_psutil.Process.return_value = mock_process
        
        monitor = VMHealthMonitor(settings=test_settings)
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        
        # First, cause some errors
        error_process_info = {"process": None, "pid": None}
        report1 = await monitor.check_vm_health(vm_id, error_process_info)
        assert report1.error_count == 1
        
        # Now with healthy process
        healthy_process_info = {
            "process": mock_process,
            "pid": 12345,
            "started_at": datetime.now(timezone.utc).isoformat()
        }
        report2 = await monitor.check_vm_health(vm_id, healthy_process_info)
        assert report2.error_count == 0  # Should reset
    
    def test_validate_vm_id_valid(self, mock_psutil, test_settings):
        """Test VM ID validation with valid IDs."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        valid_ids = [
            "12345678-1234-1234-1234-123456789012",
            "87654321-4321-4321-4321-210987654321",
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        ]
        
        for vm_id in valid_ids:
            monitor._validate_vm_id(vm_id)  # Should not raise
    
    def test_validate_vm_id_invalid(self, mock_psutil, test_settings):
        """Test VM ID validation with invalid IDs."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        invalid_ids = [
            "",  # Empty
            None,  # None
            123,  # Not string
            "short",  # Too short
            "x" * 101,  # Too long
            "not-a-uuid",  # Invalid format
            "12345678-1234-1234-1234-12345678901x"  # Invalid character
        ]
        
        for vm_id in invalid_ids:
            with pytest.raises(HealthMonitorError):
                monitor._validate_vm_id(vm_id)
    
    def test_create_metric_healthy(self, mock_psutil, test_settings):
        """Test creating healthy metric."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        metric = monitor._create_metric("cpu_usage", 50.0, "%", "cpu_usage")
        
        assert metric.name == "cpu_usage"
        assert metric.value == 50.0
        assert metric.unit == "%"
        assert metric.status == HealthStatus.HEALTHY
        assert metric.threshold_warning == 80.0
        assert metric.threshold_critical == 95.0
    
    def test_create_metric_warning(self, mock_psutil, test_settings):
        """Test creating warning metric."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        metric = monitor._create_metric("cpu_usage", 85.0, "%", "cpu_usage")
        
        assert metric.status == HealthStatus.WARNING
        assert metric.value == 85.0
    
    def test_create_metric_critical(self, mock_psutil, test_settings):
        """Test creating critical metric."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        metric = monitor._create_metric("cpu_usage", 98.0, "%", "cpu_usage")
        
        assert metric.status == HealthStatus.CRITICAL
        assert metric.value == 98.0
    
    def test_create_metric_no_thresholds(self, mock_psutil, test_settings):
        """Test creating metric with no thresholds."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        metric = monitor._create_metric("custom_metric", 100.0, "units", "nonexistent")
        
        assert metric.status == HealthStatus.HEALTHY
        assert metric.threshold_warning is None
        assert metric.threshold_critical is None
    
    def test_get_monitoring_status(self, mock_psutil, test_settings):
        """Test getting monitoring status."""
        monitor = VMHealthMonitor(settings=test_settings, check_interval=60, max_error_count=5)
        
        status = monitor.get_monitoring_status()
        
        assert status["is_monitoring"] is False
        assert status["monitored_vms"] == 0
        assert status["check_interval"] == 60
        assert status["max_error_count"] == 5
        assert status["active_tasks"] == 0
        assert "thresholds" in status
    
    @pytest.mark.asyncio
    async def test_export_health_data_single_vm(self, mock_psutil, test_settings):
        """Test exporting health data for single VM."""
        # Setup healthy mock process
        mock_process = MagicMock()
        mock_process.pid = 12345
        mock_process.is_running.return_value = True
        mock_process.cpu_percent.return_value = 50.0
        mock_process.memory_info.return_value = MagicMock(rss=256 * 1024 * 1024)
        
        mock_psutil.Process.return_value = mock_process
        
        monitor = VMHealthMonitor(settings=test_settings)
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        process_info = {
            "process": mock_process,
            "pid": 12345,
            "started_at": datetime.now(timezone.utc).isoformat()
        }
        
        # Generate health report
        await monitor.check_vm_health(vm_id, process_info)
        
        # Export data
        export_data = await monitor.export_health_data(vm_id)
        
        assert export_data["vm_id"] == vm_id
        assert export_data["status"] == "healthy"
        assert "uptime" in export_data
        assert "last_check" in export_data
        assert "metrics" in export_data
        assert len(export_data["metrics"]) >= 3
    
    @pytest.mark.asyncio
    async def test_export_health_data_all_vms(self, mock_psutil, test_settings):
        """Test exporting health data for all VMs."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        # Add some VMs
        vm_ids = [f"{i:08d}-1234-1234-1234-123456789012" for i in range(2)]
        for vm_id in vm_ids:
            process_info = {"process": MagicMock(), "pid": 12345}
            await monitor.check_vm_health(vm_id, process_info)
        
        # Export all data
        export_data = await monitor.export_health_data()
        
        assert "monitoring_status" in export_data
        assert "vms" in export_data
        assert len(export_data["vms"]) == 2
        
        for vm_id in vm_ids:
            assert vm_id in export_data["vms"]
    
    @pytest.mark.asyncio
    async def test_export_health_data_nonexistent_vm(self, mock_psutil, test_settings):
        """Test exporting health data for non-existent VM."""
        monitor = VMHealthMonitor(settings=test_settings)
        
        vm_id = "99999999-9999-9999-9999-999999999999"  # Valid UUID but nonexistent
        export_data = await monitor.export_health_data(vm_id)
        
        assert export_data == {}
    
    @pytest.mark.asyncio
    async def test_concurrent_health_checks(self, mock_psutil, test_settings):
        """Test concurrent health checks."""
        # Setup healthy mock process
        mock_process = MagicMock()
        mock_process.pid = 12345
        mock_process.is_running.return_value = True
        mock_process.cpu_percent.return_value = 50.0
        mock_process.memory_info.return_value = MagicMock(rss=256 * 1024 * 1024)
        
        mock_psutil.Process.return_value = mock_process
        
        monitor = VMHealthMonitor(settings=test_settings)
        
        # Create multiple VMs
        vm_ids = [f"{i:08d}-1234-1234-1234-123456789012" for i in range(5)]
        process_infos = []
        
        for vm_id in vm_ids:
            process_info = {
                "process": mock_process,
                "pid": 12345 + hash(vm_id) % 1000,
                "started_at": datetime.now(timezone.utc).isoformat()
            }
            process_infos.append(process_info)
        
        # Run health checks concurrently
        tasks = [
            monitor.check_vm_health(vm_id, process_info)
            for vm_id, process_info in zip(vm_ids, process_infos)
        ]
        
        reports = await asyncio.gather(*tasks)
        
        # All should complete successfully
        assert len(reports) == 5
        for report in reports:
            # Just check that reports were generated successfully
            assert report is not None
            assert report.vm_id.endswith("-1234-1234-1234-123456789012")
            assert len(report.metrics) >= 3  # CPU, memory, uptime, response_time
            assert report.overall_status in [HealthStatus.HEALTHY, HealthStatus.WARNING, HealthStatus.CRITICAL]
    
    @pytest.mark.asyncio
    async def test_monitoring_loop_integration(self, mock_psutil, test_settings):
        """Test integration of monitoring loops."""
        # Setup healthy mock process
        mock_process = MagicMock()
        mock_process.pid = 12345
        mock_process.is_running.return_value = True
        mock_process.cpu_percent.return_value = 50.0
        mock_process.memory_info.return_value = MagicMock(rss=256 * 1024 * 1024)
        
        mock_psutil.Process.return_value = mock_process
        mock_psutil.cpu_percent.return_value = 25.0
        mock_psutil.virtual_memory.return_value = MagicMock(percent=60.0)
        mock_psutil.disk_usage.return_value = MagicMock(percent=70.0)
        
        monitor = VMHealthMonitor(settings=test_settings, check_interval=0.1)  # Fast for testing
        
        try:
            # Start monitoring
            await monitor.start_monitoring()
            
            # Add a VM
            vm_id = "12345678-1234-1234-1234-123456789012"
            process_info = {
                "process": mock_process,
                "pid": 12345,
                "started_at": datetime.now(timezone.utc).isoformat()
            }
            
            await monitor.add_vm_monitoring(vm_id, process_info)
            
            # Wait for a few monitoring cycles
            await asyncio.sleep(0.3)
            
            # Check that health reports are being updated
            health = await monitor.get_vm_health(vm_id)
            assert health is not None
            assert health.overall_status == HealthStatus.HEALTHY
            
        finally:
            # Cleanup
            await monitor.stop_monitoring()