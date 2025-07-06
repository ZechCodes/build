"""
Firecracker Integration Adapter for Snapshot Manager

Provides a seamless interface between the snapshot manager and the real
Firecracker service, replacing mock implementations with production code.
"""

import asyncio
import time
from typing import Dict, Any, Optional, List
from pathlib import Path
import structlog
import logfire

# Import the real Firecracker service
import sys

try:
    # Handle the vm-manager directory name with hyphen
    vm_manager_path = Path(__file__).parent.parent.parent.resolve() / "vm-manager"
    if vm_manager_path.exists():
        # Temporarily add to sys.path, import, then remove
        original_path = sys.path.copy()
        sys.path.insert(0, str(vm_manager_path))
        try:
            from firecracker.firecracker_service import FirecrackerService, FirecrackerError
            FIRECRACKER_AVAILABLE = True
        finally:
            # Restore original sys.path to avoid interfering with other imports
            sys.path = original_path
    else:
        raise ImportError("vm-manager directory not found")
except ImportError:
    # Firecracker service not available, use mock only
    FIRECRACKER_AVAILABLE = False
    
    class FirecrackerService:
        """Mock FirecrackerService for when real service is not available."""
        def __init__(self, *args, **kwargs):
            raise ImportError("Firecracker service not available")
    
    class FirecrackerError(Exception):
        """Mock FirecrackerError."""
        pass

logger = structlog.get_logger()


