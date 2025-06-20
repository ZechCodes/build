"""Tests for TAP device management."""

import pytest
import asyncio
import ipaddress
import subprocess
from unittest.mock import AsyncMock, patch, call, MagicMock
from pathlib import Path

from networking.tap_manager import TAPDeviceManager, NetworkError


@patch('networking.tap_manager.subprocess.run')
class TestTAPDeviceManager:
    """Test TAP device management functionality."""
    
    def test_init_default_config(self, mock_run, test_settings):
        """Test initialization with default configuration."""
        mock_run.return_value = MagicMock(returncode=0)
        
        manager = TAPDeviceManager(
            bridge_name=test_settings.bridge_name,
            subnet=test_settings.vm_subnet,
            settings=test_settings
        )
        
        assert manager.bridge_name == test_settings.bridge_name
        assert manager.subnet == ipaddress.IPv4Network(test_settings.vm_subnet)
        assert len(manager.allocated_ips) == 0
    
    def test_init_custom_config(self, mock_run, test_settings):
        """Test initialization with custom configuration."""
        mock_run.return_value = MagicMock(returncode=0)
        
        custom_bridge = "custom-bridge"
        custom_subnet = "192.168.100.0/24"
        
        manager = TAPDeviceManager(
            bridge_name=custom_bridge,
            subnet=custom_subnet,
            settings=test_settings
        )
        
        assert manager.bridge_name == custom_bridge
        assert manager.subnet == ipaddress.IPv4Network(custom_subnet)
    
    def test_setup_bridge_success(self, mock_run, test_settings):
        """Test successful bridge setup."""
        mock_run.return_value = MagicMock(returncode=0)
        
        manager = TAPDeviceManager(
            bridge_name=test_settings.bridge_name,
            subnet=test_settings.vm_subnet,
            settings=test_settings
        )
        
        # Should create bridge, assign IP, and bring up
        expected_calls = [
            call(["ip", "link", "add", "name", test_settings.bridge_name, "type", "bridge"], check=False),
            call(["ip", "addr", "add", f"{list(manager.subnet.hosts())[0]}/{manager.subnet.prefixlen}", "dev", test_settings.bridge_name], check=False),
            call(["ip", "link", "set", "dev", test_settings.bridge_name, "up"], check=True)
        ]
        
        mock_run.assert_has_calls(expected_calls)
    
    def test_setup_bridge_failure(self, mock_run, test_settings):
        """Test bridge setup failure handling."""
        mock_run.side_effect = [
            MagicMock(returncode=0),  # Bridge creation succeeds
            MagicMock(returncode=0),  # IP assignment succeeds
            subprocess.CalledProcessError(1, "ip link set")  # Bringing up fails
        ]
        
        with pytest.raises(NetworkError, match="Failed to setup bridge"):
            TAPDeviceManager(
                bridge_name=test_settings.bridge_name,
                subnet=test_settings.vm_subnet,
                settings=test_settings
            )
    
    @pytest.mark.asyncio
    async def test_create_tap_device_success(self, mock_run, test_settings):
        """Test successful TAP device creation."""
        mock_run.return_value = MagicMock(returncode=0)
        
        manager = TAPDeviceManager(
            bridge_name=test_settings.bridge_name,
            subnet=test_settings.vm_subnet,
            settings=test_settings
        )
        
        vm_id = "test-vm-12345678-1234-1234-1234-123456789012"
        
        result = await manager.create_tap_device(vm_id)
        
        assert result is not None
        assert "tap_device" in result
        assert "ip_address" in result
        assert "bridge" in result
        assert result["bridge"] == test_settings.bridge_name
        assert result["tap_device"] == f"fc-tap-{vm_id[:8]}"
        
        # Verify IP address is in subnet
        ip = ipaddress.IPv4Address(result["ip_address"])
        assert ip in manager.subnet
        
        # Verify TAP device creation commands
        expected_tap_name = f"fc-tap-{vm_id[:8]}"
        expected_calls = [
            call(["ip", "tuntap", "add", "dev", expected_tap_name, "mode", "tap"], check=True),
            call(["ip", "link", "set", "dev", expected_tap_name, "master", test_settings.bridge_name], check=True),
            call(["ip", "link", "set", "dev", expected_tap_name, "up"], check=True)
        ]
        
        # Skip bridge setup calls and check TAP creation calls
        tap_calls = mock_run.call_args_list[-3:]
        assert tap_calls == expected_calls
    
    @pytest.mark.asyncio
    async def test_create_tap_device_failure(self, mock_run, test_settings):
        """Test TAP device creation failure handling."""
        # Bridge setup succeeds, TAP creation fails
        mock_run.side_effect = [
            MagicMock(returncode=0),  # Bridge creation
            MagicMock(returncode=0),  # IP assignment
            MagicMock(returncode=0),  # Bridge up
            subprocess.CalledProcessError(1, "ip tuntap add")  # TAP creation fails
        ]
        
        manager = TAPDeviceManager(
            bridge_name=test_settings.bridge_name,
            subnet=test_settings.vm_subnet,
            settings=test_settings
        )
        
        vm_id = "test-vm-12345678"
        
        result = await manager.create_tap_device(vm_id)
        
        assert result is None
    
    @pytest.mark.asyncio
    async def test_cleanup_tap_device_success(self, mock_run, test_settings):
        """Test successful TAP device cleanup."""
        mock_run.return_value = MagicMock(returncode=0)
        
        manager = TAPDeviceManager(
            bridge_name=test_settings.bridge_name,
            subnet=test_settings.vm_subnet,
            settings=test_settings
        )
        
        tap_name = "fc-tap-test123"
        
        result = await manager.cleanup_tap_device(tap_name)
        
        assert result is True
        
        # Verify cleanup command (skip bridge setup calls)
        cleanup_call = mock_run.call_args_list[-1]
        expected_call = call(["ip", "link", "delete", "dev", tap_name], check=True)
        assert cleanup_call == expected_call
    
    @pytest.mark.asyncio
    async def test_cleanup_tap_device_failure(self, mock_run, test_settings):
        """Test TAP device cleanup failure handling."""
        # Bridge setup succeeds, cleanup fails
        mock_run.side_effect = [
            MagicMock(returncode=0),  # Bridge creation
            MagicMock(returncode=0),  # IP assignment
            MagicMock(returncode=0),  # Bridge up
            subprocess.CalledProcessError(1, "ip link delete")  # Cleanup fails
        ]
        
        manager = TAPDeviceManager(
            bridge_name=test_settings.bridge_name,
            subnet=test_settings.vm_subnet,
            settings=test_settings
        )
        
        tap_name = "fc-tap-test123"
        
        result = await manager.cleanup_tap_device(tap_name)
        
        assert result is False
    
    def test_allocate_ip_address(self, mock_run, test_settings):
        """Test IP address allocation."""
        mock_run.return_value = MagicMock(returncode=0)
        
        manager = TAPDeviceManager(
            bridge_name=test_settings.bridge_name,
            subnet=test_settings.vm_subnet,
            settings=test_settings
        )
        
        # Allocate first IP
        ip1 = manager._allocate_ip()
        assert ip1 in [str(ip) for ip in manager.subnet.hosts()]
        assert ip1 in manager.allocated_ips
        
        # Allocate second IP (should be different)
        ip2 = manager._allocate_ip()
        assert ip2 != ip1
        assert ip2 in manager.allocated_ips
        
        # Both IPs should be allocated
        assert len(manager.allocated_ips) == 2
    
    def test_allocate_ip_exhaustion(self, mock_run, test_settings):
        """Test IP address allocation exhaustion."""
        mock_run.return_value = MagicMock(returncode=0)
        
        # Use a very small subnet for testing
        small_subnet = "10.1.0.0/30"  # Only 2 usable hosts (minus 1 for bridge = 1 available)
        
        manager = TAPDeviceManager(
            bridge_name=test_settings.bridge_name,
            subnet=small_subnet,
            settings=test_settings
        )
        
        # Allocate all available IPs (skip bridge IP)
        available_ips = list(manager.subnet.hosts())[1:]  # Skip bridge IP
        allocated = []
        
        for _ in range(len(available_ips)):
            ip = manager._allocate_ip()
            allocated.append(ip)
        
        # Next allocation should fail
        with pytest.raises(NetworkError, match="No available IP addresses"):
            manager._allocate_ip()
    
    def test_release_ip_address(self, mock_run, test_settings):
        """Test IP address release."""
        mock_run.return_value = MagicMock(returncode=0)
        
        manager = TAPDeviceManager(
            bridge_name=test_settings.bridge_name,
            subnet=test_settings.vm_subnet,
            settings=test_settings
        )
        
        # Allocate IP
        ip = manager._allocate_ip()
        assert ip in manager.allocated_ips
        
        # Release IP
        manager.release_ip(ip)
        assert ip not in manager.allocated_ips
        
        # Should be able to allocate same IP again
        ip2 = manager._allocate_ip()
        assert ip2 == ip
    
    def test_validate_vm_id(self, mock_run, test_settings):
        """Test VM ID validation."""
        mock_run.return_value = MagicMock(returncode=0)
        
        manager = TAPDeviceManager(
            bridge_name=test_settings.bridge_name,
            subnet=test_settings.vm_subnet,
            settings=test_settings
        )
        
        # Valid VM ID should pass
        valid_vm_id = "12345678-1234-1234-1234-123456789012"
        manager._validate_vm_id(valid_vm_id)  # Should not raise
        
        # Invalid VM IDs should fail
        invalid_ids = [
            "",
            "short",
            "contains spaces",
            "contains/slash",
            "contains\\backslash",
            "../path-traversal",
            "very-long-" + "x" * 100 + "-vm-id"
        ]
        
        for invalid_id in invalid_ids:
            with pytest.raises(NetworkError):
                manager._validate_vm_id(invalid_id)
    
    def test_validate_tap_name(self, mock_run, test_settings):
        """Test TAP device name validation."""
        mock_run.return_value = MagicMock(returncode=0)
        
        manager = TAPDeviceManager(
            bridge_name=test_settings.bridge_name,
            subnet=test_settings.vm_subnet,
            settings=test_settings
        )
        
        # Valid TAP names should pass
        valid_names = [
            "fc-tap-12345678",
            "test-tap-device",
            "tap123"
        ]
        
        for name in valid_names:
            manager._validate_tap_name(name)  # Should not raise
        
        # Invalid TAP names should fail
        invalid_names = [
            "",
            "contains spaces",
            "contains/slash",
            "contains\\backslash",
            "../path-traversal",
            "very-long-" + "x" * 100 + "-tap-name"
        ]
        
        for name in invalid_names:
            with pytest.raises(NetworkError):
                manager._validate_tap_name(name)
    
    @pytest.mark.asyncio
    async def test_concurrent_tap_creation(self, mock_run, test_settings):
        """Test concurrent TAP device creation."""
        mock_run.return_value = MagicMock(returncode=0)
        
        manager = TAPDeviceManager(
            bridge_name=test_settings.bridge_name,
            subnet=test_settings.vm_subnet,
            settings=test_settings
        )
        
        # Create multiple TAP devices concurrently
        vm_ids = [f"vm-{i:08d}-1234-1234-1234-123456789012" for i in range(5)]
        
        tasks = [manager.create_tap_device(vm_id) for vm_id in vm_ids]
        results = await asyncio.gather(*tasks)
        
        # All should succeed
        assert all(result is not None for result in results)
        
        # All should have different IP addresses
        ips = [result["ip_address"] for result in results]
        assert len(set(ips)) == len(ips)
        
        # All should have correct TAP device names
        for i, result in enumerate(results):
            expected_tap = f"fc-tap-{vm_ids[i][:8]}"
            assert result["tap_device"] == expected_tap
    
    def test_security_features(self, mock_run, test_settings):
        """Test security-related features."""
        mock_run.return_value = MagicMock(returncode=0)
        
        manager = TAPDeviceManager(
            bridge_name=test_settings.bridge_name,
            subnet=test_settings.vm_subnet,
            settings=test_settings
        )
        
        # Bridge name should be validated
        with pytest.raises(NetworkError):
            TAPDeviceManager(
                bridge_name="../etc/passwd",
                subnet=test_settings.vm_subnet,
                settings=test_settings
            )
        
        # Subnet should be validated
        with pytest.raises(ValueError):
            TAPDeviceManager(
                bridge_name=test_settings.bridge_name,
                subnet="invalid.subnet",
                settings=test_settings
            )