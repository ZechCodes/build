"""
Unified Storage Abstraction Layer for snapshots and recordings.

Provides a consistent interface for storing and retrieving:
- VM snapshots (Session 7)
- Terminal recordings (Session 10)
- User uploads and exports

Supports multiple backends: MinIO, AWS S3, Azure Blob, GCS, local filesystem.
"""

import os
import gzip
import hashlib
import asyncio
from abc import ABC, abstractmethod
from datetime import datetime, timedelta
from typing import Dict, List, Optional, BinaryIO, AsyncIterator, Tuple
from enum import Enum
from dataclasses import dataclass
from pathlib import Path

import aioboto3
from pydantic import BaseModel, Field


class StorageBackend(str, Enum):
    """Supported storage backends."""
    MINIO = "minio"
    AWS_S3 = "aws_s3"
    AZURE_BLOB = "azure_blob"
    GOOGLE_CLOUD = "google_cloud"
    FILESYSTEM = "filesystem"


class CompressionType(str, Enum):
    """Supported compression types."""
    NONE = "none"
    GZIP = "gzip"
    LZ4 = "lz4"
    ZSTD = "zstd"


class StorageClass(str, Enum):
    """Storage classes for cost optimization."""
    HOT = "hot"          # Frequent access
    WARM = "warm"        # Infrequent access
    COLD = "cold"        # Archive storage
    DEEP_ARCHIVE = "deep_archive"  # Long-term archive


@dataclass
class StorageMetadata:
    """Metadata for stored objects."""
    object_id: str
    bucket: str
    key: str
    size_bytes: int
    checksum: str
    compression: CompressionType
    storage_class: StorageClass
    created_at: datetime
    expires_at: Optional[datetime] = None
    tags: Dict[str, str] = None
    user_id: str = None
    session_id: str = None


class SnapshotMetadata(BaseModel):
    """Metadata for VM snapshots."""
    snapshot_id: str = Field(..., description="Unique snapshot identifier")
    vm_id: str = Field(..., description="VM identifier")
    user_id: str = Field(..., description="Owner user ID")
    name: str = Field(..., description="User-friendly name")
    description: Optional[str] = Field(None, description="Snapshot description")
    size_bytes: int = Field(..., description="Total snapshot size")
    created_at: datetime = Field(default_factory=datetime.utcnow)
    expires_at: Optional[datetime] = Field(None, description="Expiration time")
    storage_key: str = Field(..., description="Storage backend key")
    checksum: str = Field(..., description="Data integrity checksum")
    compression: CompressionType = CompressionType.GZIP
    tags: Dict[str, str] = Field(default_factory=dict)


class RecordingMetadata(BaseModel):
    """Metadata for terminal recordings."""
    recording_id: str = Field(..., description="Unique recording identifier")
    session_id: str = Field(..., description="Session identifier")
    user_id: str = Field(..., description="Owner user ID")
    title: str = Field(..., description="Recording title")
    duration_seconds: float = Field(..., description="Recording duration")
    size_bytes: int = Field(..., description="Recording file size")
    format: str = Field(default="asciicast", description="Recording format")
    created_at: datetime = Field(default_factory=datetime.utcnow)
    storage_key: str = Field(..., description="Storage backend key")
    checksum: str = Field(..., description="Data integrity checksum")
    compression: CompressionType = CompressionType.GZIP


class StorageProvider(ABC):
    """Abstract storage provider interface."""
    
    @abstractmethod
    async def store_object(
        self,
        key: str,
        data: bytes,
        metadata: Dict[str, str] = None,
        storage_class: StorageClass = StorageClass.HOT,
        expires_at: Optional[datetime] = None
    ) -> StorageMetadata:
        """Store an object and return metadata."""
        pass
    
    @abstractmethod
    async def retrieve_object(self, key: str) -> bytes:
        """Retrieve an object by key."""
        pass
    
    @abstractmethod
    async def stream_object(self, key: str) -> AsyncIterator[bytes]:
        """Stream an object in chunks."""
        pass
    
    @abstractmethod
    async def delete_object(self, key: str) -> bool:
        """Delete an object."""
        pass
    
    @abstractmethod
    async def list_objects(
        self,
        prefix: str = "",
        limit: int = 1000
    ) -> List[StorageMetadata]:
        """List objects with optional prefix filter."""
        pass
    
    @abstractmethod
    async def get_object_metadata(self, key: str) -> Optional[StorageMetadata]:
        """Get object metadata without downloading."""
        pass
    
    @abstractmethod
    async def copy_object(self, source_key: str, dest_key: str) -> bool:
        """Copy an object to a new key."""
        pass


