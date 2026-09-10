# Session 7: VM Snapshot System

## Objective
Implement a comprehensive VM snapshot system that enables users to capture, store, restore, and manage VM states with versioning, deduplication, and quota management while ensuring security and performance.

## Overview
This session builds upon the VM management core from Session 3 to create a robust snapshot system. It implements Firecracker snapshot capabilities, integrates with MinIO S3 storage, provides snapshot metadata management, versioning support, and user quota enforcement. The system enables users to save their work environments and restore them quickly.

## Prerequisites
- Session 1 (Core Infrastructure) completed successfully
- Session 2 (Authentication) completed successfully
- Session 3 (VM Management) completed successfully
- Session 4 (PTY Layer) completed successfully
- Session 5 (WebSocket Layer) completed successfully
- Session 6 (Session Management) completed successfully
- MinIO S3 storage operational
- Firecracker snapshot functionality available

## Components to Implement

### 1. Snapshot Manager Core
**Location**: `snapshot-manager/core/`

#### Snapshot Management System
```python
# snapshot-manager/core/snapshot_manager.py
import asyncio
import hashlib
import json
import time
from typing import Dict, Any, Optional, List, BinaryIO
from dataclasses import dataclass, asdict
from enum import Enum
from pathlib import Path
import aiofiles
import structlog

logger = structlog.get_logger()

class SnapshotState(Enum):
    CREATING = "creating"
    AVAILABLE = "available"
    RESTORING = "restoring"
    DELETING = "deleting"
    ERROR = "error"
    CORRUPTED = "corrupted"

class SnapshotType(Enum):
    MANUAL = "manual"
    AUTOMATIC = "automatic"
    SCHEDULED = "scheduled"

@dataclass
class SnapshotMetadata:
    id: str
    vm_id: str
    user_id: str
    name: str
    description: str
    snapshot_type: SnapshotType
    state: SnapshotState
    size_bytes: int
    compressed_size_bytes: int
    checksum_sha256: str
    storage_path: str
    created_at: float
    updated_at: float
    tags: List[str]
    vm_config: Dict[str, Any]
    restore_count: int
    version: int
    parent_snapshot_id: Optional[str] = None

class SnapshotManager:
    def __init__(self, storage_backend, firecracker_manager, database):
        self.storage = storage_backend
        self.firecracker = firecracker_manager
        self.db = database
        self.temp_dir = Path("/tmp/snapshots")
        self.temp_dir.mkdir(exist_ok=True)
        self.compression_enabled = True
        self.deduplication_enabled = True
        self.max_snapshots_per_user = 50
        self.max_storage_per_user = 100 * 1024 * 1024 * 1024  # 100GB
        
    async def create_snapshot(self, vm_id: str, user_id: str, name: str, 
                            description: str = "", tags: List[str] = None,
                            snapshot_type: SnapshotType = SnapshotType.MANUAL) -> str:
        """Create a new VM snapshot"""
        try:
            # Validate user quotas
            if not await self._check_user_quotas(user_id):
                raise ValueError("User snapshot quota exceeded")
            
            # Generate snapshot ID
            snapshot_id = self._generate_snapshot_id(vm_id, user_id)
            
            # Get VM configuration and state
            vm_config = await self.firecracker.get_vm_config(vm_id)
            if not vm_config:
                raise ValueError(f"VM {vm_id} not found or not accessible")
            
            # Create snapshot metadata
            metadata = SnapshotMetadata(
                id=snapshot_id,
                vm_id=vm_id,
                user_id=user_id,
                name=name,
                description=description,
                snapshot_type=snapshot_type,
                state=SnapshotState.CREATING,
                size_bytes=0,
                compressed_size_bytes=0,
                checksum_sha256="",
                storage_path="",
                created_at=time.time(),
                updated_at=time.time(),
                tags=tags or [],
                vm_config=vm_config,
                restore_count=0,
                version=1
            )
            
            # Store initial metadata
            await self._store_metadata(metadata)
            
            # Start snapshot creation task
            asyncio.create_task(self._create_snapshot_task(metadata))
            
            logger.info("Snapshot creation initiated", snapshot_id=snapshot_id, 
                       vm_id=vm_id, user_id=user_id)
            return snapshot_id
            
        except Exception as e:
            logger.error("Failed to initiate snapshot creation", vm_id=vm_id, 
                        user_id=user_id, error=str(e))
            raise
    
    async def _create_snapshot_task(self, metadata: SnapshotMetadata):
        """Background task to create snapshot"""
        try:
            # Pause VM if running
            vm_was_running = await self.firecracker.is_vm_running(metadata.vm_id)
            if vm_was_running:
                await self.firecracker.pause_vm(metadata.vm_id)
            
            # Create Firecracker snapshot
            snapshot_files = await self.firecracker.create_snapshot(
                metadata.vm_id, str(self.temp_dir / metadata.id)
            )
            
            # Resume VM if it was running
            if vm_was_running:
                await self.firecracker.resume_vm(metadata.vm_id)
            
            # Calculate total size
            total_size = sum(Path(f).stat().st_size for f in snapshot_files)
            
            # Compress and deduplicate if enabled
            if self.compression_enabled:
                snapshot_files = await self._compress_snapshot_files(
                    snapshot_files, metadata.id
                )
            
            # Calculate checksum
            checksum = await self._calculate_checksum(snapshot_files)
            
            # Check for existing snapshots with same checksum (deduplication)
            if self.deduplication_enabled:
                existing_snapshot = await self._find_duplicate_snapshot(
                    checksum, metadata.user_id
                )
                if existing_snapshot:
                    await self._create_snapshot_reference(metadata, existing_snapshot)
                    return
            
            # Upload to storage
            storage_path = await self._upload_snapshot(snapshot_files, metadata)
            
            # Calculate compressed size
            compressed_size = sum(Path(f).stat().st_size for f in snapshot_files)
            
            # Update metadata
            metadata.state = SnapshotState.AVAILABLE
            metadata.size_bytes = total_size
            metadata.compressed_size_bytes = compressed_size
            metadata.checksum_sha256 = checksum
            metadata.storage_path = storage_path
            metadata.updated_at = time.time()
            
            await self._store_metadata(metadata)
            
            # Cleanup temporary files
            await self._cleanup_temp_files(snapshot_files)
            
            logger.info("Snapshot creation completed", snapshot_id=metadata.id,
                       size_mb=total_size // (1024*1024),
                       compressed_mb=compressed_size // (1024*1024))
            
        except Exception as e:
            logger.error("Snapshot creation failed", snapshot_id=metadata.id, 
                        error=str(e))
            
            # Update metadata to error state
            metadata.state = SnapshotState.ERROR
            metadata.updated_at = time.time()
            await self._store_metadata(metadata)
            
            # Cleanup temporary files
            temp_pattern = self.temp_dir / f"{metadata.id}*"
            for temp_file in temp_pattern.parent.glob(temp_pattern.name):
                try:
                    temp_file.unlink()
                except:
                    pass
    
    async def restore_snapshot(self, snapshot_id: str, user_id: str, 
                             target_vm_id: str = None) -> str:
        """Restore a snapshot to a VM"""
        try:
            # Get snapshot metadata
            metadata = await self._get_metadata(snapshot_id)
            if not metadata or metadata.user_id != user_id:
                raise ValueError("Snapshot not found or access denied")
            
            if metadata.state != SnapshotState.AVAILABLE:
                raise ValueError(f"Snapshot not available for restore: {metadata.state}")
            
            # Use original VM ID if target not specified
            if not target_vm_id:
                target_vm_id = metadata.vm_id
            
            # Verify target VM ownership
            if not await self.firecracker.verify_vm_ownership(target_vm_id, user_id):
                raise ValueError("Target VM not found or access denied")
            
            # Update metadata state
            metadata.state = SnapshotState.RESTORING
            metadata.updated_at = time.time()
            await self._store_metadata(metadata)
            
            # Start restore task
            restore_task_id = f"restore_{snapshot_id}_{target_vm_id}_{int(time.time())}"
            asyncio.create_task(self._restore_snapshot_task(
                metadata, target_vm_id, restore_task_id
            ))
            
            logger.info("Snapshot restore initiated", snapshot_id=snapshot_id,
                       target_vm_id=target_vm_id, user_id=user_id)
            return restore_task_id
            
        except Exception as e:
            logger.error("Failed to initiate snapshot restore", 
                        snapshot_id=snapshot_id, error=str(e))
            raise
    
    async def _restore_snapshot_task(self, metadata: SnapshotMetadata, 
                                   target_vm_id: str, task_id: str):
        """Background task to restore snapshot"""
        try:
            # Stop target VM if running
            vm_was_running = await self.firecracker.is_vm_running(target_vm_id)
            if vm_was_running:
                await self.firecracker.stop_vm(target_vm_id)
            
            # Download snapshot files
            temp_restore_dir = self.temp_dir / f"restore_{task_id}"
            temp_restore_dir.mkdir(exist_ok=True)
            
            snapshot_files = await self._download_snapshot(
                metadata.storage_path, temp_restore_dir
            )
            
            # Decompress if needed
            if self.compression_enabled:
                snapshot_files = await self._decompress_snapshot_files(snapshot_files)
            
            # Verify checksum
            checksum = await self._calculate_checksum(snapshot_files)
            if checksum != metadata.checksum_sha256:
                raise ValueError("Snapshot checksum verification failed")
            
            # Restore VM from snapshot
            await self.firecracker.restore_vm_from_snapshot(
                target_vm_id, snapshot_files, metadata.vm_config
            )
            
            # Start VM if it was running before
            if vm_was_running:
                await self.firecracker.start_vm(target_vm_id)
            
            # Update restore count
            metadata.restore_count += 1
            metadata.state = SnapshotState.AVAILABLE
            metadata.updated_at = time.time()
            await self._store_metadata(metadata)
            
            # Cleanup temporary files
            await self._cleanup_temp_files(snapshot_files)
            temp_restore_dir.rmdir()
            
            logger.info("Snapshot restore completed", snapshot_id=metadata.id,
                       target_vm_id=target_vm_id, restore_count=metadata.restore_count)
            
        except Exception as e:
            logger.error("Snapshot restore failed", snapshot_id=metadata.id,
                        target_vm_id=target_vm_id, error=str(e))
            
            # Reset snapshot state
            metadata.state = SnapshotState.AVAILABLE
            metadata.updated_at = time.time()
            await self._store_metadata(metadata)
    
    async def delete_snapshot(self, snapshot_id: str, user_id: str) -> bool:
        """Delete a snapshot"""
        try:
            # Get snapshot metadata
            metadata = await self._get_metadata(snapshot_id)
            if not metadata or metadata.user_id != user_id:
                return False
            
            # Update state to deleting
            metadata.state = SnapshotState.DELETING
            metadata.updated_at = time.time()
            await self._store_metadata(metadata)
            
            # Delete from storage
            await self.storage.delete_object(metadata.storage_path)
            
            # Remove metadata
            await self._delete_metadata(snapshot_id)
            
            logger.info("Snapshot deleted", snapshot_id=snapshot_id, user_id=user_id)
            return True
            
        except Exception as e:
            logger.error("Failed to delete snapshot", snapshot_id=snapshot_id, 
                        error=str(e))
            return False
    
    def _generate_snapshot_id(self, vm_id: str, user_id: str) -> str:
        """Generate unique snapshot ID"""
        timestamp = str(int(time.time() * 1000000))  # microseconds
        data = f"{vm_id}:{user_id}:{timestamp}"
        return f"snap_{hashlib.sha256(data.encode()).hexdigest()[:16]}"
    
    async def _check_user_quotas(self, user_id: str) -> bool:
        """Check if user is within snapshot quotas"""
        try:
            user_snapshots = await self.list_user_snapshots(user_id)
            
            # Check snapshot count
            if len(user_snapshots) >= self.max_snapshots_per_user:
                return False
            
            # Check storage usage
            total_storage = sum(s.compressed_size_bytes for s in user_snapshots)
            if total_storage >= self.max_storage_per_user:
                return False
            
            return True
            
        except Exception as e:
            logger.error("Failed to check user quotas", user_id=user_id, error=str(e))
            return False
```

