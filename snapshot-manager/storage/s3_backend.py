"""
MinIO S3-compatible storage backend for VM snapshots.

Provides efficient storage, retrieval, and management of snapshot data
with multipart uploads, compression, and comprehensive error handling.
"""

import asyncio
import hashlib
import time
import json
from typing import Dict, Any, Optional, List, BinaryIO
from dataclasses import dataclass
from pathlib import Path
import aiofiles
import aioboto3
import structlog
import logfire
from botocore.exceptions import ClientError, NoCredentialsError

logger = structlog.get_logger()


@dataclass
class StorageMetrics:
    """Storage operation performance metrics."""
    operation: str
    duration_ms: float
    size_bytes: int
    success: bool
    error_message: Optional[str] = None


class S3StorageBackend:
    """
    S3-compatible storage backend for VM snapshots.
    
    Provides high-performance snapshot storage with multipart uploads,
    error handling, and comprehensive monitoring integration.
    """

    def __init__(self, endpoint_url: str, access_key: str, secret_key: str,
                 bucket_name: str, region: str = "us-east-1", 
                 enable_encryption: bool = True):
        """Initialize S3 storage backend."""
        self.endpoint_url = endpoint_url
        self.access_key = access_key
        self.secret_key = secret_key
        self.bucket_name = bucket_name
        self.region = region
        self.session = None
        
        # Encryption configuration (disable for local MinIO without KMS)
        self.enable_encryption = enable_encryption
        
        # Storage configuration
        self.multipart_threshold = 100 * 1024 * 1024  # 100MB
        self.multipart_chunksize = 10 * 1024 * 1024   # 10MB
        self.max_retries = 3
        self.retry_delay = 1.0
        
        # Performance tracking
        self.metrics: List[StorageMetrics] = []

    async def initialize(self):
        """Initialize S3 storage backend and ensure bucket exists."""
        try:
            self.session = aioboto3.Session()
            
            # Test connection and ensure bucket exists
            await self._test_connection()
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
        """
        Store snapshot data in S3 with multipart upload support.
        
        Args:
            snapshot_id: Unique identifier for the snapshot
            data: Binary snapshot data to store
            metadata: Optional metadata dictionary
            
        Returns:
            str: S3 storage path for the snapshot
        """
        start_time = time.time()
        
        try:
            # Generate storage key with hierarchical structure
            storage_key = self._generate_storage_key(snapshot_id)
            
            # Prepare metadata for S3
            s3_metadata = self._prepare_s3_metadata(metadata or {})
            
            # Calculate and store checksum
            checksum = hashlib.sha256(data).hexdigest()
            s3_metadata['checksum'] = checksum
            s3_metadata['upload_timestamp'] = str(int(time.time()))
            
            async with self.session.client(
                's3',
                endpoint_url=self.endpoint_url,
                aws_access_key_id=self.access_key,
                aws_secret_access_key=self.secret_key,
                region_name=self.region
            ) as s3:
                
                if len(data) > self.multipart_threshold:
                    # Use multipart upload for large snapshots
                    storage_path = await self._multipart_upload(
                        s3, storage_key, data, s3_metadata
                    )
                else:
                    # Use simple upload for smaller snapshots
                    put_object_args = {
                        'Bucket': self.bucket_name,
                        'Key': storage_key,
                        'Body': data,
                        'Metadata': s3_metadata,
                        'ContentType': 'application/octet-stream'
                    }
                    
                    # Only add encryption if enabled (for production S3/MinIO with KMS)
                    if self.enable_encryption:
                        put_object_args['ServerSideEncryption'] = 'AES256'
                    
                    await s3.put_object(**put_object_args)
                    storage_path = f"s3://{self.bucket_name}/{storage_key}"
            
            # Record performance metrics
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
                        checksum=checksum,
                        throughput_mbps=(len(data) / (1024 * 1024)) / (duration_ms / 1000) if duration_ms > 0 else 0)
            
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
        """
        Retrieve snapshot data from S3 storage.
        
        Args:
            snapshot_id: Unique identifier for the snapshot
            
        Returns:
            bytes: Binary snapshot data
        """
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
                        raise ValueError("Snapshot data corruption detected - checksum mismatch")
            
            # Record performance metrics
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
                        duration_ms=duration_ms,
                        throughput_mbps=(len(data) / (1024 * 1024)) / (duration_ms / 1000) if duration_ms > 0 else 0)
            
            return data
            
        except ClientError as e:
            if e.response['Error']['Code'] == 'NoSuchKey':
                logger.warning("Snapshot not found in storage", 
                              snapshot_id=snapshot_id)
                raise FileNotFoundError(f"Snapshot {snapshot_id} not found in storage")
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
        """
        Delete snapshot data from S3 storage.
        
        Args:
            snapshot_id: Unique identifier for the snapshot
            
        Returns:
            bool: True if deletion successful, False otherwise
        """
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

    # Private helper methods

    def _generate_storage_key(self, snapshot_id: str) -> str:
        """Generate hierarchical storage key for snapshot."""
        # Extract user info from snapshot ID for organization
        # Format: snapshots/{snapshot_id}/data
        return f"snapshots/{snapshot_id}/data"

    def _prepare_s3_metadata(self, metadata: Dict[str, Any]) -> Dict[str, str]:
        """Prepare metadata dictionary for S3 storage."""
        s3_metadata = {}
        for key, value in metadata.items():
            # S3 metadata must be string values
            if isinstance(value, (str, int, float, bool)):
                s3_metadata[str(key)] = str(value)
        return s3_metadata

    async def _multipart_upload(self, s3_client, storage_key: str, 
                              data: bytes, metadata: Dict[str, str]) -> str:
        """Perform multipart upload for large snapshots."""
        upload_id = None
        
        try:
            # Initiate multipart upload
            create_upload_args = {
                'Bucket': self.bucket_name,
                'Key': storage_key,
                'Metadata': metadata,
                'ContentType': 'application/octet-stream'
            }
            
            # Only add encryption if enabled (for production S3/MinIO with KMS)
            if self.enable_encryption:
                create_upload_args['ServerSideEncryption'] = 'AES256'
            
            response = await s3_client.create_multipart_upload(**create_upload_args)
            
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
                
                logger.debug("Uploaded multipart chunk", 
                           part_number=part_number-1,
                           chunk_size=len(part_data))
            
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
            if upload_id:
                try:
                    await s3_client.abort_multipart_upload(
                        Bucket=self.bucket_name,
                        Key=storage_key,
                        UploadId=upload_id
                    )
                    logger.info("Aborted failed multipart upload", 
                               upload_id=upload_id)
                except Exception:
                    pass  # Best effort cleanup
            
            logger.error("Multipart upload failed", 
                        storage_key=storage_key, error=str(e))
            raise

    async def _test_connection(self):
        """Test connection to S3 storage."""
        try:
            async with self.session.client(
                's3',
                endpoint_url=self.endpoint_url,
                aws_access_key_id=self.access_key,
                aws_secret_access_key=self.secret_key,
                region_name=self.region
            ) as s3:
                # Simple operation to test connectivity
                await s3.list_buckets()
                
        except Exception as e:
            logger.error("S3 connection test failed", error=str(e))
            raise ConnectionError(f"Cannot connect to S3 storage: {e}")

    async def _ensure_bucket_exists(self):
        """Ensure the storage bucket exists, create if necessary."""
        try:
            async with self.session.client(
                's3',
                endpoint_url=self.endpoint_url,
                aws_access_key_id=self.access_key,
                aws_secret_access_key=self.secret_key,
                region_name=self.region
            ) as s3:
                
                try:
                    await s3.head_bucket(Bucket=self.bucket_name)
                    logger.debug("Storage bucket exists", bucket=self.bucket_name)
                except ClientError as e:
                    if e.response['Error']['Code'] == '404':
                        # Bucket doesn't exist, create it
                        await s3.create_bucket(Bucket=self.bucket_name)
                        logger.info("Created storage bucket", bucket=self.bucket_name)
                    else:
                        raise
                        
        except Exception as e:
            logger.error("Failed to ensure bucket exists", 
                        bucket=self.bucket_name, error=str(e))
            raise

    def _record_metrics(self, metrics: StorageMetrics):
        """Record storage performance metrics."""
        self.metrics.append(metrics)
        
        # Keep only recent metrics (last 1000 operations)
        if len(self.metrics) > 1000:
            self.metrics = self.metrics[-1000:]
        
        # Log performance metrics
        if metrics.success:
            logger.debug("Storage operation completed", 
                        operation=metrics.operation,
                        duration_ms=metrics.duration_ms,
                        size_bytes=metrics.size_bytes)
        else:
            logger.warning("Storage operation failed", 
                          operation=metrics.operation,
                          duration_ms=metrics.duration_ms,
                          error=metrics.error_message)

    def get_performance_stats(self) -> Dict[str, Any]:
        """Get storage performance statistics."""
        if not self.metrics:
            return {}
        
        successful_ops = [m for m in self.metrics if m.success]
        failed_ops = [m for m in self.metrics if not m.success]
        
        stats = {
            "total_operations": len(self.metrics),
            "successful_operations": len(successful_ops),
            "failed_operations": len(failed_ops),
            "success_rate": len(successful_ops) / len(self.metrics) * 100,
        }
        
        if successful_ops:
            durations = [m.duration_ms for m in successful_ops]
            sizes = [m.size_bytes for m in successful_ops if m.size_bytes > 0]
            
            stats.update({
                "avg_duration_ms": sum(durations) / len(durations),
                "max_duration_ms": max(durations),
                "min_duration_ms": min(durations),
            })
            
            if sizes:
                stats.update({
                    "avg_size_bytes": sum(sizes) / len(sizes),
                    "total_bytes_processed": sum(sizes),
                })
        
        return stats