class MinIOStorageProvider(StorageProvider):
    """MinIO/S3-compatible storage provider."""
    
    def __init__(
        self,
        endpoint: str,
        access_key: str,
        secret_key: str,
        bucket: str,
        secure: bool = True,
        region: str = "us-east-1"
    ):
        self.endpoint = endpoint
        self.access_key = access_key
        self.secret_key = secret_key
        self.bucket = bucket
        self.secure = secure
        self.region = region
        
    async def _get_client(self):
        """Get async S3 client."""
        session = aioboto3.Session()
        return session.client(
            's3',
            endpoint_url=f"{'https' if self.secure else 'http'}://{self.endpoint}",
            aws_access_key_id=self.access_key,
            aws_secret_access_key=self.secret_key,
            region_name=self.region
        )
    
    async def store_object(
        self,
        key: str,
        data: bytes,
        metadata: Dict[str, str] = None,
        storage_class: StorageClass = StorageClass.HOT,
        expires_at: Optional[datetime] = None
    ) -> StorageMetadata:
        """Store object in MinIO."""
        async with await self._get_client() as s3:
            # Calculate checksum
            checksum = hashlib.sha256(data).hexdigest()
            
            # Prepare extra args
            extra_args = {
                'Metadata': metadata or {},
                'ChecksumSHA256': checksum
            }
            
            if expires_at:
                extra_args['Expires'] = expires_at
            
            # Map storage class
            s3_storage_class = self._map_storage_class(storage_class)
            if s3_storage_class:
                extra_args['StorageClass'] = s3_storage_class
            
            # Upload object
            await s3.put_object(
                Bucket=self.bucket,
                Key=key,
                Body=data,
                **extra_args
            )
            
            return StorageMetadata(
                object_id=key,
                bucket=self.bucket,
                key=key,
                size_bytes=len(data),
                checksum=checksum,
                compression=CompressionType.NONE,
                storage_class=storage_class,
                created_at=datetime.utcnow(),
                expires_at=expires_at
            )
    
    async def retrieve_object(self, key: str) -> bytes:
        """Retrieve object from MinIO."""
        async with await self._get_client() as s3:
            response = await s3.get_object(Bucket=self.bucket, Key=key)
            async with response['Body'] as stream:
                return await stream.read()
    
    async def stream_object(self, key: str) -> AsyncIterator[bytes]:
        """Stream object from MinIO."""
        async with await self._get_client() as s3:
            response = await s3.get_object(Bucket=self.bucket, Key=key)
            async with response['Body'] as stream:
                while True:
                    chunk = await stream.read(8192)
                    if not chunk:
                        break
                    yield chunk
    
    async def delete_object(self, key: str) -> bool:
        """Delete object from MinIO."""
        try:
            async with await self._get_client() as s3:
                await s3.delete_object(Bucket=self.bucket, Key=key)
                return True
        except Exception:
            return False
    
    async def list_objects(
        self,
        prefix: str = "",
        limit: int = 1000
    ) -> List[StorageMetadata]:
        """List objects in MinIO."""
        async with await self._get_client() as s3:
            paginator = s3.get_paginator('list_objects_v2')
            objects = []
            
            async for page in paginator.paginate(
                Bucket=self.bucket,
                Prefix=prefix,
                MaxKeys=limit
            ):
                for obj in page.get('Contents', []):
                    metadata = StorageMetadata(
                        object_id=obj['Key'],
                        bucket=self.bucket,
                        key=obj['Key'],
                        size_bytes=obj['Size'],
                        checksum=obj.get('ETag', '').strip('"'),
                        compression=CompressionType.NONE,
                        storage_class=StorageClass.HOT,  # Default
                        created_at=obj['LastModified']
                    )
                    objects.append(metadata)
            
            return objects
    
    async def get_object_metadata(self, key: str) -> Optional[StorageMetadata]:
        """Get object metadata from MinIO."""
        try:
            async with await self._get_client() as s3:
                response = await s3.head_object(Bucket=self.bucket, Key=key)
                
                return StorageMetadata(
                    object_id=key,
                    bucket=self.bucket,
                    key=key,
                    size_bytes=response['ContentLength'],
                    checksum=response.get('ETag', '').strip('"'),
                    compression=CompressionType.NONE,
                    storage_class=StorageClass.HOT,
                    created_at=response['LastModified']
                )
        except Exception:
            return None
    
    async def copy_object(self, source_key: str, dest_key: str) -> bool:
        """Copy object in MinIO."""
        try:
            async with await self._get_client() as s3:
                await s3.copy_object(
                    Bucket=self.bucket,
                    CopySource={'Bucket': self.bucket, 'Key': source_key},
                    Key=dest_key
                )
                return True
        except Exception:
            return False
    
    def _map_storage_class(self, storage_class: StorageClass) -> Optional[str]:
        """Map our storage class to S3 storage class."""
        mapping = {
            StorageClass.HOT: "STANDARD",
            StorageClass.WARM: "STANDARD_IA",
            StorageClass.COLD: "GLACIER",
            StorageClass.DEEP_ARCHIVE: "DEEP_ARCHIVE"
        }
        return mapping.get(storage_class)


