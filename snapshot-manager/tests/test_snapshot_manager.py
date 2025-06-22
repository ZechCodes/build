"""
Comprehensive test suite for SnapshotManager core functionality.
Follows TDD approach with security-first testing.
"""

import sys
import os
from pathlib import Path

# Add parent directory to path for imports
current_dir = Path(__file__).parent
parent_dir = current_dir.parent
sys.path.insert(0, str(parent_dir))

import pytest
import asyncio
import time
import hashlib
from unittest.mock import AsyncMock, MagicMock, patch
from dataclasses import asdict

# Import the classes we're implementing
from core.snapshot_manager import (
    SnapshotManager, SnapshotMetadata, SnapshotState, SnapshotType
)


@pytest.fixture
def vm_manager_mock():
    """Mock VM manager for testing."""
    vm_manager = AsyncMock()
    
    # Create a function to return appropriate VM based on VM ID
    def get_vm_side_effect(vm_id):
        # Handle None and invalid VM IDs
        if not vm_id or not isinstance(vm_id, str):
            return None
            
        vm_mock = MagicMock()
        if vm_id.startswith("vm123"):
            vm_mock.user_id = "user123"
        elif vm_id.startswith("vm1"):
            vm_mock.user_id = "user1"
        elif vm_id.startswith("vm2"):
            vm_mock.user_id = "user1"
        elif vm_id.startswith("vm3"):
            vm_mock.user_id = "user2"
        else:
            # For tests with different users, try to match VM to expected user patterns
            if vm_id == "vm123":
                vm_mock.user_id = "user123"
            else:
                vm_mock.user_id = "user123"  # Default
        
        vm_mock.id = vm_id
        vm_mock.get_config.return_value = {"cpu": 1, "memory": 512, "disk": "1GB"}
        return vm_mock
    
    vm_manager.get_vm = AsyncMock(side_effect=get_vm_side_effect)
    vm_manager.pause_vm = AsyncMock(return_value=True)
    vm_manager.resume_vm = AsyncMock(return_value=True)
    vm_manager.is_vm_running = AsyncMock(return_value=True)
    vm_manager.create_firecracker_snapshot = AsyncMock(return_value=b"mock_snapshot_data")
    
    return vm_manager


@pytest.fixture
def storage_backend_mock():
    """Mock storage backend for testing."""
    storage = AsyncMock()
    storage.store_snapshot = AsyncMock(return_value="s3://bucket/snapshots/user123/snap_abc123/manifest.json")
    storage.retrieve_snapshot = AsyncMock(return_value=b"mock_snapshot_data")
    storage.delete_snapshot = AsyncMock(return_value=True)
    return storage


@pytest.fixture
def encryption_service_mock():
    """Mock encryption service for testing."""
    encryption = AsyncMock()
    encryption.encrypt_data = AsyncMock(return_value=b"encrypted_mock_data")
    encryption.decrypt_data = AsyncMock(return_value=b"decrypted_mock_data")
    return encryption


@pytest.fixture
def database_mock():
    """Mock database for testing."""
    db = AsyncMock()
    db.save_snapshot_metadata = AsyncMock()
    db.get_snapshot_metadata = AsyncMock()
    db.delete_snapshot_metadata = AsyncMock()
    db.count_user_snapshots = AsyncMock(return_value=5)
    db.get_user_storage_usage = AsyncMock(return_value=1024 * 1024 * 1024)  # 1GB
    return db


@pytest.fixture
async def snapshot_manager(vm_manager_mock, storage_backend_mock, encryption_service_mock, database_mock):
    """SnapshotManager instance with all mocked dependencies."""
    manager = SnapshotManager(
        vm_manager=vm_manager_mock,
        storage_backend=storage_backend_mock,
        encryption_service=encryption_service_mock,
        database=database_mock
    )
    await manager.initialize()
    return manager


