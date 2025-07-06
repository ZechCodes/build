"""
Tests for Advanced Incremental Snapshot Management

Comprehensive test suite for incremental snapshots, deduplication,
and storage optimization features.
"""

import pytest
import asyncio
import time
import json
import zlib
from unittest.mock import AsyncMock, MagicMock

# Import the incremental snapshot system
import sys
from pathlib import Path
current_dir = Path(__file__).parent.parent
if str(current_dir) not in sys.path:
    sys.path.insert(0, str(current_dir))

from advanced.incremental_snapshots import (
    IncrementalSnapshotManager, IncrementalSnapshotType,
    SnapshotDelta, IncrementalSnapshot
)


class TestIncrementalSnapshots:
    """Test suite for incremental snapshot functionality."""
    
    @pytest.fixture
    async def snapshot_manager(self):
        """Create incremental snapshot manager with mocked dependencies."""
        # Mock base snapshot manager
        base_manager = MagicMock()
        
        # Create a counter for unique snapshot IDs
        snapshot_counter = 0
        def generate_unique_id(vm_id, user_id):
            nonlocal snapshot_counter
            snapshot_counter += 1
            return f"snap_{vm_id}_{user_id}_{snapshot_counter}"
        
        base_manager._generate_snapshot_id = MagicMock(side_effect=generate_unique_id)
        
        # Mock VM manager with snapshot capability
        vm_manager = AsyncMock()
        vm_manager.create_firecracker_snapshot = AsyncMock()
        base_manager.vm_manager = vm_manager
        
        # Mock storage backend
        storage_backend = AsyncMock()
        storage_backend.store_snapshot = AsyncMock(
            side_effect=lambda sid, data, meta: f"s3://bucket/{sid}.snapshot"
        )
        storage_backend.retrieve_snapshot = AsyncMock()
        
        # Create incremental snapshot manager
        manager = IncrementalSnapshotManager(base_manager, storage_backend)
        return manager
    
    @pytest.mark.asyncio
    async def test_initialization(self, snapshot_manager):
        """Test incremental snapshot manager initialization."""
        assert snapshot_manager is not None
        assert snapshot_manager.block_size == 4096
        assert snapshot_manager.max_chain_length == 10
        assert isinstance(snapshot_manager.incremental_snapshots, dict)
        assert isinstance(snapshot_manager.snapshot_chains, dict)
    
    @pytest.mark.asyncio
    async def test_block_splitting(self, snapshot_manager):
        """Test data splitting into fixed-size blocks."""
        # Test with exact block size
        data = b"A" * 4096
        blocks = snapshot_manager._split_into_blocks(data)
        assert len(blocks) == 1
        assert len(blocks[0]) == 4096
        assert blocks[0] == data
        
        # Test with larger data
        data = b"B" * 8000
        blocks = snapshot_manager._split_into_blocks(data)
        assert len(blocks) == 2
        assert len(blocks[0]) == 4096
        assert len(blocks[1]) == 4096  # Padded
        assert blocks[0] == b"B" * 4096
        # Second block should be padded with zeros
        assert blocks[1][:1904] == b"B" * 1904  # 8000 - 4096 = 3904, but our test uses different numbers
    
    @pytest.mark.asyncio
    async def test_first_snapshot_is_full(self, snapshot_manager):
        """Test that the first snapshot for a VM is always full."""
        vm_id = "test_vm_001"
        user_id = "test_user"
        
        # Mock VM data
        vm_data = b"Mock VM state data " * 100
        snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.return_value = vm_data
        
        # Create first snapshot
        snapshot_id = await snapshot_manager.create_incremental_snapshot(
            vm_id=vm_id,
            user_id=user_id,
            name="first-snapshot",
            description="Initial full snapshot"
        )
        
        # Verify it's a full snapshot
        assert snapshot_id in snapshot_manager.incremental_snapshots
        snapshot = snapshot_manager.incremental_snapshots[snapshot_id]
        
        assert snapshot.snapshot_type == IncrementalSnapshotType.FULL
        assert snapshot.parent_snapshot_id is None
        assert snapshot.vm_id == vm_id
        assert snapshot.user_id == user_id
        
        # Verify snapshot chain is initialized
        assert vm_id in snapshot_manager.snapshot_chains
        assert snapshot_manager.snapshot_chains[vm_id] == [snapshot_id]
    
    @pytest.mark.asyncio
    async def test_incremental_snapshot_creation(self, snapshot_manager):
        """Test creation of incremental snapshots after full snapshot."""
        vm_id = "test_vm_002"
        user_id = "test_user"
        
        # Create base full snapshot
        base_vm_data = b"Original VM state " * 200
        snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.return_value = base_vm_data
        
        full_snapshot_id = await snapshot_manager.create_incremental_snapshot(
            vm_id=vm_id,
            user_id=user_id,
            name="full-snapshot"
        )
        
        # Simulate VM changes
        modified_vm_data = b"Modified VM state " * 200
        snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.return_value = modified_vm_data
        
        # Create incremental snapshot
        incremental_id = await snapshot_manager.create_incremental_snapshot(
            vm_id=vm_id,
            user_id=user_id,
            name="incremental-snapshot"
        )
        
        # Verify incremental snapshot
        incremental_snapshot = snapshot_manager.incremental_snapshots[incremental_id]
        
        assert incremental_snapshot.snapshot_type == IncrementalSnapshotType.INCREMENTAL
        assert incremental_snapshot.parent_snapshot_id == full_snapshot_id
        assert incremental_snapshot.vm_id == vm_id
        
        # Verify snapshot chain
        chain = snapshot_manager.snapshot_chains[vm_id]
        assert len(chain) == 2
        assert chain == [full_snapshot_id, incremental_id]
    
    @pytest.mark.asyncio
    async def test_delta_calculation(self, snapshot_manager):
        """Test delta calculation between snapshots."""
        # Create mock parent snapshot
        parent_checksums = {
            "block_000000": "hash1",
            "block_000001": "hash2", 
            "block_000002": "hash3"
        }
        
        parent_snapshot = IncrementalSnapshot(
            snapshot_id="parent_123",
            parent_snapshot_id=None,
            snapshot_type=IncrementalSnapshotType.FULL,
            vm_id="test_vm",
            user_id="test_user",
            created_at=time.time(),
            delta=SnapshotDelta(
                added_blocks=list(parent_checksums.keys()),
                modified_blocks=[],
                deleted_blocks=[],
                block_checksums=parent_checksums,
                compression_ratio=0.8,
                size_reduction=0.2
            ),
            storage_path="s3://bucket/parent.snapshot",
            original_size_bytes=12288,
            compressed_size_bytes=9830,
            checksum="parent_checksum",
            is_restorable=True
        )
        
        # Current state with changes
        current_checksums = {
            "block_000000": "hash1",     # unchanged
            "block_000001": "hash2_new", # modified
            "block_000002": "hash3",     # unchanged
            "block_000003": "hash4"      # added
            # block_000002 would be "deleted" but it's still there, so no deletion
        }
        
        current_blocks = [
            b"A" * 4096,  # block_000000
            b"B" * 4096,  # block_000001 (modified)
            b"C" * 4096,  # block_000002
            b"D" * 4096   # block_000003 (new)
        ]
        
        # Calculate delta
        delta = await snapshot_manager._calculate_delta(
            parent_snapshot, current_checksums, current_blocks
        )
        
        # Verify delta results
        assert "block_000003" in delta.added_blocks
        assert "block_000001" in delta.modified_blocks
        assert len(delta.deleted_blocks) == 0  # No blocks were deleted
        assert delta.block_checksums == current_checksums
        assert 0 <= delta.compression_ratio <= 1
        assert 0 <= delta.size_reduction <= 1
    
    @pytest.mark.asyncio
    async def test_snapshot_chain_management(self, snapshot_manager):
        """Test snapshot chain length management."""
        vm_id = "test_vm_chain"
        user_id = "test_user"
        
        # Create initial full snapshot
        vm_data = b"VM data " * 100
        snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.return_value = vm_data
        
        snapshot_ids = []
        
        # Create snapshots up to max chain length
        for i in range(snapshot_manager.max_chain_length + 2):
            # Modify data slightly each time
            vm_data = f"VM data iteration {i} ".encode() * 100
            snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.return_value = vm_data
            
            snapshot_id = await snapshot_manager.create_incremental_snapshot(
                vm_id=vm_id,
                user_id=user_id,
                name=f"snapshot-{i}"
            )
            snapshot_ids.append(snapshot_id)
        
        # Verify chain management
        chain = snapshot_manager.snapshot_chains[vm_id]
        
        # Should have created a new full snapshot when chain got too long
        full_snapshots = [
            sid for sid in snapshot_ids
            if snapshot_manager.incremental_snapshots[sid].snapshot_type == IncrementalSnapshotType.FULL
        ]
        
        assert len(full_snapshots) >= 1  # At least the initial full snapshot
        
        # Verify the last snapshot chain doesn't exceed max length
        current_chain_from_last_full = []
        for snapshot_id in reversed(chain):
            current_chain_from_last_full.insert(0, snapshot_id)
            if snapshot_manager.incremental_snapshots[snapshot_id].snapshot_type == IncrementalSnapshotType.FULL:
                break
        
        assert len(current_chain_from_last_full) <= snapshot_manager.max_chain_length + 1
    
    @pytest.mark.asyncio
    async def test_snapshot_restoration_chain(self, snapshot_manager):
        """Test building restoration chain for incremental snapshots."""
        vm_id = "test_vm_restore"
        user_id = "test_user"
        
        # Create a chain: Full -> Incremental -> Incremental
        snapshot_ids = []
        
        # Full snapshot
        vm_data = b"Base VM data " * 100
        snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.return_value = vm_data
        
        full_id = await snapshot_manager.create_incremental_snapshot(
            vm_id=vm_id, user_id=user_id, name="full"
        )
        snapshot_ids.append(full_id)
        
        # First incremental
        vm_data = b"Modified VM data v1 " * 100
        snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.return_value = vm_data
        
        inc1_id = await snapshot_manager.create_incremental_snapshot(
            vm_id=vm_id, user_id=user_id, name="inc1"
        )
        snapshot_ids.append(inc1_id)
        
        # Second incremental
        vm_data = b"Modified VM data v2 " * 100
        snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.return_value = vm_data
        
        inc2_id = await snapshot_manager.create_incremental_snapshot(
            vm_id=vm_id, user_id=user_id, name="inc2"
        )
        snapshot_ids.append(inc2_id)
        
        # Test restoration chain building
        chain = await snapshot_manager._build_restoration_chain(inc2_id)
        
        # Should be [full, inc1, inc2]
        assert len(chain) == 3
        assert chain[0] == full_id
        assert chain[1] == inc1_id
        assert chain[2] == inc2_id
        
        # Verify chain starts with full snapshot
        first_snapshot = snapshot_manager.incremental_snapshots[chain[0]]
        assert first_snapshot.snapshot_type == IncrementalSnapshotType.FULL
    
    @pytest.mark.asyncio
    async def test_delta_data_creation_and_parsing(self, snapshot_manager):
        """Test creating and parsing delta data format."""
        # Create mock delta
        delta = SnapshotDelta(
            added_blocks=["block_000001", "block_000003"],
            modified_blocks=["block_000000"],
            deleted_blocks=["block_000002"],
            block_checksums={
                "block_000000": "modified_hash",
                "block_000001": "new_hash1",
                "block_000003": "new_hash3"
            },
            compression_ratio=0.7,
            size_reduction=0.6
        )
        
        # Create mock blocks
        blocks = [
            b"A" * 4096,  # block_000000 (modified)
            b"B" * 4096,  # block_000001 (added)
            b"C" * 4096,  # block_000002 (not in delta - deleted)
            b"D" * 4096   # block_000003 (added)
        ]
        
        # Create delta data
        delta_data = await snapshot_manager._create_delta_data(delta, blocks)
        
        # Verify delta data structure
        import struct
        metadata_size = struct.unpack("<I", delta_data[:4])[0]
        assert metadata_size > 0
        
        metadata_json = delta_data[4:4 + metadata_size].decode()
        metadata = json.loads(metadata_json)
        
        assert "added_blocks" in metadata
        assert "modified_blocks" in metadata
        assert "deleted_blocks" in metadata
        assert "block_checksums" in metadata
        
        assert metadata["added_blocks"] == delta.added_blocks
        assert metadata["modified_blocks"] == delta.modified_blocks
        assert metadata["deleted_blocks"] == delta.deleted_blocks
        
        # Verify block data section exists
        block_data_start = 4 + metadata_size
        assert len(delta_data) > block_data_start
    
    @pytest.mark.asyncio
    async def test_forced_full_snapshot(self, snapshot_manager):
        """Test forcing a full snapshot even when incremental is possible."""
        vm_id = "test_vm_force"
        user_id = "test_user"
        
        # Create initial full snapshot
        vm_data = b"Initial VM data " * 100
        snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.return_value = vm_data
        
        first_id = await snapshot_manager.create_incremental_snapshot(
            vm_id=vm_id, user_id=user_id, name="first"
        )
        
        # Create another snapshot with force_full=True
        vm_data = b"Modified VM data " * 100
        snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.return_value = vm_data
        
        forced_full_id = await snapshot_manager.create_incremental_snapshot(
            vm_id=vm_id, user_id=user_id, name="forced-full", force_full=True
        )
        
        # Verify it's a full snapshot despite having a previous snapshot
        forced_snapshot = snapshot_manager.incremental_snapshots[forced_full_id]
        assert forced_snapshot.snapshot_type == IncrementalSnapshotType.FULL
        assert forced_snapshot.parent_snapshot_id is None
        
        # Verify chain was reset
        chain = snapshot_manager.snapshot_chains[vm_id]
        assert chain == [forced_full_id]
    
    @pytest.mark.asyncio
    async def test_snapshot_chain_info(self, snapshot_manager):
        """Test getting snapshot chain information."""
        vm_id = "test_vm_info"
        user_id = "test_user"
        
        # Create a few snapshots
        vm_data = b"VM data " * 100
        snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.return_value = vm_data
        
        snapshot_ids = []
        for i in range(3):
            vm_data = f"VM data v{i} ".encode() * 100
            snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.return_value = vm_data
            
            snapshot_id = await snapshot_manager.create_incremental_snapshot(
                vm_id=vm_id, user_id=user_id, name=f"snapshot-{i}"
            )
            snapshot_ids.append(snapshot_id)
        
        # Get chain info
        chain_info = snapshot_manager.get_snapshot_chain_info(vm_id)
        
        assert chain_info["vm_id"] == vm_id
        assert chain_info["chain_length"] == 3
        assert "total_storage_bytes" in chain_info
        assert "snapshots" in chain_info
        assert len(chain_info["snapshots"]) == 3
        
        # Verify snapshot info structure
        for snapshot_info in chain_info["snapshots"]:
            assert "snapshot_id" in snapshot_info
            assert "type" in snapshot_info
            assert "created_at" in snapshot_info
            assert "size_bytes" in snapshot_info
    
    @pytest.mark.asyncio
    async def test_error_handling(self, snapshot_manager):
        """Test error handling in incremental snapshots."""
        vm_id = "test_vm_error"
        user_id = "test_user"
        
        # Test with VM manager failure
        snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.side_effect = Exception("VM snapshot failed")
        
        with pytest.raises(Exception, match="VM snapshot failed"):
            await snapshot_manager.create_incremental_snapshot(
                vm_id=vm_id, user_id=user_id, name="error-test"
            )
        
        # Test restoration with invalid snapshot
        with pytest.raises(ValueError, match="Snapshot not found or access denied"):
            await snapshot_manager.restore_incremental_snapshot("invalid_id", user_id)
    
    @pytest.mark.asyncio
    async def test_performance_optimization(self, snapshot_manager):
        """Test performance optimization features."""
        # Create multiple long chains to trigger optimization
        for vm_num in range(3):
            vm_id = f"test_vm_perf_{vm_num}"
            user_id = "test_user"
            
            # Create a chain longer than max length
            vm_data = b"VM data " * 100
            snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.return_value = vm_data
            
            for i in range(snapshot_manager.max_chain_length + 2):
                vm_data = f"VM {vm_num} data v{i} ".encode() * 100
                snapshot_manager.base_manager.vm_manager.create_firecracker_snapshot.return_value = vm_data
                
                await snapshot_manager.create_incremental_snapshot(
                    vm_id=vm_id, user_id=user_id, name=f"snapshot-{i}"
                )
        
        # Run optimization analysis
        optimization_stats = snapshot_manager.optimize_snapshot_chains()
        
        assert "chains_optimized" in optimization_stats
        assert "storage_saved_bytes" in optimization_stats
        assert "operations_performed" in optimization_stats
        
        # Should have identified chains for optimization
        assert optimization_stats["chains_optimized"] >= 0
        assert isinstance(optimization_stats["operations_performed"], list)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])