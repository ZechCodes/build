"""VM management service."""

import uuid
from datetime import datetime, timezone
from typing import Optional, List
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select
from fastapi import HTTPException, status
import structlog

from app.models.vm import VMInstance
from app.models.user import User
from app.schemas.vm import VMCreate, VMUpdate
from app.services.audit import AuditService

logger = structlog.get_logger(__name__)


class VMService:
    """Service for managing VM instances."""
    
    @staticmethod
    async def create_vm(
        db: AsyncSession, 
        user_id: uuid.UUID, 
        vm_create: VMCreate,
        ip_address: Optional[str] = None
    ) -> VMInstance:
        """Create a new VM instance."""
        # Check user VM limit
        result = await db.execute(
            select(VMInstance).where(VMInstance.user_id == user_id)
        )
        existing_vms = result.scalars().all()
        
        if len(existing_vms) >= 5:  # Default limit
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="VM limit reached. Maximum 5 VMs per user."
            )
        
        # Create VM configuration
        vm_config = {
            "cpu_count": vm_create.cpu_count,
            "memory_mb": vm_create.memory_mb,
            "disk_gb": vm_create.disk_gb,
            "template": vm_create.template,
            **vm_create.config
        }
        
        # Create VM instance
        vm = VMInstance(
            user_id=user_id,
            name=vm_create.name,
            state="stopped",
            config=vm_config
        )
        
        db.add(vm)
        await db.commit()
        await db.refresh(vm)
        
        # Log audit event
        await AuditService.log_action(
            db, "vm_created", "vm",
            user_id=user_id,
            resource_id=vm.id,
            ip_address=ip_address,
            details={
                "vm_name": vm.name,
                "config": vm_config
            }
        )
        
        logger.info("VM created", vm_id=str(vm.id), user_id=str(user_id), vm_name=vm.name)
        return vm
    
    @staticmethod
    async def list_user_vms(db: AsyncSession, user_id: uuid.UUID) -> List[VMInstance]:
        """List all VMs for a user."""
        result = await db.execute(
            select(VMInstance)
            .where(VMInstance.user_id == user_id)
            .order_by(VMInstance.created_at.desc())
        )
        return list(result.scalars().all())
    
    @staticmethod
    async def get_vm(
        db: AsyncSession, 
        vm_id: str, 
        user_id: uuid.UUID
    ) -> Optional[VMInstance]:
        """Get a VM instance by ID."""
        try:
            vm_uuid = uuid.UUID(vm_id)
        except ValueError:
            return None
        
        result = await db.execute(
            select(VMInstance).where(
                VMInstance.id == vm_uuid,
                VMInstance.user_id == user_id
            )
        )
        return result.scalar_one_or_none()
    
    @staticmethod
    async def update_vm(
        db: AsyncSession,
        vm_id: str,
        user_id: uuid.UUID,
        vm_update: VMUpdate,
        ip_address: Optional[str] = None
    ) -> Optional[VMInstance]:
        """Update a VM instance."""
        vm = await VMService.get_vm(db, vm_id, user_id)
        if not vm:
            return None
        
        # Only allow updates when VM is stopped
        if vm.state != "stopped":
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="VM must be stopped to update configuration"
            )
        
        # Update fields
        update_data = vm_update.dict(exclude_unset=True)
        for field, value in update_data.items():
            if field == "config" and value is not None:
                # Merge config
                vm.config = {**vm.config, **value}
            elif field in ["cpu_count", "memory_mb", "disk_gb"]:
                # Update config for resource changes
                vm.config[field] = value
            elif hasattr(vm, field):
                setattr(vm, field, value)
        
        vm.updated_at = datetime.now(timezone.utc)
        await db.commit()
        await db.refresh(vm)
        
        # Log audit event
        await AuditService.log_action(
            db, "vm_updated", "vm",
            user_id=user_id,
            resource_id=vm.id,
            ip_address=ip_address,
            details={"updates": update_data}
        )
        
        logger.info("VM updated", vm_id=str(vm.id), user_id=str(user_id))
        return vm
    
    @staticmethod
    async def start_vm(
        db: AsyncSession,
        vm_id: str,
        user_id: uuid.UUID,
        ip_address: Optional[str] = None
    ) -> Optional[VMInstance]:
        """Start a VM instance."""
        vm = await VMService.get_vm(db, vm_id, user_id)
        if not vm:
            return None
        
        if vm.state == "running":
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="VM is already running"
            )
        
        # TODO: Integrate with actual Firecracker VM management
        # For now, we'll just update the state
        vm.state = "running"
        vm.started_at = datetime.now(timezone.utc)
        vm.stopped_at = None
        # vm.firecracker_id = f"fc-{vm.id}"  # Would be set by actual Firecracker
        
        await db.commit()
        await db.refresh(vm)
        
        # Log audit event
        await AuditService.log_action(
            db, "vm_started", "vm",
            user_id=user_id,
            resource_id=vm.id,
            ip_address=ip_address
        )
        
        logger.info("VM started", vm_id=str(vm.id), user_id=str(user_id))
        return vm
    
    @staticmethod
    async def stop_vm(
        db: AsyncSession,
        vm_id: str,
        user_id: uuid.UUID,
        ip_address: Optional[str] = None
    ) -> Optional[VMInstance]:
        """Stop a VM instance."""
        vm = await VMService.get_vm(db, vm_id, user_id)
        if not vm:
            return None
        
        if vm.state == "stopped":
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="VM is already stopped"
            )
        
        # TODO: Integrate with actual Firecracker VM management
        vm.state = "stopped"
        vm.stopped_at = datetime.now(timezone.utc)
        vm.firecracker_id = None
        
        await db.commit()
        await db.refresh(vm)
        
        # Log audit event
        await AuditService.log_action(
            db, "vm_stopped", "vm",
            user_id=user_id,
            resource_id=vm.id,
            ip_address=ip_address
        )
        
        logger.info("VM stopped", vm_id=str(vm.id), user_id=str(user_id))
        return vm
    
    @staticmethod
    async def delete_vm(
        db: AsyncSession,
        vm_id: str,
        user_id: uuid.UUID,
        ip_address: Optional[str] = None
    ) -> bool:
        """Delete a VM instance."""
        vm = await VMService.get_vm(db, vm_id, user_id)
        if not vm:
            return False
        
        # Stop VM if running
        if vm.state == "running":
            await VMService.stop_vm(db, vm_id, user_id, ip_address)
        
        # Log audit event before deletion
        await AuditService.log_action(
            db, "vm_deleted", "vm",
            user_id=user_id,
            resource_id=vm.id,
            ip_address=ip_address,
            details={"vm_name": vm.name}
        )
        
        await db.delete(vm)
        await db.commit()
        
        logger.info("VM deleted", vm_id=str(vm.id), user_id=str(user_id))
        return True