### 2. Storage Backend Integration
**Location**: `snapshot-manager/storage/`

#### MinIO S3 Storage Backend
```python
# snapshot-manager/storage/s3_backend.py
import asyncio
import aiofiles
from typing import List, Dict, Any, Optional, BinaryIO
from pathlib import Path
import aioboto3
import structlog

logger = structlog.get_logger()

class S3StorageBackend:
    def __init__(self, endpoint_url: str, access_key: str, secret_key: str, 
                 bucket_name: str, region: str = "us-east-1"):
        self.endpoint_url = endpoint_url
        self.access_key = access_key
        self.secret_key = secret_key
        self.bucket_name = bucket_name
        self.region = region
        self.session = aioboto3.Session()
        
    async def initialize(self):
        """Initialize storage backend and ensure bucket exists"""
        try:
            async with self.session.client(
                's3',
                endpoint_url=self.endpoint_url,
                aws_access_key_id=self.access_key,
                aws_secret_access_key=self.secret_key,
                region_name=self.region
            ) as s3:
                # Check if bucket exists
                try:
                    await s3.head_bucket(Bucket=self.bucket_name)
                except:
                    # Create bucket if it doesn't exist
                    await s3.create_bucket(Bucket=self.bucket_name)
                    logger.info("Created snapshot storage bucket", bucket=self.bucket_name)
            
            logger.info("S3 storage backend initialized", bucket=self.bucket_name)
            
        except Exception as e:
            logger.error("Failed to initialize S3 storage backend", error=str(e))
            raise
    
    async def upload_snapshot(self, snapshot_files: List[str], 
                            snapshot_id: str, user_id: str) -> str:
        """Upload snapshot files to S3 storage"""
        try:
            async with self.session.client(
                's3',
                endpoint_url=self.endpoint_url,
                aws_access_key_id=self.access_key,
                aws_secret_access_key=self.secret_key,
                region_name=self.region
            ) as s3:
                
                storage_paths = []
                
                for file_path in snapshot_files:
                    file_obj = Path(file_path)
                    object_key = f"snapshots/{user_id}/{snapshot_id}/{file_obj.name}"
                    
                    # Upload file with metadata
                    async with aiofiles.open(file_path, 'rb') as f:
                        await s3.upload_fileobj(
                            f, self.bucket_name, object_key,
                            ExtraArgs={
                                'Metadata': {
                                    'snapshot-id': snapshot_id,
                                    'user-id': user_id,
                                    'upload-timestamp': str(int(time.time()))
                                },
                                'ServerSideEncryption': 'AES256'
                            }
                        )
                    
                    storage_paths.append(object_key)
                    logger.debug("Uploaded snapshot file", file=file_obj.name, 
                               object_key=object_key)
                
                # Create manifest file
                manifest = {
                    'snapshot_id': snapshot_id,
                    'user_id': user_id,
                    'files': storage_paths,
                    'upload_timestamp': time.time()
                }
                
                manifest_key = f"snapshots/{user_id}/{snapshot_id}/manifest.json"
                await s3.put_object(
                    Bucket=self.bucket_name,
                    Key=manifest_key,
                    Body=json.dumps(manifest),
                    ContentType='application/json',
                    ServerSideEncryption='AES256'
                )
                
                logger.info("Snapshot uploaded to S3", snapshot_id=snapshot_id,
                           files_count=len(storage_paths))
                return manifest_key
                
        except Exception as e:
            logger.error("Failed to upload snapshot to S3", 
                        snapshot_id=snapshot_id, error=str(e))
            raise
    
    async def download_snapshot(self, storage_path: str, 
                              local_dir: Path) -> List[str]:
        """Download snapshot files from S3 storage"""
        try:
            async with self.session.client(
                's3',
                endpoint_url=self.endpoint_url,
                aws_access_key_id=self.access_key,
                aws_secret_access_key=self.secret_key,
                region_name=self.region
            ) as s3:
                
                # Download manifest
                manifest_response = await s3.get_object(
                    Bucket=self.bucket_name, Key=storage_path
                )
                manifest_data = await manifest_response['Body'].read()
                manifest = json.loads(manifest_data)
                
                downloaded_files = []
                
                # Download each file
                for object_key in manifest['files']:
                    file_name = Path(object_key).name
                    local_file_path = local_dir / file_name
                    
                    async with aiofiles.open(local_file_path, 'wb') as f:
                        response = await s3.get_object(
                            Bucket=self.bucket_name, Key=object_key
                        )
                        async for chunk in response['Body'].iter_chunks():
                            await f.write(chunk)
                    
                    downloaded_files.append(str(local_file_path))
                    logger.debug("Downloaded snapshot file", 
                               object_key=object_key, local_path=local_file_path)
                
                logger.info("Snapshot downloaded from S3", 
                           storage_path=storage_path, files_count=len(downloaded_files))
                return downloaded_files
                
        except Exception as e:
            logger.error("Failed to download snapshot from S3", 
                        storage_path=storage_path, error=str(e))
            raise
    
    async def delete_object(self, object_key: str) -> bool:
        """Delete object from S3 storage"""
        try:
            async with self.session.client(
                's3',
                endpoint_url=self.endpoint_url,
                aws_access_key_id=self.access_key,
                aws_secret_access_key=self.secret_key,
                region_name=self.region
            ) as s3:
                
                # If it's a manifest, delete all associated files
                if object_key.endswith('manifest.json'):
                    try:
                        manifest_response = await s3.get_object(
                            Bucket=self.bucket_name, Key=object_key
                        )
                        manifest_data = await manifest_response['Body'].read()
                        manifest = json.loads(manifest_data)
                        
                        # Delete all files in the snapshot
                        delete_objects = [{'Key': key} for key in manifest['files']]
                        delete_objects.append({'Key': object_key})  # Include manifest
                        
                        await s3.delete_objects(
                            Bucket=self.bucket_name,
                            Delete={'Objects': delete_objects}
                        )
                        
                    except Exception as e:
                        # Fallback to deleting just the manifest
                        await s3.delete_object(Bucket=self.bucket_name, Key=object_key)
                else:
                    await s3.delete_object(Bucket=self.bucket_name, Key=object_key)
                
                logger.info("Object deleted from S3", object_key=object_key)
                return True
                
        except Exception as e:
            logger.error("Failed to delete object from S3", 
                        object_key=object_key, error=str(e))
            return False
```