class FirecrackerVMManager:
    """
    Production Firecracker VM Manager for Snapshot Operations.
    
    Provides a high-level interface for VM management integrated with 
    snapshot operations, wrapping the low-level Firecracker service.
    """
    
    def __init__(self, lima_vm_name: str = "firecracker-dev"):
        """Initialize Firecracker VM manager."""
        self.firecracker_service = FirecrackerService(lima_vm_name)
        self.user_vm_mapping: Dict[str, str] = {}  # vm_id -> user_id
        self.vm_metadata: Dict[str, Dict[str, Any]] = {}
        
    async def initialize(self):
        """Initialize the VM manager and underlying Firecracker service."""
        try:
            await self.firecracker_service.initialize()
            logger.info("Firecracker VM manager initialized")
            logfire.info("VM manager ready", service="firecracker-adapter")
            
        except Exception as e:
            logger.error("Failed to initialize Firecracker VM manager", error=str(e))
            logfire.error("VM manager initialization failed", error=str(e))
            raise
    
    async def create_vm(self, vm_id: str, user_id: str, config: Optional[Dict[str, Any]] = None) -> bool:
        """
        Create a new VM with user ownership tracking.
        
        Args:
            vm_id: Unique VM identifier
            user_id: User who owns the VM
            config: Optional VM configuration
            
        Returns:
            bool: True if VM created successfully
        """
        try:
            # Create VM with Firecracker service
            success = await self.firecracker_service.create_vm(vm_id, config)
            
            if success:
                # Register user ownership
                self.user_vm_mapping[vm_id] = user_id
                self.vm_metadata[vm_id] = {
                    "user_id": user_id,
                    "created_at": time.time(),
                    "config": config or {},
                    "status": "running"
                }
                
                logger.info("VM created and registered", vm_id=vm_id, user_id=user_id)
                logfire.info("VM creation completed", vm_id=vm_id, user_id=user_id)
            
            return success
            
        except Exception as e:
            logger.error("Failed to create VM", vm_id=vm_id, user_id=user_id, error=str(e))
            logfire.error("VM creation failed", vm_id=vm_id, error=str(e))
            raise
    
    async def get_vm(self, vm_id: str) -> Optional[object]:
        """
        Get VM object with ownership and configuration information.
        
        Args:
            vm_id: VM identifier
            
        Returns:
            VM object with user_id and get_config() method, or None if not found
        """
        if vm_id not in self.vm_metadata:
            return None
        
        vm_info = self.vm_metadata[vm_id]
        
        # Create mock VM object with required attributes
        class VMObject:
            def __init__(self, vm_id: str, user_id: str, config: Dict[str, Any]):
                self.id = vm_id
                self.user_id = user_id
                self.config = config
                self.status = vm_info.get("status", "unknown")
                self.created_at = vm_info.get("created_at", time.time())
            
            def get_config(self) -> Dict[str, Any]:
                """Get VM configuration."""
                return self.config
        
        return VMObject(vm_id, vm_info["user_id"], vm_info["config"])
    
    async def create_firecracker_snapshot(self, vm_id: str) -> bytes:
        """
        Create a Firecracker snapshot of the specified VM.
        
        Args:
            vm_id: VM to snapshot
            
        Returns:
            bytes: Snapshot data
        """
        try:
            # Verify VM exists
            if vm_id not in self.vm_metadata:
                raise ValueError(f"VM {vm_id} not found")
            
            # Create snapshot using Firecracker service
            snapshot_data = await self.firecracker_service.create_snapshot(vm_id)
            
            logger.info("Firecracker snapshot created", 
                       vm_id=vm_id, 
                       size_bytes=len(snapshot_data))
            
            logfire.info("Snapshot operation completed",
                        vm_id=vm_id,
                        user_id=self.vm_metadata[vm_id]["user_id"],
                        snapshot_size_bytes=len(snapshot_data),
                        operation="create_snapshot")
            
            return snapshot_data
            
        except Exception as e:
            logger.error("Firecracker snapshot creation failed", vm_id=vm_id, error=str(e))
            logfire.error("Snapshot creation error", vm_id=vm_id, error=str(e))
            raise
    
    async def restore_vm_from_snapshot(self, vm_id: str, snapshot_data: bytes) -> bool:
        """
        Restore a VM from snapshot data.
        
        Args:
            vm_id: VM to restore
            snapshot_data: Snapshot data to restore from
            
        Returns:
            bool: True if restoration successful
        """
        try:
            # Restore using Firecracker service
            success = await self.firecracker_service.restore_snapshot(vm_id, snapshot_data)
            
            if success and vm_id in self.vm_metadata:
                # Update VM metadata
                self.vm_metadata[vm_id]["status"] = "running"
                self.vm_metadata[vm_id]["last_restored"] = time.time()
                
                logger.info("VM restored from snapshot", vm_id=vm_id)
                logfire.info("VM restoration completed",
                            vm_id=vm_id,
                            user_id=self.vm_metadata[vm_id]["user_id"],
                            operation="restore_snapshot")
            
            return success
            
        except Exception as e:
            logger.error("VM restoration failed", vm_id=vm_id, error=str(e))
            logfire.error("VM restoration error", vm_id=vm_id, error=str(e))
            raise
    
    async def is_vm_running(self, vm_id: str) -> bool:
        """Check if VM is currently running."""
        try:
            return await self.firecracker_service.is_vm_running(vm_id)
        except Exception as e:
            logger.warning("Failed to check VM status", vm_id=vm_id, error=str(e))
            return False
    
    async def pause_vm(self, vm_id: str) -> bool:
        """Pause a running VM."""
        try:
            success = await self.firecracker_service.pause_vm(vm_id)
            
            if success and vm_id in self.vm_metadata:
                self.vm_metadata[vm_id]["status"] = "paused"
                
                logger.info("VM paused", vm_id=vm_id)
                logfire.info("VM paused", vm_id=vm_id, 
                           user_id=self.vm_metadata[vm_id]["user_id"])
            
            return success
            
        except Exception as e:
            logger.error("Failed to pause VM", vm_id=vm_id, error=str(e))
            return False
    
    async def resume_vm(self, vm_id: str) -> bool:
        """Resume a paused VM."""
        try:
            success = await self.firecracker_service.resume_vm(vm_id)
            
            if success and vm_id in self.vm_metadata:
                self.vm_metadata[vm_id]["status"] = "running"
                
                logger.info("VM resumed", vm_id=vm_id)
                logfire.info("VM resumed", vm_id=vm_id,
                           user_id=self.vm_metadata[vm_id]["user_id"])
            
            return success
            
        except Exception as e:
            logger.error("Failed to resume VM", vm_id=vm_id, error=str(e))
            return False
    
    async def stop_vm(self, vm_id: str) -> bool:
        """Stop a VM."""
        try:
            success = await self.firecracker_service.stop_vm(vm_id)
            
            if success and vm_id in self.vm_metadata:
                self.vm_metadata[vm_id]["status"] = "stopped"
                
                logger.info("VM stopped", vm_id=vm_id)
                logfire.info("VM stopped", vm_id=vm_id,
                           user_id=self.vm_metadata[vm_id]["user_id"])
            
            return success
            
        except Exception as e:
            logger.error("Failed to stop VM", vm_id=vm_id, error=str(e))
            return False
    
    async def verify_vm_ownership(self, vm_id: str, user_id: str) -> bool:
        """Verify that a VM is owned by the specified user."""
        if vm_id not in self.user_vm_mapping:
            return False
        
        return self.user_vm_mapping[vm_id] == user_id
    
    def get_vm_config(self, vm_id: str) -> Optional[Dict[str, Any]]:
        """Get VM configuration."""
        if vm_id in self.vm_metadata:
            return self.vm_metadata[vm_id]["config"]
        return None
    
    def list_user_vms(self, user_id: str) -> List[str]:
        """List all VMs owned by a user."""
        return [vm_id for vm_id, owner in self.user_vm_mapping.items() 
                if owner == user_id]
    
    def get_vm_stats(self) -> Dict[str, Any]:
        """Get VM manager statistics."""
        running_vms = sum(1 for vm_id in self.vm_metadata.keys() 
                         if self.vm_metadata[vm_id].get("status") == "running")
        
        return {
            "total_vms": len(self.vm_metadata),
            "running_vms": running_vms,
            "users_with_vms": len(set(self.user_vm_mapping.values())),
            "firecracker_active": len(self.firecracker_service.active_vms)
        }