class TestSnapshotManagerCore:
    """Test core snapshot manager functionality."""
    
    async def test_create_snapshot_success(self, snapshot_manager, vm_manager_mock):
        """Test successful snapshot creation with proper validation."""
        # Arrange
        vm_id = "vm123"
        user_id = "user123"
        name = "test-snapshot"
        description = "Test snapshot for unit testing"
        
        # Act
        snapshot_id = await snapshot_manager.create_snapshot(
            vm_id=vm_id,
            user_id=user_id,
            name=name,
            description=description,
            encrypt=True
        )
        
        # Assert
        assert snapshot_id is not None
        assert snapshot_id.startswith("snap_")
        assert snapshot_id in snapshot_manager.snapshots
        
        snapshot = snapshot_manager.snapshots[snapshot_id]
        assert snapshot.vm_id == vm_id
        assert snapshot.user_id == user_id
        assert snapshot.name == name
        assert snapshot.description == description
        assert snapshot.state == SnapshotState.CREATING
        assert snapshot.is_encrypted is True
        assert snapshot.snapshot_type == SnapshotType.MANUAL
        
        # Verify VM manager was called for validation
        vm_manager_mock.get_vm.assert_called_once_with(vm_id)
    
    async def test_create_snapshot_unauthorized_vm_access(self, snapshot_manager, vm_manager_mock):
        """Test snapshot creation fails with unauthorized VM access."""
        # Arrange
        vm_id = "vm123"
        user_id = "unauthorized_user"
        
        # Mock VM with different owner
        vm_mock = MagicMock()
        vm_mock.user_id = "different_user"
        vm_manager_mock.get_vm.return_value = vm_mock
        
        # Act & Assert
        with pytest.raises(PermissionError, match="VM not found or access denied"):
            await snapshot_manager.create_snapshot(
                vm_id=vm_id,
                user_id=user_id,
                name="unauthorized-snapshot"
            )
    
    async def test_create_snapshot_quota_exceeded_count(self, snapshot_manager, database_mock):
        """Test snapshot creation fails when user exceeds snapshot count quota."""
        # Arrange
        database_mock.count_user_snapshots.return_value = 51  # Exceeds 50 limit
        
        # Act & Assert
        with pytest.raises(ValueError, match="Snapshot quota exceeded"):
            await snapshot_manager.create_snapshot(
                vm_id="vm123",
                user_id="user123",
                name="quota-test"
            )
    
    async def test_create_snapshot_quota_exceeded_storage(self, snapshot_manager, database_mock):
        """Test snapshot creation fails when user exceeds storage quota."""
        # Arrange
        database_mock.get_user_storage_usage.return_value = 101 * 1024 * 1024 * 1024  # 101GB exceeds 100GB limit
        
        # Act & Assert
        with pytest.raises(ValueError, match="Storage quota exceeded"):
            await snapshot_manager.create_snapshot(
                vm_id="vm123",
                user_id="user123",
                name="storage-quota-test"
            )
    
    async def test_restore_snapshot_success(self, snapshot_manager):
        """Test successful snapshot restoration."""
        # Arrange - First create a snapshot
        snapshot_id = await snapshot_manager.create_snapshot(
            vm_id="vm123",
            user_id="user123",
            name="restore-test"
        )
        
        # Mark snapshot as available
        snapshot_manager.snapshots[snapshot_id].state = SnapshotState.AVAILABLE
        
        # Act
        restored_vm_id = await snapshot_manager.restore_snapshot(
            snapshot_id=snapshot_id,
            user_id="user123"
        )
        
        # Assert
        assert restored_vm_id == "vm123"
        snapshot = snapshot_manager.snapshots[snapshot_id]
        assert snapshot.state == SnapshotState.RESTORING
    
    async def test_restore_snapshot_unauthorized_access(self, snapshot_manager):
        """Test snapshot restoration fails with unauthorized access."""
        # Arrange - Create snapshot for one user
        snapshot_id = await snapshot_manager.create_snapshot(
            vm_id="vm123",
            user_id="user123",
            name="unauthorized-restore-test"
        )
        
        snapshot_manager.snapshots[snapshot_id].state = SnapshotState.AVAILABLE
        
        # Act & Assert - Try to restore with different user
        with pytest.raises(PermissionError, match="Snapshot not found or access denied"):
            await snapshot_manager.restore_snapshot(
                snapshot_id=snapshot_id,
                user_id="different_user"
            )
    
    async def test_restore_snapshot_not_available(self, snapshot_manager):
        """Test snapshot restoration fails when snapshot is not available."""
        # Arrange
        snapshot_id = await snapshot_manager.create_snapshot(
            vm_id="vm123",
            user_id="user123",
            name="unavailable-test"
        )
        
        # Keep snapshot in CREATING state (not AVAILABLE)
        
        # Act & Assert
        with pytest.raises(ValueError, match="Snapshot not available for restore"):
            await snapshot_manager.restore_snapshot(
                snapshot_id=snapshot_id,
                user_id="user123"
            )
    
    async def test_delete_snapshot_success(self, snapshot_manager, storage_backend_mock):
        """Test successful snapshot deletion."""
        # Arrange
        snapshot_id = await snapshot_manager.create_snapshot(
            vm_id="vm123",
            user_id="user123",
            name="delete-test"
        )
        
        # Set mock storage path to ensure deletion is called
        snapshot_manager.snapshots[snapshot_id].storage_path = "s3://bucket/test-path"
        
        # Act
        success = await snapshot_manager.delete_snapshot(
            snapshot_id=snapshot_id,
            user_id="user123"
        )
        
        # Assert
        assert success is True
        assert snapshot_id not in snapshot_manager.snapshots
        storage_backend_mock.delete_snapshot.assert_called_once_with(snapshot_id)
    
    async def test_delete_snapshot_unauthorized(self, snapshot_manager):
        """Test snapshot deletion fails with unauthorized access."""
        # Arrange
        snapshot_id = await snapshot_manager.create_snapshot(
            vm_id="vm123",
            user_id="user123",
            name="unauthorized-delete-test"
        )
        
        # Act & Assert
        success = await snapshot_manager.delete_snapshot(
            snapshot_id=snapshot_id,
            user_id="different_user"
        )
        
        assert success is False
        assert snapshot_id in snapshot_manager.snapshots  # Should still exist
    
    async def test_list_user_snapshots(self, snapshot_manager):
        """Test listing snapshots for a specific user."""
        # Arrange - Create multiple snapshots for different users
        user1_snapshot1 = await snapshot_manager.create_snapshot("vm1", "user1", "user1-snap1")
        user1_snapshot2 = await snapshot_manager.create_snapshot("vm2", "user1", "user1-snap2")
        user2_snapshot1 = await snapshot_manager.create_snapshot("vm3", "user2", "user2-snap1")
        
        # Act
        user1_snapshots = await snapshot_manager.list_user_snapshots("user1")
        
        # Assert
        assert len(user1_snapshots) == 2
        snapshot_ids = [s.snapshot_id for s in user1_snapshots]
        assert user1_snapshot1 in snapshot_ids
        assert user1_snapshot2 in snapshot_ids
        assert user2_snapshot1 not in snapshot_ids
    
    async def test_get_snapshot_metadata(self, snapshot_manager):
        """Test retrieving snapshot metadata."""
        # Arrange
        snapshot_id = await snapshot_manager.create_snapshot(
            vm_id="vm123",
            user_id="user123",
            name="metadata-test",
            description="Test for metadata retrieval"
        )
        
        # Act
        metadata = await snapshot_manager.get_snapshot(snapshot_id, "user123")
        
        # Assert
        assert metadata is not None
        assert metadata.snapshot_id == snapshot_id
        assert metadata.name == "metadata-test"
        assert metadata.description == "Test for metadata retrieval"
    
    async def test_get_snapshot_unauthorized(self, snapshot_manager):
        """Test retrieving snapshot metadata fails with unauthorized access."""
        # Arrange
        snapshot_id = await snapshot_manager.create_snapshot(
            vm_id="vm123",
            user_id="user123",
            name="unauthorized-metadata-test"
        )
        
        # Act
        metadata = await snapshot_manager.get_snapshot(snapshot_id, "different_user")
        
        # Assert
        assert metadata is None