### 3. Snapshot API Endpoints
**Location**: `api/app/api/v1/endpoints/`

#### Snapshot Management API
```python
# api/app/api/v1/endpoints/snapshots.py
from fastapi import APIRouter, Depends, HTTPException, status, BackgroundTasks
from fastapi.security import HTTPBearer
from typing import List, Optional
from uuid import UUID

from app.schemas.snapshot import (
    SnapshotCreate, SnapshotResponse, SnapshotList,
    SnapshotRestore, SnapshotUpdate
)
from app.services.snapshot import SnapshotService
from app.core.deps import get_current_user, get_snapshot_service
from app.models.user import User
from app.authorization.permissions import require_permission, Permission

router = APIRouter()
security = HTTPBearer()

@router.post("/", response_model=SnapshotResponse, status_code=status.HTTP_201_CREATED)
async def create_snapshot(
    snapshot_data: SnapshotCreate,
    current_user: User = Depends(get_current_user),
    snapshot_service: SnapshotService = Depends(get_snapshot_service),
    background_tasks: BackgroundTasks = BackgroundTasks()
):
    """Create a new VM snapshot"""
    try:
        # Verify VM ownership
        await require_permission(Permission.VM_MANAGE, current_user.id, snapshot_data.vm_id)
        
        snapshot_id = await snapshot_service.create_snapshot(
            vm_id=str(snapshot_data.vm_id),
            user_id=str(current_user.id),
            name=snapshot_data.name,
            description=snapshot_data.description,
            tags=snapshot_data.tags
        )
        
        return SnapshotResponse(
            id=snapshot_id,
            vm_id=snapshot_data.vm_id,
            name=snapshot_data.name,
            description=snapshot_data.description,
            state="creating",
            size_bytes=0,
            created_at=time.time(),
            tags=snapshot_data.tags
        )
        
    except ValueError as e:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=str(e)
        )
    except Exception as e:
        logger.error("Snapshot creation failed", user_id=current_user.id, 
                    vm_id=snapshot_data.vm_id, error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to create snapshot"
        )

@router.get("/", response_model=SnapshotList)
async def list_snapshots(
    vm_id: Optional[UUID] = None,
    limit: int = 50,
    offset: int = 0,
    current_user: User = Depends(get_current_user),
    snapshot_service: SnapshotService = Depends(get_snapshot_service)
):
    """List user's snapshots"""
    try:
        snapshots = await snapshot_service.list_user_snapshots(
            user_id=str(current_user.id),
            vm_id=str(vm_id) if vm_id else None,
            limit=limit,
            offset=offset
        )
        
        total_count = await snapshot_service.count_user_snapshots(
            user_id=str(current_user.id),
            vm_id=str(vm_id) if vm_id else None
        )
        
        return SnapshotList(
            snapshots=snapshots,
            total_count=total_count,
            limit=limit,
            offset=offset
        )
        
    except Exception as e:
        logger.error("Failed to list snapshots", user_id=current_user.id, error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to list snapshots"
        )

@router.get("/{snapshot_id}", response_model=SnapshotResponse)
async def get_snapshot(
    snapshot_id: str,
    current_user: User = Depends(get_current_user),
    snapshot_service: SnapshotService = Depends(get_snapshot_service)
):
    """Get snapshot details"""
    try:
        snapshot = await snapshot_service.get_snapshot(snapshot_id, str(current_user.id))
        
        if not snapshot:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Snapshot not found"
            )
        
        return snapshot
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Failed to get snapshot", snapshot_id=snapshot_id, 
                    user_id=current_user.id, error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to get snapshot"
        )

@router.post("/{snapshot_id}/restore", status_code=status.HTTP_202_ACCEPTED)
async def restore_snapshot(
    snapshot_id: str,
    restore_data: SnapshotRestore,
    current_user: User = Depends(get_current_user),
    snapshot_service: SnapshotService = Depends(get_snapshot_service)
):
    """Restore snapshot to a VM"""
    try:
        # Verify target VM ownership if specified
        target_vm_id = str(restore_data.target_vm_id) if restore_data.target_vm_id else None
        if target_vm_id:
            await require_permission(Permission.VM_MANAGE, current_user.id, target_vm_id)
        
        restore_task_id = await snapshot_service.restore_snapshot(
            snapshot_id=snapshot_id,
            user_id=str(current_user.id),
            target_vm_id=target_vm_id
        )
        
        return {"task_id": restore_task_id, "status": "restore_initiated"}
        
    except ValueError as e:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=str(e)
        )
    except Exception as e:
        logger.error("Snapshot restore failed", snapshot_id=snapshot_id,
                    user_id=current_user.id, error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to restore snapshot"
        )

@router.delete("/{snapshot_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_snapshot(
    snapshot_id: str,
    current_user: User = Depends(get_current_user),
    snapshot_service: SnapshotService = Depends(get_snapshot_service)
):
    """Delete a snapshot"""
    try:
        success = await snapshot_service.delete_snapshot(snapshot_id, str(current_user.id))
        
        if not success:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Snapshot not found"
            )
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Snapshot deletion failed", snapshot_id=snapshot_id,
                    user_id=current_user.id, error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to delete snapshot"
        )
```