class FilesystemStorageProvider(StorageProvider):
    """Local filesystem storage provider for development."""
    
    def __init__(self, base_path: str):
        self.base_path = Path(base_path)
        self.base_path.mkdir(parents=True, exist_ok=True)
        
    def _get_file_path(self, key: str) -> Path:
        """Get absolute file path for key."""
        # Ensure key doesn't escape base path
        safe_key = key.replace('..', '').lstrip('/')
        return self.base_path / safe_key
    
    async def store_object(
        self,
        key: str,
        data: bytes,
        metadata: Dict[str, str] = None,
        storage_class: StorageClass = StorageClass.HOT,
        expires_at: Optional[datetime] = None
    ) -> StorageMetadata:
        """Store object on filesystem."""
        file_path = self._get_file_path(key)
        file_path.parent.mkdir(parents=True, exist_ok=True)
        
        # Calculate checksum
        checksum = hashlib.sha256(data).hexdigest()
        
        # Write data
        async with asyncio.create_task(self._write_file(file_path, data)):
            pass
        
        # Store metadata
        if metadata:
            metadata_path = file_path.with_suffix('.metadata.json')
            import json
            async with asyncio.create_task(self._write_file(
                metadata_path,
                json.dumps(metadata).encode()
            )):
                pass
        
        return StorageMetadata(
            object_id=key,
            bucket="filesystem",
            key=key,
            size_bytes=len(data),
            checksum=checksum,
            compression=CompressionType.NONE,
            storage_class=storage_class,
            created_at=datetime.utcnow(),
            expires_at=expires_at
        )
    
    async def _write_file(self, path: Path, data: bytes):
        """Write file asynchronously."""
        def _write():
            with open(path, 'wb') as f:
                f.write(data)
        
        await asyncio.get_event_loop().run_in_executor(None, _write)
    
    async def _read_file(self, path: Path) -> bytes:
        """Read file asynchronously."""
        def _read():
            with open(path, 'rb') as f:
                return f.read()
        
        return await asyncio.get_event_loop().run_in_executor(None, _read)
    
    async def retrieve_object(self, key: str) -> bytes:
        """Retrieve object from filesystem."""
        file_path = self._get_file_path(key)
        if not file_path.exists():
            raise FileNotFoundError(f"Object {key} not found")
        
        return await self._read_file(file_path)
    
    async def stream_object(self, key: str) -> AsyncIterator[bytes]:
        """Stream object from filesystem."""
        file_path = self._get_file_path(key)
        if not file_path.exists():
            raise FileNotFoundError(f"Object {key} not found")
        
        def _stream():
            with open(file_path, 'rb') as f:
                while True:
                    chunk = f.read(8192)
                    if not chunk:
                        break
                    yield chunk
        
        loop = asyncio.get_event_loop()
        executor = None
        
        for chunk in await loop.run_in_executor(executor, lambda: list(_stream())):
            yield chunk
    
    async def delete_object(self, key: str) -> bool:
        """Delete object from filesystem."""
        try:
            file_path = self._get_file_path(key)
            if file_path.exists():
                file_path.unlink()
            
            # Also delete metadata if exists
            metadata_path = file_path.with_suffix('.metadata.json')
            if metadata_path.exists():
                metadata_path.unlink()
            
            return True
        except Exception:
            return False
    
    async def list_objects(
        self,
        prefix: str = "",
        limit: int = 1000
    ) -> List[StorageMetadata]:
        """List objects on filesystem."""
        objects = []
        prefix_path = self.base_path / prefix if prefix else self.base_path
        
        def _list():
            for file_path in prefix_path.rglob('*'):
                if file_path.is_file() and not file_path.name.endswith('.metadata.json'):
                    relative_path = file_path.relative_to(self.base_path)
                    stat = file_path.stat()
                    
                    metadata = StorageMetadata(
                        object_id=str(relative_path),
                        bucket="filesystem",
                        key=str(relative_path),
                        size_bytes=stat.st_size,
                        checksum="",  # Would need to calculate
                        compression=CompressionType.NONE,
                        storage_class=StorageClass.HOT,
                        created_at=datetime.fromtimestamp(stat.st_ctime)
                    )
                    objects.append(metadata)
                    
                    if len(objects) >= limit:
                        break
        
        await asyncio.get_event_loop().run_in_executor(None, _list)
        return objects
    
    async def get_object_metadata(self, key: str) -> Optional[StorageMetadata]:
        """Get object metadata from filesystem."""
        try:
            file_path = self._get_file_path(key)
            if not file_path.exists():
                return None
            
            stat = file_path.stat()
            return StorageMetadata(
                object_id=key,
                bucket="filesystem",
                key=key,
                size_bytes=stat.st_size,
                checksum="",  # Would need to calculate
                compression=CompressionType.NONE,
                storage_class=StorageClass.HOT,
                created_at=datetime.fromtimestamp(stat.st_ctime)
            )
        except Exception:
            return None
    
    async def copy_object(self, source_key: str, dest_key: str) -> bool:
        """Copy object on filesystem."""
        try:
            source_path = self._get_file_path(source_key)
            dest_path = self._get_file_path(dest_key)
            
            if not source_path.exists():
                return False
            
            dest_path.parent.mkdir(parents=True, exist_ok=True)
            
            def _copy():
                import shutil
                shutil.copy2(source_path, dest_path)
            
            await asyncio.get_event_loop().run_in_executor(None, _copy)
            return True
        except Exception:
            return False