class MockVMManager:
    """
    Mock VM Manager for testing environments where Firecracker is not available.
    
    Provides the same interface as FirecrackerVMManager but with simulated operations.
    """
    
    def __init__(self):
        """Initialize mock VM manager."""
        self.vms: Dict[str, Dict[str, Any]] = {}
        self.user_vm_mapping: Dict[str, str] = {}
        
    async def initialize(self):
        """Initialize mock VM manager."""
        logger.info("Mock VM manager initialized")
        logfire.info("Mock VM manager ready", service="mock-adapter")
    
    async def get_vm(self, vm_id: str) -> Optional[object]:
        """Get mock VM object."""
        if vm_id not in self.vms:
            return None
        
        vm_info = self.vms[vm_id]
        
        class MockVM:
            def __init__(self, vm_id: str, user_id: str, config: Dict[str, Any]):
                self.id = vm_id
                self.user_id = user_id
                self.config = config
            
            def get_config(self) -> Dict[str, Any]:
                return self.config
        
        return MockVM(vm_id, vm_info["user_id"], vm_info["config"])
    
    async def create_firecracker_snapshot(self, vm_id: str) -> bytes:
        """Create mock snapshot data."""
        if vm_id not in self.vms:
            raise ValueError(f"VM {vm_id} not found")
        
        # Generate realistic mock snapshot data
        import json
        mock_snapshot = {
            "vm_id": vm_id,
            "timestamp": time.time(),
            "memory_state": "mock_memory_data",
            "disk_state": "mock_disk_data",
            "cpu_state": "mock_cpu_registers"
        }
        
        mock_data = json.dumps(mock_snapshot).encode() * 100  # ~10KB mock data
        
        logger.info("Mock snapshot created", vm_id=vm_id, size_bytes=len(mock_data))
        return mock_data
    
    async def restore_vm_from_snapshot(self, vm_id: str, snapshot_data: bytes) -> bool:
        """Mock snapshot restoration."""
        logger.info("Mock VM restored from snapshot", vm_id=vm_id, 
                   snapshot_size=len(snapshot_data))
        return True
    
    async def is_vm_running(self, vm_id: str) -> bool:
        """Mock VM running check."""
        return vm_id in self.vms and self.vms[vm_id].get("status") == "running"
    
    async def pause_vm(self, vm_id: str) -> bool:
        """Mock VM pause."""
        if vm_id in self.vms:
            self.vms[vm_id]["status"] = "paused"
            return True
        return False
    
    async def resume_vm(self, vm_id: str) -> bool:
        """Mock VM resume."""
        if vm_id in self.vms:
            self.vms[vm_id]["status"] = "running"
            return True
        return False
    
    async def verify_vm_ownership(self, vm_id: str, user_id: str) -> bool:
        """Mock ownership verification."""
        return vm_id in self.user_vm_mapping and self.user_vm_mapping[vm_id] == user_id


def get_vm_manager(use_real_firecracker: bool = True) -> object:
    """
    Factory function to get appropriate VM manager.
    
    Args:
        use_real_firecracker: If True, use real Firecracker integration,
                             otherwise use mock for testing
    
    Returns:
        VM manager instance (real or mock)
    """
    if use_real_firecracker and FIRECRACKER_AVAILABLE:
        try:
            return FirecrackerVMManager()
        except Exception as e:
            logger.warning("Failed to initialize Firecracker VM manager, falling back to mock",
                          error=str(e))
            logfire.warning("Firecracker unavailable, using mock", error=str(e))
            return MockVMManager()
    else:
        return MockVMManager()


# Convenience functions for snapshot manager integration

async def create_production_vm_manager(lima_vm_name: str = "firecracker-dev") -> FirecrackerVMManager:
    """Create and initialize production VM manager."""
    if not FIRECRACKER_AVAILABLE:
        raise ImportError("Firecracker service not available")
    
    vm_manager = FirecrackerVMManager(lima_vm_name)
    await vm_manager.initialize()
    return vm_manager


async def create_test_vm_manager() -> MockVMManager:
    """Create and initialize test VM manager."""
    vm_manager = MockVMManager()
    await vm_manager.initialize()
    return vm_manager