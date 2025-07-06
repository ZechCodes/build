"""
Tests for Resource Manager and Performance Optimization

Comprehensive test suite for resource management, performance monitoring,
and optimization features.
"""

import pytest
import asyncio
import time
import tempfile
from unittest.mock import AsyncMock, MagicMock, patch
from pathlib import Path

# Import the resource manager
import sys
current_dir = Path(__file__).parent.parent
if str(current_dir) not in sys.path:
    sys.path.insert(0, str(current_dir))

from performance.resource_manager import (
    ResourceManager, ResourceUsage, PerformanceMetrics
)


class TestResourceManager:
    """Test suite for resource manager functionality."""
    
    @pytest.fixture
    def resource_manager(self):
        """Create resource manager with test configuration."""
        config = {
            "max_memory_percent": 70,
            "max_disk_percent": 80,
            "max_concurrent_operations": 3,
            "cleanup_interval": 60,
            "cache_max_size_mb": 50,
            "background_cleanup": False  # Disable for testing
        }
        return ResourceManager(config)
    
    @pytest.mark.asyncio
    async def test_initialization(self, resource_manager):
        """Test resource manager initialization."""
        with patch('psutil.virtual_memory') as mock_memory, \
             patch('psutil.disk_usage') as mock_disk:
            
            # Mock sufficient resources
            mock_memory.return_value = MagicMock(total=8 * 1024**3)  # 8GB
            mock_disk.return_value = MagicMock(free=50 * 1024**3)    # 50GB
            
            await resource_manager.initialize()
            
            assert resource_manager.max_memory_percent == 70
            assert resource_manager.max_disk_percent == 80
            assert resource_manager.max_concurrent_operations == 3
            assert len(resource_manager.operation_metrics) == 0
            assert len(resource_manager.active_operations) == 0
    
    @pytest.mark.asyncio
    async def test_resource_availability_check(self, resource_manager):
        """Test resource availability checking."""
        with patch('psutil.cpu_percent', return_value=25.0), \
             patch('psutil.virtual_memory') as mock_memory, \
             patch('psutil.disk_usage') as mock_disk, \
             patch('psutil.net_io_counters') as mock_network:
            
            # Mock good resource conditions
            mock_memory.return_value = MagicMock(
                percent=50.0,
                available=2 * 1024**3  # 2GB available
            )
            mock_disk.return_value = MagicMock(
                used=30 * 1024**3,
                total=100 * 1024**3,
                free=70 * 1024**3
            )
            mock_network.return_value = MagicMock(
                bytes_sent=1000000,
                bytes_recv=2000000
            )
            
            # Test with good conditions
            available = await resource_manager.check_resource_availability(
                "snapshot", 100 * 1024 * 1024  # 100MB
            )
            assert available is True
            
            # Test with high memory usage
            mock_memory.return_value.percent = 85.0
            available = await resource_manager.check_resource_availability(
                "snapshot", 100 * 1024 * 1024
            )
            assert available is False
            
            # Test with insufficient memory
            mock_memory.return_value.percent = 50.0
            mock_memory.return_value.available = 50 * 1024 * 1024  # 50MB
            available = await resource_manager.check_resource_availability(
                "snapshot", 2 * 1024**3  # 2GB operation
            )
            assert available is False
            
            # Test with high disk usage
            mock_memory.return_value.available = 2 * 1024**3
            mock_disk.return_value.used = 90 * 1024**3
            available = await resource_manager.check_resource_availability(
                "snapshot", 100 * 1024 * 1024
            )
            assert available is False
    
    @pytest.mark.asyncio
    async def test_operation_tracking(self, resource_manager):
        """Test operation tracking and metrics collection."""
        with patch('psutil.cpu_percent', return_value=25.0), \
             patch('psutil.virtual_memory') as mock_memory, \
             patch('psutil.disk_usage') as mock_disk, \
             patch('psutil.net_io_counters') as mock_network:
            
            # Mock good resource conditions
            mock_memory.return_value = MagicMock(
                percent=50.0,
                available=2 * 1024**3,
                used=2 * 1024**3
            )
            mock_disk.return_value = MagicMock(
                used=30 * 1024**3,
                total=100 * 1024**3,
                free=70 * 1024**3
            )
            mock_network.return_value = MagicMock(
                bytes_sent=1000000,
                bytes_recv=2000000
            )
            
            operation_id = "test_op_001"
            
            # Start operation tracking
            started = await resource_manager.start_operation_tracking(
                operation_id, "snapshot", 50 * 1024 * 1024  # 50MB
            )
            assert started is True
            assert operation_id in resource_manager.active_operations
            
            # Verify operation details
            operation = resource_manager.active_operations[operation_id]
            assert operation["type"] == "snapshot"
            assert operation["estimated_size"] == 50 * 1024 * 1024
            assert "start_time" in operation
            assert "start_cpu" in operation
            assert "start_memory" in operation
            
            # Simulate operation completion
            await asyncio.sleep(0.1)  # Small delay to measure duration
            
            await resource_manager.complete_operation_tracking(
                operation_id, success=True, data_size_bytes=45 * 1024 * 1024
            )
            
            # Verify operation was removed from active operations
            assert operation_id not in resource_manager.active_operations
            
            # Verify metrics were recorded
            assert len(resource_manager.operation_metrics) == 1
            metric = resource_manager.operation_metrics[0]
            assert metric.operation_type == "snapshot"
            assert metric.success is True
            assert metric.data_size_bytes == 45 * 1024 * 1024
            assert metric.duration_seconds > 0
            assert metric.throughput_mbps > 0
    
    @pytest.mark.asyncio
    async def test_concurrent_operation_limits(self, resource_manager):
        """Test concurrent operation limits."""
        with patch('psutil.cpu_percent', return_value=25.0), \
             patch('psutil.virtual_memory') as mock_memory, \
             patch('psutil.disk_usage') as mock_disk, \
             patch('psutil.net_io_counters') as mock_network:
            
            # Mock good resource conditions
            mock_memory.return_value = MagicMock(
                percent=30.0,
                available=4 * 1024**3
            )
            mock_disk.return_value = MagicMock(
                used=20 * 1024**3,
                total=100 * 1024**3,
                free=80 * 1024**3
            )
            mock_network.return_value = MagicMock(
                bytes_sent=1000000,
                bytes_recv=2000000
            )
            
            # Start operations up to the limit (3)
            operation_ids = []
            for i in range(3):
                op_id = f"test_op_{i}"
                started = await resource_manager.start_operation_tracking(
                    op_id, "snapshot", 10 * 1024 * 1024
                )
                assert started is True
                operation_ids.append(op_id)
            
            # Try to start one more operation (should fail)
            extra_op_started = await resource_manager.start_operation_tracking(
                "extra_op", "snapshot", 10 * 1024 * 1024
            )
            assert extra_op_started is False
            
            # Complete one operation
            await resource_manager.complete_operation_tracking(
                operation_ids[0], success=True, data_size_bytes=10 * 1024 * 1024
            )
            
            # Now should be able to start another operation
            new_op_started = await resource_manager.start_operation_tracking(
                "new_op", "snapshot", 10 * 1024 * 1024
            )
            assert new_op_started is True
    
    @pytest.mark.asyncio
    async def test_memory_optimization(self, resource_manager):
        """Test memory optimization functionality."""
        with patch('psutil.virtual_memory') as mock_memory, \
             patch('gc.collect', return_value=100) as mock_gc:
            
            mock_memory.return_value = MagicMock(percent=75.0)
            
            # Add some cache entries
            resource_manager.cache_set("key1", "value1" * 1000)
            resource_manager.cache_set("key2", "value2" * 1000)
            
            # Add expired cache entry
            old_time = time.time() - 3700  # 1 hour + 100 seconds ago
            resource_manager.cache_entries["expired_key"] = ("old_value", old_time)
            
            initial_cache_size = len(resource_manager.cache_entries)
            assert initial_cache_size == 3
            
            # Run memory optimization
            await resource_manager.optimize_memory_usage()
            
            # Verify expired entry was removed
            assert "expired_key" not in resource_manager.cache_entries
            assert len(resource_manager.cache_entries) == 2
            
            # Verify garbage collection was called
            mock_gc.assert_called_once()
    
    @pytest.mark.asyncio
    async def test_temp_file_cleanup(self, resource_manager):
        """Test temporary file cleanup."""
        # Create temporary files
        temp_files = []
        for i in range(3):
            with tempfile.NamedTemporaryFile(delete=False, prefix="test_snap_") as tf:
                tf.write(b"test data" * 100)
                temp_files.append(tf.name)
                resource_manager.register_temp_file(tf.name)
        
        # Verify files exist
        for file_path in temp_files:
            assert Path(file_path).exists()
        
        assert len(resource_manager.temp_files) == 3
        
        # Run cleanup
        await resource_manager.cleanup_temp_files()
        
        # Verify files were removed
        for file_path in temp_files:
            assert not Path(file_path).exists()
        
        assert len(resource_manager.temp_files) == 0
    
    def test_cache_operations(self, resource_manager):
        """Test cache set/get operations."""
        # Test basic cache operations
        resource_manager.cache_set("test_key", "test_value")
        
        # Should retrieve the value
        value = resource_manager.cache_get("test_key")
        assert value == "test_value"
        
        # Test TTL expiration (mock time)
        with patch('time.time', return_value=time.time() + 3700):  # 1 hour + 100 seconds
            expired_value = resource_manager.cache_get("test_key")
            assert expired_value is None
            
            # Key should be removed from cache
            assert "test_key" not in resource_manager.cache_entries
        
        # Test non-existent key
        missing_value = resource_manager.cache_get("missing_key")
        assert missing_value is None
    
    def test_performance_report_generation(self, resource_manager):
        """Test performance report generation."""
        with patch('psutil.virtual_memory') as mock_memory, \
             patch('psutil.disk_usage') as mock_disk:
            
            mock_memory.return_value = MagicMock(
                percent=45.0,
                available=3 * 1024**3
            )
            mock_disk.return_value = MagicMock(
                used=40 * 1024**3,
                total=100 * 1024**3,
                free=60 * 1024**3
            )
            
            # Add some test metrics
            test_metrics = [
                PerformanceMetrics(
                    operation_type="snapshot",
                    duration_seconds=2.5,
                    data_size_bytes=100 * 1024 * 1024,
                    throughput_mbps=40.0,
                    cpu_usage_during=15.0,
                    memory_peak_mb=512,
                    success=True
                ),
                PerformanceMetrics(
                    operation_type="restore",
                    duration_seconds=1.8,
                    data_size_bytes=100 * 1024 * 1024,
                    throughput_mbps=55.6,
                    cpu_usage_during=12.0,
                    memory_peak_mb=256,
                    success=True
                ),
                PerformanceMetrics(
                    operation_type="snapshot",
                    duration_seconds=0.0,
                    data_size_bytes=0,
                    throughput_mbps=0.0,
                    cpu_usage_during=5.0,
                    memory_peak_mb=128,
                    success=False,
                    error_message="Test error"
                )
            ]
            
            resource_manager.operation_metrics = test_metrics
            
            # Generate report
            report = resource_manager.get_performance_report()
            
            # Verify report structure
            assert "performance_summary" in report
            assert "operations_by_type" in report
            assert "current_system_status" in report
            assert "resource_limits" in report
            
            # Verify performance summary
            summary = report["performance_summary"]
            assert summary["total_operations"] == 3
            assert summary["successful_operations"] == 2
            assert summary["failed_operations"] == 1
            assert summary["success_rate"] == 2/3
            
            # Verify operations by type
            ops_by_type = report["operations_by_type"]
            assert "snapshot" in ops_by_type
            assert "restore" in ops_by_type
            
            snapshot_stats = ops_by_type["snapshot"]
            assert snapshot_stats["total_operations"] == 2
            assert snapshot_stats["successful_operations"] == 1
            assert snapshot_stats["success_rate"] == 0.5
            
            restore_stats = ops_by_type["restore"]
            assert restore_stats["total_operations"] == 1
            assert restore_stats["successful_operations"] == 1
            assert restore_stats["success_rate"] == 1.0
            
            # Verify current system status
            system_status = report["current_system_status"]
            assert system_status["memory_usage_percent"] == 45.0
            assert system_status["disk_usage_percent"] == 40.0
            assert "memory_available_mb" in system_status
            assert "disk_available_gb" in system_status
    
    def test_performance_report_empty_metrics(self, resource_manager):
        """Test performance report with no metrics."""
        report = resource_manager.get_performance_report()
        assert "error" in report
        assert "No performance data available" in report["error"]
    
    @pytest.mark.asyncio
    async def test_error_handling(self, resource_manager):
        """Test error handling in resource manager operations."""
        # Test operation tracking with invalid operation
        with patch('psutil.cpu_percent', side_effect=Exception("Test error")):
            available = await resource_manager.check_resource_availability("test", 100)
            assert available is False
        
        # Test completing non-existent operation
        await resource_manager.complete_operation_tracking(
            "nonexistent_op", success=False, error_message="Not found"
        )
        # Should not raise an exception
        
        # Test memory optimization with psutil error
        with patch('psutil.virtual_memory', side_effect=Exception("Test error")):
            # Should not raise an exception
            await resource_manager.optimize_memory_usage()
    
    @pytest.mark.asyncio
    async def test_resource_usage_calculation(self, resource_manager):
        """Test resource usage calculation."""
        with patch('psutil.cpu_percent', return_value=35.5), \
             patch('psutil.virtual_memory') as mock_memory, \
             patch('psutil.disk_usage') as mock_disk, \
             patch('psutil.net_io_counters') as mock_network:
            
            mock_memory.return_value = MagicMock(
                percent=62.3,
                available=1536 * 1024 * 1024  # 1.5GB
            )
            mock_disk.return_value = MagicMock(
                used=450 * 1024**3,
                total=1000 * 1024**3,
                free=550 * 1024**3
            )
            mock_network.return_value = MagicMock(
                bytes_sent=5000000,
                bytes_recv=8000000
            )
            
            usage = await resource_manager._get_current_resource_usage()
            
            assert usage.cpu_percent == 35.5
            assert usage.memory_percent == 62.3
            assert usage.memory_available_mb == 1536
            assert usage.disk_usage_percent == 45.0  # 450/1000 * 100
            assert usage.disk_available_gb == 550
            assert usage.network_io_bytes == 13000000  # 5M + 8M
            assert usage.timestamp > 0


if __name__ == "__main__":
    pytest.main([__file__, "-v"])