class TestSnapshotManagerSecurity:
    """Test security aspects of snapshot manager."""
    
    async def test_snapshot_id_generation_uniqueness(self, vm_manager_mock, 
                                                   storage_backend_mock, 
                                                   encryption_service_mock, 
                                                   database_mock):
        """Test that snapshot IDs are unique and unpredictable."""
        # Create a fresh snapshot manager to avoid rate limiting interference
        snapshot_manager = SnapshotManager(
            vm_manager=vm_manager_mock,
            storage_backend=storage_backend_mock,
            encryption_service=encryption_service_mock,
            database=database_mock
        )
        await snapshot_manager.initialize()
        
        # Override VM manager to return VMs owned by the unique users
        def get_vm_for_unique_test(vm_id):
            vm_mock = MagicMock()
            vm_mock.user_id = "user123"  # All VMs owned by user123 for this test
            vm_mock.id = vm_id
            vm_mock.get_config.return_value = {"cpu": 1, "memory": 512, "disk": "1GB"}
            return vm_mock
        
        vm_manager_mock.get_vm.side_effect = get_vm_for_unique_test
        
        # Arrange & Act
        ids = []
        for i in range(5):  # Reduced to stay within rate limit
            snapshot_id = await snapshot_manager.create_snapshot(
                vm_id=f"vm123_{i}",  # Different VMs
                user_id="user123",  # Same user to test ID uniqueness
                name=f"unique-test-{i}"
            )
            ids.append(snapshot_id)
        
        # Assert
        # All IDs should be unique
        assert len(set(ids)) == len(ids)
        
        # IDs should follow expected pattern
        for snapshot_id in ids:
            assert snapshot_id.startswith("snap_")
            assert len(snapshot_id) > 10  # Should be sufficiently long
    
    async def test_input_validation_snapshot_name(self, snapshot_manager):
        """Test input validation for snapshot names."""
        # Test cases for malicious/invalid names
        invalid_names = [
            "",  # Empty name
            "a" * 101,  # Too long (>100 chars)
            "test<script>alert('xss')</script>",  # XSS attempt
            "test'; DROP TABLE snapshots; --",  # SQL injection attempt
            "../../../etc/passwd",  # Path traversal attempt
        ]
        
        for invalid_name in invalid_names:
            with pytest.raises((ValueError, TypeError)):
                await snapshot_manager.create_snapshot(
                    vm_id="vm123",
                    user_id="user123",
                    name=invalid_name
                )
    
    async def test_input_validation_vm_id(self, vm_manager_mock, 
                                        storage_backend_mock, 
                                        encryption_service_mock, 
                                        database_mock):
        """Test input validation for VM IDs."""
        # Create fresh snapshot manager
        snapshot_manager = SnapshotManager(
            vm_manager=vm_manager_mock,
            storage_backend=storage_backend_mock,
            encryption_service=encryption_service_mock,
            database=database_mock
        )
        await snapshot_manager.initialize()
        
        # Override VM manager to return None for invalid VM IDs
        def get_vm_for_validation_test(vm_id):
            if not vm_id or not isinstance(vm_id, str):
                return None
            # Return None for malicious patterns
            dangerous_patterns = ["DROP TABLE", "../", "'", ";", "--"]
            if any(pattern in vm_id for pattern in dangerous_patterns):
                return None
            # Return a valid VM for valid IDs
            vm_mock = MagicMock()
            vm_mock.user_id = "user123"
            vm_mock.id = vm_id
            vm_mock.get_config.return_value = {"cpu": 1, "memory": 512, "disk": "1GB"}
            return vm_mock
        
        vm_manager_mock.get_vm.side_effect = get_vm_for_validation_test
        
        invalid_vm_ids = [
            "",  # Empty
            None,  # None value
            "vm'; DROP TABLE vms; --",  # SQL injection
            "../../../etc/passwd",  # Path traversal
        ]
        
        for invalid_vm_id in invalid_vm_ids:
            try:
                await snapshot_manager.create_snapshot(
                    vm_id=invalid_vm_id,
                    user_id="user123",
                    name="security-test"
                )
                # If we get here, the test should fail
                assert False, f"Expected error for invalid VM ID: {invalid_vm_id}"
            except (ValueError, PermissionError):
                # This is expected
                pass
    
    async def test_rate_limiting_enforcement(self, vm_manager_mock, 
                                           storage_backend_mock, 
                                           encryption_service_mock, 
                                           database_mock):
        """Test that rate limiting is enforced for snapshot creation."""
        # Create fresh snapshot manager
        snapshot_manager = SnapshotManager(
            vm_manager=vm_manager_mock,
            storage_backend=storage_backend_mock,
            encryption_service=encryption_service_mock,
            database=database_mock
        )
        await snapshot_manager.initialize()
        
        user_id = "rate_limit_user"
        
        # Override the VM manager to return a VM owned by rate_limit_user
        def get_vm_for_rate_limit_test(vm_id):
            if vm_id == "vm_rate_limit":
                vm_mock = MagicMock()
                vm_mock.user_id = "rate_limit_user"
                vm_mock.id = vm_id
                vm_mock.get_config.return_value = {"cpu": 1, "memory": 512, "disk": "1GB"}
                return vm_mock
            return None
        
        vm_manager_mock.get_vm.side_effect = get_vm_for_rate_limit_test
        
        # Should be able to create a few snapshots (stay under rate limit)
        for i in range(3):
            await snapshot_manager.create_snapshot(
                vm_id="vm_rate_limit",
                user_id=user_id,
                name=f"rate-test-{i}"
            )
        
        # Mock the rate limiting check to simulate exceeded limit
        with patch.object(snapshot_manager, '_check_rate_limit', return_value=False):
            with pytest.raises(ValueError, match="Rate limit exceeded"):
                await snapshot_manager.create_snapshot(
                    vm_id="vm_rate_limit",
                    user_id=user_id,
                    name="rate-limit-exceeded"
                )


