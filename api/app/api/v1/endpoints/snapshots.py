"""Snapshot management API endpoints."""

import uuid
from typing import Optional, List
from fastapi import APIRouter, Depends, HTTPException, status, BackgroundTasks
from sqlalchemy.ext.asyncio import AsyncSession

from app.core.deps import get_current_user, get_db
from app.models.user import User
from app.schemas.snapshot import (
    SnapshotCreate,
    SnapshotUpdate,
    SnapshotResponse,
    SnapshotList,
    SnapshotRestore,
    SnapshotQuota,
    SnapshotStats,
)
from app.services.snapshot import SnapshotService
from app.authorization.permissions import Permission, PermissionChecker
import structlog

logger = structlog.get_logger(__name__)

router = APIRouter()


def get_snapshot_service() -> SnapshotService:
    """Get snapshot service instance."""
    return SnapshotService()


@router.post("/", response_model=SnapshotResponse, status_code=status.HTTP_201_CREATED)
async def create_snapshot(
    snapshot_data: SnapshotCreate,
    background_tasks: BackgroundTasks,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
    snapshot_service: SnapshotService = Depends(get_snapshot_service),
):
    """Create a new VM snapshot."""
    try:
        # Check snapshot creation permission
        if not PermissionChecker.user_has_permission(current_user, Permission.SNAPSHOT_CREATE):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Insufficient permissions to create snapshots"
            )
        
        snapshot = await snapshot_service.create_snapshot(
            db=db,
            user_id=current_user.id,
            snapshot_create=snapshot_data,
            ip_address=None  # Would be extracted from request in production
        )
        
        logger.info(
            "Snapshot creation requested",
            snapshot_id=str(snapshot.id),
            user_id=str(current_user.id),
            vm_id=str(snapshot_data.vm_instance_id)
        )
        
        return SnapshotResponse.model_validate(snapshot)
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error(
            "Snapshot creation failed",
            user_id=str(current_user.id),
            vm_id=str(snapshot_data.vm_instance_id),
            error=str(e)
        )
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to create snapshot"
        )


@router.get("/", response_model=SnapshotList)
async def list_snapshots(
    vm_id: Optional[str] = None,
    limit: int = 50,
    offset: int = 0,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
    snapshot_service: SnapshotService = Depends(get_snapshot_service),
):
    """List user's snapshots."""
    try:
        # Check snapshot view permission
        if not PermissionChecker.user_has_permission(current_user, Permission.SNAPSHOT_VIEW):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Insufficient permissions to view snapshots"
            )
        
        # Validate VM ID if provided
        vm_uuid = None
        if vm_id:
            try:
                vm_uuid = uuid.UUID(vm_id)
            except ValueError:
                raise HTTPException(
                    status_code=status.HTTP_400_BAD_REQUEST,
                    detail="Invalid VM ID format"
                )
        
        # Validate pagination parameters
        if limit < 1 or limit > 1000:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Limit must be between 1 and 1000"
            )
        
        if offset < 0:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Offset must be non-negative"
            )
        
        snapshots = await snapshot_service.list_user_snapshots(
            db=db,
            user_id=current_user.id,
            vm_id=vm_uuid,
            limit=limit,
            offset=offset
        )
        
        total_count = await snapshot_service.count_user_snapshots(
            db=db,
            user_id=current_user.id,
            vm_id=vm_uuid
        )
        
        snapshot_responses = [
            SnapshotResponse.model_validate(snapshot) for snapshot in snapshots
        ]
        
        return SnapshotList(
            snapshots=snapshot_responses,
            total_count=total_count,
            limit=limit,
            offset=offset
        )
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error(
            "Failed to list snapshots",
            user_id=str(current_user.id),
            error=str(e)
        )
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to list snapshots"
        )


@router.get("/{snapshot_id}", response_model=SnapshotResponse)
async def get_snapshot(
    snapshot_id: str,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
    snapshot_service: SnapshotService = Depends(get_snapshot_service),
):
    """Get snapshot details."""
    try:
        # Check snapshot view permission
        if not PermissionChecker.user_has_permission(current_user, Permission.SNAPSHOT_VIEW):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Insufficient permissions to view snapshots"
            )
        # Validate snapshot ID format
        try:
            snapshot_uuid = uuid.UUID(snapshot_id)
        except ValueError:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Invalid snapshot ID format"
            )
        
        snapshot = await snapshot_service.get_snapshot(
            db=db,
            snapshot_id=snapshot_uuid,
            user_id=current_user.id
        )
        
        if not snapshot:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Snapshot not found"
            )
        
        return SnapshotResponse.model_validate(snapshot)
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error(
            "Failed to get snapshot",
            snapshot_id=snapshot_id,
            user_id=str(current_user.id),
            error=str(e)
        )
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to get snapshot"
        )


