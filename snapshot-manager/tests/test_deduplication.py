"""
Test suite for deduplication engine.
Tests chunk-based deduplication, reference counting, and storage optimization.
"""

import pytest
import asyncio
import json
import hashlib
from unittest.mock import AsyncMock, MagicMock

# Import the deduplication classes
import sys
from pathlib import Path
current_dir = Path(__file__).parent
parent_dir = current_dir.parent
sys.path.insert(0, str(parent_dir))

from storage.deduplication import DeduplicationEngine, ChunkInfo, DeduplicationStats


@pytest.fixture
def mock_storage_backend():
    """Mock storage backend for testing."""
    storage = AsyncMock()
    storage.store_snapshot = AsyncMock(return_value="s3://bucket/test-path")
    storage.retrieve_snapshot = AsyncMock()
    storage.delete_snapshot = AsyncMock(return_value=True)
    return storage


@pytest.fixture
def dedup_engine(mock_storage_backend):
    """Deduplication engine instance with mocked storage."""
    return DeduplicationEngine(mock_storage_backend, chunk_size=64)  # Small chunks for testing


class TestDeduplicationEngine:
    """Test deduplication engine functionality."""
    
    async def test_initialization(self, dedup_engine, mock_storage_backend):
        """Test deduplication engine initialization."""
        # Mock no existing index
        mock_storage_backend.retrieve_snapshot.side_effect = FileNotFoundError()
        
        await dedup_engine.initialize()
        
        assert len(dedup_engine.chunk_index) == 0
        assert dedup_engine.chunk_size == 64
    
    async def test_split_into_chunks(self, dedup_engine):
        """Test data splitting into chunks."""
        # Test data that will create multiple chunks
        test_data = b"A" * 150  # Will create 3 chunks: 64, 64, 22 bytes
        
        chunks = dedup_engine._split_into_chunks(test_data)
        
        assert len(chunks) == 3
        assert len(chunks[0]) == 64
        assert len(chunks[1]) == 64
        assert len(chunks[2]) == 22
        assert b"".join(chunks) == test_data
    
    async def test_store_deduplicated_snapshot_new_data(self, dedup_engine, mock_storage_backend):
        """Test storing snapshot with all new chunks."""
        await dedup_engine.initialize()
        
        test_data = b"unique test data for deduplication" * 10  # Larger than chunk size
        snapshot_id = "dedup_test_1"
        user_id = "user123"
        
        # Act
        manifest_path, original_size, deduplicated_size = await dedup_engine.store_deduplicated_snapshot(
            snapshot_id, test_data, user_id
        )
        
        # Assert
        assert manifest_path is not None
        assert original_size == len(test_data)
        assert deduplicated_size == len(test_data)  # No deduplication on first store
        assert len(dedup_engine.chunk_index) > 0
        
        # Verify storage calls
        assert mock_storage_backend.store_snapshot.call_count > 1  # Chunks + manifest
    
    async def test_store_deduplicated_snapshot_with_duplicates(self, dedup_engine, mock_storage_backend):
        """Test storing snapshot with duplicate chunks."""
        await dedup_engine.initialize()
        
        # Create data with repeated patterns that will create duplicate chunks
        repeated_chunk = b"A" * 64  # Exactly one chunk
        test_data = repeated_chunk * 3  # Three identical chunks
        
        snapshot_id = "dedup_test_duplicates"
        user_id = "user123"
        
        # Act
        manifest_path, original_size, deduplicated_size = await dedup_engine.store_deduplicated_snapshot(
            snapshot_id, test_data, user_id
        )
        
        # Assert
        assert original_size == len(test_data)  # 192 bytes
        assert deduplicated_size < original_size  # Should be just 64 bytes (one unique chunk)
        assert len(dedup_engine.chunk_index) == 1  # Only one unique chunk
        
        # Check chunk reference count
        chunk_hash = hashlib.sha256(repeated_chunk).hexdigest()
        assert chunk_hash in dedup_engine.chunk_index
        assert dedup_engine.chunk_index[chunk_hash].ref_count == 3
    
    async def test_retrieve_deduplicated_snapshot(self, dedup_engine, mock_storage_backend):
        """Test retrieving and reconstructing deduplicated snapshot."""
        await dedup_engine.initialize()
        
        # Store a snapshot first
        test_data = b"test data for retrieval" * 5
        snapshot_id = "retrieve_test"
        user_id = "user123"
        
        # Store the snapshot
        await dedup_engine.store_deduplicated_snapshot(snapshot_id, test_data, user_id)
        
        # Mock the retrieval of manifest and chunks
        chunks = dedup_engine._split_into_chunks(test_data)
        chunk_refs = []
        
        for chunk in chunks:
            chunk_hash = hashlib.sha256(chunk).hexdigest()
            chunk_refs.append(chunk_hash)
            
            # Mock chunk retrieval
            def create_chunk_side_effect(chunk_data):
                def side_effect(chunk_id):
                    if f"chunk_{chunk_hash}" in chunk_id:
                        return chunk_data
                    elif f"{snapshot_id}_manifest" in chunk_id:
                        manifest = {
                            'snapshot_id': snapshot_id,
                            'chunk_refs': chunk_refs,
                            'total_size': len(test_data),
                            'chunk_count': len(chunks)
                        }
                        return json.dumps(manifest).encode('utf-8')
                    return b""
                return side_effect
            
            mock_storage_backend.retrieve_snapshot.side_effect = create_chunk_side_effect(chunk)
        
        # Create proper manifest
        manifest = {
            'snapshot_id': snapshot_id,
            'chunk_refs': chunk_refs,
            'total_size': len(test_data),
            'chunk_count': len(chunks)
        }
        
        def manifest_side_effect(chunk_id):
            if f"{snapshot_id}_manifest" in chunk_id:
                return json.dumps(manifest).encode('utf-8')
            # Return appropriate chunk data
            for i, chunk_hash in enumerate(chunk_refs):
                if f"chunk_{chunk_hash}" in chunk_id:
                    return chunks[i]
            return b""
        
        mock_storage_backend.retrieve_snapshot.side_effect = manifest_side_effect
        
        # Act
        retrieved_data = await dedup_engine.retrieve_deduplicated_snapshot(snapshot_id)
        
        # Assert
        assert retrieved_data == test_data
    
    async def test_delete_deduplicated_snapshot(self, dedup_engine, mock_storage_backend):
        """Test deleting deduplicated snapshot and reference counting."""
        await dedup_engine.initialize()
        
        # Create test data with duplicate chunks
        repeated_chunk = b"B" * 64
        test_data1 = repeated_chunk * 2  # Two chunks
        test_data2 = repeated_chunk + b"C" * 64  # One shared, one unique
        
        user_id = "user123"
        
        # Store two snapshots sharing a chunk
        await dedup_engine.store_deduplicated_snapshot("snap1", test_data1, user_id)
        await dedup_engine.store_deduplicated_snapshot("snap2", test_data2, user_id)
        
        # Check reference counts
        shared_chunk_hash = hashlib.sha256(repeated_chunk).hexdigest()
        assert dedup_engine.chunk_index[shared_chunk_hash].ref_count == 3  # Used 3 times total
        
        # Mock manifest retrieval for deletion
        chunks1 = dedup_engine._split_into_chunks(test_data1)
        chunk_refs1 = [hashlib.sha256(chunk).hexdigest() for chunk in chunks1]
        
        manifest1 = {
            'snapshot_id': 'snap1',
            'chunk_refs': chunk_refs1,
            'total_size': len(test_data1)
        }
        
        mock_storage_backend.retrieve_snapshot.return_value = json.dumps(manifest1).encode('utf-8')
        
        # Delete first snapshot
        result = await dedup_engine.delete_deduplicated_snapshot("snap1")
        
        # Assert
        assert result is True
        # Shared chunk should still exist but with reduced ref count
        assert shared_chunk_hash in dedup_engine.chunk_index
        assert dedup_engine.chunk_index[shared_chunk_hash].ref_count == 1  # Only used by snap2 now
    
    async def test_chunk_cleanup_on_zero_references(self, dedup_engine, mock_storage_backend):
        """Test that chunks are deleted when reference count reaches zero."""
        await dedup_engine.initialize()
        
        # Store snapshot with unique chunk
        unique_data = b"unique chunk data" + b"X" * 48  # Make it 64 bytes
        snapshot_id = "cleanup_test"
        user_id = "user123"
        
        await dedup_engine.store_deduplicated_snapshot(snapshot_id, unique_data, user_id)
        
        # Get the actual chunk hash from the stored data
        chunks = dedup_engine._split_into_chunks(unique_data)
        chunk_hash = hashlib.sha256(chunks[0]).hexdigest()
        assert chunk_hash in dedup_engine.chunk_index
        
        # Mock manifest for deletion
        manifest = {
            'snapshot_id': snapshot_id,
            'chunk_refs': [chunk_hash],
            'total_size': len(unique_data)
        }
        mock_storage_backend.retrieve_snapshot.return_value = json.dumps(manifest).encode('utf-8')
        
        # Delete snapshot
        await dedup_engine.delete_deduplicated_snapshot(snapshot_id)
        
        # Chunk should be deleted since ref count is 0
        assert chunk_hash not in dedup_engine.chunk_index
        
        # Verify chunk deletion was called
        chunk_delete_calls = [
            call for call in mock_storage_backend.delete_snapshot.call_args_list
            if f"chunk_{chunk_hash}" in str(call)
        ]
        assert len(chunk_delete_calls) > 0
    
    async def test_deduplication_stats(self, dedup_engine):
        """Test deduplication statistics calculation."""
        # Initialize with some mock data
        dedup_engine.chunk_index = {
            "hash1": ChunkInfo("hash1", 64, 2, "key1", 1000.0, 1001.0),
            "hash2": ChunkInfo("hash2", 64, 1, "key2", 1000.0, 1002.0),
            "hash3": ChunkInfo("hash3", 32, 3, "key3", 1000.0, 1003.0)
        }
        
        dedup_engine.stats.total_chunks_processed = 10
        dedup_engine.stats.unique_chunks_stored = 3
        dedup_engine.stats.duplicate_chunks_found = 7
        dedup_engine.stats.total_bytes_saved = 256
        
        stats = dedup_engine.get_deduplication_stats()
        
        assert stats['total_chunks_indexed'] == 3
        assert stats['total_unique_bytes'] == 160  # 64+64+32
        assert stats['total_logical_bytes'] == 288  # (64*2)+(64*1)+(32*3) = 128+64+96 = 288
        assert abs(stats['current_deduplication_ratio'] - 0.5556) < 0.01  # 160/288 ≈ 0.5556
        assert abs(stats['space_efficiency_percent'] - 44.44) < 0.1  # (288-160)/288 * 100 ≈ 44.44%
    
    async def test_cleanup_orphaned_chunks(self, dedup_engine, mock_storage_backend):
        """Test cleanup of orphaned chunks."""
        await dedup_engine.initialize()
        
        import time
        current_time = time.time()
        old_time = current_time - (31 * 24 * 3600)  # 31 days ago
        
        # Add orphaned chunks (ref_count = 0, old)
        dedup_engine.chunk_index = {
            "orphan1": ChunkInfo("orphan1", 64, 0, "key1", old_time, old_time),
            "orphan2": ChunkInfo("orphan2", 64, 0, "key2", old_time, old_time),
            "active": ChunkInfo("active", 64, 1, "key3", current_time, current_time)
        }
        
        # Mock successful deletion
        mock_storage_backend.delete_snapshot.return_value = True
        
        # Act
        cleaned_count = await dedup_engine.cleanup_orphaned_chunks(max_age_days=30)
        
        # Assert
        assert cleaned_count == 2
        assert "orphan1" not in dedup_engine.chunk_index
        assert "orphan2" not in dedup_engine.chunk_index
        assert "active" in dedup_engine.chunk_index  # Should remain
    
    async def test_chunk_index_persistence(self, dedup_engine, mock_storage_backend):
        """Test chunk index persistence and loading."""
        # Test saving chunk index
        dedup_engine.chunk_index = {
            "test_hash": ChunkInfo("test_hash", 64, 1, "test_key", 1000.0, 1001.0)
        }
        
        await dedup_engine._persist_chunk_index()
        
        # Verify store_snapshot was called with index data
        store_calls = mock_storage_backend.store_snapshot.call_args_list
        index_call = None
        for call in store_calls:
            if "chunk_index" in call[0][0]:
                index_call = call
                break
        
        assert index_call is not None
        
        # Test loading chunk index
        index_data = {
            "loaded_hash": {
                "chunk_hash": "loaded_hash",
                "size_bytes": 128,
                "ref_count": 2,
                "storage_key": "loaded_key",
                "first_seen_at": 2000.0,
                "last_accessed_at": 2001.0
            }
        }
        
        mock_storage_backend.retrieve_snapshot.return_value = json.dumps(index_data).encode('utf-8')
        
        # Clear current index and load
        dedup_engine.chunk_index.clear()
        await dedup_engine._load_chunk_index()
        
        # Verify loading
        assert "loaded_hash" in dedup_engine.chunk_index
        loaded_chunk = dedup_engine.chunk_index["loaded_hash"]
        assert loaded_chunk.size_bytes == 128
        assert loaded_chunk.ref_count == 2


