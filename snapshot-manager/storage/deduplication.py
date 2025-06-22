"""
Deduplication engine for VM snapshot storage optimization.

Provides chunk-based deduplication with SHA-256 hashing,
reference counting, and storage space optimization.
"""

import asyncio
import hashlib
import json
import time
from typing import Dict, List, Optional, Set, Tuple, Any
from dataclasses import dataclass, asdict
import structlog
import logfire

logger = structlog.get_logger()


@dataclass
class ChunkInfo:
    """Information about a deduplicated chunk."""
    chunk_hash: str
    size_bytes: int
    ref_count: int
    storage_key: str
    first_seen_at: float
    last_accessed_at: float


@dataclass
class DeduplicationStats:
    """Statistics for deduplication operations."""
    total_chunks_processed: int
    unique_chunks_stored: int
    duplicate_chunks_found: int
    total_bytes_saved: int
    deduplication_ratio: float


class DeduplicationEngine:
    """
    Chunk-based deduplication engine for VM snapshots.
    
    Provides storage optimization through SHA-256 based chunk deduplication
    with reference counting and efficient storage management.
    """

    def __init__(self, storage_backend, chunk_size: int = 1024 * 1024):  # 1MB chunks
        """Initialize deduplication engine."""
        self.storage = storage_backend
        self.chunk_size = chunk_size
        self.chunk_index: Dict[str, ChunkInfo] = {}
        self.stats = DeduplicationStats(
            total_chunks_processed=0,
            unique_chunks_stored=0,
            duplicate_chunks_found=0,
            total_bytes_saved=0,
            deduplication_ratio=0.0
        )

    async def initialize(self):
        """Initialize deduplication engine and load existing chunk index."""
        try:
            # Load existing chunk index from persistent storage
            await self._load_chunk_index()
            
            logger.info("Deduplication engine initialized", 
                       chunks_indexed=len(self.chunk_index),
                       chunk_size_kb=self.chunk_size // 1024)
            
            logfire.info("Deduplication engine started",
                        chunks_count=len(self.chunk_index),
                        chunk_size_bytes=self.chunk_size)
                        
        except Exception as e:
            logger.error("Failed to initialize deduplication engine", error=str(e))
            logfire.error("Deduplication engine initialization failed", error=str(e))
            raise

    async def store_deduplicated_snapshot(self, snapshot_id: str, 
                                        data: bytes, user_id: str) -> Tuple[str, int, int]:
        """
        Store snapshot with deduplication optimization.
        
        Args:
            snapshot_id: Unique identifier for the snapshot
            data: Binary snapshot data
            user_id: User ID for scoped deduplication
            
        Returns:
            Tuple[str, int, int]: (manifest_path, original_size, deduplicated_size)
        """
        start_time = time.time()
        
        try:
            # Split data into chunks
            chunks = self._split_into_chunks(data)
            
            # Process chunks for deduplication
            chunk_refs = []
            new_chunks_stored = 0
            duplicate_chunks_found = 0
            total_size = len(data)
            
            for chunk_data in chunks:
                chunk_hash = hashlib.sha256(chunk_data).hexdigest()
                self.stats.total_chunks_processed += 1
                
                if chunk_hash in self.chunk_index:
                    # Chunk already exists, increment reference count
                    chunk_info = self.chunk_index[chunk_hash]
                    chunk_info.ref_count += 1
                    chunk_info.last_accessed_at = time.time()
                    chunk_refs.append(chunk_hash)
                    duplicate_chunks_found += 1
                    self.stats.duplicate_chunks_found += 1
                    self.stats.total_bytes_saved += len(chunk_data)
                    
                    logger.debug("Duplicate chunk found", 
                               chunk_hash=chunk_hash[:16],
                               ref_count=chunk_info.ref_count)
                else:
                    # Store new chunk
                    chunk_key = f"chunks/{user_id}/{chunk_hash}"
                    await self.storage.store_snapshot(
                        f"chunk_{chunk_hash}", chunk_data,
                        metadata={
                            'chunk_hash': chunk_hash,
                            'user_id': user_id,
                            'chunk_size': len(chunk_data)
                        }
                    )
                    
                    # Add to index
                    current_time = time.time()
                    self.chunk_index[chunk_hash] = ChunkInfo(
                        chunk_hash=chunk_hash,
                        size_bytes=len(chunk_data),
                        ref_count=1,
                        storage_key=chunk_key,
                        first_seen_at=current_time,
                        last_accessed_at=current_time
                    )
                    
                    chunk_refs.append(chunk_hash)
                    new_chunks_stored += 1
                    self.stats.unique_chunks_stored += 1
                    
                    logger.debug("New chunk stored", 
                               chunk_hash=chunk_hash[:16],
                               size_bytes=len(chunk_data))
            
            # Create and store manifest
            manifest = {
                'snapshot_id': snapshot_id,
                'user_id': user_id,
                'chunk_refs': chunk_refs,
                'total_size': total_size,
                'chunk_count': len(chunks),
                'chunk_size': self.chunk_size,
                'created_at': time.time(),
                'deduplication_stats': {
                    'new_chunks': new_chunks_stored,
                    'duplicate_chunks': duplicate_chunks_found,
                    'total_chunks': len(chunks)
                }
            }
            
            manifest_data = json.dumps(manifest, indent=2).encode('utf-8')
            manifest_path = await self.storage.store_snapshot(
                f"{snapshot_id}_manifest", manifest_data,
                metadata={
                    'snapshot_id': snapshot_id,
                    'user_id': user_id,
                    'manifest_version': '1.0'
                }
            )
            
            # Calculate deduplication effectiveness
            unique_chunk_size = sum(
                self.chunk_index[ref].size_bytes 
                for ref in set(chunk_refs)  # Only count unique chunks
            )
            
            deduplication_ratio = unique_chunk_size / total_size if total_size > 0 else 1.0
            self.stats.deduplication_ratio = (
                self.stats.total_bytes_saved / 
                (self.stats.total_bytes_saved + sum(c.size_bytes for c in self.chunk_index.values()))
                if self.chunk_index else 0.0
            )
            
            # Persist updated chunk index
            await self._persist_chunk_index()
            
            duration_ms = (time.time() - start_time) * 1000
            
            logger.info("Deduplicated snapshot stored", 
                       snapshot_id=snapshot_id,
                       original_size=total_size,
                       unique_size=unique_chunk_size,
                       deduplication_ratio=deduplication_ratio,
                       new_chunks=new_chunks_stored,
                       duplicate_chunks=duplicate_chunks_found,
                       duration_ms=duration_ms)
            
            # Log comprehensive deduplication metrics to Logfire
            logfire.info("Snapshot deduplication completed",
                        snapshot_id=snapshot_id,
                        user_id=user_id,
                        original_size_bytes=total_size,
                        deduplicated_size_bytes=unique_chunk_size,
                        deduplication_ratio=deduplication_ratio,
                        space_saved_bytes=total_size - unique_chunk_size,
                        space_saved_percent=(1 - deduplication_ratio) * 100,
                        new_chunks_stored=new_chunks_stored,
                        duplicate_chunks_found=duplicate_chunks_found,
                        total_chunks=len(chunks),
                        duration_ms=duration_ms,
                        throughput_mbps=(total_size / (1024 * 1024)) / (duration_ms / 1000) if duration_ms > 0 else 0)
            
            return manifest_path, total_size, unique_chunk_size
            
        except Exception as e:
            logger.error("Deduplication storage failed", 
                        snapshot_id=snapshot_id, error=str(e))
            logfire.error("Snapshot deduplication failed",
                         snapshot_id=snapshot_id, error=str(e))
            raise

    async def retrieve_deduplicated_snapshot(self, snapshot_id: str) -> bytes:
        """
        Retrieve and reconstruct snapshot from deduplicated chunks.
        
        Args:
            snapshot_id: Unique identifier for the snapshot
            
        Returns:
            bytes: Reconstructed snapshot data
        """
        start_time = time.time()
        
        try:
            # Retrieve manifest
            manifest_data = await self.storage.retrieve_snapshot(f"{snapshot_id}_manifest")
            manifest = json.loads(manifest_data.decode('utf-8'))
            
            # Validate manifest
            if manifest['snapshot_id'] != snapshot_id:
                raise ValueError("Manifest snapshot ID mismatch")
            
            chunk_refs = manifest['chunk_refs']
            expected_size = manifest['total_size']
            
            # Retrieve and reconstruct chunks
            reconstructed_chunks = []
            
            for chunk_hash in chunk_refs:
                if chunk_hash not in self.chunk_index:
                    raise ValueError(f"Chunk {chunk_hash} not found in index")
                
                # Retrieve chunk data
                chunk_data = await self.storage.retrieve_snapshot(f"chunk_{chunk_hash}")
                
                # Verify chunk integrity
                calculated_hash = hashlib.sha256(chunk_data).hexdigest()
                if calculated_hash != chunk_hash:
                    raise ValueError(f"Chunk integrity check failed for {chunk_hash}")
                
                reconstructed_chunks.append(chunk_data)
                
                # Update access time
                self.chunk_index[chunk_hash].last_accessed_at = time.time()
            
            # Reconstruct full snapshot
            reconstructed_data = b''.join(reconstructed_chunks)
            
            # Verify total size
            if len(reconstructed_data) != expected_size:
                raise ValueError(f"Reconstructed size mismatch: expected {expected_size}, got {len(reconstructed_data)}")
            
            duration_ms = (time.time() - start_time) * 1000
            
            logger.info("Deduplicated snapshot retrieved", 
                       snapshot_id=snapshot_id,
                       size_bytes=len(reconstructed_data),
                       chunks_retrieved=len(chunk_refs),
                       duration_ms=duration_ms)
            
            logfire.info("Snapshot deduplication retrieval completed",
                        snapshot_id=snapshot_id,
                        size_bytes=len(reconstructed_data),
                        chunks_count=len(chunk_refs),
                        duration_ms=duration_ms,
                        throughput_mbps=(len(reconstructed_data) / (1024 * 1024)) / (duration_ms / 1000) if duration_ms > 0 else 0)
            
            return reconstructed_data
            
        except Exception as e:
            logger.error("Deduplicated snapshot retrieval failed", 
                        snapshot_id=snapshot_id, error=str(e))
            logfire.error("Snapshot deduplication retrieval failed",
                         snapshot_id=snapshot_id, error=str(e))
            raise

    async def delete_deduplicated_snapshot(self, snapshot_id: str) -> bool:
        """
        Delete deduplicated snapshot and clean up unused chunks.
        
        Args:
            snapshot_id: Unique identifier for the snapshot
            
        Returns:
            bool: True if deletion successful
        """
        try:
            # Retrieve manifest to get chunk references
            try:
                manifest_data = await self.storage.retrieve_snapshot(f"{snapshot_id}_manifest")
                manifest = json.loads(manifest_data.decode('utf-8'))
                chunk_refs = manifest['chunk_refs']
            except FileNotFoundError:
                logger.warning("Manifest not found for snapshot deletion", 
                              snapshot_id=snapshot_id)
                return True  # Consider it deleted
            
            # Decrement reference counts and clean up unreferenced chunks
            chunks_deleted = 0
            for chunk_hash in chunk_refs:
                if chunk_hash in self.chunk_index:
                    chunk_info = self.chunk_index[chunk_hash]
                    chunk_info.ref_count -= 1
                    
                    if chunk_info.ref_count <= 0:
                        # Delete chunk from storage
                        await self.storage.delete_snapshot(f"chunk_{chunk_hash}")
                        del self.chunk_index[chunk_hash]
                        chunks_deleted += 1
                        
                        logger.debug("Deleted unreferenced chunk", 
                                   chunk_hash=chunk_hash[:16])
            
            # Delete manifest
            await self.storage.delete_snapshot(f"{snapshot_id}_manifest")
            
            # Persist updated chunk index
            await self._persist_chunk_index()
            
            logger.info("Deduplicated snapshot deleted", 
                       snapshot_id=snapshot_id,
                       chunks_deleted=chunks_deleted)
            
            logfire.info("Snapshot deduplication deletion completed",
                        snapshot_id=snapshot_id,
                        chunks_deleted=chunks_deleted)
            
            return True
            
        except Exception as e:
            logger.error("Failed to delete deduplicated snapshot", 
                        snapshot_id=snapshot_id, error=str(e))
            logfire.error("Snapshot deduplication deletion failed",
                         snapshot_id=snapshot_id, error=str(e))
            return False

    def _split_into_chunks(self, data: bytes) -> List[bytes]:
        """Split data into fixed-size chunks for deduplication."""
        chunks = []
        offset = 0
        
        while offset < len(data):
            end_offset = min(offset + self.chunk_size, len(data))
            chunk = data[offset:end_offset]
            chunks.append(chunk)
            offset = end_offset
        
        return chunks

    async def _load_chunk_index(self):
        """Load chunk index from persistent storage."""
        try:
            # Try to load existing chunk index
            index_data = await self.storage.retrieve_snapshot("chunk_index")
            index_json = json.loads(index_data.decode('utf-8'))
            
            # Reconstruct chunk index
            for chunk_hash, chunk_data in index_json.items():
                self.chunk_index[chunk_hash] = ChunkInfo(**chunk_data)
                
            logger.info("Loaded chunk index", chunks_count=len(self.chunk_index))
            
        except FileNotFoundError:
            logger.info("No existing chunk index found, starting fresh")
        except Exception as e:
            logger.warning("Failed to load chunk index", error=str(e))

    async def _persist_chunk_index(self):
        """Persist chunk index to storage."""
        try:
            # Convert chunk index to JSON
            index_data = {}
            for chunk_hash, chunk_info in self.chunk_index.items():
                index_data[chunk_hash] = asdict(chunk_info)
            
            index_json = json.dumps(index_data, indent=2).encode('utf-8')
            
            # Store chunk index
            await self.storage.store_snapshot(
                "chunk_index", index_json,
                metadata={
                    'index_version': '1.0',
                    'chunks_count': len(self.chunk_index),
                    'updated_at': time.time()
                }
            )
            
            logger.debug("Persisted chunk index", chunks_count=len(self.chunk_index))
            
        except Exception as e:
            logger.error("Failed to persist chunk index", error=str(e))

    def get_deduplication_stats(self) -> Dict[str, Any]:
        """Get comprehensive deduplication statistics."""
        if not self.chunk_index:
            return asdict(self.stats)
        
        # Calculate additional statistics
        total_unique_size = sum(chunk.size_bytes for chunk in self.chunk_index.values())
        total_logical_size = sum(
            chunk.size_bytes * chunk.ref_count 
            for chunk in self.chunk_index.values()
        )
        
        current_stats = asdict(self.stats)
        current_stats.update({
            'total_chunks_indexed': len(self.chunk_index),
            'total_unique_bytes': total_unique_size,
            'total_logical_bytes': total_logical_size,
            'current_deduplication_ratio': total_unique_size / total_logical_size if total_logical_size > 0 else 1.0,
            'space_efficiency_percent': ((total_logical_size - total_unique_size) / total_logical_size * 100) if total_logical_size > 0 else 0.0,
            'average_chunk_size': total_unique_size // len(self.chunk_index) if self.chunk_index else 0,
            'average_references_per_chunk': sum(chunk.ref_count for chunk in self.chunk_index.values()) / len(self.chunk_index) if self.chunk_index else 0
        })
        
        return current_stats

    async def cleanup_orphaned_chunks(self, max_age_days: int = 30) -> int:
        """
        Clean up orphaned chunks that haven't been accessed recently.
        
        Args:
            max_age_days: Maximum age in days for unused chunks
            
        Returns:
            int: Number of chunks cleaned up
        """
        cutoff_time = time.time() - (max_age_days * 24 * 3600)
        chunks_cleaned = 0
        
        chunks_to_remove = []
        for chunk_hash, chunk_info in self.chunk_index.items():
            if (chunk_info.ref_count <= 0 and 
                chunk_info.last_accessed_at < cutoff_time):
                chunks_to_remove.append(chunk_hash)
        
        for chunk_hash in chunks_to_remove:
            try:
                await self.storage.delete_snapshot(f"chunk_{chunk_hash}")
                del self.chunk_index[chunk_hash]
                chunks_cleaned += 1
            except Exception as e:
                logger.warning("Failed to cleanup chunk", 
                              chunk_hash=chunk_hash[:16], error=str(e))
        
        if chunks_cleaned > 0:
            await self._persist_chunk_index()
            logger.info("Cleaned up orphaned chunks", 
                       chunks_cleaned=chunks_cleaned)
        
        return chunks_cleaned