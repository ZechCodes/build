"""Storage service for snapshot management."""

import asyncio
import hashlib
from abc import ABC, abstractmethod
from typing import Optional
from pathlib import Path

import aioboto3
import structlog

from app.core.config import get_settings

logger = structlog.get_logger(__name__)
settings = get_settings()


class StorageBackend(ABC):
    """Abstract storage backend interface."""
    
    @abstractmethod
    async def upload_file(self, key: str, data: bytes) -> str:
        """Upload file and return storage path."""
        pass
    
    @abstractmethod
    async def download_file(self, storage_path: str) -> bytes:
        """Download file from storage path."""
        pass
    
    @abstractmethod
    async def delete_file(self, storage_path: str) -> bool:
        """Delete file from storage."""
        pass


class MinIOStorageBackend(StorageBackend):
    """MinIO/S3-compatible storage backend."""
    
    def __init__(
        self,
        endpoint: str,
        access_key: str,
        secret_key: str,
        bucket_name: str,
        secure: bool = False
    ):
        self.endpoint = endpoint
        self.access_key = access_key
        self.secret_key = secret_key
        self.bucket_name = bucket_name
        self.secure = secure
        
    async def _get_client(self):
        """Get async S3 client."""
        session = aioboto3.Session()
        return session.client(
            's3',
            endpoint_url=f"{'https' if self.secure else 'http'}://{self.endpoint}",
            aws_access_key_id=self.access_key,
            aws_secret_access_key=self.secret_key
        )
    
    async def upload_file(self, key: str, data: bytes) -> str:
        """Upload file to MinIO."""
        try:
            async with await self._get_client() as s3:
                # Calculate checksum for integrity
                checksum = hashlib.sha256(data).hexdigest()
                
                await s3.put_object(
                    Bucket=self.bucket_name,
                    Key=key,
                    Body=data,
                    Metadata={
                        'checksum': checksum,
                        'size': str(len(data))
                    }
                )
                
                storage_path = f"s3://{self.bucket_name}/{key}"
                logger.info(
                    "File uploaded to MinIO",
                    key=key,
                    size_bytes=len(data),
                    checksum=checksum
                )
                
                return storage_path
                
        except Exception as e:
            logger.error("Failed to upload file to MinIO", key=key, error=str(e))
            raise
    
    async def download_file(self, storage_path: str) -> bytes:
        """Download file from MinIO."""
        try:
            # Extract key from storage path
            if storage_path.startswith(f"s3://{self.bucket_name}/"):
                key = storage_path[len(f"s3://{self.bucket_name}/"):]
            else:
                key = storage_path
            
            async with await self._get_client() as s3:
                response = await s3.get_object(Bucket=self.bucket_name, Key=key)
                data = await response['Body'].read()
                
                # Verify checksum if available
                metadata = response.get('Metadata', {})
                if 'checksum' in metadata:
                    expected_checksum = metadata['checksum']
                    actual_checksum = hashlib.sha256(data).hexdigest()
                    if expected_checksum != actual_checksum:
                        raise ValueError("Checksum verification failed")
                
                logger.info(
                    "File downloaded from MinIO",
                    key=key,
                    size_bytes=len(data)
                )
                
                return data
                
        except Exception as e:
            logger.error("Failed to download file from MinIO", storage_path=storage_path, error=str(e))
            raise
    
    async def delete_file(self, storage_path: str) -> bool:
        """Delete file from MinIO."""
        try:
            # Extract key from storage path
            if storage_path.startswith(f"s3://{self.bucket_name}/"):
                key = storage_path[len(f"s3://{self.bucket_name}/"):]
            else:
                key = storage_path
            
            async with await self._get_client() as s3:
                await s3.delete_object(Bucket=self.bucket_name, Key=key)
                
                logger.info("File deleted from MinIO", key=key)
                return True
                
        except Exception as e:
            logger.warning("Failed to delete file from MinIO", storage_path=storage_path, error=str(e))
            return False


class FilesystemStorageBackend(StorageBackend):
    """Local filesystem storage backend for development."""
    
    def __init__(self, base_path: str):
        self.base_path = Path(base_path)
        self.base_path.mkdir(parents=True, exist_ok=True)
        
    def _get_file_path(self, key: str) -> Path:
        """Get absolute file path for key."""
        # Ensure key doesn't escape base path
        safe_key = key.replace('..', '').lstrip('/')
        return self.base_path / safe_key
    
    async def upload_file(self, key: str, data: bytes) -> str:
        """Upload file to filesystem."""
        try:
            file_path = self._get_file_path(key)
            file_path.parent.mkdir(parents=True, exist_ok=True)
            
            # Write file asynchronously
            await asyncio.get_event_loop().run_in_executor(
                None, file_path.write_bytes, data
            )
            
            storage_path = f"file://{file_path}"
            logger.info(
                "File uploaded to filesystem",
                key=key,
                path=str(file_path),
                size_bytes=len(data)
            )
            
            return storage_path
            
        except Exception as e:
            logger.error("Failed to upload file to filesystem", key=key, error=str(e))
            raise
    
    async def download_file(self, storage_path: str) -> bytes:
        """Download file from filesystem."""
        try:
            # Extract path from storage path
            if storage_path.startswith("file://"):
                file_path = Path(storage_path[7:])
            else:
                file_path = self._get_file_path(storage_path)
            
            if not file_path.exists():
                raise FileNotFoundError(f"File not found: {file_path}")
            
            # Read file asynchronously
            data = await asyncio.get_event_loop().run_in_executor(
                None, file_path.read_bytes
            )
            
            logger.info(
                "File downloaded from filesystem",
                path=str(file_path),
                size_bytes=len(data)
            )
            
            return data
            
        except Exception as e:
            logger.error("Failed to download file from filesystem", storage_path=storage_path, error=str(e))
            raise
    
    async def delete_file(self, storage_path: str) -> bool:
        """Delete file from filesystem."""
        try:
            # Extract path from storage path
            if storage_path.startswith("file://"):
                file_path = Path(storage_path[7:])
            else:
                file_path = self._get_file_path(storage_path)
            
            if file_path.exists():
                await asyncio.get_event_loop().run_in_executor(
                    None, file_path.unlink
                )
                
                logger.info("File deleted from filesystem", path=str(file_path))
                return True
            else:
                logger.warning("File not found for deletion", path=str(file_path))
                return False
                
        except Exception as e:
            logger.warning("Failed to delete file from filesystem", storage_path=storage_path, error=str(e))
            return False


def get_default_storage_backend() -> StorageBackend:
    """Get default storage backend based on configuration."""
    if hasattr(settings, 'minio_endpoint') and settings.minio_endpoint:
        return MinIOStorageBackend(
            endpoint=settings.minio_endpoint,
            access_key=settings.minio_access_key,
            secret_key=settings.minio_secret_key,
            bucket_name="build-snapshots",
            secure=settings.minio_secure
        )
    else:
        # Fallback to filesystem storage
        return FilesystemStorageBackend("/tmp/build-snapshots")