"""Settings and configuration for VM Manager service."""

import os
from pathlib import Path
from typing import List, Optional
from pydantic import Field
from pydantic_settings import BaseSettings


class VMManagerSettings(BaseSettings):
    """Configuration settings for VM Manager service."""
    
    # Service configuration
    host: str = Field(default="0.0.0.0", env="VM_MANAGER_HOST")
    port: int = Field(default=8002, env="VM_MANAGER_PORT")
    debug: bool = Field(default=False, env="DEBUG")
    log_level: str = Field(default="INFO", env="LOG_LEVEL")
    
    # Firecracker configuration
    firecracker_binary_path: str = Field(
        default="/usr/bin/firecracker", 
        env="FIRECRACKER_BINARY_PATH"
    )
    firecracker_socket_dir: str = Field(
        default="/tmp/firecracker-sockets", 
        env="FIRECRACKER_SOCKET_DIR"
    )
    firecracker_config_dir: str = Field(
        default="/etc/firecracker", 
        env="FIRECRACKER_CONFIG_DIR"
    )
    
    # VM configuration
    max_vms_per_user: int = Field(default=5, env="MAX_VMS_PER_USER")
    max_cpu_per_vm: int = Field(default=4, env="MAX_CPU_PER_VM")
    max_memory_mb_per_vm: int = Field(default=2048, env="MAX_MEMORY_MB_PER_VM")
    max_disk_gb_per_vm: int = Field(default=50, env="MAX_DISK_GB_PER_VM")
    
    # Storage configuration
    vm_storage_base: str = Field(
        default="/opt/vm-storage", 
        env="VM_STORAGE_BASE"
    )
    vm_template_dir: str = Field(
        default="/opt/vm-templates", 
        env="VM_TEMPLATE_DIR"
    )
    default_template: str = Field(
        default="ubuntu-22.04-base.ext4", 
        env="DEFAULT_VM_TEMPLATE"
    )
    
    # Network configuration
    bridge_name: str = Field(default="fc-bridge", env="VM_BRIDGE_NAME")
    vm_subnet: str = Field(default="10.0.100.0/20", env="VM_SUBNET")
    
    # Monitoring configuration
    health_check_interval: int = Field(default=30, env="HEALTH_CHECK_INTERVAL")
    vm_startup_timeout: int = Field(default=60, env="VM_STARTUP_TIMEOUT")
    vm_shutdown_timeout: int = Field(default=30, env="VM_SHUTDOWN_TIMEOUT")
    
    # Security configuration
    run_as_user: str = Field(default="firecracker", env="FIRECRACKER_USER")
    vm_permissions: str = Field(default="700", env="VM_STORAGE_PERMISSIONS")
    enable_selinux: bool = Field(default=True, env="ENABLE_SELINUX")
    enable_apparmor: bool = Field(default=True, env="ENABLE_APPARMOR")
    
    # Development/mock configuration
    use_mock_firecracker: bool = Field(default=False, env="USE_MOCK_FIRECRACKER")
    mock_vm_delay: float = Field(default=2.0, env="MOCK_VM_DELAY")
    
    class Config:
        env_file = ".env"
        env_file_encoding = "utf-8"
        case_sensitive = False


# Global settings instance
settings = VMManagerSettings()


# Validation functions
def validate_directories():
    """Validate that required directories exist and are accessible."""
    required_dirs = [
        settings.firecracker_socket_dir,
        settings.vm_storage_base,
        settings.vm_template_dir,
    ]
    
    for dir_path in required_dirs:
        path = Path(dir_path)
        if not path.exists():
            path.mkdir(parents=True, exist_ok=True)
        
        if not path.is_dir():
            raise ValueError(f"Required directory does not exist: {dir_path}")
        
        if not os.access(path, os.R_OK | os.W_OK):
            raise ValueError(f"Insufficient permissions for directory: {dir_path}")


def validate_firecracker_binary():
    """Validate that Firecracker binary exists and is executable."""
    if settings.use_mock_firecracker:
        return  # Skip validation for mock mode
    
    binary_path = Path(settings.firecracker_binary_path)
    
    if not binary_path.exists():
        raise ValueError(f"Firecracker binary not found: {settings.firecracker_binary_path}")
    
    if not binary_path.is_file():
        raise ValueError(f"Firecracker binary is not a file: {settings.firecracker_binary_path}")
    
    if not os.access(binary_path, os.X_OK):
        raise ValueError(f"Firecracker binary is not executable: {settings.firecracker_binary_path}")


def validate_templates():
    """Validate that VM templates exist."""
    template_dir = Path(settings.vm_template_dir)
    default_template = template_dir / settings.default_template
    
    if not default_template.exists():
        # Create a placeholder for development
        if settings.debug:
            default_template.touch()
            return
        
        raise ValueError(f"Default VM template not found: {default_template}")


def validate_all():
    """Run all validation checks."""
    validate_directories()
    validate_firecracker_binary()
    validate_templates()