"""
Advanced Incremental Snapshot Management

Provides intelligent incremental snapshot capabilities that optimize storage
through differential snapshots, deduplication, and smart scheduling.
"""

import asyncio
import hashlib
import json
import time
import zlib
from typing import Dict, Any, Optional, List, Set, Tuple
from dataclasses import dataclass, asdict
from enum import Enum
from pathlib import Path
import structlog
import logfire

logger = structlog.get_logger()


class IncrementalSnapshotType(Enum):
    """Types of incremental snapshots."""
    FULL = "full"           # Complete snapshot
    DIFFERENTIAL = "diff"   # Changes since last full snapshot
    INCREMENTAL = "incr"    # Changes since last snapshot (any type)


@dataclass
class SnapshotDelta:
    """Represents changes between snapshots."""
    added_blocks: List[str]
    modified_blocks: List[str]
    deleted_blocks: List[str]
    block_checksums: Dict[str, str]
    compression_ratio: float
    size_reduction: float


@dataclass
class IncrementalSnapshot:
    """Metadata for incremental snapshot."""
    snapshot_id: str
    parent_snapshot_id: Optional[str]
    snapshot_type: IncrementalSnapshotType
    vm_id: str
    user_id: str
    created_at: float
    delta: Optional[SnapshotDelta]
    storage_path: str
    original_size_bytes: int
    compressed_size_bytes: int
    checksum: str
    is_restorable: bool = True


