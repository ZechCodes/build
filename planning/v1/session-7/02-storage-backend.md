# Session 7.2: Storage Backend & MinIO Integration

## Objective
Implement robust storage backend with MinIO S3 integration, providing efficient snapshot storage, retrieval, deduplication, and management with comprehensive error handling and performance optimization.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for storage operation monitoring and performance tracking
- **Session 5**: Connects to database for storage metadata and indexing
- **Session 6**: Coordinates with session management for storage access control
- **MinIO Infrastructure**: Leverages S3-compatible storage for scalable snapshot storage

## Core Implementation

### S3 Storage Backend
**Location**: `snapshot-manager/storage/s3_backend.py`

```python
# snapshot-manager/storage/s3_backend.py
import asyncio
import hashlib
import time
from typing import Dict, Any, Optional, List, BinaryIO
from dataclasses import dataclass
import aioboto3
import structlog
import logfire
from botocore.exceptions import ClientError, NoCredentialsError

logger = structlog.get_logger()

@dataclass
class StorageMetrics:
    operation: str
    duration_ms: float
    size_bytes: int
    success: bool
    error_message: Optional[str] = None

class S3StorageBackend:
    def __init__(self, endpoint_url: str, access_key: str, secret_key: str,
                 bucket_name: str, region: str = "us-east-1"):
        self.endpoint_url = endpoint_url
        self.access_key = access_key
        self.secret_key = secret_key
        self.bucket_name = bucket_name
        self.region = region
        self.session = None
        self.s3_client = None
        
        # Storage configuration
        self.multipart_threshold = 100 * 1024 * 1024  # 100MB
        self.multipart_chunksize = 10 * 1024 * 1024   # 10MB
        self.max_retries = 3
        self.retry_delay = 1.0
        
        # Performance tracking
        self.metrics: List[StorageMetrics] = []
        
    async def initialize(self):
        """Initialize S3 storage backend"""
        try:
            self.session = aioboto3.Session()
            
            # Test connection
            await self._test_connection()
            
            # Ensure bucket exists
            await self._ensure_bucket_exists()
            
            logger.info("S3 storage backend initialized", 
                       endpoint=self.endpoint_url,
                       bucket=self.bucket_name)
            
            # Log to Logfire
            logfire.info("S3 storage backend started",
                        endpoint=self.endpoint_url,
                        bucket=self.bucket_name,
                        service="snapshot-storage")
                        
        except Exception as e:
            logger.error("Failed to initialize S3 storage", error=str(e))
            logfire.error("S3 storage initialization failed", error=str(e))
            raise
    
    async def store_snapshot(self, snapshot_id: str, data: bytes,
                           metadata: Dict[str, Any] = None) -> str:
        """Store snapshot data in S3"""
        start_time = time.time()
        
        try:
            # Generate storage key
            storage_key = self._generate_storage_key(snapshot_id)
            
            # Prepare metadata
            s3_metadata = self._prepare_s3_metadata(metadata or {})
            
            # Calculate checksum
            checksum = hashlib.sha256(data).hexdigest()
            s3_metadata['checksum'] = checksum
            
            async with self.session.client(
                's3',
                endpoint_url=self.endpoint_url,
                aws_access_key_id=self.access_key,
                aws_secret_access_key=self.secret_key,
                region_name=self.region
            ) as s3:
                
                if len(data) > self.multipart_threshold:
                    # Use multipart upload for large files
                    storage_path = await self._multipart_upload(
                        s3, storage_key, data, s3_metadata
                    )
                else:
                    # Use simple upload for smaller files
                    await s3.put_object(
                        Bucket=self.bucket_name,
                        Key=storage_key,
                        Body=data,
                        Metadata=s3_metadata,
                        ContentType='application/octet-stream'
                    )
                    storage_path = f"s3://{self.bucket_name}/{storage_key}"
            
            # Record metrics
            duration_ms = (time.time() - start_time) * 1000
            self._record_metrics(StorageMetrics(
                operation="store_snapshot",
                duration_ms=duration_ms,
                size_bytes=len(data),
                success=True
            ))
            
            logger.info("Snapshot stored successfully", 
                       snapshot_id=snapshot_id,
                       storage_key=storage_key,
                       size_bytes=len(data),
                       duration_ms=duration_ms)
            
            # Log to Logfire with performance metrics
            logfire.info("Snapshot storage completed",
                        snapshot_id=snapshot_id,
                        storage_path=storage_path,
                        size_bytes=len(data),
                        duration_ms=duration_ms,
                        checksum=checksum)
            
            return storage_path
            
        except Exception as e:
            duration_ms = (time.time() - start_time) * 1000
            self._record_metrics(StorageMetrics(
                operation="store_snapshot",
                duration_ms=duration_ms,
                size_bytes=len(data),
                success=False,
                error_message=str(e)
            ))
            
            logger.error("Failed to store snapshot", 
                        snapshot_id=snapshot_id, error=str(e))
            logfire.error("Snapshot storage failed",
                         snapshot_id=snapshot_id, error=str(e))
            raise
    
    async def retrieve_snapshot(self, snapshot_id: str) -> bytes:
        """Retrieve snapshot data from S3"""
        start_time = time.time()
        
        try:
            storage_key = self._generate_storage_key(snapshot_id)
            
            async with self.session.client(
                's3',
                endpoint_url=self.endpoint_url,
                aws_access_key_id=self.access_key,
                aws_secret_access_key=self.secret_key,
                region_name=self.region
            ) as s3:
                
                response = await s3.get_object(
                    Bucket=self.bucket_name,
                    Key=storage_key
                )
                
                # Read data
                data = await response['Body'].read()
                
                # Verify checksum if available
                stored_checksum = response.get('Metadata', {}).get('checksum')
                if stored_checksum:
                    calculated_checksum = hashlib.sha256(data).hexdigest()
                    if stored_checksum != calculated_checksum:
                        raise ValueError("Snapshot data corruption detected")
            
            # Record metrics
            duration_ms = (time.time() - start_time) * 1000
            self._record_metrics(StorageMetrics(
                operation="retrieve_snapshot",
                duration_ms=duration_ms,
                size_bytes=len(data),
                success=True
            ))
            
            logger.info("Snapshot retrieved successfully", 
                       snapshot_id=snapshot_id,
                       size_bytes=len(data),
                       duration_ms=duration_ms)
            
            # Log to Logfire
            logfire.info("Snapshot retrieval completed",
                        snapshot_id=snapshot_id,
                        size_bytes=len(data),
                        duration_ms=duration_ms)
            
            return data
            
        except ClientError as e:
            if e.response['Error']['Code'] == 'NoSuchKey':
                logger.warning("Snapshot not found in storage", 
                              snapshot_id=snapshot_id)
                raise FileNotFoundError(f"Snapshot {snapshot_id} not found")
            else:
                raise
                
        except Exception as e:
            duration_ms = (time.time() - start_time) * 1000
            self._record_metrics(StorageMetrics(
                operation="retrieve_snapshot",
                duration_ms=duration_ms,
                size_bytes=0,
                success=False,
                error_message=str(e)
            ))
            
            logger.error("Failed to retrieve snapshot", 
                        snapshot_id=snapshot_id, error=str(e))
            logfire.error("Snapshot retrieval failed",
                         snapshot_id=snapshot_id, error=str(e))
            raise
    
    async def delete_snapshot(self, snapshot_id: str) -> bool:
        """Delete snapshot data from S3"""
        start_time = time.time()
        
        try:
            storage_key = self._generate_storage_key(snapshot_id)
            
            async with self.session.client(
                's3',
                endpoint_url=self.endpoint_url,
                aws_access_key_id=self.access_key,
                aws_secret_access_key=self.secret_key,
                region_name=self.region
            ) as s3:
                
                await s3.delete_object(
                    Bucket=self.bucket_name,
                    Key=storage_key
                )
            
            duration_ms = (time.time() - start_time) * 1000
            self._record_metrics(StorageMetrics(
                operation="delete_snapshot",
                duration_ms=duration_ms,
                size_bytes=0,
                success=True
            ))
            
            logger.info("Snapshot deleted successfully", 
                       snapshot_id=snapshot_id,
                       duration_ms=duration_ms)
            
            # Log to Logfire
            logfire.info("Snapshot deletion completed",
                        snapshot_id=snapshot_id,
                        duration_ms=duration_ms)
            
            return True
            
        except Exception as e:
            duration_ms = (time.time() - start_time) * 1000
            self._record_metrics(StorageMetrics(
                operation="delete_snapshot",
                duration_ms=duration_ms,
                size_bytes=0,
                success=False,
                error_message=str(e)
            ))
            
            logger.error("Failed to delete snapshot", 
                        snapshot_id=snapshot_id, error=str(e))
            logfire.error("Snapshot deletion failed",
                         snapshot_id=snapshot_id, error=str(e))
            return False
    
    async def _multipart_upload(self, s3_client, storage_key: str, 
                              data: bytes, metadata: Dict[str, str]) -> str:
        """Perform multipart upload for large snapshots"""
        try:
            # Initiate multipart upload
            response = await s3_client.create_multipart_upload(
                Bucket=self.bucket_name,
                Key=storage_key,
                Metadata=metadata,
                ContentType='application/octet-stream'
            )
            
            upload_id = response['UploadId']
            parts = []
            
            # Upload parts
            part_number = 1
            offset = 0
            
            while offset < len(data):
                end_offset = min(offset + self.multipart_chunksize, len(data))
                part_data = data[offset:end_offset]
                
                part_response = await s3_client.upload_part(
                    Bucket=self.bucket_name,
                    Key=storage_key,
                    PartNumber=part_number,
                    UploadId=upload_id,
                    Body=part_data
                )
                
                parts.append({
                    'ETag': part_response['ETag'],
                    'PartNumber': part_number
                })
                
                part_number += 1
                offset = end_offset
            
            # Complete multipart upload
            await s3_client.complete_multipart_upload(
                Bucket=self.bucket_name,
                Key=storage_key,
                UploadId=upload_id,
                MultipartUpload={'Parts': parts}
            )
            
            logger.info("Multipart upload completed", 
                       storage_key=storage_key,
                       parts_count=len(parts),
                       total_size=len(data))
            
            return f"s3://{self.bucket_name}/{storage_key}"
            
        except Exception as e:
            # Abort multipart upload on failure
            try:
                await s3_client.abort_multipart_upload(
                    Bucket=self.bucket_name,
                    Key=storage_key,
                    UploadId=upload_id
                )
            except Exception:
                pass  # Best effort cleanup
            
            logger.error("Multipart upload failed", 
                        storage_key=storage_key, error=str(e))
            raise
```

