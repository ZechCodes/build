"""Snapshot management service."""

import uuid
from datetime import datetime, timezone
from typing import Optional, List, Dict, Any
from pathlib import Path
import asyncio
import hashlib
import json

from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select, func, desc
from fastapi import HTTPException, status
import structlog

from app.models.snapshot import Snapshot
from app.models.vm import VMInstance
from app.models.user import User
from app.schemas.snapshot import SnapshotCreate, SnapshotUpdate, SnapshotQuota, SnapshotStats
from app.services.audit import AuditService
from app.services.storage import StorageBackend, get_default_storage_backend
from app.core.config import get_settings

logger = structlog.get_logger(__name__)
settings = get_settings()


class SnapshotService:
    """Service for managing VM snapshots."""
    
    # Configuration constants
    MAX_SNAPSHOTS_PER_USER = 50
    MAX_STORAGE_GB_PER_USER = 100
    SNAPSHOTS_PER_HOUR_LIMIT = 5
    
    def __init__(self, storage_backend: Optional[StorageBackend] = None):
        """Initialize snapshot service with storage backend."""
        self.storage_backend = storage_backend or self._get_default_storage_backend()
    
    def _get_default_storage_backend(self) -> StorageBackend:
        """Get default storage backend from configuration."""
        return get_default_storage_backend()
    
    async def create_snapshot(
        self,
        db: AsyncSession,
        user_id: uuid.UUID,
        snapshot_create: SnapshotCreate,
        ip_address: Optional[str] = None
    ) -> Snapshot:
        """Create a new VM snapshot."""
        # Verify VM ownership and existence
        vm = await self._get_user_vm(db, snapshot_create.vm_instance_id, user_id)
        if not vm:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="VM not found or access denied"
            )
        
        # Check snapshot quotas
        await self._check_snapshot_quotas(db, user_id)
        
        # Check rate limiting
        await self._check_rate_limiting(db, user_id)
        
        # Generate storage path
        storage_path = self._generate_storage_path(user_id, vm.id)
        
        # Create snapshot record
        snapshot = Snapshot(
            vm_instance_id=snapshot_create.vm_instance_id,
            user_id=user_id,
            name=snapshot_create.name,
            description=snapshot_create.description,
            storage_path=storage_path,
            size_bytes=None,  # Will be updated after snapshot creation
            snapshot_metadata={
                "tags": snapshot_create.tags or [],
                "vm_config": vm.config,
                "created_by": "api",
                "status": "creating"
            }
        )
        
        db.add(snapshot)
        await db.commit()
        await db.refresh(snapshot)
        
        # Start background snapshot creation task
        asyncio.create_task(
            self._create_snapshot_background(snapshot.id, vm, ip_address)
        )
        
        # Log audit event
        await AuditService.log_action(
            db,
            action="snapshot_created",
            user_id=user_id,
            resource_type="snapshot",
            resource_id=snapshot.id,
            ip_address=ip_address,
            details={
                "snapshot_name": snapshot.name,
                "vm_id": str(vm.id),
                "vm_name": vm.name
            }
        )
        
        logger.info(
            "Snapshot creation initiated",
            snapshot_id=str(snapshot.id),
            user_id=str(user_id),
            vm_id=str(vm.id)
        )
        
        return snapshot
    
    async def _create_snapshot_background(
        self,
        snapshot_id: uuid.UUID,
        vm: VMInstance,
        ip_address: Optional[str] = None
    ):
        """Background task to create the actual snapshot."""
        try:
            # Simulate snapshot creation (in production, this would interact with Firecracker)
            await asyncio.sleep(2)  # Simulate snapshot creation time
            
            # Generate mock snapshot data
            snapshot_data = self._generate_mock_snapshot_data(vm)
            
            # Upload to storage
            storage_path = await self.storage_backend.upload_file(
                f"snapshots/{vm.user_id}/{snapshot_id}/vm_state.tar.gz",
                snapshot_data
            )
            
            # Update snapshot record
            from app.core.database import AsyncSessionLocal
            async with AsyncSessionLocal() as db:
                result = await db.execute(
                    select(Snapshot).where(Snapshot.id == snapshot_id)
                )
                snapshot = result.scalar_one_or_none()
                
                if snapshot:
                    snapshot.size_bytes = len(snapshot_data)
                    snapshot.storage_path = storage_path
                    snapshot.snapshot_metadata.update({
                        "status": "completed",
                        "compression_ratio": 0.65,  # Mock ratio
                        "creation_duration_seconds": 2.0
                    })
                    
                    await db.commit()
                    
                    logger.info(
                        "Snapshot creation completed",
                        snapshot_id=str(snapshot_id),
                        size_bytes=len(snapshot_data)
                    )
        
        except Exception as e:
            logger.error(
                "Snapshot creation failed",
                snapshot_id=str(snapshot_id),
                error=str(e)
            )
            
            # Update snapshot to error state
            try:
                from app.core.database import AsyncSessionLocal
                async with AsyncSessionLocal() as db:
                    result = await db.execute(
                        select(Snapshot).where(Snapshot.id == snapshot_id)
                    )
                    snapshot = result.scalar_one_or_none()
                    
                    if snapshot:
                        snapshot.snapshot_metadata.update({
                            "status": "error",
                            "error_message": str(e)
                        })
                        await db.commit()
            except Exception as update_error:
                logger.error(
                    "Failed to update snapshot error state",
                    snapshot_id=str(snapshot_id),
                    error=str(update_error)
                )
    
    def _generate_mock_snapshot_data(self, vm: VMInstance) -> bytes:
        """Generate mock snapshot data for development."""
        # In production, this would create actual VM snapshot
        snapshot_info = {
            "vm_id": str(vm.id),
            "vm_name": vm.name,
            "config": vm.config,
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "type": "full_snapshot"
        }
        
        # Create mock compressed data
        mock_data = json.dumps(snapshot_info, indent=2).encode('utf-8')
        # Simulate compression by repeating data (in reality would use gzip/lz4)
        return mock_data * 1000  # ~10KB mock snapshot
    
    async def list_user_snapshots(
        self,
        db: AsyncSession,
        user_id: uuid.UUID,
        vm_id: Optional[uuid.UUID] = None,
        limit: int = 50,
        offset: int = 0
    ) -> List[Snapshot]:
        """List snapshots for a user."""
        query = select(Snapshot).where(Snapshot.user_id == user_id)
        
        if vm_id:
            query = query.where(Snapshot.vm_instance_id == vm_id)
        
        query = query.order_by(desc(Snapshot.created_at)).limit(limit).offset(offset)
        
        result = await db.execute(query)
        return list(result.scalars().all())
    
    async def count_user_snapshots(
        self,
        db: AsyncSession,
        user_id: uuid.UUID,
        vm_id: Optional[uuid.UUID] = None
    ) -> int:
        """Count total snapshots for a user."""
        query = select(func.count(Snapshot.id)).where(Snapshot.user_id == user_id)
        
        if vm_id:
            query = query.where(Snapshot.vm_instance_id == vm_id)
        
        result = await db.execute(query)
        return result.scalar() or 0
    
    async def get_snapshot(
        self,
        db: AsyncSession,
        snapshot_id: uuid.UUID,
        user_id: uuid.UUID
    ) -> Optional[Snapshot]:
        """Get a snapshot by ID."""
        result = await db.execute(
            select(Snapshot).where(
                Snapshot.id == snapshot_id,
                Snapshot.user_id == user_id
            )
        )
        return result.scalar_one_or_none()
    
    async def update_snapshot(
        self,
        db: AsyncSession,
        snapshot_id: uuid.UUID,
        user_id: uuid.UUID,
        snapshot_update: SnapshotUpdate,
        ip_address: Optional[str] = None
    ) -> Optional[Snapshot]:
        """Update a snapshot."""
        snapshot = await self.get_snapshot(db, snapshot_id, user_id)
        if not snapshot:
            return None
        
        # Update fields
        update_data = snapshot_update.dict(exclude_unset=True)
        for field, value in update_data.items():
            if hasattr(snapshot, field):
                setattr(snapshot, field, value)
        
        snapshot.updated_at = datetime.now(timezone.utc)
        await db.commit()
        await db.refresh(snapshot)
        
        # Log audit event
        await AuditService.log_action(
            db,
            action="snapshot_updated",
            user_id=user_id,
            resource_type="snapshot",
            resource_id=snapshot.id,
            ip_address=ip_address,
            details={"updates": update_data}
        )
        
        logger.info("Snapshot updated", snapshot_id=str(snapshot.id), user_id=str(user_id))
        return snapshot
    
    async def delete_snapshot(
        self,
        db: AsyncSession,
        snapshot_id: uuid.UUID,
        user_id: uuid.UUID,
        ip_address: Optional[str] = None
    ) -> bool:
        """Delete a snapshot."""
        snapshot = await self.get_snapshot(db, snapshot_id, user_id)
        if not snapshot:
            return False
        
        try:
            # Delete from storage
            await self.storage_backend.delete_file(snapshot.storage_path)
        except Exception as e:
            logger.warning(
                "Failed to delete snapshot from storage",
                snapshot_id=str(snapshot_id),
                storage_path=snapshot.storage_path,
                error=str(e)
            )
        
        # Log audit event before deletion
        await AuditService.log_action(
            db,
            action="snapshot_deleted",
            user_id=user_id,
            resource_type="snapshot",
            resource_id=snapshot.id,
            ip_address=ip_address,
            details={"snapshot_name": snapshot.name}
        )
        
        await db.delete(snapshot)
        await db.commit()
        
        logger.info("Snapshot deleted", snapshot_id=str(snapshot_id), user_id=str(user_id))
        return True
    
    async def restore_snapshot(
        self,
        db: AsyncSession,
        snapshot_id: uuid.UUID,
        user_id: uuid.UUID,
        target_vm_id: Optional[uuid.UUID] = None,
        ip_address: Optional[str] = None
    ) -> bool:
        """Restore a snapshot to a VM."""
        snapshot = await self.get_snapshot(db, snapshot_id, user_id)
        if not snapshot:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Snapshot not found"
            )
        
        # Determine target VM
        if target_vm_id:
            target_vm = await self._get_user_vm(db, target_vm_id, user_id)
        else:
            target_vm = await self._get_user_vm(db, snapshot.vm_instance_id, user_id)
        
        if not target_vm:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Target VM not found or access denied"
            )
        
        # VM must be stopped for restore
        if target_vm.state != "stopped":
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="VM must be stopped before restore"
            )
        
        # Start background restore task
        asyncio.create_task(
            self._restore_snapshot_background(snapshot, target_vm, ip_address)
        )
        
        # Log audit event
        await AuditService.log_action(
            db,
            action="snapshot_restore",
            user_id=user_id,
            resource_type="snapshot",
            resource_id=snapshot.id,
            ip_address=ip_address,
            details={
                "target_vm_id": str(target_vm.id),
                "target_vm_name": target_vm.name
            }
        )
        
        logger.info(
            "Snapshot restore initiated",
            snapshot_id=str(snapshot_id),
            target_vm_id=str(target_vm.id),
            user_id=str(user_id)
        )
        
        return True
    
    async def _restore_snapshot_background(
        self,
        snapshot: Snapshot,
        target_vm: VMInstance,
        ip_address: Optional[str] = None
    ):
        """Background task to restore a snapshot."""
        try:
            # Download snapshot data
            snapshot_data = await self.storage_backend.download_file(snapshot.storage_path)
            
            # Simulate restore process (in production, would restore to Firecracker VM)
            await asyncio.sleep(1)  # Simulate restore time
            
            logger.info(
                "Snapshot restore completed",
                snapshot_id=str(snapshot.id),
                target_vm_id=str(target_vm.id),
                size_bytes=len(snapshot_data)
            )
        
        except Exception as e:
            logger.error(
                "Snapshot restore failed",
                snapshot_id=str(snapshot.id),
                target_vm_id=str(target_vm.id),
                error=str(e)
            )
    
    async def get_user_quota(
        self,
        db: AsyncSession,
        user_id: uuid.UUID
    ) -> SnapshotQuota:
        """Get user's snapshot quota information."""
        # Count current snapshots
        current_count = await self.count_user_snapshots(db, user_id)
        
        # Calculate current storage usage
        result = await db.execute(
            select(func.sum(Snapshot.size_bytes)).where(Snapshot.user_id == user_id)
        )
        total_bytes = result.scalar() or 0
        current_storage_gb = total_bytes / (1024 ** 3)
        
        return SnapshotQuota(
            user_id=user_id,
            max_snapshots=self.MAX_SNAPSHOTS_PER_USER,
            current_snapshots=current_count,
            max_storage_gb=self.MAX_STORAGE_GB_PER_USER,
            current_storage_gb=round(current_storage_gb, 2),
            remaining_snapshots=max(0, self.MAX_SNAPSHOTS_PER_USER - current_count),
            remaining_storage_gb=round(
                max(0, self.MAX_STORAGE_GB_PER_USER - current_storage_gb), 2
            )
        )
    
    async def get_snapshot_stats(
        self,
        db: AsyncSession,
        user_id: uuid.UUID
    ) -> SnapshotStats:
        """Get snapshot statistics for a user."""
        # Get basic stats
        result = await db.execute(
            select(
                func.count(Snapshot.id).label('total_count'),
                func.sum(Snapshot.size_bytes).label('total_bytes'),
                func.min(Snapshot.created_at).label('oldest'),
                func.max(Snapshot.created_at).label('newest')
            ).where(Snapshot.user_id == user_id)
        )
        stats = result.first()
        
        total_count = stats.total_count or 0
        total_bytes = stats.total_bytes or 0
        total_gb = total_bytes / (1024 ** 3)
        
        return SnapshotStats(
            total_snapshots=total_count,
            total_size_gb=round(total_gb, 2),
            compression_ratio=0.65,  # Mock compression ratio
            deduplication_savings=0.25,  # Mock deduplication savings
            oldest_snapshot=stats.oldest,
            newest_snapshot=stats.newest
        )
    
    async def _check_snapshot_quotas(self, db: AsyncSession, user_id: uuid.UUID):
        """Check if user is within snapshot quotas."""
        quota = await self.get_user_quota(db, user_id)
        
        if quota.current_snapshots >= quota.max_snapshots:
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail=f"Snapshot limit reached. Maximum {quota.max_snapshots} snapshots per user."
            )
        
        if quota.current_storage_gb >= quota.max_storage_gb:
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail=f"Storage limit reached. Maximum {quota.max_storage_gb}GB per user."
            )
    
    async def _check_rate_limiting(self, db: AsyncSession, user_id: uuid.UUID):
        """Check snapshot creation rate limiting."""
        # Check snapshots created in the last hour
        one_hour_ago = datetime.now(timezone.utc).replace(minute=0, second=0, microsecond=0)
        
        result = await db.execute(
            select(func.count(Snapshot.id))
            .where(
                Snapshot.user_id == user_id,
                Snapshot.created_at >= one_hour_ago
            )
        )
        recent_count = result.scalar() or 0
        
        if recent_count >= self.SNAPSHOTS_PER_HOUR_LIMIT:
            raise HTTPException(
                status_code=status.HTTP_429_TOO_MANY_REQUESTS,
                detail=f"Rate limit exceeded. Maximum {self.SNAPSHOTS_PER_HOUR_LIMIT} snapshots per hour."
            )
    
    async def _get_user_vm(
        self,
        db: AsyncSession,
        vm_id: uuid.UUID,
        user_id: uuid.UUID
    ) -> Optional[VMInstance]:
        """Get a VM instance owned by the user."""
        result = await db.execute(
            select(VMInstance).where(
                VMInstance.id == vm_id,
                VMInstance.user_id == user_id
            )
        )
        return result.scalar_one_or_none()
    
    def _generate_storage_path(self, user_id: uuid.UUID, vm_id: uuid.UUID) -> str:
        """Generate storage path for snapshot."""
        timestamp = datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S")
        snapshot_hash = hashlib.sha256(f"{user_id}:{vm_id}:{timestamp}".encode()).hexdigest()[:8]
        return f"snapshots/{user_id}/{vm_id}/{timestamp}_{snapshot_hash}"