## Critical Decisions

### Snapshot Format and Compression
- **Decision**: Use Firecracker native snapshot format with gzip compression
- **Rationale**: Native format ensures compatibility, compression reduces storage costs
- **Implementation**: Transparent compression/decompression in storage layer

### Storage Location Strategy
- **Decision**: Hierarchical storage in MinIO with user/snapshot organization
- **Rationale**: Enables efficient access controls and quota management
- **Path Format**: `snapshots/{user_id}/{snapshot_id}/{files}`

### Retention Policies
- **Decision**: User-controlled retention with system-wide quotas
- **Rationale**: Balance user flexibility with resource management
- **Limits**: 50 snapshots per user, 100GB total storage per user

### Deduplication Approach
- **Decision**: SHA-256 checksum-based deduplication within user scope
- **Rationale**: Reduces storage usage while maintaining security isolation
- **Implementation**: Reference counting for shared snapshot data

## Security Checklist ✅

### Snapshot Access Control
- [ ] Snapshot ownership validation for all operations
- [ ] Cross-user snapshot access prevention
- [ ] VM ownership verification before snapshot creation
- [ ] Target VM ownership verification before restore
- [ ] Snapshot enumeration prevention via access controls
- [ ] API endpoint authentication and authorization
- [ ] Rate limiting on snapshot operations (10 operations/minute per user)
- [ ] Audit logging for all snapshot operations
- [ ] Permission-based snapshot sharing controls
- [ ] Secure snapshot deletion with data wiping