### Deduplication Engine
**Location**: `snapshot-manager/storage/deduplication.py`

```python
# snapshot-manager/storage/deduplication.py
import asyncio
import hashlib
from typing import Dict, List, Optional, Set, Tuple
from dataclasses import dataclass
import structlog
import logfire

logger = structlog.get_logger()

@dataclass
class ChunkInfo:
    chunk_hash: str
    size_bytes: int
    ref_count: int
    storage_key: str

class DeduplicationEngine:
    def __init__(self, storage_backend, chunk_size: int = 1024 * 1024):  # 1MB chunks
        self.storage = storage_backend
        self.chunk_size = chunk_size
        self.chunk_index: Dict[str, ChunkInfo] = {}
        
    async def initialize(self):
        """Initialize deduplication engine"""
        # Load existing chunk index from database
        await self._load_chunk_index()
        
        logger.info("Deduplication engine initialized", 
                   chunks_indexed=len(self.chunk_index))
        logfire.info("Deduplication engine started",
                    chunks_count=len(self.chunk_index))
    
    async def store_deduplicated_snapshot(self, snapshot_id: str, 
                                        data: bytes) -> Tuple[str, int, int]:
        """Store snapshot with deduplication"""
        try:
            # Split into chunks
            chunks = self._split_into_chunks(data)
            
            # Process chunks for deduplication
            chunk_refs = []
            new_chunks_stored = 0
            total_size = len(data)
            
            for chunk_data in chunks:
                chunk_hash = hashlib.sha256(chunk_data).hexdigest()
                
                if chunk_hash in self.chunk_index:
                    # Chunk already exists, increment reference count
                    self.chunk_index[chunk_hash].ref_count += 1
                    chunk_refs.append(chunk_hash)
                else:
                    # Store new chunk
                    chunk_key = f"chunks/{chunk_hash}"
                    await self.storage.store_snapshot(
                        f"chunk_{chunk_hash}", chunk_data
                    )
                    
                    # Add to index
                    self.chunk_index[chunk_hash] = ChunkInfo(
                        chunk_hash=chunk_hash,
                        size_bytes=len(chunk_data),
                        ref_count=1,
                        storage_key=chunk_key
                    )
                    
                    chunk_refs.append(chunk_hash)
                    new_chunks_stored += 1
            
            # Store manifest
            manifest = {
                'snapshot_id': snapshot_id,
                'chunk_refs': chunk_refs,
                'total_size': total_size,
                'chunk_count': len(chunks)
            }
            
            manifest_data = json.dumps(manifest).encode('utf-8')
            manifest_key = await self.storage.store_snapshot(
                f"{snapshot_id}_manifest", manifest_data
            )
            
            # Calculate deduplication savings
            deduplicated_size = sum(
                self.chunk_index[ref].size_bytes 
                for ref in set(chunk_refs)  # Only count unique chunks
            )
            
            logger.info("Deduplicated snapshot stored", 
                       snapshot_id=snapshot_id,
                       original_size=total_size,
                       deduplicated_size=deduplicated_size,
                       savings_percent=(1 - deduplicated_size/total_size) * 100,
                       new_chunks=new_chunks_stored)
            
            # Log to Logfire
            logfire.info("Snapshot deduplication completed",
                        snapshot_id=snapshot_id,
                        original_size_bytes=total_size,
                        deduplicated_size_bytes=deduplicated_size,
                        deduplication_ratio=deduplicated_size/total_size,
                        new_chunks_stored=new_chunks_stored)
            
            return manifest_key, total_size, deduplicated_size
            
        except Exception as e:
            logger.error("Deduplication storage failed", 
                        snapshot_id=snapshot_id, error=str(e))
            logfire.error("Snapshot deduplication failed",
                         snapshot_id=snapshot_id, error=str(e))
            raise
    
    def _split_into_chunks(self, data: bytes) -> List[bytes]:
        """Split data into fixed-size chunks"""
        chunks = []
        offset = 0
        
        while offset < len(data):
            end_offset = min(offset + self.chunk_size, len(data))
            chunk = data[offset:end_offset]
            chunks.append(chunk)
            offset = end_offset
        
        return chunks
```

