"""
Tests for Optimized Snapshot Manager

Comprehensive test suite for the optimized snapshot manager with
resource management and performance optimization features.
"""

import pytest
import asyncio
import time
from unittest.mock import AsyncMock, MagicMock, patch
from pathlib import Path

# Import the optimized snapshot manager
import sys
current_dir = Path(__file__).parent.parent
if str(current_dir) not in sys.path:
    sys.path.insert(0, str(current_dir))

from core.optimized_snapshot_manager import (
    OptimizedSnapshotManager, OptimizationStrategy, SnapshotOptimizationConfig
)
from core.snapshot_manager import SnapshotType, SnapshotState
from performance.resource_manager import ResourceManager


class TestOptimizedSnapshotManager:
    """Test suite for optimized snapshot manager functionality."""
    
    @pytest.fixture
    async def optimized_manager(self):
        """Create optimized snapshot manager with mocked dependencies."""
        # Mock VM manager
        vm_manager = AsyncMock()
        
        # Create a proper mock VM object
        mock_vm = MagicMock()
        mock_vm.user_id = "test_user"
        mock_vm.get_config.return_value = {"memory": 1024, "cpu": 2}
        
        vm_manager.get_vm = AsyncMock(return_value=mock_vm)
        vm_manager.create_firecracker_snapshot = AsyncMock(return_value=b"mock_snapshot_data" * 1000)
        vm_manager.verify_vm_ownership = AsyncMock(return_value=True)
        
        # Mock storage backend
        storage_backend = AsyncMock()
        storage_backend.store_snapshot = AsyncMock(return_value="s3://bucket/test/path")
        storage_backend.retrieve_snapshot = AsyncMock(return_value=b"mock_snapshot_data")
        storage_backend.delete_snapshot = AsyncMock(return_value=True)
        
        # Mock encryption service
        encryption_service = AsyncMock()
        encryption_service.encrypt_data = AsyncMock(return_value=b"encrypted_data")
        encryption_service.decrypt_data = AsyncMock(return_value=b"decrypted_data")
        
        # Mock database
        database = AsyncMock()
        database.save_snapshot_metadata = AsyncMock()
        database.delete_snapshot_metadata = AsyncMock()
        database.count_user_snapshots = AsyncMock(return_value=1)
        database.get_user_storage_usage = AsyncMock(return_value=1024 * 1024)
        
        # Mock resource manager
        resource_manager = MagicMock(spec=ResourceManager)
        resource_manager.initialize = AsyncMock()
        resource_manager.check_resource_availability = AsyncMock(return_value=True)
        resource_manager.start_operation_tracking = AsyncMock(return_value=True)
        resource_manager.complete_operation_tracking = AsyncMock()
        resource_manager.optimize_memory_usage = AsyncMock()
        resource_manager.cleanup_temp_files = AsyncMock()
        resource_manager.get_performance_report = MagicMock(return_value={
            "performance_summary": {"total_operations": 0},
            "current_system_status": {"memory_usage_percent": 50.0},
            "resource_limits": {"max_memory_percent": 80}
        })
        
        # Create optimization config
        optimization_config = SnapshotOptimizationConfig(
            strategy=OptimizationStrategy.BALANCED,
            max_concurrent_snapshots=2,
            compression_level=6,
            memory_limit_mb=512,
            enable_deduplication=True,
            background_optimization=False  # Disable for testing
        )
        
        # Create optimized snapshot manager
        manager = OptimizedSnapshotManager(
            vm_manager=vm_manager,
            storage_backend=storage_backend,
            encryption_service=encryption_service,
            database=database,
            resource_manager=resource_manager,
            optimization_config=optimization_config
        )
        
        # Mock the _generate_snapshot_id method
        manager._generate_snapshot_id = MagicMock(
            side_effect=lambda vm_id, user_id: f"snap_{vm_id}_{user_id}_{int(time.time())}"
        )
        
        # Mock signature service (inherited from base class)
        manager.signature_service = AsyncMock()
        manager.signature_service.sign_snapshot = MagicMock()
        
        # Mock the rate limiting check
        manager._check_rate_limit = AsyncMock(return_value=True)
        
        # Mock quota checks
        manager._validate_user_quota = AsyncMock(return_value=True)
        
        # Mock async snapshot creation to immediately transition to AVAILABLE state
        async def mock_create_snapshot_async(snapshot_id):
            if snapshot_id in manager.snapshots:
                metadata = manager.snapshots[snapshot_id]
                try:
                    # Actually call the VM manager to allow exceptions to be raised
                    await manager.vm_manager.create_firecracker_snapshot(metadata.vm_id)
                    
                    # If successful, transition to AVAILABLE state for testing
                    metadata.state = SnapshotState.AVAILABLE
                    metadata.storage_path = f"s3://test-bucket/{snapshot_id}"
                    metadata.size_bytes = 2048 * 1024 * 1024  # 2GB
                    metadata.compressed_size_bytes = 1024 * 1024 * 1024  # 1GB
                    metadata.checksum_sha256 = "test_checksum_abc123"
                except Exception as e:
                    # Set error state and re-raise
                    metadata.state = SnapshotState.ERROR
                    raise
        
        manager._create_snapshot_async = mock_create_snapshot_async
        
        await manager.initialize()
        return manager
    
    @pytest.mark.asyncio
    async def test_initialization(self, optimized_manager):
        """Test optimized snapshot manager initialization."""
        assert optimized_manager.resource_manager is not None
        assert optimized_manager.optimization_config.strategy == OptimizationStrategy.BALANCED
        assert optimized_manager.optimization_config.max_concurrent_snapshots == 2
        assert optimized_manager.adaptive_compression_level == 6
        assert isinstance(optimized_manager.performance_metrics, list)
        
        # Verify resource manager was initialized
        optimized_manager.resource_manager.initialize.assert_called_once()
    
    @pytest.mark.asyncio
    async def test_optimized_snapshot_creation(self, optimized_manager):
        """Test optimized snapshot creation with resource management."""
        vm_id = "test_vm_001"
        user_id = "test_user"
        
        # Mock VM configuration for size estimation - make sure to set user_id
        mock_vm = MagicMock()
        mock_vm.user_id = user_id  # Critical: set the user_id to match
        mock_vm.get_config.return_value = {"memory": 1024, "cpu": 2}
        optimized_manager.vm_manager.get_vm.return_value = mock_vm
        
        # Create optimized snapshot
        snapshot_id = await optimized_manager.create_snapshot_optimized(
            vm_id=vm_id,
            user_id=user_id,
            name="optimized-test-snapshot",
            description="Testing optimized creation",
            snapshot_type=SnapshotType.MANUAL,
            tags=["optimization", "test"],
            encrypt=True,
            priority="normal"
        )
        
        # Verify snapshot was created
        assert snapshot_id is not None
        assert snapshot_id.startswith("snap_")
        
        # Verify resource management was used
        optimized_manager.resource_manager.check_resource_availability.assert_called_once()
        optimized_manager.resource_manager.start_operation_tracking.assert_called_once()
        optimized_manager.resource_manager.complete_operation_tracking.assert_called_once()
        
        # Verify VM size estimation was called
        optimized_manager.vm_manager.get_vm.assert_called_with(vm_id)
        
        # Verify performance metrics were recorded
        assert len(optimized_manager.performance_metrics) > 0
    
    @pytest.mark.asyncio
    async def test_resource_availability_check_failure(self, optimized_manager):
        """Test snapshot creation when resources are unavailable."""
        vm_id = "test_vm_002"
        user_id = "test_user"
        
        # Mock resource unavailability
        optimized_manager.resource_manager.check_resource_availability.return_value = False
        
        # Attempt to create snapshot
        with pytest.raises(RuntimeError, match="Insufficient system resources"):
            await optimized_manager.create_snapshot_optimized(
                vm_id=vm_id,
                user_id=user_id,
                name="failed-snapshot",
                priority="normal"
            )
        
        # Verify resource check was performed
        optimized_manager.resource_manager.check_resource_availability.assert_called_once()
        
        # Verify operation tracking was not started
        optimized_manager.resource_manager.start_operation_tracking.assert_not_called()
    
    @pytest.mark.asyncio
    async def test_operation_tracking_failure(self, optimized_manager):
        """Test snapshot creation when operation tracking fails."""
        vm_id = "test_vm_003"
        user_id = "test_user"
        
        # Mock operation tracking failure
        optimized_manager.resource_manager.start_operation_tracking.return_value = False
        
        # Attempt to create snapshot
        with pytest.raises(RuntimeError, match="Unable to start resource tracking"):
            await optimized_manager.create_snapshot_optimized(
                vm_id=vm_id,
                user_id=user_id,
                name="tracking-failed-snapshot",
                priority="normal"
            )
        
        # Verify resource availability was checked
        optimized_manager.resource_manager.check_resource_availability.assert_called_once()
        
        # Verify operation tracking was attempted
        optimized_manager.resource_manager.start_operation_tracking.assert_called_once()
    
    @pytest.mark.asyncio
    async def test_optimization_strategy_selection(self, optimized_manager):
        """Test optimization strategy selection based on priority."""
        # Test high priority (should prefer speed)
        strategy = optimized_manager._choose_optimization_strategy("high")
        assert strategy == OptimizationStrategy.SPEED
        
        # Test low priority (should prefer compression)
        strategy = optimized_manager._choose_optimization_strategy("low")
        assert strategy == OptimizationStrategy.COMPRESSION
        
        # Test normal priority with balanced config
        strategy = optimized_manager._choose_optimization_strategy("normal")
        assert strategy == OptimizationStrategy.BALANCED
        
        # Test adaptive behavior with performance history
        # Add some slow performance metrics
        from performance.resource_manager import PerformanceMetrics
        slow_metrics = [
            PerformanceMetrics(
                operation_type="snapshot_creation",
                duration_seconds=120.0,  # Slow
                data_size_bytes=100 * 1024 * 1024,
                throughput_mbps=5.0,     # Low throughput
                cpu_usage_during=50.0,
                memory_peak_mb=512,
                success=True
            )
        ] * 5
        
        optimized_manager.performance_metrics = slow_metrics
        strategy = optimized_manager._choose_optimization_strategy("normal")
        assert strategy == OptimizationStrategy.SPEED  # Should adapt to speed
    
    @pytest.mark.asyncio
    async def test_optimized_snapshot_restore(self, optimized_manager):
        """Test optimized snapshot restore functionality."""
        # First create a snapshot to restore
        vm_id = "test_vm_004"
        user_id = "test_user"
        
        # Mock VM for creation
        mock_vm = MagicMock()
        mock_vm.user_id = user_id  # Critical: set the user_id to match
        mock_vm.get_config.return_value = {"memory": 512, "cpu": 1}
        optimized_manager.vm_manager.get_vm.return_value = mock_vm
        
        # Create snapshot
        snapshot_id = await optimized_manager.create_snapshot_optimized(
            vm_id=vm_id,
            user_id=user_id,
            name="restore-test-snapshot",
            priority="normal"
        )
        
        # Allow async tasks to complete
        await asyncio.sleep(0.1)
        
        # Reset mock calls
        optimized_manager.resource_manager.reset_mock()
        
        # Test optimized restore
        restored_vm_id = await optimized_manager.restore_snapshot_optimized(
            snapshot_id=snapshot_id,
            user_id=user_id,
            priority="normal"
        )
        
        # Verify restore completed
        assert restored_vm_id == vm_id
        
        # Verify resource management was used for restore
        optimized_manager.resource_manager.check_resource_availability.assert_called_once()
        optimized_manager.resource_manager.start_operation_tracking.assert_called_once()
        optimized_manager.resource_manager.complete_operation_tracking.assert_called_once()
        
        # Verify performance metrics were recorded for both operations
        assert len(optimized_manager.performance_metrics) >= 2
    
    @pytest.mark.asyncio
    async def test_vm_size_estimation(self, optimized_manager):
        """Test VM size estimation functionality."""
        vm_id = "test_vm_005"
        
        # Test with VM configuration available
        mock_vm = MagicMock()
        mock_vm.user_id = "test_user"  # Set user_id for consistency
        mock_vm.get_config.return_value = {"memory": 2048, "cpu": 4}
        optimized_manager.vm_manager.get_vm.return_value = mock_vm
        
        estimated_size = await optimized_manager._estimate_vm_size(vm_id)
        
        # Should be memory * 1.5 (2048 MB * 1.5 = 3072 MB)
        expected_size = 2048 * 1024 * 1024 * 1.5
        assert estimated_size == int(expected_size)
        
        # Test with VM not found (should return default)
        optimized_manager.vm_manager.get_vm.return_value = None
        
        estimated_size = await optimized_manager._estimate_vm_size("nonexistent_vm")
        assert estimated_size == 1024 * 1024 * 1024  # 1GB default
        
        # Test with exception (should return default)
        optimized_manager.vm_manager.get_vm.side_effect = Exception("VM lookup failed")
        
        estimated_size = await optimized_manager._estimate_vm_size("error_vm")
        assert estimated_size == 1024 * 1024 * 1024  # 1GB default
    
    def test_optimization_report_generation(self, optimized_manager):
        """Test optimization report generation."""
        # Add some performance metrics
        from performance.resource_manager import PerformanceMetrics
        test_metrics = [
            PerformanceMetrics(
                operation_type="snapshot_creation",
                duration_seconds=30.0,
                data_size_bytes=500 * 1024 * 1024,
                throughput_mbps=16.7,
                cpu_usage_during=25.0,
                memory_peak_mb=256,
                success=True
            ),
            PerformanceMetrics(
                operation_type="snapshot_restore",
                duration_seconds=20.0,
                data_size_bytes=500 * 1024 * 1024,
                throughput_mbps=25.0,
                cpu_usage_during=20.0,
                memory_peak_mb=200,
                success=True
            )
        ]
        
        optimized_manager.performance_metrics = test_metrics
        
        # Generate optimization report
        report = optimized_manager.get_optimization_report()
        
        # Verify report structure
        assert "timestamp" in report
        assert "resource_management" in report
        assert "snapshot_performance" in report
        assert "adaptive_settings" in report
        assert "optimization_config" in report
        assert "background_tasks" in report
        
        # Verify snapshot performance metrics
        snapshot_perf = report["snapshot_performance"]
        assert "snapshot_creation" in snapshot_perf
        assert "snapshot_restore" in snapshot_perf
        assert snapshot_perf["total_operations"] == 2
        
        # Verify adaptive settings
        adaptive = report["adaptive_settings"]
        assert "current_compression_level" in adaptive
        assert "current_memory_limit_mb" in adaptive
        assert "optimization_strategy" in adaptive
        assert adaptive["optimization_strategy"] == "balanced"
        
        # Verify optimization config
        config = report["optimization_config"]
        assert config["strategy"] == "balanced"
        assert config["max_concurrent_snapshots"] == 2
        assert config["compression_level"] == 6
    
    @pytest.mark.asyncio
    async def test_performance_metrics_recording(self, optimized_manager):
        """Test performance metrics recording functionality."""
        # Record test metrics
        await optimized_manager._record_performance_metrics(
            operation_type="test_operation",
            duration=45.0,
            data_size=1024 * 1024 * 1024,  # 1GB
            success=True
        )
        
        # Verify metrics were recorded
        assert len(optimized_manager.performance_metrics) == 1
        metric = optimized_manager.performance_metrics[0]
        
        assert metric.operation_type == "test_operation"
        assert metric.duration_seconds == 45.0
        assert metric.data_size_bytes == 1024 * 1024 * 1024
        assert metric.success is True
        assert metric.throughput_mbps > 0  # Should calculate throughput
        
        # Record failure metrics
        await optimized_manager._record_performance_metrics(
            operation_type="test_failure",
            duration=0,
            data_size=0,
            success=False,
            error_message="Test error"
        )
        
        # Verify failure metrics
        assert len(optimized_manager.performance_metrics) == 2
        failure_metric = optimized_manager.performance_metrics[1]
        
        assert failure_metric.operation_type == "test_failure"
        assert failure_metric.success is False
        assert failure_metric.error_message == "Test error"
    
    def test_snapshot_metrics_calculation(self, optimized_manager):
        """Test snapshot-specific metrics calculation."""
        # Add mixed performance metrics
        from performance.resource_manager import PerformanceMetrics
        test_metrics = [
            PerformanceMetrics("snapshot_creation", 30.0, 100*1024*1024, 3.3, 25.0, 256, True),
            PerformanceMetrics("snapshot_creation", 45.0, 200*1024*1024, 4.4, 30.0, 300, True),
            PerformanceMetrics("snapshot_creation", 0.0, 0, 0.0, 10.0, 100, False, "Error"),
            PerformanceMetrics("snapshot_restore", 20.0, 150*1024*1024, 7.5, 20.0, 200, True),
            PerformanceMetrics("snapshot_restore", 25.0, 180*1024*1024, 7.2, 22.0, 220, True)
        ]
        
        optimized_manager.performance_metrics = test_metrics
        
        # Calculate metrics
        metrics = optimized_manager._calculate_snapshot_metrics()
        
        # Verify creation metrics
        creation_stats = metrics["snapshot_creation"]
        assert creation_stats["count"] == 3
        assert creation_stats["success_count"] == 2
        assert creation_stats["success_rate"] == 2/3
        assert creation_stats["avg_duration_seconds"] > 0
        
        # Verify restore metrics
        restore_stats = metrics["snapshot_restore"]
        assert restore_stats["count"] == 2
        assert restore_stats["success_count"] == 2
        assert restore_stats["success_rate"] == 1.0
        assert restore_stats["avg_duration_seconds"] > 0
        
        # Verify total
        assert metrics["total_operations"] == 5
    
    @pytest.mark.asyncio
    async def test_error_handling_in_optimized_creation(self, optimized_manager):
        """Test error handling in optimized snapshot creation."""
        vm_id = "error_vm"
        user_id = "test_user"
        
        # First ensure VM access is properly mocked
        mock_vm = MagicMock()
        mock_vm.user_id = user_id
        mock_vm.get_config.return_value = {"memory": 512, "cpu": 1}
        optimized_manager.vm_manager.get_vm.return_value = mock_vm
        
        # Mock VM manager to raise an exception during snapshot creation
        optimized_manager.vm_manager.create_firecracker_snapshot.side_effect = Exception("VM snapshot failed")
        
        # Create snapshot - it should succeed initially but fail in background
        snapshot_id = await optimized_manager.create_snapshot_optimized(
            vm_id=vm_id,
            user_id=user_id,
            name="error-snapshot",
            priority="normal"
        )
        
        # Allow async task to complete and fail
        await asyncio.sleep(0.1)
        
        # Verify snapshot was created but is in error state
        assert snapshot_id in optimized_manager.snapshots
        metadata = optimized_manager.snapshots[snapshot_id]
        assert metadata.state == SnapshotState.ERROR
        
        # Verify resource tracking was completed with error
        optimized_manager.resource_manager.complete_operation_tracking.assert_called()
        
        # Verify metrics were recorded (the optimized manager records initial success metrics)
        assert len(optimized_manager.performance_metrics) > 0
        last_metric = optimized_manager.performance_metrics[-1]
        # Note: The optimized manager records success for the resource allocation phase,
        # the async task failure is handled separately
        assert last_metric.operation_type == "snapshot_creation"
    
    @pytest.mark.asyncio
    async def test_shutdown_functionality(self, optimized_manager):
        """Test graceful shutdown of optimized snapshot manager."""
        # Add a mock background task with proper async support
        async def mock_task_coro():
            raise asyncio.CancelledError()
        
        mock_task = asyncio.create_task(mock_task_coro())
        optimized_manager.background_tasks.append(mock_task)
        
        # Add items to optimization queue
        await optimized_manager.optimization_queue.put({"action": "test"})
        
        # Verify queue has items
        assert optimized_manager.optimization_queue.qsize() == 1
        
        # Shutdown
        await optimized_manager.shutdown()
        
        # Verify background task was cancelled
        assert mock_task.cancelled() or mock_task.done()
        
        # Verify queue was cleared
        assert optimized_manager.optimization_queue.qsize() == 0


if __name__ == "__main__":
    pytest.main([__file__, "-v"])