### Storage Security
- [ ] Snapshots encrypted at rest using AES-256
- [ ] Encrypted transmission to/from storage backend
- [ ] Storage access credentials secured and rotated
- [ ] Object-level access controls in storage backend
- [ ] Integrity verification using checksums
- [ ] Secure deletion of snapshot data
- [ ] Storage path randomization to prevent enumeration
- [ ] Backup encryption for disaster recovery
- [ ] Cross-region replication security
- [ ] Storage quota enforcement and monitoring

### Snapshot Integrity
- [ ] SHA-256 checksum verification for all snapshots
- [ ] Corruption detection during restore operations
- [ ] Integrity checks during storage upload/download
- [ ] Metadata consistency validation
- [ ] Snapshot version integrity verification
- [ ] Tamper detection for snapshot files
- [ ] Recovery procedures for corrupted snapshots
- [ ] Backup verification processes
- [ ] Chain-of-custody logging for snapshots
- [ ] Cryptographic signatures for snapshot authenticity

### Process Security
- [ ] VM isolation during snapshot creation
- [ ] Secure temporary file handling
- [ ] Process privilege minimization
- [ ] Resource usage monitoring and limits
- [ ] Cleanup procedures for failed operations
- [ ] Secure inter-service communication
- [ ] Error handling without information leakage
- [ ] Background task security isolation
- [ ] Recovery process access controls
- [ ] Snapshot operation timeout enforcement