## TDD Implementation Cycle

### Test-Driven Development Process

1. **Red Phase**: Write failing storage tests
   ```bash
   # Create storage test file
   touch snapshot-manager/tests/test_s3_storage.py
   
   # Run failing test
   pytest snapshot-manager/tests/test_s3_storage.py::test_store_snapshot -v
   ```

2. **Green Phase**: Implement basic storage operations
   ```bash
   # Implement S3 storage functionality
   pytest snapshot-manager/tests/test_s3_storage.py::test_store_snapshot -v
   ```

3. **Refactor Phase**: Optimize storage performance
   ```bash
   # Add multipart uploads and error handling
   pytest snapshot-manager/tests/ -v
   ```

4. **Commit**: Commit storage functionality
   ```bash
   git add snapshot-manager/storage/ snapshot-manager/tests/test_s3_storage.py
   git commit -m "feat: implement S3 storage backend with MinIO integration
   
   - Add S3StorageBackend with multipart upload support
   - Implement comprehensive error handling and retry logic
   - Add deduplication engine for storage optimization
   - Include performance monitoring and metrics collection
   - Integrate with Logfire for storage operation tracking
   
   Tests: Added comprehensive storage test suite with error scenarios
   Performance: Multipart uploads for large snapshots, <100ms metadata ops
   Security: Checksum validation and secure credential handling"
   ```

