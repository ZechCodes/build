"""Tests for Firecracker configuration generator."""

import pytest
import json
import uuid
from pathlib import Path
from unittest.mock import patch, mock_open

from firecracker.config_generator import FirecrackerConfigGenerator, ValidationError


class TestFirecrackerConfigGenerator:
    """Test Firecracker configuration generation."""
    
    def test_init_with_valid_config_path(self, test_settings):
        """Test initialization with valid configuration path."""
        config_path = test_settings.firecracker_config_dir
        generator = FirecrackerConfigGenerator(config_path, test_settings)
        
        assert generator.base_config_path == Path(config_path)
        assert "boot-source" in generator.default_config
        assert "machine-config" in generator.default_config
        assert generator.default_config["machine-config"]["vcpu_count"] == 1
        assert generator.default_config["machine-config"]["mem_size_mib"] == 512
    
    def test_generate_vm_config_basic(self, test_settings):
        """Test basic VM configuration generation."""
        generator = FirecrackerConfigGenerator(test_settings.firecracker_config_dir, test_settings)
        
        vm_id = str(uuid.uuid4())
        user_id = str(uuid.uuid4())
        
        config = generator.generate_vm_config(
            vm_id=vm_id,
            user_id=user_id,
            cpu_count=2,
            memory_mb=1024,
            disk_size_gb=20
        )
        
        # Check machine configuration
        assert config["machine-config"]["vcpu_count"] == 2
        assert config["machine-config"]["mem_size_mib"] == 1024
        assert config["machine-config"]["ht_enabled"] is False
        
        # Check boot source
        assert "boot-source" in config
        assert "kernel_image_path" in config["boot-source"]
        
        # Check drives
        assert len(config["drives"]) == 1
        rootfs_drive = config["drives"][0]
        assert rootfs_drive["drive_id"] == "rootfs"
        assert rootfs_drive["is_root_device"] is True
        assert rootfs_drive["is_read_only"] is False
        assert f"{user_id}/{vm_id}/rootfs.ext4" in rootfs_drive["path_on_host"]
        
        # Check logger
        assert config["logger"]["log_path"] == f"/var/log/firecracker/{vm_id}.log"
        assert config["logger"]["level"] == "Info"
    
    def test_generate_vm_config_with_network(self, test_settings, sample_network_config):
        """Test VM configuration generation with network interface."""
        generator = FirecrackerConfigGenerator(test_settings.firecracker_config_dir, test_settings)
        
        vm_id = str(uuid.uuid4())
        user_id = str(uuid.uuid4())
        
        config = generator.generate_vm_config(
            vm_id=vm_id,
            user_id=user_id,
            network_config=sample_network_config
        )
        
        # Check network interface
        assert len(config["network-interfaces"]) == 1
        network_interface = config["network-interfaces"][0]
        assert network_interface["iface_id"] == "eth0"
        assert network_interface["host_dev_name"] == sample_network_config["tap_device"]
        assert "guest_mac" in network_interface
        assert network_interface["guest_mac"].startswith("02:00:")
    
    def test_generate_vm_config_resource_limits(self, test_settings):
        """Test that resource limits are enforced."""
        generator = FirecrackerConfigGenerator(test_settings.firecracker_config_dir, test_settings)
        
        vm_id = str(uuid.uuid4())
        user_id = str(uuid.uuid4())
        
        # Test CPU limit enforcement
        config = generator.generate_vm_config(
            vm_id=vm_id,
            user_id=user_id,
            cpu_count=8,  # Request more than max
            memory_mb=4096  # Request more than max
        )
        
        # Should be capped at max values from settings
        assert config["machine-config"]["vcpu_count"] == test_settings.max_cpu_per_vm
        assert config["machine-config"]["mem_size_mib"] == test_settings.max_memory_mb_per_vm
    
    def test_generate_mac_address_format(self, test_settings):
        """Test MAC address generation format."""
        generator = FirecrackerConfigGenerator(test_settings.firecracker_config_dir, test_settings)
        
        mac = generator._generate_mac_address()
        
        # Check format: XX:XX:XX:XX:XX:XX
        assert len(mac) == 17
        assert mac.count(":") == 5
        assert mac.startswith("02:00:")  # Local/unicast prefix
        
        # Check all parts are valid hex
        parts = mac.split(":")
        for part in parts:
            assert len(part) == 2
            int(part, 16)  # Should not raise ValueError
    
    def test_generate_mac_address_uniqueness(self, test_settings):
        """Test that generated MAC addresses are unique."""
        generator = FirecrackerConfigGenerator(test_settings.firecracker_config_dir, test_settings)
        
        macs = set()
        for _ in range(100):
            mac = generator._generate_mac_address()
            assert mac not in macs
            macs.add(mac)
    
    def test_validate_vm_id_format(self, test_settings):
        """Test VM ID validation."""
        generator = FirecrackerConfigGenerator(test_settings.firecracker_config_dir, test_settings)
        
        # Valid UUID should work
        valid_vm_id = str(uuid.uuid4())
        user_id = str(uuid.uuid4())
        
        config = generator.generate_vm_config(vm_id=valid_vm_id, user_id=user_id)
        assert config is not None
        
        # Invalid VM ID should raise error
        with pytest.raises(ValidationError, match="Invalid VM ID format"):
            generator.generate_vm_config(vm_id="invalid-id", user_id=user_id)
    
    def test_validate_user_id_format(self, test_settings):
        """Test user ID validation."""
        generator = FirecrackerConfigGenerator(test_settings.firecracker_config_dir, test_settings)
        
        vm_id = str(uuid.uuid4())
        
        # Invalid user ID should raise error
        with pytest.raises(ValidationError, match="Invalid user ID format"):
            generator.generate_vm_config(vm_id=vm_id, user_id="invalid-user")
    
    def test_validate_resource_parameters(self, test_settings):
        """Test validation of resource parameters."""
        generator = FirecrackerConfigGenerator(test_settings.firecracker_config_dir, test_settings)
        
        vm_id = str(uuid.uuid4())
        user_id = str(uuid.uuid4())
        
        # Invalid CPU count
        with pytest.raises(ValidationError, match="CPU count must be between"):
            generator.generate_vm_config(
                vm_id=vm_id, user_id=user_id, cpu_count=0
            )
        
        # Invalid memory
        with pytest.raises(ValidationError, match="Memory must be between"):
            generator.generate_vm_config(
                vm_id=vm_id, user_id=user_id, memory_mb=100
            )
        
        # Invalid disk size
        with pytest.raises(ValidationError, match="Disk size must be between"):
            generator.generate_vm_config(
                vm_id=vm_id, user_id=user_id, disk_size_gb=5
            )
    
    def test_security_validation(self, test_settings):
        """Test security-related configuration validation."""
        generator = FirecrackerConfigGenerator(test_settings.firecracker_config_dir, test_settings)
        
        vm_id = str(uuid.uuid4())
        user_id = str(uuid.uuid4())
        
        config = generator.generate_vm_config(vm_id=vm_id, user_id=user_id)
        
        # Check security settings
        assert config["machine-config"]["ht_enabled"] is False  # Hyperthreading disabled
        
        # Check that paths are properly sanitized
        rootfs_path = config["drives"][0]["path_on_host"]
        assert ".." not in rootfs_path  # No path traversal
        assert user_id in rootfs_path
        assert vm_id in rootfs_path
        
        # Check log path is secure
        log_path = config["logger"]["log_path"]
        assert log_path.startswith("/var/log/firecracker/")
        assert ".." not in log_path
    
    def test_template_validation(self, test_settings):
        """Test template name validation."""
        generator = FirecrackerConfigGenerator(test_settings.firecracker_config_dir, test_settings)
        
        vm_id = str(uuid.uuid4())
        user_id = str(uuid.uuid4())
        
        # Valid template should work
        config = generator.generate_vm_config(
            vm_id=vm_id, 
            user_id=user_id,
            template="ubuntu-22.04-base.ext4"
        )
        assert config is not None
        
        # Invalid template with path traversal should fail
        with pytest.raises(ValidationError, match="Invalid template name"):
            generator.generate_vm_config(
                vm_id=vm_id,
                user_id=user_id,
                template="../../../etc/passwd"
            )
        
        # Template with special characters should fail
        with pytest.raises(ValidationError, match="Invalid template name"):
            generator.generate_vm_config(
                vm_id=vm_id,
                user_id=user_id,
                template="template; rm -rf /"
            )
    
    def test_config_immutability(self, test_settings):
        """Test that the default config is not modified."""
        generator = FirecrackerConfigGenerator(test_settings.firecracker_config_dir, test_settings)
        
        original_config = generator.default_config.copy()
        
        vm_id = str(uuid.uuid4())
        user_id = str(uuid.uuid4())
        
        # Generate multiple configs
        for _ in range(3):
            generator.generate_vm_config(vm_id=vm_id, user_id=user_id)
        
        # Default config should remain unchanged
        assert generator.default_config == original_config