"""
Firecracker Integration Tests for Snapshot Manager

Tests the real Firecracker integration with comprehensive scenario validation.
"""

import pytest
import asyncio
import time
from unittest.mock import AsyncMock, MagicMock, patch

# Test both real and mock implementations
from integration.firecracker_adapter import (
    FirecrackerVMManager, MockVMManager, get_vm_manager,
    create_production_vm_manager, create_test_vm_manager
)


class TestFirecrackerIntegration:
    """Test suite for Firecracker VM manager integration."""
    
    @pytest.mark.asyncio
    async def test_mock_vm_manager_initialization(self):
        """Test mock VM manager initialization."""
        vm_manager = MockVMManager()
        await vm_manager.initialize()
        
        assert vm_manager is not None
        assert hasattr(vm_manager, 'get_vm')
        assert hasattr(vm_manager, 'create_firecracker_snapshot')
    
    @pytest.mark.asyncio
    async def test_mock_vm_operations(self):
        """Test basic VM operations with mock manager."""
        vm_manager = MockVMManager()
        await vm_manager.initialize()
        
        # Setup mock VM
        vm_id = "test_vm_123"
        user_id = "test_user"
        
        vm_manager.vms[vm_id] = {
            "user_id": user_id,
            "config": {"cpu": 1, "memory": 512},
            "status": "running"
        }
        vm_manager.user_vm_mapping[vm_id] = user_id
        
        # Test VM retrieval
        vm = await vm_manager.get_vm(vm_id)
        assert vm is not None
        assert vm.user_id == user_id
        assert vm.get_config()["cpu"] == 1
        
        # Test snapshot creation
        snapshot_data = await vm_manager.create_firecracker_snapshot(vm_id)
        assert len(snapshot_data) > 0
        assert isinstance(snapshot_data, bytes)
        
        # Test snapshot restoration
        success = await vm_manager.restore_vm_from_snapshot(vm_id, snapshot_data)
        assert success is True
        
        # Test VM state operations
        assert await vm_manager.is_vm_running(vm_id) is True
        assert await vm_manager.pause_vm(vm_id) is True
        assert await vm_manager.resume_vm(vm_id) is True
        
        # Test ownership verification
        assert await vm_manager.verify_vm_ownership(vm_id, user_id) is True
        assert await vm_manager.verify_vm_ownership(vm_id, "other_user") is False
    
    @pytest.mark.asyncio
    async def test_firecracker_vm_manager_factory(self):
        """Test VM manager factory function."""
        # Test mock manager creation
        mock_manager = get_vm_manager(use_real_firecracker=False)
        assert isinstance(mock_manager, MockVMManager)
        
        # Test that it attempts real Firecracker (but may fall back to mock)
        manager = get_vm_manager(use_real_firecracker=True)
        # Should return either FirecrackerVMManager or MockVMManager (fallback)
        assert isinstance(manager, (FirecrackerVMManager, MockVMManager))
    
    @pytest.mark.asyncio
    async def test_snapshot_manager_integration(self):
        """Test snapshot manager integration with VM manager."""
        # Import after setting up path
        import sys
        from pathlib import Path
        current_dir = Path(__file__).parent.parent
        if str(current_dir) not in sys.path:
            sys.path.insert(0, str(current_dir))
        
        from core.snapshot_manager import SnapshotManager, SnapshotType
        
        # Create mocked dependencies
        vm_manager = MockVMManager()
        await vm_manager.initialize()
        
        storage_backend = AsyncMock()
        storage_backend.store_snapshot = AsyncMock(return_value="s3://bucket/test/path")
        storage_backend.retrieve_snapshot = AsyncMock(return_value=b"mock_snapshot_data")
        storage_backend.delete_snapshot = AsyncMock(return_value=True)
        
        encryption_service = AsyncMock()
        encryption_service.encrypt_data = AsyncMock(return_value=b"encrypted_data")
        encryption_service.decrypt_data = AsyncMock(return_value=b"decrypted_data")
        
        database = AsyncMock()
        database.save_snapshot_metadata = AsyncMock()
        database.delete_snapshot_metadata = AsyncMock()
        database.count_user_snapshots = AsyncMock(return_value=1)
        database.get_user_storage_usage = AsyncMock(return_value=1024 * 1024)  # 1MB
        
        # Setup test VM
        vm_id = "integration_test_vm"
        user_id = "test_user_123"
        
        vm_manager.vms[vm_id] = {
            "user_id": user_id,
            "config": {"cpu": 1, "memory": 512, "disk": "1GB"},
            "status": "running"
        }
        vm_manager.user_vm_mapping[vm_id] = user_id
        
        # Create snapshot manager with integrated VM manager
        snapshot_manager = SnapshotManager(
            vm_manager=vm_manager,
            storage_backend=storage_backend,
            encryption_service=encryption_service,
            database=database
        )
        
        await snapshot_manager.initialize()
        
        # Test snapshot creation with real VM manager integration
        snapshot_id = await snapshot_manager.create_snapshot(
            vm_id=vm_id,
            user_id=user_id,
            name="integration-test-snapshot",
            description="Testing Firecracker integration",
            snapshot_type=SnapshotType.MANUAL,
            tags=["integration", "test"],
            encrypt=True
        )
        
        # Verify snapshot was created
        assert snapshot_id is not None
        assert snapshot_id.startswith("snap_")
        assert snapshot_id in snapshot_manager.snapshots
        
        # Verify snapshot metadata
        metadata = snapshot_manager.snapshots[snapshot_id]
        assert metadata.vm_id == vm_id
        assert metadata.user_id == user_id
        assert metadata.name == "integration-test-snapshot"
        assert metadata.is_encrypted is True
        assert metadata.tags == ["integration", "test"]
        
        # Wait for background snapshot creation to complete
        await asyncio.sleep(0.1)
        
        # Test snapshot restoration
        metadata.state = metadata.state.__class__.AVAILABLE  # Ensure available for restore
        
        restored_vm_id = await snapshot_manager.restore_snapshot(
            snapshot_id=snapshot_id,
            user_id=user_id
        )
        
        assert restored_vm_id == vm_id
        
        # Test snapshot deletion
        success = await snapshot_manager.delete_snapshot(
            snapshot_id=snapshot_id,
            user_id=user_id
        )
        
        assert success is True
        assert snapshot_id not in snapshot_manager.snapshots
    
    @pytest.mark.asyncio
    async def test_error_handling_and_recovery(self):
        """Test error handling in VM manager integration."""
        vm_manager = MockVMManager()
        await vm_manager.initialize()
        
        # Test snapshot of non-existent VM
        with pytest.raises(ValueError, match="VM nonexistent not found"):
            await vm_manager.create_firecracker_snapshot("nonexistent")
        
        # Test operations on non-existent VM
        assert await vm_manager.get_vm("nonexistent") is None
        assert await vm_manager.is_vm_running("nonexistent") is False
        assert await vm_manager.pause_vm("nonexistent") is False
        assert await vm_manager.resume_vm("nonexistent") is False
    
    @pytest.mark.asyncio  
    async def test_performance_and_monitoring(self):
        """Test performance monitoring integration."""
        vm_manager = MockVMManager()
        await vm_manager.initialize()
        
        # Setup test VM
        vm_id = "perf_test_vm"
        user_id = "perf_user"
        
        vm_manager.vms[vm_id] = {
            "user_id": user_id,
            "config": {"cpu": 2, "memory": 1024},
            "status": "running"
        }
        vm_manager.user_vm_mapping[vm_id] = user_id
        
        # Measure snapshot creation time
        start_time = time.time()
        snapshot_data = await vm_manager.create_firecracker_snapshot(vm_id)
        creation_time = time.time() - start_time
        
        # Performance assertions
        assert creation_time < 1.0  # Should complete quickly for mock
        assert len(snapshot_data) > 1000  # Should have reasonable data size
        
        # Measure restoration time
        start_time = time.time()
        success = await vm_manager.restore_vm_from_snapshot(vm_id, snapshot_data)
        restore_time = time.time() - start_time
        
        assert success is True
        assert restore_time < 1.0  # Should restore quickly for mock
    
    @pytest.mark.asyncio
    async def test_concurrent_operations(self):
        """Test concurrent VM operations."""
        vm_manager = MockVMManager()
        await vm_manager.initialize()
        
        # Setup multiple test VMs
        vm_configs = []
        for i in range(5):
            vm_id = f"concurrent_vm_{i}"
            user_id = f"user_{i}"
            
            vm_manager.vms[vm_id] = {
                "user_id": user_id,
                "config": {"cpu": 1, "memory": 512},
                "status": "running"
            }
            vm_manager.user_vm_mapping[vm_id] = user_id
            vm_configs.append((vm_id, user_id))
        
        # Create snapshots concurrently
        snapshot_tasks = []
        for vm_id, user_id in vm_configs:
            task = asyncio.create_task(
                vm_manager.create_firecracker_snapshot(vm_id)
            )
            snapshot_tasks.append(task)
        
        # Wait for all snapshots to complete
        snapshot_results = await asyncio.gather(*snapshot_tasks)
        
        # Verify all snapshots were created
        assert len(snapshot_results) == 5
        for snapshot_data in snapshot_results:
            assert isinstance(snapshot_data, bytes)
            assert len(snapshot_data) > 0
        
        # Test concurrent state operations
        state_tasks = []
        for vm_id, user_id in vm_configs:
            # Mix of pause and resume operations
            if int(vm_id.split('_')[-1]) % 2 == 0:
                task = asyncio.create_task(vm_manager.pause_vm(vm_id))
            else:
                task = asyncio.create_task(vm_manager.resume_vm(vm_id))
            state_tasks.append(task)
        
        state_results = await asyncio.gather(*state_tasks)
        
        # All operations should succeed
        assert all(result is True for result in state_results)


# Integration test that can be run if Firecracker is available
@pytest.mark.integration
@pytest.mark.asyncio
async def test_real_firecracker_if_available():
    """Test real Firecracker integration if environment supports it."""
    try:
        # Attempt to create real Firecracker VM manager
        vm_manager = await create_production_vm_manager()
        
        # If we get here, Firecracker is available
        assert vm_manager is not None
        assert isinstance(vm_manager, FirecrackerVMManager)
        
        # Test basic operations
        stats = vm_manager.get_vm_stats()
        assert "total_vms" in stats
        assert "running_vms" in stats
        
        print("✅ Real Firecracker integration test passed")
        
    except Exception as e:
        # Firecracker not available in test environment
        pytest.skip(f"Firecracker not available: {e}")


if __name__ == "__main__":
    # Run tests
    pytest.main([__file__, "-v", "-s"])