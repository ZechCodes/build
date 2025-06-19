"""VM management endpoints with permission-based authorization."""

from typing import List
from fastapi import APIRouter, Depends, HTTPException, status, Request
from sqlalchemy.ext.asyncio import AsyncSession
import structlog

from app.core.deps import get_db, get_current_user
from app.models.user import User
from app.models.vm import VMInstance
from app.schemas.vm import VMCreate, VMResponse, VMUpdate
from app.services.vm import VMService
from app.authorization.permissions import Permission, PermissionChecker

logger = structlog.get_logger(__name__)
router = APIRouter()


@router.post("/", response_model=VMResponse, status_code=status.HTTP_201_CREATED)
async def create_vm(
    vm_create: VMCreate,
    request: Request,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db)
):
    """Create a new VM instance."""
    # Check VM_CREATE permission
    if not PermissionChecker.user_has_permission(current_user, Permission.VM_CREATE):
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Insufficient permissions to create VMs"
        )
    
    client_ip = request.client.host if request.client else "unknown"
    
    try:
        vm = await VMService.create_vm(db, current_user.id, vm_create, ip_address=client_ip)
        return VMResponse.from_orm(vm)
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Failed to create VM", user_id=str(current_user.id), error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to create VM instance"
        )


@router.get("/", response_model=List[VMResponse])
async def list_vms(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db)
):
    """List user's VM instances."""
    # Check VM_VIEW permission
    if not PermissionChecker.user_has_permission(current_user, Permission.VM_VIEW):
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Insufficient permissions to view VMs"
        )
    
    try:
        vms = await VMService.list_user_vms(db, current_user.id)
        return [VMResponse.from_orm(vm) for vm in vms]
    except Exception as e:
        logger.error("Failed to list VMs", user_id=str(current_user.id), error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to retrieve VM instances"
        )


@router.get("/{vm_id}", response_model=VMResponse)
async def get_vm(
    vm_id: str,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db)
):
    """Get VM instance details."""
    # Check VM_VIEW permission
    if not PermissionChecker.user_has_permission(current_user, Permission.VM_VIEW):
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Insufficient permissions to view VMs"
        )
    
    try:
        vm = await VMService.get_vm(db, vm_id, current_user.id)
        if not vm:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="VM instance not found"
            )
        return VMResponse.from_orm(vm)
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Failed to get VM", vm_id=vm_id, user_id=str(current_user.id), error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to retrieve VM instance"
        )


@router.put("/{vm_id}", response_model=VMResponse)
async def update_vm(
    vm_id: str,
    vm_update: VMUpdate,
    request: Request,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db)
):
    """Update VM instance."""
    # Check VM_MODIFY permission
    if not PermissionChecker.user_has_permission(current_user, Permission.VM_MODIFY):
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Insufficient permissions to modify VMs"
        )
    
    client_ip = request.client.host if request.client else "unknown"
    
    try:
        vm = await VMService.update_vm(db, vm_id, current_user.id, vm_update, ip_address=client_ip)
        if not vm:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="VM instance not found"
            )
        return VMResponse.from_orm(vm)
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Failed to update VM", vm_id=vm_id, user_id=str(current_user.id), error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to update VM instance"
        )


@router.post("/{vm_id}/start", response_model=VMResponse)
async def start_vm(
    vm_id: str,
    request: Request,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db)
):
    """Start VM instance."""
    # Check VM_MODIFY permission
    if not PermissionChecker.user_has_permission(current_user, Permission.VM_MODIFY):
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Insufficient permissions to modify VMs"
        )
    
    client_ip = request.client.host if request.client else "unknown"
    
    try:
        vm = await VMService.start_vm(db, vm_id, current_user.id, ip_address=client_ip)
        if not vm:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="VM instance not found"
            )
        return VMResponse.from_orm(vm)
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Failed to start VM", vm_id=vm_id, user_id=str(current_user.id), error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to start VM instance"
        )


@router.post("/{vm_id}/stop", response_model=VMResponse)
async def stop_vm(
    vm_id: str,
    request: Request,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db)
):
    """Stop VM instance."""
    # Check VM_MODIFY permission
    if not PermissionChecker.user_has_permission(current_user, Permission.VM_MODIFY):
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Insufficient permissions to modify VMs"
        )
    
    client_ip = request.client.host if request.client else "unknown"
    
    try:
        vm = await VMService.stop_vm(db, vm_id, current_user.id, ip_address=client_ip)
        if not vm:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="VM instance not found"
            )
        return VMResponse.from_orm(vm)
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Failed to stop VM", vm_id=vm_id, user_id=str(current_user.id), error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to stop VM instance"
        )


@router.delete("/{vm_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_vm(
    vm_id: str,
    request: Request,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db)
):
    """Delete VM instance."""
    # Check VM_DELETE permission
    if not PermissionChecker.user_has_permission(current_user, Permission.VM_DELETE):
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Insufficient permissions to delete VMs"
        )
    
    client_ip = request.client.host if request.client else "unknown"
    
    try:
        success = await VMService.delete_vm(db, vm_id, current_user.id, ip_address=client_ip)
        if not success:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="VM instance not found"
            )
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Failed to delete VM", vm_id=vm_id, user_id=str(current_user.id), error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to delete VM instance"
        )