class IncrementalSnapshotManager:
    """
    Advanced incremental snapshot management system.
    
    Provides intelligent snapshot optimization through:
    - Differential and incremental snapshots
    - Block-level deduplication
    - Automated snapshot chains
    - Storage optimization
    """
    
    def __init__(self, base_snapshot_manager, storage_backend):
        """Initialize incremental snapshot manager."""
        self.base_manager = base_snapshot_manager
        self.storage_backend = storage_backend
        
        # Incremental snapshot registry
        self.incremental_snapshots: Dict[str, IncrementalSnapshot] = {}
        self.snapshot_chains: Dict[str, List[str]] = {}  # vm_id -> [snapshot_ids]
        self.block_cache: Dict[str, bytes] = {}  # block_hash -> block_data
        
        # Configuration
        self.block_size = 4096  # 4KB blocks
        self.max_chain_length = 10
        self.max_cache_size_mb = 100
        self.compression_level = 6
        
    async def create_incremental_snapshot(self, vm_id: str, user_id: str, 
                                         name: str, description: str = "",
                                         force_full: bool = False) -> str:
        """
        Create an incremental snapshot optimized for storage efficiency.
        
        Args:
            vm_id: VM to snapshot
            user_id: User creating snapshot
            name: Snapshot name
            description: Optional description
            force_full: Force full snapshot instead of incremental
            
        Returns:
            str: Snapshot ID
        """
        try:
            # Determine snapshot type
            snapshot_type, parent_id = await self._determine_snapshot_type(
                vm_id, user_id, force_full
            )
            
            logger.info("Creating incremental snapshot",
                       vm_id=vm_id,
                       user_id=user_id,
                       snapshot_type=snapshot_type.value,
                       parent_id=parent_id)
            
            # Get current VM state
            vm_data = await self.base_manager.vm_manager.create_firecracker_snapshot(vm_id)
            
            # Process snapshot based on type
            if snapshot_type == IncrementalSnapshotType.FULL:
                return await self._create_full_snapshot(
                    vm_id, user_id, name, description, vm_data
                )
            else:
                return await self._create_delta_snapshot(
                    vm_id, user_id, name, description, vm_data, 
                    parent_id, snapshot_type
                )
                
        except Exception as e:
            logger.error("Incremental snapshot creation failed",
                        vm_id=vm_id, user_id=user_id, error=str(e))
            logfire.error("Incremental snapshot failed",
                         vm_id=vm_id, error=str(e))
            raise
    
    async def _determine_snapshot_type(self, vm_id: str, user_id: str, 
                                     force_full: bool) -> Tuple[IncrementalSnapshotType, Optional[str]]:
        """Determine the most appropriate snapshot type."""
        if force_full:
            return IncrementalSnapshotType.FULL, None
        
        # Get existing snapshot chain for VM
        chain = self.snapshot_chains.get(vm_id, [])
        
        if not chain:
            # No previous snapshots - create full
            return IncrementalSnapshotType.FULL, None
        
        if len(chain) >= self.max_chain_length:
            # Chain too long - create new full snapshot
            return IncrementalSnapshotType.FULL, None
        
        # Find the most recent snapshot
        latest_snapshot_id = chain[-1]
        latest_snapshot = self.incremental_snapshots.get(latest_snapshot_id)
        
        if not latest_snapshot or not latest_snapshot.is_restorable:
            # Latest snapshot corrupted - create full
            return IncrementalSnapshotType.FULL, None
        
        # Determine if we should create differential or incremental
        full_snapshots = [s for s in chain 
                         if self.incremental_snapshots.get(s, {}).snapshot_type == IncrementalSnapshotType.FULL]
        
        if not full_snapshots:
            # No full snapshot in chain - create full
            return IncrementalSnapshotType.FULL, None
        
        latest_full_id = full_snapshots[-1]
        snapshots_since_full = len([s for s in chain if chain.index(s) > chain.index(latest_full_id)])
        
        if snapshots_since_full >= 5:
            # Too many snapshots since last full - create differential
            return IncrementalSnapshotType.DIFFERENTIAL, latest_full_id
        else:
            # Create incremental from latest
            return IncrementalSnapshotType.INCREMENTAL, latest_snapshot_id
    
    async def _create_full_snapshot(self, vm_id: str, user_id: str, 
                                   name: str, description: str, vm_data: bytes) -> str:
        """Create a full snapshot with block analysis."""
        snapshot_id = self.base_manager._generate_snapshot_id(vm_id, user_id)
        
        # Analyze VM data into blocks
        blocks = self._split_into_blocks(vm_data)
        block_checksums = {}
        
        for i, block in enumerate(blocks):
            block_hash = hashlib.sha256(block).hexdigest()
            block_checksums[f"block_{i:06d}"] = block_hash
            
            # Cache frequently accessed blocks
            if len(self.block_cache) < (self.max_cache_size_mb * 1024 * 1024) // self.block_size:
                self.block_cache[block_hash] = block
        
        # Compress VM data
        compressed_data = zlib.compress(vm_data, self.compression_level)
        
        # Store in backend
        storage_path = await self.storage_backend.store_snapshot(
            snapshot_id, compressed_data, {
                "type": "incremental_full",
                "block_count": len(blocks),
                "compression_level": self.compression_level
            }
        )
        
        # Create incremental snapshot metadata
        incremental_snapshot = IncrementalSnapshot(
            snapshot_id=snapshot_id,
            parent_snapshot_id=None,
            snapshot_type=IncrementalSnapshotType.FULL,
            vm_id=vm_id,
            user_id=user_id,
            created_at=time.time(),
            delta=SnapshotDelta(
                added_blocks=list(block_checksums.keys()),
                modified_blocks=[],
                deleted_blocks=[],
                block_checksums=block_checksums,
                compression_ratio=len(compressed_data) / len(vm_data),
                size_reduction=1.0 - (len(compressed_data) / len(vm_data))
            ),
            storage_path=storage_path,
            original_size_bytes=len(vm_data),
            compressed_size_bytes=len(compressed_data),
            checksum=hashlib.sha256(vm_data).hexdigest(),
            is_restorable=True
        )
        
        # Register snapshot
        self.incremental_snapshots[snapshot_id] = incremental_snapshot
        
        # Initialize or reset snapshot chain
        self.snapshot_chains[vm_id] = [snapshot_id]
        
        logger.info("Full incremental snapshot created",
                   snapshot_id=snapshot_id,
                   vm_id=vm_id,
                   blocks=len(blocks),
                   compression_ratio=incremental_snapshot.delta.compression_ratio)
        
        logfire.info("Full snapshot with block analysis completed",
                    snapshot_id=snapshot_id,
                    vm_id=vm_id,
                    original_size_mb=len(vm_data) // (1024 * 1024),
                    compressed_size_mb=len(compressed_data) // (1024 * 1024),
                    block_count=len(blocks))
        
        return snapshot_id
    
    async def _create_delta_snapshot(self, vm_id: str, user_id: str,
                                   name: str, description: str, vm_data: bytes,
                                   parent_id: str, snapshot_type: IncrementalSnapshotType) -> str:
        """Create a differential or incremental snapshot."""
        snapshot_id = self.base_manager._generate_snapshot_id(vm_id, user_id)
        
        # Get parent snapshot for comparison
        parent_snapshot = self.incremental_snapshots[parent_id]
        
        # Analyze current VM data into blocks
        current_blocks = self._split_into_blocks(vm_data)
        current_checksums = {}
        
        for i, block in enumerate(current_blocks):
            block_hash = hashlib.sha256(block).hexdigest()
            current_checksums[f"block_{i:06d}"] = block_hash
        
        # Compare with parent to find delta
        delta = await self._calculate_delta(parent_snapshot, current_checksums, current_blocks)
        
        # Create delta data containing only changed blocks
        delta_data = await self._create_delta_data(delta, current_blocks)
        
        # Compress delta data
        compressed_delta = zlib.compress(delta_data, self.compression_level)
        
        # Store delta in backend
        storage_path = await self.storage_backend.store_snapshot(
            snapshot_id, compressed_delta, {
                "type": f"incremental_{snapshot_type.value}",
                "parent_id": parent_id,
                "added_blocks": len(delta.added_blocks),
                "modified_blocks": len(delta.modified_blocks),
                "deleted_blocks": len(delta.deleted_blocks)
            }
        )
        
        # Calculate space savings
        size_reduction = 1.0 - (len(compressed_delta) / len(vm_data))
        
        # Create incremental snapshot metadata
        incremental_snapshot = IncrementalSnapshot(
            snapshot_id=snapshot_id,
            parent_snapshot_id=parent_id,
            snapshot_type=snapshot_type,
            vm_id=vm_id,
            user_id=user_id,
            created_at=time.time(),
            delta=delta,
            storage_path=storage_path,
            original_size_bytes=len(vm_data),
            compressed_size_bytes=len(compressed_delta),
            checksum=hashlib.sha256(vm_data).hexdigest(),
            is_restorable=True
        )
        
        # Register snapshot
        self.incremental_snapshots[snapshot_id] = incremental_snapshot
        
        # Add to snapshot chain
        if vm_id not in self.snapshot_chains:
            self.snapshot_chains[vm_id] = []
        self.snapshot_chains[vm_id].append(snapshot_id)
        
        logger.info("Delta snapshot created",
                   snapshot_id=snapshot_id,
                   vm_id=vm_id,
                   snapshot_type=snapshot_type.value,
                   parent_id=parent_id,
                   size_reduction=size_reduction,
                   added_blocks=len(delta.added_blocks),
                   modified_blocks=len(delta.modified_blocks))
        
        logfire.info("Incremental snapshot with delta optimization completed",
                    snapshot_id=snapshot_id,
                    vm_id=vm_id,
                    snapshot_type=snapshot_type.value,
                    original_size_mb=len(vm_data) // (1024 * 1024),
                    delta_size_mb=len(compressed_delta) // (1024 * 1024),
                    space_savings_percent=size_reduction * 100)
        
        return snapshot_id
    
    async def _calculate_delta(self, parent_snapshot: IncrementalSnapshot,
                             current_checksums: Dict[str, str],
                             current_blocks: List[bytes]) -> SnapshotDelta:
        """Calculate the delta between parent and current snapshots."""
        parent_checksums = parent_snapshot.delta.block_checksums
        
        added_blocks = []
        modified_blocks = []
        deleted_blocks = []
        
        # Find added and modified blocks
        for block_id, current_hash in current_checksums.items():
            if block_id not in parent_checksums:
                added_blocks.append(block_id)
            elif parent_checksums[block_id] != current_hash:
                modified_blocks.append(block_id)
        
        # Find deleted blocks
        for block_id in parent_checksums:
            if block_id not in current_checksums:
                deleted_blocks.append(block_id)
        
        # Calculate compression ratio for delta
        changed_blocks = added_blocks + modified_blocks
        if changed_blocks:
            total_changed_size = len(changed_blocks) * self.block_size
            compressed_size = len(zlib.compress(
                b''.join(current_blocks[int(bid.split('_')[1])] for bid in changed_blocks),
                self.compression_level
            ))
            compression_ratio = compressed_size / total_changed_size if total_changed_size > 0 else 1.0
        else:
            compression_ratio = 1.0
        
        # Calculate size reduction
        total_blocks = len(current_checksums)
        changed_count = len(changed_blocks)
        size_reduction = 1.0 - (changed_count / total_blocks) if total_blocks > 0 else 0.0
        
        return SnapshotDelta(
            added_blocks=added_blocks,
            modified_blocks=modified_blocks,
            deleted_blocks=deleted_blocks,
            block_checksums=current_checksums,
            compression_ratio=compression_ratio,
            size_reduction=size_reduction
        )
    
    async def _create_delta_data(self, delta: SnapshotDelta, blocks: List[bytes]) -> bytes:
        """Create delta data containing only changed blocks."""
        delta_info = {
            "added_blocks": delta.added_blocks,
            "modified_blocks": delta.modified_blocks,
            "deleted_blocks": delta.deleted_blocks,
            "block_checksums": delta.block_checksums
        }
        
        # Serialize delta metadata
        delta_metadata = json.dumps(delta_info).encode()
        metadata_size = len(delta_metadata)
        
        # Collect changed block data
        changed_blocks_data = b""
        for block_id in delta.added_blocks + delta.modified_blocks:
            block_index = int(block_id.split('_')[1])
            if block_index < len(blocks):
                changed_blocks_data += blocks[block_index]
        
        # Create delta format: [metadata_size(4 bytes)][metadata][block_data]
        import struct
        delta_data = struct.pack("<I", metadata_size)
        delta_data += delta_metadata
        delta_data += changed_blocks_data
        
        return delta_data
    
    def _split_into_blocks(self, data: bytes) -> List[bytes]:
        """Split data into fixed-size blocks."""
        blocks = []
        for i in range(0, len(data), self.block_size):
            block = data[i:i + self.block_size]
            # Pad last block if necessary
            if len(block) < self.block_size:
                block += b'\x00' * (self.block_size - len(block))
            blocks.append(block)
        return blocks
    
    async def restore_incremental_snapshot(self, snapshot_id: str, user_id: str) -> bytes:
        """
        Restore VM data from incremental snapshot by reconstructing the full state.
        
        Args:
            snapshot_id: Snapshot to restore
            user_id: User requesting restore
            
        Returns:
            bytes: Reconstructed VM data
        """
        try:
            # Validate ownership
            snapshot = self.incremental_snapshots.get(snapshot_id)
            if not snapshot or snapshot.user_id != user_id:
                raise ValueError("Snapshot not found or access denied")
            
            # Build restoration chain
            restoration_chain = await self._build_restoration_chain(snapshot_id)
            
            logger.info("Restoring incremental snapshot",
                       snapshot_id=snapshot_id,
                       chain_length=len(restoration_chain))
            
            # Reconstruct VM data from chain
            vm_data = await self._reconstruct_from_chain(restoration_chain)
            
            logger.info("Incremental snapshot restored",
                       snapshot_id=snapshot_id,
                       restored_size_bytes=len(vm_data))
            
            logfire.info("Incremental snapshot restoration completed",
                        snapshot_id=snapshot_id,
                        user_id=user_id,
                        chain_length=len(restoration_chain),
                        restored_size_mb=len(vm_data) // (1024 * 1024))
            
            return vm_data
            
        except Exception as e:
            logger.error("Incremental snapshot restoration failed",
                        snapshot_id=snapshot_id, error=str(e))
            logfire.error("Incremental restoration error",
                         snapshot_id=snapshot_id, error=str(e))
            raise
    
    async def _build_restoration_chain(self, snapshot_id: str) -> List[str]:
        """Build the chain of snapshots needed for restoration."""
        chain = []
        current_id = snapshot_id
        
        while current_id:
            snapshot = self.incremental_snapshots.get(current_id)
            if not snapshot:
                raise ValueError(f"Snapshot {current_id} not found in chain")
            
            chain.insert(0, current_id)
            
            if snapshot.snapshot_type == IncrementalSnapshotType.FULL:
                # Reached the base full snapshot
                break
            
            current_id = snapshot.parent_snapshot_id
        
        if not chain or self.incremental_snapshots[chain[0]].snapshot_type != IncrementalSnapshotType.FULL:
            raise ValueError("Cannot find base full snapshot for restoration")
        
        return chain
    
    async def _reconstruct_from_chain(self, chain: List[str]) -> bytes:
        """Reconstruct VM data by applying snapshots in order."""
        # Start with the full snapshot
        base_snapshot = self.incremental_snapshots[chain[0]]
        base_data = await self.storage_backend.retrieve_snapshot(chain[0])
        
        # Decompress base data
        vm_data = zlib.decompress(base_data)
        current_blocks = self._split_into_blocks(vm_data)
        
        # Apply each incremental/differential snapshot
        for snapshot_id in chain[1:]:
            snapshot = self.incremental_snapshots[snapshot_id]
            delta_data = await self.storage_backend.retrieve_snapshot(snapshot_id)
            
            # Decompress and parse delta
            decompressed_delta = zlib.decompress(delta_data)
            current_blocks = await self._apply_delta(current_blocks, decompressed_delta)
        
        # Reconstruct final VM data
        return b''.join(current_blocks)
    
    async def _apply_delta(self, current_blocks: List[bytes], delta_data: bytes) -> List[bytes]:
        """Apply delta changes to current blocks."""
        import struct
        
        # Parse delta format
        metadata_size = struct.unpack("<I", delta_data[:4])[0]
        metadata_json = delta_data[4:4 + metadata_size].decode()
        delta_info = json.loads(metadata_json)
        
        block_data_start = 4 + metadata_size
        changed_data = delta_data[block_data_start:]
        
        # Apply changes
        changed_blocks = delta_info["added_blocks"] + delta_info["modified_blocks"]
        
        for i, block_id in enumerate(changed_blocks):
            block_index = int(block_id.split('_')[1])
            start_pos = i * self.block_size
            end_pos = start_pos + self.block_size
            
            if end_pos <= len(changed_data):
                new_block_data = changed_data[start_pos:end_pos]
                
                # Extend blocks list if necessary
                while len(current_blocks) <= block_index:
                    current_blocks.append(b'\x00' * self.block_size)
                
                current_blocks[block_index] = new_block_data
        
        return current_blocks
    
    def get_snapshot_chain_info(self, vm_id: str) -> Dict[str, Any]:
        """Get information about snapshot chains for a VM."""
        chain = self.snapshot_chains.get(vm_id, [])
        
        if not chain:
            return {"vm_id": vm_id, "chain_length": 0, "snapshots": []}
        
        snapshots_info = []
        total_storage = 0
        
        for snapshot_id in chain:
            snapshot = self.incremental_snapshots.get(snapshot_id)
            if snapshot:
                snapshots_info.append({
                    "snapshot_id": snapshot_id,
                    "type": snapshot.snapshot_type.value,
                    "created_at": snapshot.created_at,
                    "size_bytes": snapshot.compressed_size_bytes,
                    "parent_id": snapshot.parent_snapshot_id
                })
                total_storage += snapshot.compressed_size_bytes
        
        return {
            "vm_id": vm_id,
            "chain_length": len(chain),
            "total_storage_bytes": total_storage,
            "snapshots": snapshots_info
        }
    
    def optimize_snapshot_chains(self) -> Dict[str, Any]:
        """Optimize snapshot chains by consolidating when beneficial."""
        optimization_stats = {
            "chains_optimized": 0,
            "storage_saved_bytes": 0,
            "operations_performed": []
        }
        
        for vm_id, chain in self.snapshot_chains.items():
            if len(chain) > self.max_chain_length:
                # Chain needs optimization
                logger.info("Optimizing snapshot chain", vm_id=vm_id, chain_length=len(chain))
                
                # Mark for optimization (actual optimization would be done asynchronously)
                optimization_stats["chains_optimized"] += 1
                optimization_stats["operations_performed"].append({
                    "vm_id": vm_id,
                    "operation": "chain_consolidation",
                    "original_length": len(chain),
                    "target_length": self.max_chain_length // 2
                })
        
        return optimization_stats