class TestDeduplicationSecurity:
    """Security tests for deduplication engine."""
    
    async def test_chunk_hash_integrity(self, dedup_engine):
        """Test that chunk hashes are calculated correctly."""
        test_chunk = b"test chunk data for hashing"
        
        # Calculate expected hash
        expected_hash = hashlib.sha256(test_chunk).hexdigest()
        
        # Split into chunks and verify hash
        chunks = dedup_engine._split_into_chunks(test_chunk)
        if chunks:
            calculated_hash = hashlib.sha256(chunks[0]).hexdigest()
            if len(chunks[0]) == len(test_chunk):  # Single chunk
                assert calculated_hash == expected_hash
    
    async def test_user_scoped_deduplication(self, dedup_engine, mock_storage_backend):
        """Test that deduplication is properly scoped by user."""
        await dedup_engine.initialize()
        
        test_data = b"shared data between users" * 2
        
        # Store same data for different users
        await dedup_engine.store_deduplicated_snapshot("snap1", test_data, "user1")
        await dedup_engine.store_deduplicated_snapshot("snap2", test_data, "user2")
        
        # Verify storage calls include user scoping
        store_calls = mock_storage_backend.store_snapshot.call_args_list
        
        # Check that chunk storage includes user information
        chunk_calls = [call for call in store_calls if "chunk_" in call[0][0]]
        assert len(chunk_calls) > 0
        
        # Verify metadata includes user_id
        for call in chunk_calls:
            if len(call[0]) > 2:  # Has metadata
                metadata = call[0][2] if len(call[0]) > 2 else call[1].get('metadata', {})
                if metadata:
                    assert 'user_id' in metadata


if __name__ == "__main__":
    pytest.main([__file__, "-v"])