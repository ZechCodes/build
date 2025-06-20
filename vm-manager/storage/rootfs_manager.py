"""Root filesystem management with security validation and template support."""

import os
import shutil
import subprocess
import asyncio
import uuid
import re
from pathlib import Path
from typing import Optional

import structlog

from config.settings import VMManagerSettings, settings

logger = structlog.get_logger(__name__)


class StorageError(Exception):
    """Exception raised when storage operations fail."""
    pass


class RootfsManager:
    """Manages root filesystem creation, resizing, and cleanup with security controls."""
    
    def __init__(
        self,
        storage_base: str = "/opt/vm-storage",
        template_dir: str = "/opt/vm-templates",
        default_template: str = "ubuntu-22.04-base.ext4",
        settings: Optional[VMManagerSettings] = None
    ):
        from config.settings import settings as default_settings
        self.settings = settings or default_settings
        
        # Validate and set paths
        self.storage_base = self._validate_storage_base(storage_base)
        self.template_path = self._validate_template_dir(template_dir)
        self.default_template = self._validate_template_name(default_template)
        
        # Ensure storage base exists
        self.storage_base.mkdir(parents=True, exist_ok=True)
    
    def _validate_storage_base(self, storage_base: str) -> Path:
        """Validate storage base directory."""
        if not storage_base:
            raise StorageError("Storage base cannot be empty")
        
        if not storage_base.startswith("/"):
            raise StorageError("Storage base must be an absolute path")
        
        # Check for path traversal
        if ".." in storage_base:
            raise StorageError("Storage base contains invalid path components")
        
        return Path(storage_base)
    
    def _validate_template_dir(self, template_dir: str) -> Path:
        """Validate template directory."""
        if not template_dir:
            raise StorageError("Template directory cannot be empty")
        
        if not template_dir.startswith("/"):
            raise StorageError("Template directory must be an absolute path")
        
        # Check for path traversal
        if ".." in template_dir:
            raise StorageError("Template directory contains invalid path components")
        
        return Path(template_dir)
    
    async def create_rootfs(
        self,
        user_id: str,
        vm_id: str,
        template: Optional[str] = None,
        size_gb: int = 10
    ) -> Optional[str]:
        """Create a root filesystem for a VM with security validation."""
        
        try:
            # Validate inputs
            self._validate_user_id(user_id)
            self._validate_vm_id(vm_id)
            
            template_name = template or self.default_template
            self._validate_template_name(template_name)
            
            # Validate size
            if not (10 <= size_gb <= self.settings.max_disk_gb_per_vm):
                raise StorageError(f"Invalid disk size: {size_gb}GB")
            
            # Check template exists
            template_file = self.template_path / template_name
            if not template_file.exists():
                logger.error("Template not found", template=template_name)
                return None
            
            # Create user and VM directories with secure paths
            safe_user_id = self._sanitize_path_component(user_id)
            safe_vm_id = self._sanitize_path_component(vm_id)
            
            user_dir = self.storage_base / safe_user_id
            user_dir.mkdir(parents=True, exist_ok=True)
            
            vm_dir = user_dir / safe_vm_id
            vm_dir.mkdir(exist_ok=True)
            
            rootfs_path = vm_dir / "rootfs.ext4"
            
            # Copy template to VM directory
            shutil.copy2(template_file, rootfs_path)
            
            # Resize filesystem if needed
            if size_gb > 10:  # Default template size
                await self._resize_filesystem(rootfs_path, size_gb)
            
            # Set secure permissions (owner only)
            os.chmod(rootfs_path, 0o600)
            os.chmod(vm_dir, 0o700)
            os.chmod(user_dir, 0o700)
            
            logger.info(
                "Root filesystem created successfully",
                user_id=user_id,
                vm_id=vm_id,
                template=template_name,
                size_gb=size_gb,
                path=str(rootfs_path)
            )
            
            return str(rootfs_path)
            
        except Exception as e:
            logger.error(
                "Failed to create root filesystem", 
                user_id=user_id, 
                vm_id=vm_id, 
                error=str(e)
            )
            
            # Clean up on failure
            try:
                if 'rootfs_path' in locals() and rootfs_path.exists():
                    rootfs_path.unlink()
                if 'vm_dir' in locals() and vm_dir.exists() and not any(vm_dir.iterdir()):
                    vm_dir.rmdir()
            except Exception as cleanup_error:
                logger.error("Failed to cleanup after error", error=str(cleanup_error))
            
            return None
    
    async def _resize_filesystem(self, rootfs_path: Path, size_gb: int):
        """Resize the filesystem to the specified size with validation."""
        
        try:
            # Resize the file
            subprocess.run([
                "truncate", "-s", f"{size_gb}G", str(rootfs_path)
            ], check=True)
            
            # Check and repair filesystem
            subprocess.run([
                "e2fsck", "-f", "-y", str(rootfs_path)
            ], check=True)
            
            # Resize the filesystem
            subprocess.run([
                "resize2fs", str(rootfs_path)
            ], check=True)
            
            logger.info(
                "Filesystem resized successfully", 
                path=str(rootfs_path), 
                size_gb=size_gb
            )
            
        except subprocess.CalledProcessError as e:
            logger.error("Failed to resize filesystem", path=str(rootfs_path), error=str(e))
            raise StorageError(f"Failed to resize filesystem: {e}")
    
    async def delete_rootfs(self, user_id: str, vm_id: str) -> bool:
        """Delete the root filesystem for a VM with validation."""
        
        try:
            # Validate inputs
            self._validate_user_id(user_id)
            self._validate_vm_id(vm_id)
            
            # Build secure paths
            safe_user_id = self._sanitize_path_component(user_id)
            safe_vm_id = self._sanitize_path_component(vm_id)
            
            vm_dir = self.storage_base / safe_user_id / safe_vm_id
            
            if vm_dir.exists():
                # Secure deletion
                shutil.rmtree(vm_dir)
                
                # Clean up empty user directory
                user_dir = vm_dir.parent
                try:
                    if user_dir.exists() and not any(user_dir.iterdir()):
                        user_dir.rmdir()
                except OSError:
                    # Directory not empty, which is fine
                    pass
                
                logger.info(
                    "Root filesystem deleted successfully",
                    user_id=user_id,
                    vm_id=vm_id,
                    path=str(vm_dir)
                )
                return True
            else:
                logger.warning(
                    "Root filesystem directory not found",
                    user_id=user_id,
                    vm_id=vm_id,
                    path=str(vm_dir)
                )
                return False
                
        except Exception as e:
            logger.error(
                "Failed to delete root filesystem", 
                user_id=user_id, 
                vm_id=vm_id, 
                error=str(e)
            )
            return False
    
    def _validate_user_id(self, user_id: str):
        """Validate user ID for security."""
        if not user_id:
            raise StorageError("User ID cannot be empty")
        
        # Check UUID format (more strict validation)
        try:
            uuid.UUID(user_id)
        except ValueError:
            raise StorageError(f"Invalid user ID format: {user_id}")
        
        # Additional security checks
        if len(user_id) > 100:
            raise StorageError(f"User ID too long: {user_id}")
        
        # Check for dangerous characters
        if not re.match(r'^[a-fA-F0-9-]+$', user_id):
            raise StorageError(f"Invalid user ID characters: {user_id}")
    
    def _validate_vm_id(self, vm_id: str):
        """Validate VM ID for security."""
        if not vm_id:
            raise StorageError("VM ID cannot be empty")
        
        # Check UUID format (more strict validation)
        try:
            uuid.UUID(vm_id)
        except ValueError:
            raise StorageError(f"Invalid VM ID format: {vm_id}")
        
        # Additional security checks
        if len(vm_id) > 100:
            raise StorageError(f"VM ID too long: {vm_id}")
        
        # Check for dangerous characters
        if not re.match(r'^[a-fA-F0-9-]+$', vm_id):
            raise StorageError(f"Invalid VM ID characters: {vm_id}")
    
    def _validate_template_name(self, template_name: str) -> str:
        """Validate template name for security."""
        if not template_name:
            raise StorageError("Template name cannot be empty")
        
        # Check for path traversal attempts
        if ".." in template_name or "/" in template_name or "\\" in template_name:
            raise StorageError(f"Invalid template name: {template_name}")
        
        # Check for special characters that could be used for injection
        if not re.match(r'^[a-zA-Z0-9._-]+$', template_name):
            raise StorageError(f"Invalid template name characters: {template_name}")
        
        # Check length
        if len(template_name) > 100:
            raise StorageError(f"Template name too long: {template_name}")
        
        return template_name
    
    def _sanitize_path_component(self, component: str) -> str:
        """Sanitize a path component to prevent traversal and injection."""
        # Remove any path separators and special characters
        sanitized = re.sub(r'[^\w.-]', '', component)
        
        # Remove leading dots and dashes
        sanitized = sanitized.lstrip('.-')
        
        # Limit length
        return sanitized[:100]
    
    def get_storage_info(self, user_id: str) -> dict:
        """Get storage information for a user."""
        try:
            self._validate_user_id(user_id)
            safe_user_id = self._sanitize_path_component(user_id)
            user_dir = self.storage_base / safe_user_id
            
            if not user_dir.exists():
                return {
                    "total_size": 0,
                    "vm_count": 0,
                    "vms": []
                }
            
            total_size = 0
            vms = []
            
            for vm_dir in user_dir.iterdir():
                if vm_dir.is_dir():
                    rootfs_file = vm_dir / "rootfs.ext4"
                    if rootfs_file.exists():
                        size = rootfs_file.stat().st_size
                        total_size += size
                        vms.append({
                            "vm_id": vm_dir.name,
                            "size_bytes": size,
                            "path": str(rootfs_file)
                        })
            
            return {
                "total_size": total_size,
                "vm_count": len(vms),
                "vms": vms
            }
            
        except Exception as e:
            logger.error("Failed to get storage info", user_id=user_id, error=str(e))
            return {"total_size": 0, "vm_count": 0, "vms": []}
    
    def list_templates(self) -> list:
        """List available VM templates."""
        try:
            if not self.template_path.exists():
                return []
            
            templates = []
            for template_file in self.template_path.glob("*.ext4"):
                templates.append({
                    "name": template_file.name,
                    "size_bytes": template_file.stat().st_size,
                    "modified": template_file.stat().st_mtime
                })
            
            return sorted(templates, key=lambda x: x["name"])
            
        except Exception as e:
            logger.error("Failed to list templates", error=str(e))
            return []