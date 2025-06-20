"""Tests for root filesystem management."""

import pytest
import asyncio
import shutil
import subprocess
from pathlib import Path
from unittest.mock import AsyncMock, patch, call, MagicMock, mock_open

from storage.rootfs_manager import RootfsManager, StorageError


@patch('storage.rootfs_manager.subprocess.run')
class TestRootfsManager:
    """Test root filesystem management functionality."""
    
    def test_init_default_config(self, mock_run, test_settings):
        """Test initialization with default configuration."""
        manager = RootfsManager(
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        assert manager.storage_base == Path(test_settings.vm_storage_base)
        assert manager.template_path == Path(test_settings.vm_template_dir)
        assert manager.default_template == test_settings.default_template
    
    def test_init_custom_config(self, mock_run, test_settings, temp_dir):
        """Test initialization with custom configuration."""
        custom_storage = str(temp_dir / "custom_storage")
        custom_template_dir = str(temp_dir / "custom_templates")
        custom_template = "custom-template.ext4"
        
        manager = RootfsManager(
            storage_base=custom_storage,
            template_dir=custom_template_dir,
            default_template=custom_template,
            settings=test_settings
        )
        
        assert manager.storage_base == Path(custom_storage)
        assert manager.template_path == Path(custom_template_dir)
        assert manager.default_template == custom_template
    
    @pytest.mark.asyncio
    async def test_create_rootfs_success(self, mock_run, test_settings, temp_dir):
        """Test successful root filesystem creation."""
        mock_run.return_value = MagicMock(returncode=0)
        
        # Setup template file
        template_dir = temp_dir / "templates"
        template_dir.mkdir(exist_ok=True)
        template_file = template_dir / test_settings.default_template
        template_file.write_text("mock template content")
        
        storage_dir = temp_dir / "storage"
        
        manager = RootfsManager(
            storage_base=str(storage_dir),
            template_dir=str(template_dir),
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        user_id = "12345678-1234-1234-1234-123456789012"
        vm_id = "87654321-4321-4321-4321-210987654321"
        
        with patch('shutil.copy2') as mock_copy, patch('storage.rootfs_manager.os.chmod'):
            result = await manager.create_rootfs(user_id, vm_id)
        
        # Should return the path to the created rootfs
        assert result is not None
        assert user_id in result
        assert vm_id in result
        assert "rootfs.ext4" in result
        
        # Should copy template to target location
        mock_copy.assert_called_once()
        
        # Verify directory structure creation
        expected_path = storage_dir / user_id / vm_id
        assert expected_path.exists()
    
    @pytest.mark.asyncio
    async def test_create_rootfs_with_template(self, mock_run, test_settings, temp_dir):
        """Test root filesystem creation with specific template."""
        mock_run.return_value = MagicMock(returncode=0)
        
        # Setup custom template
        template_dir = temp_dir / "templates"
        template_dir.mkdir(exist_ok=True)
        custom_template = "custom-ubuntu.ext4"
        template_file = template_dir / custom_template
        template_file.write_text("custom template content")
        
        storage_dir = temp_dir / "storage"
        
        manager = RootfsManager(
            storage_base=str(storage_dir),
            template_dir=str(template_dir),
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        user_id = "12345678-1234-1234-1234-123456789012"
        vm_id = "87654321-4321-4321-4321-210987654321"
        
        with patch('shutil.copy2') as mock_copy, patch('storage.rootfs_manager.os.chmod'):
            result = await manager.create_rootfs(
                user_id, vm_id, template=custom_template
            )
        
        assert result is not None
        mock_copy.assert_called_once()
    
    @pytest.mark.asyncio
    async def test_create_rootfs_with_resize(self, mock_run, test_settings, temp_dir):
        """Test root filesystem creation with resizing."""
        mock_run.return_value = MagicMock(returncode=0)
        
        # Setup template
        template_dir = temp_dir / "templates"
        template_dir.mkdir(exist_ok=True)
        template_file = template_dir / test_settings.default_template
        template_file.write_text("template content")
        
        storage_dir = temp_dir / "storage"
        
        manager = RootfsManager(
            storage_base=str(storage_dir),
            template_dir=str(template_dir),
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        user_id = "12345678-1234-1234-1234-123456789012"
        vm_id = "87654321-4321-4321-4321-210987654321"
        
        with patch('shutil.copy2') as mock_copy, patch('storage.rootfs_manager.os.chmod'):
            result = await manager.create_rootfs(
                user_id, vm_id, size_gb=20  # Larger than default 10GB
            )
        
        assert result is not None
        
        # Should call resize commands
        resize_calls = [call for call in mock_run.call_args_list 
                       if any('truncate' in str(arg) or 'e2fsck' in str(arg) or 'resize2fs' in str(arg) 
                             for arg in call[0]) if call[0]]
        assert len(resize_calls) >= 3  # truncate, e2fsck, resize2fs
    
    @pytest.mark.asyncio
    async def test_create_rootfs_template_not_found(self, mock_run, test_settings, temp_dir):
        """Test root filesystem creation with missing template."""
        storage_dir = temp_dir / "storage"
        template_dir = temp_dir / "templates"
        template_dir.mkdir(exist_ok=True)
        
        manager = RootfsManager(
            storage_base=str(storage_dir),
            template_dir=str(template_dir),
            default_template="nonexistent.ext4",
            settings=test_settings
        )
        
        user_id = "12345678-1234-1234-1234-123456789012"
        vm_id = "87654321-4321-4321-4321-210987654321"
        
        result = await manager.create_rootfs(user_id, vm_id)
        
        assert result is None
    
    @pytest.mark.asyncio
    async def test_create_rootfs_copy_failure(self, mock_run, test_settings, temp_dir):
        """Test root filesystem creation with copy failure."""
        # Setup template
        template_dir = temp_dir / "templates"
        template_dir.mkdir(exist_ok=True)
        template_file = template_dir / test_settings.default_template
        template_file.write_text("template content")
        
        storage_dir = temp_dir / "storage"
        
        manager = RootfsManager(
            storage_base=str(storage_dir),
            template_dir=str(template_dir),
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        user_id = "12345678-1234-1234-1234-123456789012"
        vm_id = "87654321-4321-4321-4321-210987654321"
        
        with patch('shutil.copy2', side_effect=OSError("Copy failed")), patch('storage.rootfs_manager.os.chmod'):
            result = await manager.create_rootfs(user_id, vm_id)
        
        assert result is None
    
    @pytest.mark.asyncio
    async def test_resize_filesystem_success(self, mock_run, test_settings, temp_dir):
        """Test successful filesystem resizing."""
        mock_run.return_value = MagicMock(returncode=0)
        
        storage_dir = temp_dir / "storage"
        template_dir = temp_dir / "templates"
        
        manager = RootfsManager(
            storage_base=str(storage_dir),
            template_dir=str(template_dir),
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        rootfs_path = temp_dir / "test.ext4"
        rootfs_path.touch()
        
        await manager._resize_filesystem(rootfs_path, 20)
        
        # Should call truncate, e2fsck, and resize2fs
        expected_calls = [
            call(["truncate", "-s", "20G", str(rootfs_path)], check=True),
            call(["e2fsck", "-f", "-y", str(rootfs_path)], check=True),
            call(["resize2fs", str(rootfs_path)], check=True)
        ]
        
        mock_run.assert_has_calls(expected_calls)
    
    @pytest.mark.asyncio
    async def test_resize_filesystem_failure(self, mock_run, test_settings, temp_dir):
        """Test filesystem resizing failure handling."""
        mock_run.side_effect = subprocess.CalledProcessError(1, "e2fsck")
        
        storage_dir = temp_dir / "storage"
        template_dir = temp_dir / "templates"
        
        manager = RootfsManager(
            storage_base=str(storage_dir),
            template_dir=str(template_dir),
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        rootfs_path = temp_dir / "test.ext4"
        rootfs_path.touch()
        
        with pytest.raises(StorageError):
            await manager._resize_filesystem(rootfs_path, 20)
    
    @pytest.mark.asyncio
    async def test_delete_rootfs_success(self, mock_run, test_settings, temp_dir):
        """Test successful root filesystem deletion."""
        storage_dir = temp_dir / "storage"
        
        manager = RootfsManager(
            storage_base=str(storage_dir),
            template_dir=str(temp_dir / "templates"),
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        user_id = "12345678-1234-1234-1234-123456789012"
        vm_id = "87654321-4321-4321-4321-210987654321"
        
        # Create VM directory structure
        vm_dir = storage_dir / user_id / vm_id
        vm_dir.mkdir(parents=True)
        rootfs_file = vm_dir / "rootfs.ext4"
        rootfs_file.touch()
        
        result = await manager.delete_rootfs(user_id, vm_id)
        
        assert result is True
        assert not vm_dir.exists()
    
    @pytest.mark.asyncio
    async def test_delete_rootfs_not_found(self, mock_run, test_settings, temp_dir):
        """Test root filesystem deletion when directory doesn't exist."""
        storage_dir = temp_dir / "storage"
        
        manager = RootfsManager(
            storage_base=str(storage_dir),
            template_dir=str(temp_dir / "templates"),
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        user_id = "12345678-1234-1234-1234-123456789012"
        vm_id = "87654321-4321-4321-4321-210987654321"
        
        result = await manager.delete_rootfs(user_id, vm_id)
        
        assert result is False
    
    @pytest.mark.asyncio
    async def test_delete_rootfs_permission_error(self, mock_run, test_settings, temp_dir):
        """Test root filesystem deletion with permission error."""
        storage_dir = temp_dir / "storage"
        
        manager = RootfsManager(
            storage_base=str(storage_dir),
            template_dir=str(temp_dir / "templates"),
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        user_id = "12345678-1234-1234-1234-123456789012"
        vm_id = "87654321-4321-4321-4321-210987654321"
        
        # Create VM directory
        vm_dir = storage_dir / user_id / vm_id
        vm_dir.mkdir(parents=True)
        
        with patch('shutil.rmtree', side_effect=PermissionError("Access denied")):
            result = await manager.delete_rootfs(user_id, vm_id)
        
        assert result is False
    
    def test_validate_user_id(self, mock_run, test_settings):
        """Test user ID validation."""
        manager = RootfsManager(
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        # Valid user ID should pass
        valid_user_id = "12345678-1234-1234-1234-123456789012"
        manager._validate_user_id(valid_user_id)  # Should not raise
        
        # Invalid user IDs should fail
        invalid_ids = [
            "",
            "short",
            "contains spaces",
            "contains/slash",
            "../path-traversal",
            "very-long-" + "x" * 100 + "-user-id"
        ]
        
        for invalid_id in invalid_ids:
            with pytest.raises(StorageError):
                manager._validate_user_id(invalid_id)
    
    def test_validate_vm_id(self, mock_run, test_settings):
        """Test VM ID validation."""
        manager = RootfsManager(
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        # Valid VM ID should pass
        valid_vm_id = "87654321-4321-4321-4321-210987654321"
        manager._validate_vm_id(valid_vm_id)  # Should not raise
        
        # Invalid VM IDs should fail
        invalid_ids = [
            "",
            "short",
            "contains spaces",
            "contains/slash",
            "../path-traversal",
            "very-long-" + "x" * 100 + "-vm-id"
        ]
        
        for invalid_id in invalid_ids:
            with pytest.raises(StorageError):
                manager._validate_vm_id(invalid_id)
    
    def test_validate_template_name(self, mock_run, test_settings):
        """Test template name validation."""
        manager = RootfsManager(
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        # Valid template names should pass
        valid_names = [
            "ubuntu-22.04.ext4",
            "debian-12.ext4",
            "centos-8.img"
        ]
        
        for name in valid_names:
            manager._validate_template_name(name)  # Should not raise
        
        # Invalid template names should fail
        invalid_names = [
            "",
            "../etc/passwd",
            "template; rm -rf /",
            "very-long-" + "x" * 100 + "-template.ext4",
            "template with spaces.ext4"
        ]
        
        for name in invalid_names:
            with pytest.raises(StorageError):
                manager._validate_template_name(name)
    
    def test_sanitize_path_component(self, mock_run, test_settings):
        """Test path component sanitization."""
        manager = RootfsManager(
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        # Test normal components
        assert manager._sanitize_path_component("normal-name") == "normal-name"
        assert manager._sanitize_path_component("test123") == "test123"
        
        # Test sanitization
        assert manager._sanitize_path_component("path/with/slashes") == "pathwithslashes"
        assert manager._sanitize_path_component("../traversal") == "traversal"
        assert manager._sanitize_path_component("name with spaces") == "namewithspaces"
        
        # Test length limiting
        long_name = "x" * 200
        sanitized = manager._sanitize_path_component(long_name)
        assert len(sanitized) <= 100
    
    def test_security_features(self, mock_run, test_settings):
        """Test security-related features."""
        manager = RootfsManager(
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        # Test storage base validation
        with pytest.raises(StorageError):
            RootfsManager(
                storage_base="",
                template_dir=test_settings.vm_template_dir,
                default_template=test_settings.default_template,
                settings=test_settings
            )
        
        # Test template directory validation
        with pytest.raises(StorageError):
            RootfsManager(
                storage_base=test_settings.vm_storage_base,
                template_dir="",
                default_template=test_settings.default_template,
                settings=test_settings
            )
    
    @pytest.mark.asyncio
    async def test_concurrent_operations(self, mock_run, test_settings, temp_dir):
        """Test concurrent root filesystem operations."""
        mock_run.return_value = MagicMock(returncode=0)
        
        # Setup templates
        template_dir = temp_dir / "templates"
        template_dir.mkdir(exist_ok=True)
        template_file = template_dir / test_settings.default_template
        template_file.write_text("template content")
        
        storage_dir = temp_dir / "storage"
        
        manager = RootfsManager(
            storage_base=str(storage_dir),
            template_dir=str(template_dir),
            default_template=test_settings.default_template,
            settings=test_settings
        )
        
        # Create multiple rootfs concurrently
        user_ids = [f"{i:08d}-1234-1234-1234-123456789012" for i in range(3)]
        vm_ids = [f"{i:08d}-4321-4321-4321-210987654321" for i in range(3)]
        
        with patch('shutil.copy2'), patch('storage.rootfs_manager.os.chmod'):
            tasks = [
                manager.create_rootfs(user_id, vm_id) 
                for user_id, vm_id in zip(user_ids, vm_ids)
            ]
            results = await asyncio.gather(*tasks)
        
        # All should succeed
        assert all(result is not None for result in results)
        
        # All should have unique paths
        assert len(set(results)) == len(results)