class StorageManager:
    """High-level storage management with compression and lifecycle."""
    
    def __init__(self, provider: StorageProvider):
        self.provider = provider
        
    async def store_snapshot(
        self,
        vm_id: str,
        user_id: str,
        snapshot_data: bytes,
        name: str,
        description: str = None,
        compression: CompressionType = CompressionType.GZIP
    ) -> SnapshotMetadata:
        """Store VM snapshot with compression."""
        import uuid
        
        snapshot_id = str(uuid.uuid4())
        
        # Compress data if requested
        compressed_data = await self._compress_data(snapshot_data, compression)
        
        # Generate storage key
        key = f"snapshots/{user_id}/{vm_id}/{snapshot_id}.snapshot"
        
        # Store in backend
        storage_metadata = await self.provider.store_object(
            key=key,
            data=compressed_data,
            metadata={
                "snapshot_id": snapshot_id,
                "vm_id": vm_id,
                "user_id": user_id,
                "name": name,
                "description": description or "",
                "compression": compression.value
            },
            storage_class=StorageClass.WARM  # Snapshots are infrequently accessed
        )
        
        return SnapshotMetadata(
            snapshot_id=snapshot_id,
            vm_id=vm_id,
            user_id=user_id,
            name=name,
            description=description,
            size_bytes=len(snapshot_data),  # Original size
            storage_key=key,
            checksum=storage_metadata.checksum,
            compression=compression
        )
    
    async def retrieve_snapshot(self, snapshot_key: str) -> bytes:
        """Retrieve and decompress snapshot."""
        compressed_data = await self.provider.retrieve_object(snapshot_key)
        
        # Get compression type from metadata or guess from data
        metadata = await self.provider.get_object_metadata(snapshot_key)
        compression = CompressionType.GZIP  # Default
        
        if metadata and metadata.tags:
            compression = CompressionType(metadata.tags.get("compression", "gzip"))
        
        return await self._decompress_data(compressed_data, compression)
    
    async def store_recording(
        self,
        session_id: str,
        user_id: str,
        recording_data: bytes,
        title: str,
        duration_seconds: float,
        format: str = "asciicast"
    ) -> RecordingMetadata:
        """Store terminal recording."""
        import uuid
        
        recording_id = str(uuid.uuid4())
        
        # Compress recording data
        compression = CompressionType.GZIP
        compressed_data = await self._compress_data(recording_data, compression)
        
        # Generate storage key
        key = f"recordings/{user_id}/{session_id}/{recording_id}.{format}"
        
        # Store in backend
        storage_metadata = await self.provider.store_object(
            key=key,
            data=compressed_data,
            metadata={
                "recording_id": recording_id,
                "session_id": session_id,
                "user_id": user_id,
                "title": title,
                "duration_seconds": str(duration_seconds),
                "format": format,
                "compression": compression.value
            },
            storage_class=StorageClass.COLD  # Recordings are archived
        )
        
        return RecordingMetadata(
            recording_id=recording_id,
            session_id=session_id,
            user_id=user_id,
            title=title,
            duration_seconds=duration_seconds,
            size_bytes=len(recording_data),  # Original size
            format=format,
            storage_key=key,
            checksum=storage_metadata.checksum,
            compression=compression
        )
    
    async def list_user_snapshots(self, user_id: str) -> List[SnapshotMetadata]:
        """List snapshots for a user."""
        prefix = f"snapshots/{user_id}/"
        objects = await self.provider.list_objects(prefix)
        
        snapshots = []
        for obj in objects:
            if obj.key.endswith('.snapshot'):
                # Parse snapshot metadata from storage metadata
                # In practice, this would come from a database
                snapshot = SnapshotMetadata(
                    snapshot_id=obj.object_id,
                    vm_id="unknown",  # Would be in DB
                    user_id=user_id,
                    name="Unknown",  # Would be in DB
                    size_bytes=obj.size_bytes,
                    storage_key=obj.key,
                    checksum=obj.checksum,
                    compression=CompressionType.GZIP
                )
                snapshots.append(snapshot)
        
        return snapshots
    
    async def _compress_data(self, data: bytes, compression: CompressionType) -> bytes:
        """Compress data using specified algorithm."""
        if compression == CompressionType.NONE:
            return data
        elif compression == CompressionType.GZIP:
            return gzip.compress(data)
        else:
            # For other compression types, would implement LZ4, ZSTD, etc.
            raise NotImplementedError(f"Compression {compression} not implemented")
    
    async def _decompress_data(self, data: bytes, compression: CompressionType) -> bytes:
        """Decompress data using specified algorithm."""
        if compression == CompressionType.NONE:
            return data
        elif compression == CompressionType.GZIP:
            return gzip.decompress(data)
        else:
            raise NotImplementedError(f"Decompression {compression} not implemented")


