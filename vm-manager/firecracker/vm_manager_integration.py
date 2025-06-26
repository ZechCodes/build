"""
VM Manager integration with real Firecracker service.

This module provides the bridge between the Build platform's VM management
and the Firecracker virtualization service running in Lima.
"""

import asyncio
from typing import Dict, Any, Optional, List
import structlog
import logfire
from .firecracker_service import FirecrackerService, FirecrackerError

logger = structlog.get_logger()


class FirecrackerVMManager:
    """
    VM Manager with real Firecracker integration.
    
    Replaces the mock VM manager with actual Firecracker VM operations
    for production snapshot functionality.
    """
    
    def __init__(self, lima_vm_name: str = "firecracker-dev"):
        """
        Initialize Firecracker VM Manager.
        
        Args:
            lima_vm_name: Name of the Lima VM running Firecracker
        """
        self.firecracker = FirecrackerService(lima_vm_name)
        self.vm_registry: Dict[str, Dict[str, Any]] = {}
        
    async def initialize(self):
        """Initialize the VM manager and Firecracker service."""
        try:
            await self.firecracker.initialize()
            logger.info("Firecracker VM manager initialized")
            logfire.info("VM manager started with Firecracker backend")
        except Exception as e:
            logger.error("Failed to initialize Firecracker VM manager", error=str(e))
            logfire.error("VM manager initialization failed", error=str(e))
            raise
    
    async def get_vm(self, vm_id: str) -> Optional[Dict[str, Any]]:
        """
        Get VM information.
        
        Args:
            vm_id: ID of the VM
            
        Returns:
            Optional[Dict]: VM information if exists, None otherwise
        """
        if vm_id in self.vm_registry:
            vm_info = self.vm_registry[vm_id].copy()
            
            # Check if VM is actually running
            is_running = await self.firecracker.is_vm_running(vm_id)
            vm_info["is_running"] = is_running
            vm_info["status"] = "running" if is_running else "stopped"
            
            return vm_info
        
        return None
    
    async def create_vm(self, vm_id: str, user_id: str, config: Dict[str, Any]) -> bool:
        """
        Create a new Firecracker VM.
        
        Args:
            vm_id: Unique identifier for the VM
            user_id: ID of the user owning the VM
            config: VM configuration (cpu, memory, etc.)
            
        Returns:
            bool: True if VM created successfully
        """
        try:
            # Create Firecracker VM
            await self.firecracker.create_vm(vm_id, config)
            
            # Register VM in our registry
            self.vm_registry[vm_id] = {
                "id": vm_id,
                "user_id": user_id,
                "config": config,
                "created_at": asyncio.get_event_loop().time(),
                "status": "running"
            }
            
            logger.info("VM created successfully", vm_id=vm_id, user_id=user_id)
            logfire.info("VM created", vm_id=vm_id, user_id=user_id, config=config)
            
            return True
            
        except FirecrackerError as e:
            logger.error("Failed to create VM", vm_id=vm_id, error=str(e))
            logfire.error("VM creation failed", vm_id=vm_id, error=str(e))
            return False
    
    async def is_vm_running(self, vm_id: str) -> bool:
        """
        Check if a VM is currently running.
        
        Args:
            vm_id: ID of the VM
            
        Returns:
            bool: True if VM is running
        """
        try:
            return await self.firecracker.is_vm_running(vm_id)
        except Exception as e:
            logger.warning("Failed to check VM status", vm_id=vm_id, error=str(e))
            return False
    
    async def pause_vm(self, vm_id: str) -> bool:
        """
        Pause a running VM.
        
        Args:
            vm_id: ID of the VM to pause
            
        Returns:
            bool: True if VM paused successfully
        """
        try:
            success = await self.firecracker.pause_vm(vm_id)
            if success and vm_id in self.vm_registry:
                self.vm_registry[vm_id]["status"] = "paused"
            return success
        except Exception as e:
            logger.error("Failed to pause VM", vm_id=vm_id, error=str(e))
            return False
    
    async def resume_vm(self, vm_id: str) -> bool:
        """
        Resume a paused VM.
        
        Args:
            vm_id: ID of the VM to resume
            
        Returns:
            bool: True if VM resumed successfully
        """
        try:
            success = await self.firecracker.resume_vm(vm_id)
            if success and vm_id in self.vm_registry:
                self.vm_registry[vm_id]["status"] = "running"
            return success
        except Exception as e:
            logger.error("Failed to resume VM", vm_id=vm_id, error=str(e))
            return False
    
    async def stop_vm(self, vm_id: str) -> bool:
        """
        Stop a running VM.
        
        Args:
            vm_id: ID of the VM to stop
            
        Returns:
            bool: True if VM stopped successfully
        """
        try:
            success = await self.firecracker.stop_vm(vm_id)
            if success and vm_id in self.vm_registry:
                self.vm_registry[vm_id]["status"] = "stopped"
            return success
        except Exception as e:
            logger.error("Failed to stop VM", vm_id=vm_id, error=str(e))
            return False
    
    async def create_firecracker_snapshot(self, vm_id: str) -> bytes:
        """
        Create a Firecracker snapshot of the VM.
        
        Args:
            vm_id: ID of the VM to snapshot
            
        Returns:
            bytes: Snapshot data
            
        Raises:
            FirecrackerError: If snapshot creation fails
        """
        try:
            if vm_id not in self.vm_registry:
                raise FirecrackerError(f"VM {vm_id} not found in registry")
            
            snapshot_data = await self.firecracker.create_snapshot(vm_id)
            
            logger.info("VM snapshot created", 
                       vm_id=vm_id, snapshot_size=len(snapshot_data))
            logfire.info("VM snapshot completed",
                        vm_id=vm_id, size_bytes=len(snapshot_data))
            
            return snapshot_data
            
        except Exception as e:
            logger.error("Failed to create VM snapshot", vm_id=vm_id, error=str(e))
            logfire.error("VM snapshot failed", vm_id=vm_id, error=str(e))
            raise FirecrackerError(f"Snapshot creation failed: {e}")
    
    async def restore_vm_from_snapshot(self, vm_id: str, snapshot_data: bytes, 
                                     vm_config: Dict[str, Any]) -> bool:
        """
        Restore a VM from snapshot data.
        
        Args:
            vm_id: ID of the VM to restore
            snapshot_data: Snapshot data to restore from
            vm_config: Original VM configuration
            
        Returns:
            bool: True if restoration successful
        """
        try:
            # Restore VM from snapshot
            success = await self.firecracker.restore_snapshot(vm_id, snapshot_data)
            
            if success:
                # Update registry with restored VM
                self.vm_registry[vm_id] = {
                    "id": vm_id,
                    "user_id": vm_config.get("user_id", "unknown"),
                    "config": vm_config,
                    "created_at": asyncio.get_event_loop().time(),
                    "status": "running",
                    "restored_from_snapshot": True
                }
                
                logger.info("VM restored from snapshot", vm_id=vm_id)
                logfire.info("VM restoration completed", vm_id=vm_id)
            
            return success
            
        except Exception as e:
            logger.error("Failed to restore VM from snapshot", vm_id=vm_id, error=str(e))
            logfire.error("VM restoration failed", vm_id=vm_id, error=str(e))
            return False
    
    async def get_vm_config(self, vm_id: str) -> Optional[Dict[str, Any]]:
        """
        Get VM configuration.
        
        Args:
            vm_id: ID of the VM
            
        Returns:
            Optional[Dict]: VM configuration if VM exists
        """
        if vm_id in self.vm_registry:
            return self.vm_registry[vm_id]["config"]
        
        # Try to get config from Firecracker service
        return await self.firecracker.get_vm_config(vm_id)
    
    async def list_vms(self, user_id: Optional[str] = None) -> List[Dict[str, Any]]:
        """
        List VMs, optionally filtered by user.
        
        Args:
            user_id: Optional user ID to filter VMs
            
        Returns:
            List[Dict]: List of VM information
        """
        vms = []
        
        for vm_id, vm_info in self.vm_registry.items():
            if user_id is None or vm_info.get("user_id") == user_id:
                # Get current status
                is_running = await self.firecracker.is_vm_running(vm_id)
                vm_data = vm_info.copy()
                vm_data["is_running"] = is_running
                vm_data["status"] = "running" if is_running else "stopped"
                vms.append(vm_data)
        
        return vms
    
    async def delete_vm(self, vm_id: str) -> bool:
        """
        Delete a VM and clean up resources.
        
        Args:
            vm_id: ID of the VM to delete
            
        Returns:
            bool: True if VM deleted successfully
        """
        try:
            # Stop the VM if running
            await self.firecracker.stop_vm(vm_id)
            
            # Remove from registry
            if vm_id in self.vm_registry:
                del self.vm_registry[vm_id]
            
            logger.info("VM deleted", vm_id=vm_id)
            logfire.info("VM deletion completed", vm_id=vm_id)
            
            return True
            
        except Exception as e:
            logger.error("Failed to delete VM", vm_id=vm_id, error=str(e))
            return False
    
    async def get_vm_stats(self, vm_id: str) -> Optional[Dict[str, Any]]:
        """
        Get VM resource usage statistics.
        
        Args:
            vm_id: ID of the VM
            
        Returns:
            Optional[Dict]: VM statistics if available
        """
        if vm_id not in self.vm_registry:
            return None
        
        vm_info = self.vm_registry[vm_id]
        is_running = await self.firecracker.is_vm_running(vm_id)
        
        return {
            "vm_id": vm_id,
            "status": "running" if is_running else "stopped",
            "uptime_seconds": asyncio.get_event_loop().time() - vm_info["created_at"],
            "cpu_count": vm_info["config"].get("cpu_count", 1),
            "memory_mb": vm_info["config"].get("memory_mb", 512),
            "restored_from_snapshot": vm_info.get("restored_from_snapshot", False)
        }
    
    async def cleanup(self):
        """Clean up all VMs and resources."""
        try:
            # Stop all VMs
            for vm_id in list(self.vm_registry.keys()):
                await self.firecracker.stop_vm(vm_id)
            
            self.vm_registry.clear()
            
            logger.info("VM manager cleanup completed")
            logfire.info("VM manager cleanup finished")
            
        except Exception as e:
            logger.error("VM manager cleanup failed", error=str(e))


# Factory function to create VM manager based on environment
async def create_vm_manager(use_firecracker: bool = True, 
                           lima_vm_name: str = "firecracker-dev") -> FirecrackerVMManager:
    """
    Create and initialize a VM manager instance.
    
    Args:
        use_firecracker: Whether to use real Firecracker (True) or mocks (False)
        lima_vm_name: Name of the Lima VM for Firecracker
        
    Returns:
        FirecrackerVMManager: Initialized VM manager instance
    """
    if use_firecracker:
        vm_manager = FirecrackerVMManager(lima_vm_name)
        await vm_manager.initialize()
        return vm_manager
    else:
        # Return mock implementation for testing
        from ..mocks.vm_manager_mock import MockVMManager
        return MockVMManager()