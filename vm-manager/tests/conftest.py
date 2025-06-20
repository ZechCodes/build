"""Test configuration and fixtures for VM Manager tests."""

import pytest
import tempfile
import shutil
from pathlib import Path
from unittest.mock import patch

from config.settings import VMManagerSettings


@pytest.fixture
def temp_dir():
    """Create a temporary directory for tests."""
    temp_path = tempfile.mkdtemp()
    yield Path(temp_path)
    shutil.rmtree(temp_path, ignore_errors=True)


@pytest.fixture
def test_settings(temp_dir):
    """Test settings with temporary directories."""
    settings = VMManagerSettings(
        debug=True,
        use_mock_firecracker=True,
        firecracker_socket_dir=str(temp_dir / "sockets"),
        vm_storage_base=str(temp_dir / "storage"),
        vm_template_dir=str(temp_dir / "templates"),
        firecracker_config_dir=str(temp_dir / "config"),
        bridge_name="test-bridge",
        vm_subnet="10.1.0.0/24",
        max_vms_per_user=3,
        max_cpu_per_vm=2,
        max_memory_mb_per_vm=1024,
        max_disk_gb_per_vm=20
    )
    
    # Create required directories
    for dir_path in [
        settings.firecracker_socket_dir,
        settings.vm_storage_base,
        settings.vm_template_dir,
        settings.firecracker_config_dir,
    ]:
        Path(dir_path).mkdir(parents=True, exist_ok=True)
    
    # Create default template file
    template_path = Path(settings.vm_template_dir) / settings.default_template
    template_path.touch()
    
    return settings


@pytest.fixture
def mock_settings(test_settings):
    """Mock the global settings with test settings."""
    with patch('config.settings.settings', test_settings):
        yield test_settings


@pytest.fixture
def sample_vm_config():
    """Sample VM configuration for testing."""
    return {
        "cpu_count": 2,
        "memory_mb": 1024,
        "disk_gb": 20,
        "template": "ubuntu-22.04-base.ext4"
    }


@pytest.fixture
def sample_network_config():
    """Sample network configuration for testing."""
    return {
        "tap_device": "fc-tap-test123",
        "ip_address": "10.1.0.100",
        "bridge": "test-bridge"
    }