## Testing Requirements

### Snapshot Creation Testing
- [ ] Snapshot creation with running VMs
- [ ] Snapshot creation with stopped VMs
- [ ] Concurrent snapshot creation limits
- [ ] Snapshot creation failure scenarios
- [ ] VM state preservation during snapshots
- [ ] Quota enforcement during creation
- [ ] Storage failure handling
- [ ] Temporary file cleanup verification

### Snapshot Restore Testing
- [ ] Restore to original VM
- [ ] Restore to different VM
- [ ] Restore with VM configuration changes
- [ ] Concurrent restore operations
- [ ] Restore failure recovery
- [ ] Checksum verification during restore
- [ ] VM state after restore validation
- [ ] Performance impact of restore operations

### Storage Integration Testing
- [ ] MinIO connectivity and authentication
- [ ] Upload and download operations
- [ ] Storage error handling
- [ ] Network interruption recovery
- [ ] Large snapshot handling
- [ ] Concurrent storage operations
- [ ] Storage quota enforcement
- [ ] Backup and recovery procedures

### Security Testing
- [ ] Cross-user access prevention
- [ ] Snapshot enumeration prevention
- [ ] Unauthorized restore attempts
- [ ] Storage access validation
- [ ] Encryption verification
- [ ] Integrity violation detection
- [ ] Rate limiting effectiveness
- [ ] Audit trail validation

