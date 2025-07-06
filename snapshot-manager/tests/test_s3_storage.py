"""
Comprehensive test suite for S3 storage backend.
Tests MinIO integration, multipart uploads, error handling, and performance.
"""

import pytest
import asyncio
import time
import hashlib
from unittest.mock import AsyncMock, MagicMock, patch
from botocore.exceptions import ClientError

# Import the storage classes
import sys
from pathlib import Path
current_dir = Path(__file__).parent
parent_dir = current_dir.parent
sys.path.insert(0, str(parent_dir))

from storage.s3_backend import S3StorageBackend, StorageMetrics


@pytest.fixture
def storage_config():
    """Storage configuration for testing."""
    return {
        'endpoint_url': 'http://localhost:9002',
        'access_key': 'test_access_key',
        'secret_key': 'test_secret_key',
        'bucket_name': 'test-snapshots',
        'region': 'us-east-1'
    }


@pytest.fixture
def s3_storage(storage_config):
    """S3 storage backend instance."""
    return S3StorageBackend(**storage_config)


@pytest.fixture
def mock_s3_session():
    """Mock aioboto3 session for testing."""
    session = AsyncMock()
    s3_client = AsyncMock()
    
    # Create async context manager mock
    async_context_manager = AsyncMock()
    async_context_manager.__aenter__ = AsyncMock(return_value=s3_client)
    async_context_manager.__aexit__ = AsyncMock(return_value=None)
    
    # Make session.client return the async context manager
    session.client = MagicMock(return_value=async_context_manager)
    
    return session, s3_client