### Storage Test Cases

```python
# snapshot-manager/tests/test_s3_storage.py
import pytest
import asyncio
from unittest.mock import AsyncMock, patch
from snapshot_manager.storage.s3_backend import S3StorageBackend

@pytest.fixture
def storage_config():
    """Storage configuration for testing"""
    return {
        'endpoint_url': 'http://localhost:9000',
        'access_key': 'test_access_key',
        'secret_key': 'test_secret_key',
        'bucket_name': 'test-snapshots',
        'region': 'us-east-1'
    }

@pytest.fixture
async def s3_storage(storage_config):
    """S3 storage backend instance"""
    storage = S3StorageBackend(**storage_config)
    # Mock the initialization to avoid actual S3 connection
    with patch.object(storage, '_test_connection'), \
         patch.object(storage, '_ensure_bucket_exists'):
        await storage.initialize()
    return storage

class TestS3StorageBackend:
    async def test_store_snapshot_success(self, s3_storage):
        """Test successful snapshot storage"""
        # Mock S3 operations
        with patch.object(s3_storage.session, 'client') as mock_client:
            mock_s3 = AsyncMock()
            mock_client.return_value.__aenter__.return_value = mock_s3
            
            # Arrange
            snapshot_id = "snapshot123"
            test_data = b"test snapshot data"
            
            # Act
            storage_path = await s3_storage.store_snapshot(snapshot_id, test_data)
            
            # Assert
            assert storage_path.startswith("s3://")
            mock_s3.put_object.assert_called_once()
    
    async def test_retrieve_snapshot_success(self, s3_storage):
        """Test successful snapshot retrieval"""
        with patch.object(s3_storage.session, 'client') as mock_client:
            mock_s3 = AsyncMock()
            mock_client.return_value.__aenter__.return_value = mock_s3
            
            # Mock response
            test_data = b"test snapshot data"
            mock_response = {
                'Body': AsyncMock(),
                'Metadata': {'checksum': hashlib.sha256(test_data).hexdigest()}
            }
            mock_response['Body'].read = AsyncMock(return_value=test_data)
            mock_s3.get_object.return_value = mock_response
            
            # Act
            retrieved_data = await s3_storage.retrieve_snapshot("snapshot123")
            
            # Assert
            assert retrieved_data == test_data
    
    async def test_multipart_upload_threshold(self, s3_storage):
        """Test multipart upload for large snapshots"""
        with patch.object(s3_storage.session, 'client') as mock_client:
            mock_s3 = AsyncMock()
            mock_client.return_value.__aenter__.return_value = mock_s3
            
            # Create large data that exceeds multipart threshold
            large_data = b"x" * (s3_storage.multipart_threshold + 1)
            
            # Mock multipart upload responses
            mock_s3.create_multipart_upload.return_value = {'UploadId': 'test-upload-id'}
            mock_s3.upload_part.return_value = {'ETag': 'test-etag'}
            
            # Act
            await s3_storage.store_snapshot("large_snapshot", large_data)
            
            # Assert multipart upload was used
            mock_s3.create_multipart_upload.assert_called_once()
            mock_s3.complete_multipart_upload.assert_called_once()
```