## Performance Targets

### Snapshot Operations
- Snapshot creation initiation < 2 seconds
- Small VM snapshot (1GB) creation < 5 minutes
- Large VM snapshot (10GB) creation < 20 minutes
- Snapshot restore initiation < 2 seconds
- Small snapshot restore < 3 minutes
- Large snapshot restore < 15 minutes

### Storage Operations
- Storage upload throughput > 50 MB/s
- Storage download throughput > 100 MB/s
- Checksum calculation < 10% of transfer time
- Compression ratio > 30% for typical workloads
- Deduplication effectiveness > 20% storage savings

### System Performance
- API response times < 200ms
- Concurrent snapshot operations (10 per node)
- Memory usage < 1GB per active snapshot operation
- CPU usage < 50% during snapshot operations
- Storage backend connection pool efficiency

## Monitoring & Alerting

### Snapshot Metrics
- Snapshot creation success/failure rates
- Snapshot restore success/failure rates
- Average snapshot sizes and creation times
- Storage usage per user and globally
- Deduplication effectiveness ratios
- Snapshot operation queue depth

### Storage Metrics
- Storage backend availability and latency
- Upload/download throughput metrics
- Storage space utilization
- Failed storage operations
- Backup success rates
- Storage integrity check results

### Performance Metrics
- Snapshot operation response times
- VM downtime during snapshot operations
- Concurrent operation capacity
- Resource utilization during operations
- Background task completion rates
- Error rates by operation type