class TestS3StorageBackend:
    """Test S3 storage backend functionality."""
    
    async def test_initialization_success(self, s3_storage, mock_s3_session):
        """Test successful S3 storage initialization."""
        session, s3_client = mock_s3_session
        
        with patch('storage.s3_backend.aioboto3.Session', return_value=session):
            # Mock successful bucket operations
            s3_client.list_buckets = AsyncMock()
            s3_client.head_bucket = AsyncMock()
            
            await s3_storage.initialize()
            
            # Verify initialization calls
            s3_client.list_buckets.assert_called_once()
            s3_client.head_bucket.assert_called_once_with(Bucket='test-snapshots')
    
    async def test_initialization_creates_bucket(self, s3_storage, mock_s3_session):
        """Test bucket creation during initialization."""
        session, s3_client = mock_s3_session
        
        with patch('storage.s3_backend.aioboto3.Session', return_value=session):
            # Mock bucket doesn't exist
            s3_client.list_buckets = AsyncMock()
            s3_client.head_bucket = AsyncMock(side_effect=ClientError(
                {'Error': {'Code': '404'}}, 'HeadBucket'
            ))
            s3_client.create_bucket = AsyncMock()
            
            await s3_storage.initialize()
            
            # Verify bucket creation
            s3_client.create_bucket.assert_called_once_with(Bucket='test-snapshots')
    
    async def test_store_snapshot_small(self, s3_storage, mock_s3_session):
        """Test storing small snapshot with simple upload."""
        session, s3_client = mock_s3_session
        s3_storage.session = session
        
        # Test data smaller than multipart threshold
        test_data = b"test snapshot data" * 100  # Small data
        snapshot_id = "test_snapshot_123"
        
        s3_client.put_object = AsyncMock()
        
        # Act
        storage_path = await s3_storage.store_snapshot(snapshot_id, test_data)
        
        # Assert
        assert storage_path.startswith("s3://test-snapshots/")
        s3_client.put_object.assert_called_once()
        
        # Verify call arguments
        call_args = s3_client.put_object.call_args
        assert call_args[1]['Bucket'] == 'test-snapshots'
        assert call_args[1]['Body'] == test_data
        assert 'ServerSideEncryption' in call_args[1]
    
    async def test_store_snapshot_large_multipart(self, s3_storage, mock_s3_session):
        """Test storing large snapshot with multipart upload."""
        session, s3_client = mock_s3_session
        s3_storage.session = session
        
        # Create large data that exceeds multipart threshold
        large_data = b"x" * (s3_storage.multipart_threshold + 1000)
        snapshot_id = "large_snapshot_123"
        
        # Mock multipart upload responses
        s3_client.create_multipart_upload = AsyncMock(return_value={'UploadId': 'test-upload-id'})
        s3_client.upload_part = AsyncMock(return_value={'ETag': 'test-etag'})
        s3_client.complete_multipart_upload = AsyncMock()
        
        # Act
        storage_path = await s3_storage.store_snapshot(snapshot_id, large_data)
        
        # Assert multipart upload was used
        assert storage_path.startswith("s3://test-snapshots/")
        s3_client.create_multipart_upload.assert_called_once()
        s3_client.complete_multipart_upload.assert_called_once()
        
        # Verify multiple parts were uploaded
        assert s3_client.upload_part.call_count > 1
    
    async def test_retrieve_snapshot_success(self, s3_storage, mock_s3_session):
        """Test successful snapshot retrieval."""
        session, s3_client = mock_s3_session
        s3_storage.session = session
        
        # Test data
        test_data = b"retrieved snapshot data"
        snapshot_id = "retrieve_test_123"
        checksum = hashlib.sha256(test_data).hexdigest()
        
        # Mock S3 response
        mock_response = {
            'Body': AsyncMock(),
            'Metadata': {'checksum': checksum}
        }
        mock_response['Body'].read = AsyncMock(return_value=test_data)
        s3_client.get_object = AsyncMock(return_value=mock_response)
        
        # Act
        retrieved_data = await s3_storage.retrieve_snapshot(snapshot_id)
        
        # Assert
        assert retrieved_data == test_data
        s3_client.get_object.assert_called_once()
    
    async def test_retrieve_snapshot_not_found(self, s3_storage, mock_s3_session):
        """Test snapshot retrieval when snapshot doesn't exist."""
        session, s3_client = mock_s3_session
        s3_storage.session = session
        
        # Mock not found error
        s3_client.get_object = AsyncMock(side_effect=ClientError(
            {'Error': {'Code': 'NoSuchKey'}}, 'GetObject'
        ))
        
        # Act & Assert
        with pytest.raises(FileNotFoundError, match="not found in storage"):
            await s3_storage.retrieve_snapshot("nonexistent_snapshot")
    
    async def test_retrieve_snapshot_checksum_mismatch(self, s3_storage, mock_s3_session):
        """Test snapshot retrieval with checksum validation failure."""
        session, s3_client = mock_s3_session
        s3_storage.session = session
        
        # Test data with wrong checksum
        test_data = b"corrupted snapshot data"
        wrong_checksum = "wrong_checksum_value"
        
        mock_response = {
            'Body': AsyncMock(),
            'Metadata': {'checksum': wrong_checksum}
        }
        mock_response['Body'].read = AsyncMock(return_value=test_data)
        s3_client.get_object = AsyncMock(return_value=mock_response)
        
        # Act & Assert
        with pytest.raises(ValueError, match="checksum mismatch"):
            await s3_storage.retrieve_snapshot("corrupted_snapshot")
    
    async def test_delete_snapshot_success(self, s3_storage, mock_s3_session):
        """Test successful snapshot deletion."""
        session, s3_client = mock_s3_session
        s3_storage.session = session
        
        s3_client.delete_object = AsyncMock()
        
        # Act
        result = await s3_storage.delete_snapshot("delete_test_123")
        
        # Assert
        assert result is True
        s3_client.delete_object.assert_called_once()
    
    async def test_delete_snapshot_failure(self, s3_storage, mock_s3_session):
        """Test snapshot deletion failure handling."""
        session, s3_client = mock_s3_session
        s3_storage.session = session
        
        s3_client.delete_object = AsyncMock(side_effect=Exception("Delete failed"))
        
        # Act
        result = await s3_storage.delete_snapshot("delete_fail_test")
        
        # Assert
        assert result is False
    
    async def test_multipart_upload_abort_on_failure(self, s3_storage, mock_s3_session):
        """Test multipart upload abortion on failure."""
        session, s3_client = mock_s3_session
        s3_storage.session = session
        
        # Large data for multipart
        large_data = b"x" * (s3_storage.multipart_threshold + 1000)
        
        # Mock successful start but failed upload
        s3_client.create_multipart_upload = AsyncMock(return_value={'UploadId': 'test-upload-id'})
        s3_client.upload_part = AsyncMock(side_effect=Exception("Upload failed"))
        s3_client.abort_multipart_upload = AsyncMock()
        
        # Act & Assert
        with pytest.raises(Exception, match="Upload failed"):
            await s3_storage.store_snapshot("multipart_fail_test", large_data)
        
        # Verify abort was called
        s3_client.abort_multipart_upload.assert_called_once()
    
    async def test_performance_metrics_recording(self, s3_storage, mock_s3_session):
        """Test that performance metrics are recorded correctly."""
        session, s3_client = mock_s3_session
        s3_storage.session = session
        
        test_data = b"metrics test data"
        s3_client.put_object = AsyncMock()
        
        # Ensure no metrics initially
        assert len(s3_storage.metrics) == 0
        
        # Act
        await s3_storage.store_snapshot("metrics_test", test_data)
        
        # Assert metrics were recorded
        assert len(s3_storage.metrics) == 1
        metric = s3_storage.metrics[0]
        assert metric.operation == "store_snapshot"
        assert metric.success is True
        assert metric.size_bytes == len(test_data)
        assert metric.duration_ms > 0
    
    async def test_performance_stats_calculation(self, s3_storage):
        """Test performance statistics calculation."""
        # Add mock metrics
        s3_storage.metrics = [
            StorageMetrics("store_snapshot", 100.0, 1000, True),
            StorageMetrics("store_snapshot", 200.0, 2000, True),
            StorageMetrics("retrieve_snapshot", 50.0, 1500, True),
            StorageMetrics("store_snapshot", 150.0, 0, False, "Error")
        ]
        
        stats = s3_storage.get_performance_stats()
        
        assert stats['total_operations'] == 4
        assert stats['successful_operations'] == 3
        assert stats['failed_operations'] == 1
        assert stats['success_rate'] == 75.0
        assert abs(stats['avg_duration_ms'] - 116.67) < 0.01  # Average of successful ops
        assert stats['total_bytes_processed'] == 4500