## Security Checklist for Storage Backend

### Storage Access Control
- [ ] S3 bucket access policies restricting unauthorized access
- [ ] IAM roles and policies for service account access
- [ ] Storage credential encryption and secure management
- [ ] Network access controls for MinIO/S3 endpoints
- [ ] Bucket enumeration protection with private bucket configuration
- [ ] Cross-origin resource sharing (CORS) restrictions
- [ ] Storage operation audit logging with detailed access records
- [ ] Rate limiting on storage operations to prevent abuse
- [ ] Storage quota enforcement per user and tenant
- [ ] Secure deletion with data wiping for sensitive snapshots

### Data Protection in Storage
- [ ] Snapshot data encryption at rest with AES-256
- [ ] Snapshot data encryption in transit with TLS 1.3
- [ ] Checksum validation for data integrity verification
- [ ] Backup encryption with separate key management
- [ ] Data retention policies with automatic expiration
- [ ] Geographic data residency compliance
- [ ] Data classification and handling based on sensitivity
- [ ] Protection against data corruption with redundancy
- [ ] Secure key rotation for encryption keys
- [ ] Compliance with data protection regulations (GDPR, etc.)

### Storage Security Monitoring
- [ ] Storage access monitoring with anomaly detection
- [ ] Failed access attempt alerting and investigation
- [ ] Storage usage monitoring for capacity planning
- [ ] Performance monitoring for storage operations
- [ ] Security event correlation with user activities
- [ ] Storage backup verification and testing
- [ ] Incident response procedures for storage compromises
- [ ] Regular security assessments of storage infrastructure
- [ ] Vendor security compliance validation for cloud storage
- [ ] Storage disaster recovery testing and validation

## Performance Requirements