### Alert Conditions
- Snapshot creation failure rate > 5%
- Storage backend unavailability
- User storage quota approaching limits (>90%)
- Global storage utilization > 85%
- Snapshot operation timeouts
- Integrity verification failures

## Documentation Deliverables

### Technical Documentation
- [ ] Snapshot API specification with examples
- [ ] Storage backend integration guide
- [ ] Firecracker snapshot integration documentation
- [ ] Deduplication algorithm specification
- [ ] Performance optimization guidelines
- [ ] Security architecture documentation

### Operational Documentation
- [ ] Snapshot operation runbook
- [ ] Storage management procedures
- [ ] Backup and recovery procedures
- [ ] Troubleshooting guide for snapshot issues
- [ ] User quota management guide
- [ ] Monitoring and alerting setup guide

## Next Steps

Upon successful completion of Session 7:
1. VM snapshot system operational with full lifecycle management
2. MinIO S3 integration providing reliable storage
3. Deduplication and compression reducing storage costs
4. Security measures fully implemented and tested
5. Performance targets met under load testing
6. User quota management preventing resource abuse
7. Integration with existing VM management validated
8. Proceed to Session 8: Frontend Terminal Implementation

## Risk Mitigation

### Technical Risks
1. **Snapshot corruption**: Checksums, verification, redundant storage
2. **Storage failures**: Multi-backend support, automatic retries
3. **VM downtime**: Minimized snapshot times, pause/resume optimization
4. **Storage exhaustion**: Quota management, cleanup procedures
5. **Performance degradation**: Asynchronous operations, resource limits

### Security Risks
1. **Unauthorized access**: Strong access controls, audit logging
2. **Data leakage**: Encryption, secure deletion, access isolation
3. **Storage compromise**: Encryption keys, access monitoring
4. **Snapshot tampering**: Integrity checks, cryptographic signatures
5. **Quota bypass**: Enforcement validation, monitoring alerts

---

**Session 7 Success Criteria:**
- VM snapshot system fully operational with create/restore/delete operations
- MinIO S3 storage integration providing encrypted and reliable storage
- Deduplication and compression systems reducing storage requirements
- Security checklist 100% complete with comprehensive access controls
- Performance targets achieved for all snapshot operations
- User quota management preventing abuse and ensuring fair usage
- Integration with Sessions 1-6 validated and working seamlessly
- All tests passing with >80% coverage including security tests
- Documentation complete with operational procedures
- Ready for Session 8 frontend terminal implementation