@router.put("/{snapshot_id}", response_model=SnapshotResponse)
async def update_snapshot(
    snapshot_id: str,
    snapshot_update: SnapshotUpdate,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
    snapshot_service: SnapshotService = Depends(get_snapshot_service),
):
    """Update snapshot metadata."""
    try:
        # Check snapshot creation permission (needed to modify snapshots)
        if not PermissionChecker.user_has_permission(current_user, Permission.SNAPSHOT_CREATE):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Insufficient permissions to update snapshots"
            )
        # Validate snapshot ID format
        try:
            snapshot_uuid = uuid.UUID(snapshot_id)
        except ValueError:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Invalid snapshot ID format"
            )
        
        snapshot = await snapshot_service.update_snapshot(
            db=db,
            snapshot_id=snapshot_uuid,
            user_id=current_user.id,
            snapshot_update=snapshot_update,
            ip_address=None  # Would be extracted from request in production
        )
        
        if not snapshot:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Snapshot not found"
            )
        
        return SnapshotResponse.model_validate(snapshot)
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error(
            "Failed to update snapshot",
            snapshot_id=snapshot_id,
            user_id=str(current_user.id),
            error=str(e)
        )
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to update snapshot"
        )


@router.post("/{snapshot_id}/restore", status_code=status.HTTP_202_ACCEPTED)
async def restore_snapshot(
    snapshot_id: str,
    restore_data: SnapshotRestore,
    background_tasks: BackgroundTasks,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
    snapshot_service: SnapshotService = Depends(get_snapshot_service),
):
    """Restore a snapshot to a VM."""
    try:
        # Validate snapshot ID format
        try:
            snapshot_uuid = uuid.UUID(snapshot_id)
        except ValueError:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Invalid snapshot ID format"
            )
        
        # Check snapshot restore permission
        if not PermissionChecker.user_has_permission(current_user, Permission.SNAPSHOT_RESTORE):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Insufficient permissions to restore snapshots"
            )
        
        # Validate target VM ID if provided
        target_vm_uuid = None
        if restore_data.target_vm_id:
            target_vm_uuid = restore_data.target_vm_id
        
        success = await snapshot_service.restore_snapshot(
            db=db,
            snapshot_id=snapshot_uuid,
            user_id=current_user.id,
            target_vm_id=target_vm_uuid,
            ip_address=None  # Would be extracted from request in production
        )
        
        if not success:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Snapshot not found"
            )
        
        return {
            "message": "Snapshot restore initiated",
            "snapshot_id": snapshot_id,
            "status": "restoring"
        }
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error(
            "Failed to restore snapshot",
            snapshot_id=snapshot_id,
            user_id=str(current_user.id),
            error=str(e)
        )
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to restore snapshot"
        )


@router.delete("/{snapshot_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_snapshot(
    snapshot_id: str,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
    snapshot_service: SnapshotService = Depends(get_snapshot_service),
):
    """Delete a snapshot."""
    try:
        # Check snapshot deletion permission
        if not PermissionChecker.user_has_permission(current_user, Permission.SNAPSHOT_DELETE):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Insufficient permissions to delete snapshots"
            )
        
        # Validate snapshot ID format
        try:
            snapshot_uuid = uuid.UUID(snapshot_id)
        except ValueError:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Invalid snapshot ID format"
            )
        
        success = await snapshot_service.delete_snapshot(
            db=db,
            snapshot_id=snapshot_uuid,
            user_id=current_user.id,
            ip_address=None  # Would be extracted from request in production
        )
        
        if not success:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Snapshot not found"
            )
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error(
            "Failed to delete snapshot",
            snapshot_id=snapshot_id,
            user_id=str(current_user.id),
            error=str(e)
        )
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to delete snapshot"
        )


@router.get("/quota/info", response_model=SnapshotQuota)
async def get_snapshot_quota(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
    snapshot_service: SnapshotService = Depends(get_snapshot_service),
):
    """Get user's snapshot quota information."""
    try:
        quota = await snapshot_service.get_user_quota(
            db=db,
            user_id=current_user.id
        )
        
        return quota
        
    except Exception as e:
        logger.error(
            "Failed to get snapshot quota",
            user_id=str(current_user.id),
            error=str(e)
        )
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to get quota information"
        )


@router.get("/stats/summary", response_model=SnapshotStats)
async def get_snapshot_stats(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
    snapshot_service: SnapshotService = Depends(get_snapshot_service),
):
    """Get user's snapshot statistics."""
    try:
        stats = await snapshot_service.get_snapshot_stats(
            db=db,
            user_id=current_user.id
        )
        
        return stats
        
    except Exception as e:
        logger.error(
            "Failed to get snapshot stats",
            user_id=str(current_user.id),
            error=str(e)
        )
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to get snapshot statistics"
        )