"""Firecracker process management with comprehensive security and lifecycle control."""

import asyncio
import subprocess
import signal
import os
import json
import uuid
import re
import time
from typing import Dict, Optional, Any
from pathlib import Path
from datetime import datetime, timezone

import structlog

from config.settings import VMManagerSettings, settings
from firecracker.config_generator import FirecrackerConfigGenerator
from networking.tap_manager import TAPDeviceManager
from storage.rootfs_manager import RootfsManager

logger = structlog.get_logger(__name__)


class ProcessError(Exception):
    """Exception raised when process operations fail."""
    pass


class FirecrackerProcessManager:
    """Manages Firecracker VM processes with security controls and lifecycle management."""
    
    def __init__(
        self,
        socket_dir: str = "/tmp/firecracker-sockets",
        storage_base: str = "/opt/vm-storage",
        template_dir: str = "/opt/vm-templates",
        settings: Optional[VMManagerSettings] = None
    ):
        from config.settings import settings as default_settings
        self.settings = settings or default_settings
        
        self.socket_dir = Path(socket_dir)
        self.socket_dir.mkdir(parents=True, exist_ok=True)
        
        # Dictionary to track active processes
        self.processes: Dict[str, Dict[str, Any]] = {}
        
        # Initialize component managers
        self.config_generator = FirecrackerConfigGenerator(
            base_config_path=self.settings.firecracker_config_dir,
            vm_settings=self.settings
        )
        
        self.tap_manager = TAPDeviceManager(
            bridge_name=self.settings.bridge_name,
            subnet=self.settings.vm_subnet,
            settings=self.settings
        )
        
        self.rootfs_manager = RootfsManager(
            storage_base=storage_base,
            template_dir=template_dir,
            default_template=self.settings.default_template,
            settings=self.settings
        )
        
        logger.info("Firecracker process manager initialized")
    
    async def start_vm(
        self,
        vm_id: str,
        user_id: str,
        vm_config: Dict[str, Any]
    ) -> bool:
        """Start a Firecracker VM with comprehensive validation and setup."""
        
        try:
            # Validate inputs
            self._validate_vm_id(vm_id)
            self._validate_user_id(user_id)
            self._validate_vm_config(vm_config)
            
            # Check if VM is already running
            if vm_id in self.processes:
                logger.warning("VM already running", vm_id=vm_id)
                return False
            
            # Apply resource limits
            safe_config = self._apply_resource_limits(vm_config)
            
            logger.info("Starting VM", vm_id=vm_id, user_id=user_id, config=safe_config)
            
            # Create root filesystem
            rootfs_path = await self.rootfs_manager.create_rootfs(
                user_id=user_id,
                vm_id=vm_id,
                template=safe_config.get("template"),
                size_gb=safe_config.get("disk_gb", 10)
            )
            
            if not rootfs_path:
                logger.error("Failed to create root filesystem", vm_id=vm_id)
                return False
            
            # Setup network if requested
            network_config = None
            if safe_config.get("enable_network", False):
                network_config = await self.tap_manager.create_tap_device(vm_id)
                if not network_config:
                    logger.error("Failed to create network interface", vm_id=vm_id)
                    # Cleanup rootfs on network failure
                    await self.rootfs_manager.delete_rootfs(user_id, vm_id)
                    return False
            
            # Generate VM configuration
            firecracker_config = self.config_generator.generate_vm_config(
                vm_id=vm_id,
                user_id=user_id,
                cpu_count=safe_config.get("cpu_count", 1),
                memory_mb=safe_config.get("memory_mb", 512),
                disk_size_gb=safe_config.get("disk_gb", 10),
                template=safe_config.get("template"),
                network_config=network_config
            )
            
            # Generate file paths
            socket_path = self._generate_socket_path(vm_id)
            config_path = self._generate_config_path(vm_id)
            
            # Write configuration file
            with open(config_path, 'w') as f:
                json.dump(firecracker_config, f, indent=2)
            
            # Set secure permissions
            os.chmod(config_path, 0o600)
            
            # Start Firecracker process
            if self.settings.use_mock_firecracker:
                process = await self._start_mock_firecracker(socket_path, config_path)
            else:
                process = await self._start_real_firecracker(socket_path, config_path)
            
            if not process:
                logger.error("Failed to start Firecracker process", vm_id=vm_id)
                await self._cleanup_vm_resources(vm_id, user_id, network_config, config_path)
                return False
            
            # Track the process
            self.processes[vm_id] = {
                "process": process,
                "status": "running",
                "pid": process.pid,
                "socket_path": socket_path,
                "config_path": config_path,
                "network_config": network_config,
                "user_id": user_id,
                "started_at": datetime.now(timezone.utc).isoformat(),
                "vm_config": safe_config
            }
            
            logger.info(
                "VM started successfully",
                vm_id=vm_id,
                pid=process.pid,
                socket_path=socket_path
            )
            
            return True
            
        except Exception as e:
            logger.error("Failed to start VM", vm_id=vm_id, error=str(e))
            await self._cleanup_vm_resources(vm_id, user_id, 
                                           locals().get("network_config"),
                                           locals().get("config_path"))
            return False
    
    async def _start_real_firecracker(self, socket_path: str, config_path: str):
        """Start real Firecracker process."""
        cmd = [
            self.settings.firecracker_binary_path,
            "--api-sock", socket_path,
            "--config-file", config_path
        ]
        
        process = await asyncio.create_subprocess_exec(
            *cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            preexec_fn=os.setsid  # Create new process group for isolation
        )
        
        return process
    
    async def _start_mock_firecracker(self, socket_path: str, config_path: str):
        """Start mock Firecracker process for development."""
        # Create a simple mock process that sleeps
        cmd = ["sleep", "3600"]  # Sleep for 1 hour as mock
        
        process = await asyncio.create_subprocess_exec(
            *cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            preexec_fn=os.setsid
        )
        
        logger.debug("Started mock Firecracker process", pid=process.pid)
        return process
    
    async def stop_vm(self, vm_id: str, force: bool = False) -> bool:
        """Stop a Firecracker VM with graceful shutdown option."""
        
        try:
            self._validate_vm_id(vm_id)
            
            if vm_id not in self.processes:
                logger.warning("VM not found in processes", vm_id=vm_id)
                return False
            
            vm_info = self.processes[vm_id]
            process = vm_info["process"]
            
            logger.info("Stopping VM", vm_id=vm_id, force=force, pid=process.pid)
            
            try:
                if force:
                    # Force kill the process group
                    os.killpg(os.getpgid(process.pid), signal.SIGKILL)
                    logger.info("VM force killed", vm_id=vm_id)
                else:
                    # Graceful shutdown
                    os.killpg(os.getpgid(process.pid), signal.SIGTERM)
                    
                    # Wait for graceful shutdown with timeout
                    try:
                        await asyncio.wait_for(
                            process.wait(), 
                            timeout=self.settings.vm_shutdown_timeout
                        )
                        logger.info("VM shutdown gracefully", vm_id=vm_id)
                    except asyncio.TimeoutError:
                        logger.warning(
                            "VM didn't shutdown gracefully, force killing", 
                            vm_id=vm_id
                        )
                        os.killpg(os.getpgid(process.pid), signal.SIGKILL)
                
                # Cleanup resources
                await self._cleanup_vm_resources(
                    vm_id,
                    vm_info["user_id"], 
                    vm_info.get("network_config"),
                    vm_info["config_path"]
                )
                
                # Remove from tracking
                del self.processes[vm_id]
                
                logger.info("VM stopped and cleaned up", vm_id=vm_id)
                return True
                
            except ProcessLookupError:
                # Process already dead
                logger.info("VM process already terminated", vm_id=vm_id)
                del self.processes[vm_id]
                return True
                
        except Exception as e:
            logger.error("Failed to stop VM", vm_id=vm_id, error=str(e))
            return False
    
    async def get_vm_status(self, vm_id: str) -> Optional[Dict[str, Any]]:
        """Get detailed status information for a VM."""
        
        try:
            self._validate_vm_id(vm_id)
            
            if vm_id not in self.processes:
                return None
            
            vm_info = self.processes[vm_id]
            process = vm_info["process"]
            
            # Check if process is still alive
            poll_result = process.poll()
            is_running = poll_result is None
            
            # Calculate uptime
            started_at = datetime.fromisoformat(vm_info["started_at"])
            uptime = datetime.now(timezone.utc) - started_at
            
            status = {
                "vm_id": vm_id,
                "status": "running" if is_running else "stopped",
                "pid": vm_info["pid"],
                "started_at": vm_info["started_at"],
                "uptime": str(uptime),
                "config": vm_info["vm_config"],
                "socket_path": vm_info["socket_path"]
            }
            
            if vm_info.get("network_config"):
                status["network"] = vm_info["network_config"]
            
            return status
            
        except Exception as e:
            logger.error("Failed to get VM status", vm_id=vm_id, error=str(e))
            return None
    
    async def list_vms(self) -> list:
        """List all managed VMs with their status."""
        
        vms = []
        
        for vm_id in list(self.processes.keys()):
            status = await self.get_vm_status(vm_id)
            if status:
                vms.append(status)
        
        return vms
    
    async def cleanup(self) -> None:
        """Cleanup all VMs and resources on shutdown."""
        
        logger.info("Cleaning up all VMs", count=len(self.processes))
        
        vm_ids = list(self.processes.keys())
        
        # Stop all VMs concurrently
        stop_tasks = [self.stop_vm(vm_id) for vm_id in vm_ids]
        
        try:
            await asyncio.wait_for(
                asyncio.gather(*stop_tasks, return_exceptions=True),
                timeout=60  # 1 minute total cleanup timeout
            )
        except asyncio.TimeoutError:
            logger.warning("Cleanup timeout, force killing remaining processes")
            # Force kill any remaining processes
            for vm_id in list(self.processes.keys()):
                await self.stop_vm(vm_id, force=True)
        
        logger.info("VM cleanup completed")
    
    async def _cleanup_vm_resources(
        self, 
        vm_id: str, 
        user_id: str, 
        network_config: Optional[Dict],
        config_path: Optional[str]
    ) -> None:
        """Cleanup all resources associated with a VM."""
        
        try:
            # Cleanup network interface
            if network_config:
                await self.tap_manager.cleanup_tap_device(
                    network_config["tap_device"]
                )
                self.tap_manager.release_ip(network_config["ip_address"])
            
            # Cleanup root filesystem
            await self.rootfs_manager.delete_rootfs(user_id, vm_id)
            
            # Cleanup configuration file
            if config_path and Path(config_path).exists():
                Path(config_path).unlink()
            
            # Cleanup socket file
            socket_path = self._generate_socket_path(vm_id)
            if Path(socket_path).exists():
                Path(socket_path).unlink()
            
            logger.debug("VM resources cleaned up", vm_id=vm_id)
            
        except Exception as e:
            logger.error("Failed to cleanup VM resources", vm_id=vm_id, error=str(e))
    
    def _validate_vm_id(self, vm_id: str):
        """Validate VM ID for security."""
        if not vm_id:
            raise ProcessError("VM ID cannot be empty")
        
        try:
            uuid.UUID(vm_id)
        except ValueError:
            raise ProcessError(f"Invalid VM ID format: {vm_id}")
        
        if len(vm_id) > 100:
            raise ProcessError(f"VM ID too long: {vm_id}")
    
    def _validate_user_id(self, user_id: str):
        """Validate user ID for security."""
        if not user_id:
            raise ProcessError("User ID cannot be empty")
        
        try:
            uuid.UUID(user_id)
        except ValueError:
            raise ProcessError(f"Invalid user ID format: {user_id}")
        
        if len(user_id) > 100:
            raise ProcessError(f"User ID too long: {user_id}")
    
    def _validate_vm_config(self, config: Dict[str, Any]):
        """Validate VM configuration parameters."""
        required_fields = ["cpu_count", "memory_mb", "disk_gb"]
        
        for field in required_fields:
            if field not in config:
                raise ProcessError(f"Missing required field: {field}")
        
        # Validate ranges
        if not (1 <= config["cpu_count"] <= 16):
            raise ProcessError(f"Invalid CPU count: {config['cpu_count']}")
        
        if not (512 <= config["memory_mb"] <= 16384):
            raise ProcessError(f"Invalid memory size: {config['memory_mb']}")
        
        if not (10 <= config["disk_gb"] <= 500):
            raise ProcessError(f"Invalid disk size: {config['disk_gb']}")
    
    def _apply_resource_limits(self, config: Dict[str, Any]) -> Dict[str, Any]:
        """Apply resource limits from settings."""
        limited_config = config.copy()
        
        limited_config["cpu_count"] = min(
            config.get("cpu_count", 1), 
            self.settings.max_cpu_per_vm
        )
        
        limited_config["memory_mb"] = min(
            config.get("memory_mb", 512), 
            self.settings.max_memory_mb_per_vm
        )
        
        limited_config["disk_gb"] = min(
            config.get("disk_gb", 10), 
            self.settings.max_disk_gb_per_vm
        )
        
        return limited_config
    
    def _generate_socket_path(self, vm_id: str) -> str:
        """Generate secure socket path for VM."""
        self._validate_vm_id(vm_id)
        
        # Sanitize VM ID for file path
        safe_vm_id = re.sub(r'[^\w-]', '', vm_id)
        socket_path = self.socket_dir / f"firecracker-{safe_vm_id}.sock"
        
        # Security check
        if ".." in str(socket_path):
            raise ProcessError("Invalid socket path")
        
        return str(socket_path)
    
    def _generate_config_path(self, vm_id: str) -> str:
        """Generate secure config file path for VM."""
        self._validate_vm_id(vm_id)
        
        # Sanitize VM ID for file path
        safe_vm_id = re.sub(r'[^\w-]', '', vm_id)
        config_path = f"/tmp/firecracker-{safe_vm_id}.json"
        
        # Security check
        if ".." in config_path:
            raise ProcessError("Invalid config path")
        
        return config_path
    
    def get_process_info(self) -> Dict[str, Any]:
        """Get information about all managed processes."""
        return {
            "total_vms": len(self.processes),
            "running_vms": len([
                vm for vm in self.processes.values() 
                if vm["status"] == "running"
            ]),
            "vm_ids": list(self.processes.keys()),
            "uptime": time.time()  # Service uptime placeholder
        }