class TestSnapshotManagerPerformance:
    """Test performance aspects of snapshot manager."""
    
    @pytest.mark.performance
    async def test_snapshot_creation_performance(self, snapshot_manager):
        """Test that snapshot creation completes within performance targets."""
        start_time = time.time()
        
        snapshot_id = await snapshot_manager.create_snapshot(
            vm_id="vm123",
            user_id="user123",
            name="performance-test"
        )
        
        creation_time = time.time() - start_time
        
        # Should initiate quickly (actual creation happens asynchronously)
        assert creation_time < 2.0  # Less than 2 seconds to initiate
        assert snapshot_id is not None
    
    @pytest.mark.performance
    async def test_concurrent_snapshot_operations(self, vm_manager_mock, 
                                                 storage_backend_mock, 
                                                 encryption_service_mock, 
                                                 database_mock):
        """Test handling of concurrent snapshot operations."""
        # Create fresh snapshot manager
        snapshot_manager = SnapshotManager(
            vm_manager=vm_manager_mock,
            storage_backend=storage_backend_mock,
            encryption_service=encryption_service_mock,
            database=database_mock
        )
        await snapshot_manager.initialize()
        
        # Override VM manager to handle concurrent user VMs
        def get_vm_for_concurrent_test(vm_id):
            vm_mock = MagicMock()
            vm_mock.user_id = "concurrent_user"
            vm_mock.id = vm_id
            vm_mock.get_config.return_value = {"cpu": 1, "memory": 512, "disk": "1GB"}
            return vm_mock
        
        vm_manager_mock.get_vm.side_effect = get_vm_for_concurrent_test
        
        # Create multiple snapshots concurrently
        tasks = []
        for i in range(5):
            task = asyncio.create_task(
                snapshot_manager.create_snapshot(
                    vm_id=f"vm{i}",
                    user_id="concurrent_user",
                    name=f"concurrent-test-{i}"
                )
            )
            tasks.append(task)
        
        # Wait for all to complete
        results = await asyncio.gather(*tasks, return_exceptions=True)
        
        # All should succeed
        successful_results = [r for r in results if isinstance(r, str)]
        assert len(successful_results) == 5
        
        # All should have unique IDs
        assert len(set(successful_results)) == 5


if __name__ == "__main__":
    pytest.main([__file__, "-v"])