class TestS3StorageIntegration:
    """Integration tests for S3 storage (requires running MinIO)."""
    
    @pytest.mark.integration
    async def test_real_minio_integration(self, storage_config):
        """Test actual MinIO integration."""
        # Skip if MinIO not available
        # Disable encryption for local MinIO testing
        config_with_no_encryption = {**storage_config, 'enable_encryption': False}
        storage = S3StorageBackend(**config_with_no_encryption)
        
        try:
            await storage.initialize()
        except Exception:
            pytest.skip("MinIO not available for integration testing")
        
        test_data = b"integration test data"
        snapshot_id = "integration_test_123"
        
        # Test store
        storage_path = await storage.store_snapshot(snapshot_id, test_data)
        assert storage_path is not None
        
        # Test retrieve
        retrieved_data = await storage.retrieve_snapshot(snapshot_id)
        assert retrieved_data == test_data
        
        # Test delete
        deleted = await storage.delete_snapshot(snapshot_id)
        assert deleted is True
        
        # Verify it's actually deleted
        with pytest.raises(FileNotFoundError):
            await storage.retrieve_snapshot(snapshot_id)


class TestS3StorageSecurity:
    """Security tests for S3 storage backend."""
    
    async def test_encryption_at_rest(self, s3_storage, mock_s3_session):
        """Test that server-side encryption is enabled."""
        session, s3_client = mock_s3_session
        s3_storage.session = session
        
        test_data = b"encrypted test data"
        s3_client.put_object = AsyncMock()
        
        await s3_storage.store_snapshot("encryption_test", test_data)
        
        # Verify encryption was requested
        call_args = s3_client.put_object.call_args
        assert call_args[1]['ServerSideEncryption'] == 'AES256'
    
    async def test_metadata_sanitization(self, s3_storage):
        """Test that metadata is properly sanitized for S3."""
        metadata = {
            'string_value': 'test',
            'int_value': 123,
            'float_value': 45.67,
            'bool_value': True,
            'list_value': [1, 2, 3],  # Should be filtered out
            'dict_value': {'key': 'value'}  # Should be filtered out
        }
        
        sanitized = s3_storage._prepare_s3_metadata(metadata)
        
        # Only basic types should remain
        assert 'string_value' in sanitized
        assert 'int_value' in sanitized
        assert 'float_value' in sanitized
        assert 'bool_value' in sanitized
        assert 'list_value' not in sanitized
        assert 'dict_value' not in sanitized
        
        # All values should be strings
        assert all(isinstance(v, str) for v in sanitized.values())
    
    async def test_storage_key_generation(self, s3_storage):
        """Test storage key generation for security."""
        snapshot_id = "test_snapshot_123"
        
        key = s3_storage._generate_storage_key(snapshot_id)
        
        # Verify hierarchical structure
        assert key.startswith("snapshots/")
        assert snapshot_id in key
        assert key.endswith("/data")


if __name__ == "__main__":
    pytest.main([__file__, "-v"])