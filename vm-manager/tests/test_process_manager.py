"""Tests for Firecracker process management."""

import pytest
import asyncio
import subprocess
import signal
import os
from unittest.mock import AsyncMock, patch, call, MagicMock
from pathlib import Path

from firecracker.process_manager import FirecrackerProcessManager, ProcessError


@patch('firecracker.process_manager.asyncio.create_subprocess_exec')
@patch('networking.tap_manager.subprocess.run')
class TestFirecrackerProcessManager:
    """Test Firecracker process management functionality."""
    
    def test_init_default_config(self, mock_subprocess_run, mock_create_subprocess, test_settings):
        """Test initialization with default configuration."""
        manager = FirecrackerProcessManager(
            socket_dir=test_settings.firecracker_socket_dir,
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            settings=test_settings
        )
        
        assert len(manager.processes) == 0
        assert manager.config_generator is not None
        assert manager.tap_manager is not None
        assert manager.rootfs_manager is not None
    
    @pytest.mark.asyncio
    async def test_start_vm_success(self, mock_subprocess_run, mock_create_subprocess, test_settings, temp_dir):
        """Test successful VM startup."""
        # Setup mock process
        mock_process = MagicMock()
        mock_process.pid = 12345
        mock_create_subprocess.return_value = mock_process
        
        # Setup template and storage
        template_dir = temp_dir / "templates"
        template_dir.mkdir(exist_ok=True)
        template_file = template_dir / test_settings.default_template
        template_file.write_text("template content")
        
        storage_dir = temp_dir / "storage"
        socket_dir = temp_dir / "sockets"
        socket_dir.mkdir(exist_ok=True)
        
        manager = FirecrackerProcessManager(
            storage_base=str(storage_dir),
            template_dir=str(template_dir),
            socket_dir=str(socket_dir),
            settings=test_settings
        )
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        user_id = "87654321-4321-4321-4321-210987654321"
        vm_config = {
            "cpu_count": 2,
            "memory_mb": 1024,
            "disk_gb": 20
        }
        
        with patch('subprocess.run'), \
             patch('shutil.copy2'), \
             patch('firecracker.process_manager.os.chmod'):
            
            result = await manager.start_vm(vm_id, user_id, vm_config)
        
        assert result is True
        assert vm_id in manager.processes
        assert manager.processes[vm_id]["process"] == mock_process
        assert manager.processes[vm_id]["status"] == "running"
        
        # Verify process was started with correct arguments
        mock_create_subprocess.assert_called_once()
        call_args = mock_create_subprocess.call_args
        # Since we're using mock_firecracker=True, it should use sleep command
        if test_settings.use_mock_firecracker:
            assert "sleep" in call_args[0]
        else:
            assert "/usr/bin/firecracker" in call_args[0] or "firecracker" in call_args[0]
            assert "--api-sock" in call_args[0]
            assert "--config-file" in call_args[0]
    
    @pytest.mark.asyncio
    async def test_start_vm_with_network(self, mock_subprocess_run, mock_create_subprocess, test_settings, temp_dir):
        """Test VM startup with network configuration."""
        mock_process = MagicMock()
        mock_process.pid = 12345
        mock_create_subprocess.return_value = mock_process
        
        # Setup directories
        template_dir = temp_dir / "templates"
        template_dir.mkdir(exist_ok=True)
        template_file = template_dir / test_settings.default_template
        template_file.write_text("template content")
        
        storage_dir = temp_dir / "storage"
        socket_dir = temp_dir / "sockets"
        socket_dir.mkdir(exist_ok=True)
        
        manager = FirecrackerProcessManager(
            storage_base=str(storage_dir),
            template_dir=str(template_dir),
            socket_dir=str(socket_dir),
            settings=test_settings
        )
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        user_id = "87654321-4321-4321-4321-210987654321"
        vm_config = {
            "cpu_count": 1,
            "memory_mb": 512,
            "disk_gb": 10,
            "enable_network": True
        }
        
        with patch('subprocess.run'), \
             patch('shutil.copy2'), \
             patch('firecracker.process_manager.os.chmod'), \
             patch.object(manager.tap_manager, 'create_tap_device', return_value={
                 "tap_device": "fc-tap-12345678",
                 "ip_address": "10.0.100.100",
                 "bridge": "fc-bridge"
             }):
            
            result = await manager.start_vm(vm_id, user_id, vm_config)
        
        assert result is True
        assert vm_id in manager.processes
        assert "network_config" in manager.processes[vm_id]
    
    @pytest.mark.asyncio
    async def test_start_vm_firecracker_failure(self, mock_subprocess_run, mock_create_subprocess, test_settings, temp_dir):
        """Test VM startup with Firecracker process failure."""
        mock_create_subprocess.side_effect = OSError("Failed to start process")
        
        template_dir = temp_dir / "templates"
        template_dir.mkdir(exist_ok=True)
        template_file = template_dir / test_settings.default_template
        template_file.write_text("template content")
        
        storage_dir = temp_dir / "storage"
        socket_dir = temp_dir / "sockets"
        socket_dir.mkdir(exist_ok=True)
        
        manager = FirecrackerProcessManager(
            storage_base=str(storage_dir),
            template_dir=str(template_dir),
            socket_dir=str(socket_dir),
            settings=test_settings
        )
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        user_id = "87654321-4321-4321-4321-210987654321"
        vm_config = {"cpu_count": 1, "memory_mb": 512, "disk_gb": 10}
        
        with patch('subprocess.run'), \
             patch('shutil.copy2'), \
             patch('firecracker.process_manager.os.chmod'):
            
            result = await manager.start_vm(vm_id, user_id, vm_config)
        
        assert result is False
        assert vm_id not in manager.processes
    
    @pytest.mark.asyncio
    async def test_stop_vm_graceful(self, mock_subprocess_run, mock_create_subprocess, test_settings, temp_dir):
        """Test graceful VM shutdown."""
        # Setup mock process
        mock_process = MagicMock()
        mock_process.pid = 12345
        mock_process.wait = AsyncMock(return_value=0)
        
        socket_dir = temp_dir / "sockets"
        socket_dir.mkdir(exist_ok=True)
        
        manager = FirecrackerProcessManager(
            socket_dir=str(socket_dir),
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            settings=test_settings
        )
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        
        # Manually add process to manager
        manager.processes[vm_id] = {
            "process": mock_process,
            "status": "running",
            "socket_path": str(socket_dir / f"firecracker-{vm_id}.sock"),
            "config_path": f"/tmp/firecracker-{vm_id}.json",
            "user_id": "87654321-4321-4321-4321-210987654321",
            "network_config": None
        }
        
        with patch('os.killpg') as mock_killpg, \
             patch('os.getpgid', return_value=12345), \
             patch('pathlib.Path.unlink'):
            
            result = await manager.stop_vm(vm_id)
        
        assert result is True
        assert vm_id not in manager.processes
        
        # Verify graceful shutdown signal
        mock_killpg.assert_called_with(12345, signal.SIGTERM)
        mock_process.wait.assert_called_once()
    
    @pytest.mark.asyncio
    async def test_stop_vm_force_kill(self, mock_subprocess_run, mock_create_subprocess, test_settings, temp_dir):
        """Test forced VM shutdown after timeout."""
        # Setup mock process that doesn't respond to SIGTERM
        mock_process = MagicMock()
        mock_process.pid = 12345
        mock_process.wait = AsyncMock(side_effect=asyncio.TimeoutError())
        
        socket_dir = temp_dir / "sockets"
        socket_dir.mkdir(exist_ok=True)
        
        manager = FirecrackerProcessManager(
            socket_dir=str(socket_dir),
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            settings=test_settings
        )
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        
        # Manually add process to manager
        manager.processes[vm_id] = {
            "process": mock_process,
            "status": "running",
            "socket_path": str(socket_dir / f"firecracker-{vm_id}.sock"),
            "config_path": f"/tmp/firecracker-{vm_id}.json",
            "user_id": "87654321-4321-4321-4321-210987654321",
            "network_config": None
        }
        
        with patch('os.killpg') as mock_killpg, \
             patch('os.getpgid', return_value=12345), \
             patch('pathlib.Path.unlink'), \
             patch('asyncio.wait_for', side_effect=asyncio.TimeoutError()):
            
            result = await manager.stop_vm(vm_id)
        
        assert result is True
        assert vm_id not in manager.processes
        
        # Verify both SIGTERM and SIGKILL were sent
        expected_calls = [
            call(12345, signal.SIGTERM),
            call(12345, signal.SIGKILL)
        ]
        mock_killpg.assert_has_calls(expected_calls)
    
    @pytest.mark.asyncio
    async def test_stop_vm_not_found(self, mock_subprocess_run, mock_create_subprocess, test_settings):
        """Test stopping non-existent VM."""
        manager = FirecrackerProcessManager(
            socket_dir=test_settings.firecracker_socket_dir,
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            settings=test_settings
        )
        
        vm_id = "nonexistent-vm-id"
        result = await manager.stop_vm(vm_id)
        
        assert result is False
    
    @pytest.mark.asyncio
    async def test_stop_vm_forced(self, mock_subprocess_run, mock_create_subprocess, test_settings, temp_dir):
        """Test forced VM shutdown."""
        mock_process = MagicMock()
        mock_process.pid = 12345
        
        socket_dir = temp_dir / "sockets"
        socket_dir.mkdir(exist_ok=True)
        
        manager = FirecrackerProcessManager(
            socket_dir=str(socket_dir),
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            settings=test_settings
        )
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        
        manager.processes[vm_id] = {
            "process": mock_process,
            "status": "running",
            "socket_path": str(socket_dir / f"firecracker-{vm_id}.sock"),
            "config_path": f"/tmp/firecracker-{vm_id}.json",
            "user_id": "87654321-4321-4321-4321-210987654321",
            "network_config": None
        }
        
        with patch('os.killpg') as mock_killpg, \
             patch('os.getpgid', return_value=12345), \
             patch('pathlib.Path.unlink'):
            
            result = await manager.stop_vm(vm_id, force=True)
        
        assert result is True
        assert vm_id not in manager.processes
        
        # Should immediately send SIGKILL
        mock_killpg.assert_called_with(12345, signal.SIGKILL)
    
    @pytest.mark.asyncio
    async def test_get_vm_status_running(self, mock_subprocess_run, mock_create_subprocess, test_settings):
        """Test getting status of running VM."""
        mock_process = MagicMock()
        mock_process.poll.return_value = None  # Still running
        
        manager = FirecrackerProcessManager(
            socket_dir=test_settings.firecracker_socket_dir,
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            settings=test_settings
        )
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        manager.processes[vm_id] = {
            "process": mock_process,
            "status": "running",
            "started_at": "2023-01-01T00:00:00Z",
            "pid": 12345,
            "vm_config": {"cpu_count": 2, "memory_mb": 1024, "disk_gb": 20},
            "socket_path": f"{test_settings.firecracker_socket_dir}/firecracker-{vm_id}.sock"
        }
        
        status = await manager.get_vm_status(vm_id)
        
        assert status["vm_id"] == vm_id
        assert status["status"] == "running"
        assert status["pid"] == 12345
        assert "uptime" in status
    
    @pytest.mark.asyncio
    async def test_get_vm_status_not_found(self, mock_subprocess_run, mock_create_subprocess, test_settings):
        """Test getting status of non-existent VM."""
        manager = FirecrackerProcessManager(
            socket_dir=test_settings.firecracker_socket_dir,
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            settings=test_settings
        )
        
        vm_id = "nonexistent-vm-id"
        status = await manager.get_vm_status(vm_id)
        
        assert status is None
    
    @pytest.mark.asyncio
    async def test_list_vms(self, mock_subprocess_run, mock_create_subprocess, test_settings):
        """Test listing all VMs."""
        manager = FirecrackerProcessManager(
            socket_dir=test_settings.firecracker_socket_dir,
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            settings=test_settings
        )
        
        # Add multiple VMs
        vm_ids = [f"{i:08d}-1234-1234-1234-123456789012" for i in range(1, 4)]
        for i, vm_id in enumerate(vm_ids):
            mock_process = MagicMock()
            mock_process.poll.return_value = None
            manager.processes[vm_id] = {
                "process": mock_process,
                "status": "running",
                "started_at": f"2023-01-01T00:0{i}:00Z",
                "pid": 12345 + i,
                "vm_config": {"cpu_count": 1, "memory_mb": 512, "disk_gb": 10},
                "socket_path": f"{test_settings.firecracker_socket_dir}/firecracker-{vm_id}.sock"
            }
        
        vms = await manager.list_vms()
        
        assert len(vms) == 3
        for vm in vms:
            assert vm["vm_id"] in vm_ids
            assert vm["status"] == "running"
    
    @pytest.mark.asyncio
    async def test_cleanup_all_vms(self, mock_subprocess_run, mock_create_subprocess, test_settings):
        """Test cleanup of all VMs on shutdown."""
        manager = FirecrackerProcessManager(
            socket_dir=test_settings.firecracker_socket_dir,
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            settings=test_settings
        )
        
        # Add multiple VMs
        vm_ids = [f"{i:08d}-1234-1234-1234-123456789012" for i in range(1, 3)]
        for vm_id in vm_ids:
            mock_process = MagicMock()
            mock_process.pid = 12345
            mock_process.wait = AsyncMock(return_value=0)
            manager.processes[vm_id] = {
                "process": mock_process,
                "status": "running",
                "socket_path": f"/tmp/firecracker-{vm_id}.sock",
                "config_path": f"/tmp/firecracker-{vm_id}.json",
                "user_id": "87654321-4321-4321-4321-210987654321",
                "network_config": None
            }
        
        with patch('os.killpg'), \
             patch('os.getpgid', return_value=12345), \
             patch('pathlib.Path.unlink'):
            
            await manager.cleanup()
        
        assert len(manager.processes) == 0
    
    def test_validate_vm_config(self, mock_subprocess_run, mock_create_subprocess, test_settings):
        """Test VM configuration validation."""
        manager = FirecrackerProcessManager(
            socket_dir=test_settings.firecracker_socket_dir,
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            settings=test_settings
        )
        
        # Valid config should pass
        valid_config = {
            "cpu_count": 2,
            "memory_mb": 1024,
            "disk_gb": 20
        }
        manager._validate_vm_config(valid_config)  # Should not raise
        
        # Invalid configs should fail
        invalid_configs = [
            {"cpu_count": 0},  # Invalid CPU count
            {"memory_mb": 100},  # Too little memory
            {"disk_gb": 5},  # Too little disk
            {"cpu_count": 100},  # Too many CPUs
        ]
        
        for config in invalid_configs:
            with pytest.raises(ProcessError):
                manager._validate_vm_config(config)
    
    def test_generate_socket_path(self, mock_subprocess_run, mock_create_subprocess, test_settings, temp_dir):
        """Test socket path generation."""
        socket_dir = temp_dir / "sockets"
        
        manager = FirecrackerProcessManager(
            socket_dir=str(socket_dir),
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            settings=test_settings
        )
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        socket_path = manager._generate_socket_path(vm_id)
        
        assert socket_path == str(socket_dir / f"firecracker-{vm_id}.sock")
        assert ".." not in socket_path  # No path traversal
    
    def test_generate_config_path(self, mock_subprocess_run, mock_create_subprocess, test_settings):
        """Test config file path generation."""
        manager = FirecrackerProcessManager(
            socket_dir=test_settings.firecracker_socket_dir,
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            settings=test_settings
        )
        
        vm_id = "12345678-1234-1234-1234-123456789012"
        config_path = manager._generate_config_path(vm_id)
        
        assert config_path == f"/tmp/firecracker-{vm_id}.json"
        assert ".." not in config_path  # No path traversal
    
    @pytest.mark.asyncio
    async def test_concurrent_vm_operations(self, mock_subprocess_run, mock_create_subprocess, test_settings, temp_dir):
        """Test concurrent VM start/stop operations."""
        # Setup mock processes
        mock_processes = {}
        def create_mock_process(*args, **kwargs):
            vm_id = None
            # Extract VM ID from socket path in args
            for arg in args[0]:
                if "firecracker-" in arg and ".sock" in arg:
                    vm_id = arg.split("firecracker-")[1].split(".sock")[0]
                    break
            
            mock_process = MagicMock()
            mock_process.pid = hash(vm_id) % 10000  # Stable PID based on VM ID
            mock_process.wait = AsyncMock(return_value=0)  # Add async wait method
            mock_processes[vm_id] = mock_process
            return mock_process
        
        mock_create_subprocess.side_effect = create_mock_process
        
        # Setup directories
        template_dir = temp_dir / "templates"
        template_dir.mkdir(exist_ok=True)
        template_file = template_dir / test_settings.default_template
        template_file.write_text("template content")
        
        storage_dir = temp_dir / "storage"
        socket_dir = temp_dir / "sockets"
        socket_dir.mkdir(exist_ok=True)
        
        manager = FirecrackerProcessManager(
            storage_base=str(storage_dir),
            template_dir=str(template_dir),
            socket_dir=str(socket_dir),
            settings=test_settings
        )
        
        # Start multiple VMs concurrently
        vm_ids = [f"{i:08d}-1234-1234-1234-123456789012" for i in range(3)]
        user_id = "87654321-4321-4321-4321-210987654321"
        vm_config = {"cpu_count": 1, "memory_mb": 512, "disk_gb": 10}
        
        with patch('subprocess.run'), \
             patch('shutil.copy2'), \
             patch('firecracker.process_manager.os.chmod'):
            
            start_tasks = [
                manager.start_vm(vm_id, user_id, vm_config)
                for vm_id in vm_ids
            ]
            start_results = await asyncio.gather(*start_tasks)
        
        # All should start successfully
        assert all(result for result in start_results)
        assert len(manager.processes) == 3
        
        # Stop all VMs concurrently
        with patch('os.killpg'), \
             patch('os.getpgid'), \
             patch('pathlib.Path.unlink'):
            
            stop_tasks = [manager.stop_vm(vm_id) for vm_id in vm_ids]
            stop_results = await asyncio.gather(*stop_tasks)
        
        # All should stop successfully
        assert all(result for result in stop_results)
        assert len(manager.processes) == 0
    
    def test_security_features(self, mock_subprocess_run, mock_create_subprocess, test_settings):
        """Test security-related features."""
        manager = FirecrackerProcessManager(
            socket_dir=test_settings.firecracker_socket_dir,
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            settings=test_settings
        )
        
        # Test socket path security
        with pytest.raises(ProcessError):
            manager._generate_socket_path("../etc/passwd")
        
        # Test config path security
        with pytest.raises(ProcessError):
            manager._generate_config_path("vm; rm -rf /")
        
        # Test VM ID validation
        with pytest.raises(ProcessError):
            manager._validate_vm_id("")
        
        with pytest.raises(ProcessError):
            manager._validate_vm_id("contains spaces")
    
    def test_resource_limits_enforcement(self, mock_subprocess_run, mock_create_subprocess, test_settings):
        """Test that resource limits are enforced."""
        manager = FirecrackerProcessManager(
            socket_dir=test_settings.firecracker_socket_dir,
            storage_base=test_settings.vm_storage_base,
            template_dir=test_settings.vm_template_dir,
            settings=test_settings
        )
        
        # Config should be limited to settings maximums
        config = {
            "cpu_count": 100,  # Way over limit
            "memory_mb": 16384,  # Way over limit
            "disk_gb": 1000  # Way over limit
        }
        
        limited_config = manager._apply_resource_limits(config)
        
        assert limited_config["cpu_count"] <= test_settings.max_cpu_per_vm
        assert limited_config["memory_mb"] <= test_settings.max_memory_mb_per_vm
        assert limited_config["disk_gb"] <= test_settings.max_disk_gb_per_vm