### Storage Operations
- Snapshot upload speed > 50 MB/s to MinIO
- Snapshot download speed > 100 MB/s from MinIO
- Small metadata operations < 100ms response time
- Large snapshot uploads (>100MB) using multipart upload
- Concurrent storage operations (10+ simultaneous)
- Storage operation retry with exponential backoff

### Deduplication Performance
- Chunk processing speed > 100 MB/s
- Deduplication ratio > 30% for typical VM snapshots
- Chunk index lookup < 10ms per chunk
- Manifest generation < 500ms for 1GB snapshot
- Memory usage < 100MB for deduplication engine
- Chunk size optimization for storage efficiency

## Error Handling and Recovery

### Storage Failure Scenarios
- Network connectivity issues with automatic retry
- S3/MinIO service unavailability with graceful degradation
- Partial upload failures with cleanup and retry
- Data corruption detection with integrity verification
- Storage quota exceeded with user notification
- Authentication/authorization failures with proper error handling

### Recovery Mechanisms
- Automatic retry with exponential backoff
- Partial upload cleanup on failures
- Connection pooling with health checks
- Circuit breaker pattern for service failures
- Fallback storage backends for high availability
- Data recovery from backup storage

## MinIO Configuration

### MinIO Setup for Development
```yaml
# docker-compose.yml for MinIO
version: '3.8'
services:
  minio:
    image: minio/minio:latest
    command: server /data --console-address ":9001"
    environment:
      MINIO_ROOT_USER: minioadmin
      MINIO_ROOT_PASSWORD: minioadmin123
    ports:
      - "9000:9000"
      - "9001:9001"
    volumes:
      - minio_data:/data
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:9000/minio/health/live"]
      interval: 30s
      timeout: 20s
      retries: 3

volumes:
  minio_data:
```

### MinIO Production Configuration
```bash
# MinIO production setup with clustering
export MINIO_ROOT_USER=admin
export MINIO_ROOT_PASSWORD=secure_password_here
export MINIO_SERVER_URL=https://minio.getbuild.ing

# Start MinIO cluster
minio server https://minio{1...4}.getbuild.ing/data{1...2}
```

## Integration Testing

### MinIO Integration Tests
```python
async def test_minio_integration(s3_storage):
    """Test actual MinIO integration"""
    # This test requires running MinIO instance
    if not os.getenv('INTEGRATION_TESTS'):
        pytest.skip("Integration tests not enabled")
    
    snapshot_id = "integration_test_snapshot"
    test_data = b"integration test data" * 1000
    
    # Store snapshot
    storage_path = await s3_storage.store_snapshot(snapshot_id, test_data)
    assert storage_path is not None
    
    # Retrieve snapshot
    retrieved_data = await s3_storage.retrieve_snapshot(snapshot_id)
    assert retrieved_data == test_data
    
    # Delete snapshot
    deleted = await s3_storage.delete_snapshot(snapshot_id)
    assert deleted is True
```

## Monitoring and Observability

### Storage Metrics Collection
- Upload/download throughput monitoring
- Error rate tracking by operation type
- Storage space utilization tracking
- Deduplication effectiveness measurement
- Operation latency percentile tracking
- Connection pool utilization monitoring

### Logfire Integration
```python
# Storage operation logging
async def log_storage_operation(operation: str, duration_ms: float, 
                               size_bytes: int, success: bool):
    """Log storage operations to Logfire"""
    logfire.info(
        f"Storage operation: {operation}",
        operation=operation,
        duration_ms=duration_ms,
        size_bytes=size_bytes,
        success=success,
        throughput_mbps=(size_bytes / (1024 * 1024)) / (duration_ms / 1000) if duration_ms > 0 else 0
    )
```

## Next Implementation Steps

1. **Complete S3 storage backend** with all CRUD operations
2. **Implement deduplication engine** for storage optimization
3. **Add compression support** for snapshot data
4. **Create storage quota management** with user limits
5. **Add comprehensive error handling** with retry mechanisms
6. **Implement storage monitoring** with Logfire integration
7. **Create integration tests** with actual MinIO instance

## Commit Guidelines

Storage commits should include:
- **Comprehensive error handling** for all failure scenarios
- **Performance optimization** with multipart uploads and caching
- **Security validation** for storage access and data protection
- **Test coverage** for storage operations and error conditions
- **Integration verification** with MinIO and snapshot manager
- **Documentation updates** for storage configuration and procedures