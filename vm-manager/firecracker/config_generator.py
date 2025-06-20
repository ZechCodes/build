"""Firecracker VM configuration generator with security validation."""

import json
import uuid
import re
from typing import Dict, Any, Optional
from pathlib import Path
import copy

import structlog

from config.settings import settings, VMManagerSettings

logger = structlog.get_logger(__name__)


class ValidationError(Exception):
    """Exception raised when configuration validation fails."""
    pass


class FirecrackerConfigGenerator:
    """Generates secure Firecracker VM configurations."""
    
    def __init__(self, base_config_path: str, vm_settings: Optional[VMManagerSettings] = None):
        self.base_config_path = Path(base_config_path)
        self.settings = vm_settings or settings
        self.default_config = {
            "boot-source": {
                "kernel_image_path": "/opt/firecracker/vmlinux.bin",
                "boot_args": "console=ttyS0 reboot=k panic=1 pci=off"
            },
            "drives": [],
            "network-interfaces": [],
            "machine-config": {
                "vcpu_count": 1,
                "mem_size_mib": 512,
                "ht_enabled": False  # Disabled for security
            },
            "logger": {
                "log_path": "/tmp/firecracker.log",
                "level": "Info"
            }
        }
    
    def generate_vm_config(
        self,
        vm_id: str,
        user_id: str,
        cpu_count: int = 1,
        memory_mb: int = 512,
        disk_size_gb: int = 10,
        template: Optional[str] = None,
        network_config: Optional[Dict] = None
    ) -> Dict[str, Any]:
        """Generate a secure Firecracker configuration for a VM."""
        
        # Validate inputs
        self._validate_vm_id(vm_id)
        self._validate_user_id(user_id)
        self._validate_resources(cpu_count, memory_mb, disk_size_gb)
        if template:
            self._validate_template(template)
        
        # Create a deep copy of default config to avoid mutation
        config = copy.deepcopy(self.default_config)
        
        # Apply resource limits (enforce maximums)
        safe_cpu_count = min(cpu_count, self.settings.max_cpu_per_vm)
        safe_memory_mb = min(memory_mb, self.settings.max_memory_mb_per_vm)
        safe_disk_gb = min(disk_size_gb, self.settings.max_disk_gb_per_vm)
        
        # Update machine configuration
        config["machine-config"]["vcpu_count"] = safe_cpu_count
        config["machine-config"]["mem_size_mib"] = safe_memory_mb
        
        # Configure root filesystem with secure path
        rootfs_path = self._build_secure_path(
            self.settings.vm_storage_base, user_id, vm_id, "rootfs.ext4"
        )
        
        root_drive = {
            "drive_id": "rootfs",
            "path_on_host": rootfs_path,
            "is_root_device": True,
            "is_read_only": False
        }
        config["drives"].append(root_drive)
        
        # Configure network interface if provided
        if network_config:
            self._validate_network_config(network_config)
            network_interface = {
                "iface_id": "eth0",
                "guest_mac": self._generate_mac_address(),
                "host_dev_name": network_config["tap_device"]
            }
            config["network-interfaces"].append(network_interface)
        
        # Configure secure logging
        log_path = f"/var/log/firecracker/{self._sanitize_filename(vm_id)}.log"
        config["logger"]["log_path"] = log_path
        
        logger.info(
            "Generated VM configuration",
            vm_id=vm_id,
            cpu_count=safe_cpu_count,
            memory_mb=safe_memory_mb,
            disk_gb=safe_disk_gb
        )
        
        return config
    
    def _validate_vm_id(self, vm_id: str):
        """Validate VM ID format (must be valid UUID)."""
        try:
            uuid.UUID(vm_id)
        except ValueError:
            raise ValidationError(f"Invalid VM ID format: {vm_id}")
    
    def _validate_user_id(self, user_id: str):
        """Validate user ID format (must be valid UUID)."""
        try:
            uuid.UUID(user_id)
        except ValueError:
            raise ValidationError(f"Invalid user ID format: {user_id}")
    
    def _validate_resources(self, cpu_count: int, memory_mb: int, disk_gb: int):
        """Validate resource parameters."""
        if not (1 <= cpu_count <= 16):
            raise ValidationError(f"CPU count must be between 1 and 16, got {cpu_count}")
        
        if not (512 <= memory_mb <= 16384):
            raise ValidationError(f"Memory must be between 512MB and 16GB, got {memory_mb}MB")
        
        if not (10 <= disk_gb <= 500):
            raise ValidationError(f"Disk size must be between 10GB and 500GB, got {disk_gb}GB")
    
    def _validate_template(self, template: str):
        """Validate template name for security."""
        # Check for path traversal attempts
        if ".." in template or "/" in template or "\\" in template:
            raise ValidationError(f"Invalid template name: {template}")
        
        # Check for special characters that could be used for injection
        if not re.match(r'^[a-zA-Z0-9._-]+$', template):
            raise ValidationError(f"Invalid template name: {template}")
        
        # Check length
        if len(template) > 100:
            raise ValidationError(f"Template name too long: {template}")
    
    def _validate_network_config(self, network_config: Dict):
        """Validate network configuration."""
        required_keys = ["tap_device"]
        for key in required_keys:
            if key not in network_config:
                raise ValidationError(f"Missing required network config key: {key}")
        
        # Validate TAP device name
        tap_device = network_config["tap_device"]
        if not re.match(r'^[a-zA-Z0-9._-]+$', tap_device):
            raise ValidationError(f"Invalid TAP device name: {tap_device}")
    
    def _build_secure_path(self, base: str, user_id: str, vm_id: str, filename: str) -> str:
        """Build a secure file path preventing traversal attacks."""
        # Sanitize all components
        safe_user_id = self._sanitize_filename(user_id)
        safe_vm_id = self._sanitize_filename(vm_id)
        safe_filename = self._sanitize_filename(filename)
        
        return str(Path(base) / safe_user_id / safe_vm_id / safe_filename)
    
    def _sanitize_filename(self, filename: str) -> str:
        """Sanitize filename to prevent path traversal and injection."""
        # Remove any path separators and special characters
        sanitized = re.sub(r'[^\w.-]', '', filename)
        
        # Remove leading dots and dashes
        sanitized = sanitized.lstrip('.-')
        
        # Limit length
        return sanitized[:100]
    
    def _generate_mac_address(self) -> str:
        """Generate a unique MAC address for the VM."""
        # Use locally administered unicast MAC (02:00:xx:xx:xx:xx)
        # This ensures no conflicts with real network interfaces
        mac_bytes = [0x02, 0x00]
        
        # Generate 4 random bytes for the rest of the MAC
        random_uuid = uuid.uuid4()
        hex_string = random_uuid.hex[:8]
        
        # Convert to bytes and add to MAC
        for i in range(0, 8, 2):
            mac_bytes.append(int(hex_string[i:i+2], 16))
        
        # Format as MAC address string
        return ':'.join(f'{b:02x}' for b in mac_bytes)