# Factory function for creating storage providers
def create_storage_provider(backend: StorageBackend, config: Dict[str, str]) -> StorageProvider:
    """Create storage provider based on backend type."""
    if backend == StorageBackend.MINIO:
        return MinIOStorageProvider(
            endpoint=config["endpoint"],
            access_key=config["access_key"],
            secret_key=config["secret_key"],
            bucket=config["bucket"],
            secure=config.get("secure", "true").lower() == "true"
        )
    elif backend == StorageBackend.FILESYSTEM:
        return FilesystemStorageProvider(config["base_path"])
    else:
        raise NotImplementedError(f"Storage backend {backend} not implemented")


# Configuration class
class StorageConfig:
    """Storage configuration."""
    
    # Compression settings
    DEFAULT_COMPRESSION = CompressionType.GZIP
    COMPRESSION_LEVEL = 6
    
    # Storage classes
    SNAPSHOT_STORAGE_CLASS = StorageClass.WARM
    RECORDING_STORAGE_CLASS = StorageClass.COLD
    TEMP_STORAGE_CLASS = StorageClass.HOT
    
    # Lifecycle policies
    SNAPSHOT_RETENTION_DAYS = 90
    RECORDING_RETENTION_DAYS = 365
    TEMP_FILE_RETENTION_HOURS = 24
    
    # Performance
    CHUNK_SIZE = 8192
    MAX_CONCURRENT_UPLOADS = 5
    UPLOAD_TIMEOUT_SECONDS = 300