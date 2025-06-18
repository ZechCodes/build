# Session 3: Firecracker VM Management Core

## Objective
Implement the core virtual machine management system using Firecracker, providing secure, isolated development environments with proper lifecycle management.

## Overview
This session establishes the foundation for VM operations, including Firecracker process management, network allocation, resource limits, and security policies. It creates the infrastructure necessary for running isolated development environments.

## Prerequisites
- Session 1 (Core Infrastructure) completed
- Session 2 (Authentication) completed
- Firecracker binary available and tested
- Network configuration for VM isolation

## Components to Implement

### 1. Firecracker Process Management
**Location**: `vm-manager/firecracker/`

#### VM Configuration Generator
```python
# vm-manager/firecracker/config_generator.py
from typing import Dict, Any, Optional
import json
import uuid
from pathlib import Path

class FirecrackerConfigGenerator:
    def __init__(self, base_config_path: str):
        self.base_config_path = Path(base_config_path)
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
                "ht_enabled": False
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
        network_config: Optional[Dict] = None
    ) -> Dict[str, Any]:
        """Generate Firecracker configuration for a new VM"""
        
        config = self.default_config.copy()
        
        # Update machine configuration
        config["machine-config"]["vcpu_count"] = min(cpu_count, 4)  # Max 4 CPUs
        config["machine-config"]["mem_size_mib"] = min(memory_mb, 2048)  # Max 2GB
        
        # Configure root filesystem
        root_drive = {
            "drive_id": "rootfs",
            "path_on_host": f"/opt/vm-storage/{user_id}/{vm_id}/rootfs.ext4",
            "is_root_device": True,
            "is_read_only": False
        }
        config["drives"].append(root_drive)
        
        # Configure network interface
        if network_config:
            network_interface = {
                "iface_id": "eth0",
                "guest_mac": self._generate_mac_address(),
                "host_dev_name": network_config["tap_device"]
            }
            config["network-interfaces"].append(network_interface)
        
        # Update logger path
        config["logger"]["log_path"] = f"/var/log/firecracker/{vm_id}.log"
        
        return config
    
    def _generate_mac_address(self) -> str:
        """Generate a unique MAC address for the VM"""
        mac_bytes = [0x02, 0x00] + [
            int(x, 16) for x in str(uuid.uuid4()).replace('-', '')[:8]
        ][:6]
        return ':'.join(f'{b:02x}' for b in mac_bytes)

# vm-manager/firecracker/process_manager.py
import asyncio
import subprocess
import signal
import os
from typing import Dict, Optional
import psutil
import structlog

logger = structlog.get_logger()

class FirecrackerProcessManager:
    def __init__(self):
        self.processes: Dict[str, subprocess.Popen] = {}
        self.config_generator = FirecrackerConfigGenerator("/etc/firecracker/")
    
    async def start_vm(
        self,
        vm_id: str,
        user_id: str,
        vm_config: Dict,
        socket_path: str
    ) -> bool:
        """Start a Firecracker VM process"""
        
        try:
            # Generate configuration file
            config = self.config_generator.generate_vm_config(
                vm_id=vm_id,
                user_id=user_id,
                **vm_config
            )
            
            config_path = f"/tmp/firecracker-{vm_id}.json"
            with open(config_path, 'w') as f:
                json.dump(config, f, indent=2)
            
            # Start Firecracker process
            cmd = [
                "/usr/bin/firecracker",
                "--api-sock", socket_path,
                "--config-file", config_path
            ]
            
            process = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                preexec_fn=os.setsid  # Create new process group
            )
            
            self.processes[vm_id] = process
            
            logger.info("VM started", vm_id=vm_id, pid=process.pid)
            return True
            
        except Exception as e:
            logger.error("Failed to start VM", vm_id=vm_id, error=str(e))
            return False
    
    async def stop_vm(self, vm_id: str, force: bool = False) -> bool:
        """Stop a Firecracker VM process"""
        
        if vm_id not in self.processes:
            logger.warning("VM not found in processes", vm_id=vm_id)
            return False
        
        process = self.processes[vm_id]
        
        try:
            if force:
                # Force kill the process group
                os.killpg(os.getpgid(process.pid), signal.SIGKILL)
            else:
                # Graceful shutdown
                os.killpg(os.getpgid(process.pid), signal.SIGTERM)
                
                # Wait for graceful shutdown with timeout
                try:
                    await asyncio.wait_for(process.wait(), timeout=30.0)
                except asyncio.TimeoutError:
                    logger.warning("VM didn't shutdown gracefully, force killing", vm_id=vm_id)
                    os.killpg(os.getpgid(process.pid), signal.SIGKILL)
            
            del self.processes[vm_id]
            logger.info("VM stopped", vm_id=vm_id)
            return True
            
        except Exception as e:
            logger.error("Failed to stop VM", vm_id=vm_id, error=str(e))
            return False
```

### 2. Network Management
**Location**: `vm-manager/networking/`

#### TAP Device Management
```python
# vm-manager/networking/tap_manager.py
import subprocess
import ipaddress
from typing import Optional, List
import structlog

logger = structlog.get_logger()

class TAPDeviceManager:
    def __init__(self, bridge_name: str = "fc-bridge", subnet: str = "10.0.100.0/20"):
        self.bridge_name = bridge_name
        self.subnet = ipaddress.IPv4Network(subnet)
        self.allocated_ips = set()
        self._setup_bridge()
    
    def _setup_bridge(self):
        """Setup the bridge interface for VM networking"""
        try:
            # Create bridge if it doesn't exist
            subprocess.run([
                "ip", "link", "add", "name", self.bridge_name, "type", "bridge"
            ], check=False)
            
            # Configure bridge IP
            bridge_ip = str(list(self.subnet.hosts())[0])
            subprocess.run([
                "ip", "addr", "add", f"{bridge_ip}/{self.subnet.prefixlen}",
                "dev", self.bridge_name
            ], check=False)
            
            # Bring bridge up
            subprocess.run([
                "ip", "link", "set", "dev", self.bridge_name, "up"
            ], check=True)
            
            logger.info("Bridge configured", bridge=self.bridge_name, ip=bridge_ip)
            
        except subprocess.CalledProcessError as e:
            logger.error("Failed to setup bridge", error=str(e))
            raise
    
    async def create_tap_device(self, vm_id: str) -> Optional[Dict[str, str]]:
        """Create a TAP device for a VM"""
        
        tap_name = f"fc-tap-{vm_id[:8]}"
        
        try:
            # Create TAP device
            subprocess.run([
                "ip", "tuntap", "add", "dev", tap_name, "mode", "tap"
            ], check=True)
            
            # Add to bridge
            subprocess.run([
                "ip", "link", "set", "dev", tap_name, "master", self.bridge_name
            ], check=True)
            
            # Bring TAP device up
            subprocess.run([
                "ip", "link", "set", "dev", tap_name, "up"
            ], check=True)
            
            # Allocate IP address
            vm_ip = self._allocate_ip()
            
            logger.info("TAP device created", vm_id=vm_id, tap_name=tap_name, ip=vm_ip)
            
            return {
                "tap_device": tap_name,
                "ip_address": vm_ip,
                "bridge": self.bridge_name
            }
            
        except subprocess.CalledProcessError as e:
            logger.error("Failed to create TAP device", vm_id=vm_id, error=str(e))
            await self.cleanup_tap_device(tap_name)
            return None
    
    async def cleanup_tap_device(self, tap_name: str) -> bool:
        """Clean up a TAP device"""
        
        try:
            subprocess.run([
                "ip", "link", "delete", "dev", tap_name
            ], check=True)
            
            logger.info("TAP device cleaned up", tap_name=tap_name)
            return True
            
        except subprocess.CalledProcessError as e:
            logger.error("Failed to cleanup TAP device", tap_name=tap_name, error=str(e))
            return False
    
    def _allocate_ip(self) -> str:
        """Allocate an available IP address from the subnet"""
        for ip in self.subnet.hosts():
            if str(ip) not in self.allocated_ips:
                self.allocated_ips.add(str(ip))
                return str(ip)
        
        raise ValueError("No available IP addresses in subnet")
    
    def release_ip(self, ip_address: str):
        """Release an allocated IP address"""
        self.allocated_ips.discard(ip_address)
```

### 3. Storage Management
**Location**: `vm-manager/storage/`

#### Root Filesystem Management
```python
# vm-manager/storage/rootfs_manager.py
import os
import shutil
import subprocess
from pathlib import Path
from typing import Optional
import structlog

logger = structlog.get_logger()

class RootfsManager:
    def __init__(self, storage_base: str = "/opt/vm-storage"):
        self.storage_base = Path(storage_base)
        self.template_path = Path("/opt/vm-templates")
        self.default_template = "ubuntu-22.04-base.ext4"
    
    async def create_rootfs(
        self,
        user_id: str,
        vm_id: str,
        template: str = None,
        size_gb: int = 10
    ) -> Optional[str]:
        """Create a root filesystem for a VM"""
        
        template_name = template or self.default_template
        template_path = self.template_path / template_name
        
        if not template_path.exists():
            logger.error("Template not found", template=template_name)
            return None
        
        # Create user directory if it doesn't exist
        user_dir = self.storage_base / user_id
        user_dir.mkdir(parents=True, exist_ok=True)
        
        vm_dir = user_dir / vm_id
        vm_dir.mkdir(exist_ok=True)
        
        rootfs_path = vm_dir / "rootfs.ext4"
        
        try:
            # Copy template to VM directory
            shutil.copy2(template_path, rootfs_path)
            
            # Resize filesystem if needed
            if size_gb > 10:  # Default template is 10GB
                await self._resize_filesystem(rootfs_path, size_gb)
            
            # Set proper permissions
            os.chmod(rootfs_path, 0o600)
            
            logger.info("Rootfs created", vm_id=vm_id, path=str(rootfs_path))
            return str(rootfs_path)
            
        except Exception as e:
            logger.error("Failed to create rootfs", vm_id=vm_id, error=str(e))
            # Clean up on failure
            if rootfs_path.exists():
                rootfs_path.unlink()
            return None
    
    async def _resize_filesystem(self, rootfs_path: Path, size_gb: int):
        """Resize the filesystem to the specified size"""
        
        try:
            # Resize the file
            subprocess.run([
                "truncate", "-s", f"{size_gb}G", str(rootfs_path)
            ], check=True)
            
            # Resize the filesystem
            subprocess.run([
                "e2fsck", "-f", "-y", str(rootfs_path)
            ], check=True)
            
            subprocess.run([
                "resize2fs", str(rootfs_path)
            ], check=True)
            
            logger.info("Filesystem resized", path=str(rootfs_path), size_gb=size_gb)
            
        except subprocess.CalledProcessError as e:
            logger.error("Failed to resize filesystem", error=str(e))
            raise
    
    async def delete_rootfs(self, user_id: str, vm_id: str) -> bool:
        """Delete the root filesystem for a VM"""
        
        vm_dir = self.storage_base / user_id / vm_id
        
        try:
            if vm_dir.exists():
                shutil.rmtree(vm_dir)
                logger.info("Rootfs deleted", vm_id=vm_id)
                return True
            else:
                logger.warning("Rootfs directory not found", vm_id=vm_id)
                return False
                
        except Exception as e:
            logger.error("Failed to delete rootfs", vm_id=vm_id, error=str(e))
            return False
```

### 4. VM Health Monitoring
**Location**: `vm-manager/monitoring/`

#### Health Check System
```python
# vm-manager/monitoring/health_checker.py
import asyncio
import aiohttp
import time
from typing import Dict, List, Optional
from enum import Enum
import structlog

logger = structlog.get_logger()

class VMStatus(Enum):
    STARTING = "starting"
    RUNNING = "running"
    STOPPING = "stopping"
    STOPPED = "stopped"
    ERROR = "error"

class VMHealthChecker:
    def __init__(self, check_interval: int = 30):
        self.check_interval = check_interval
        self.vm_states: Dict[str, Dict] = {}
        self.running = False
    
    async def start_monitoring(self):
        """Start the health monitoring loop"""
        self.running = True
        while self.running:
            await self._check_all_vms()
            await asyncio.sleep(self.check_interval)
    
    async def stop_monitoring(self):
        """Stop the health monitoring loop"""
        self.running = False
    
    def register_vm(self, vm_id: str, api_socket: str, expected_status: VMStatus = VMStatus.STARTING):
        """Register a VM for health monitoring"""
        self.vm_states[vm_id] = {
            "api_socket": api_socket,
            "status": expected_status,
            "last_check": 0,
            "consecutive_failures": 0,
            "started_at": time.time()
        }
        logger.info("VM registered for monitoring", vm_id=vm_id)
    
    def unregister_vm(self, vm_id: str):
        """Unregister a VM from health monitoring"""
        if vm_id in self.vm_states:
            del self.vm_states[vm_id]
            logger.info("VM unregistered from monitoring", vm_id=vm_id)
    
    async def _check_all_vms(self):
        """Check health of all registered VMs"""
        for vm_id in list(self.vm_states.keys()):
            await self._check_vm_health(vm_id)
    
    async def _check_vm_health(self, vm_id: str):
        """Check health of a specific VM"""
        if vm_id not in self.vm_states:
            return
        
        vm_state = self.vm_states[vm_id]
        
        try:
            # Check if Firecracker API responds
            status = await self._ping_firecracker_api(vm_state["api_socket"])
            
            if status:
                vm_state["status"] = VMStatus.RUNNING
                vm_state["consecutive_failures"] = 0
                logger.debug("VM health check passed", vm_id=vm_id)
            else:
                vm_state["consecutive_failures"] += 1
                
                if vm_state["consecutive_failures"] >= 3:
                    vm_state["status"] = VMStatus.ERROR
                    logger.error("VM health check failed", vm_id=vm_id, 
                               failures=vm_state["consecutive_failures"])
                    await self._handle_unhealthy_vm(vm_id)
            
            vm_state["last_check"] = time.time()
            
        except Exception as e:
            logger.error("Health check exception", vm_id=vm_id, error=str(e))
            vm_state["consecutive_failures"] += 1
            vm_state["status"] = VMStatus.ERROR
    
    async def _ping_firecracker_api(self, socket_path: str) -> bool:
        """Ping the Firecracker API socket"""
        try:
            connector = aiohttp.UnixConnector(path=socket_path)
            async with aiohttp.ClientSession(connector=connector) as session:
                async with session.get("/") as response:
                    return response.status == 200
        except:
            return False
    
    async def _handle_unhealthy_vm(self, vm_id: str):
        """Handle an unhealthy VM"""
        # This would trigger recovery procedures
        # For now, just log the issue
        logger.error("VM requires attention", vm_id=vm_id)
        
        # Could implement:
        # - Restart attempts
        # - Notification to user
        # - Automatic cleanup
        # - Incident reporting
```

## Critical Decisions

### VM Resource Limits
- **CPU**: Maximum 4 vCPUs per VM
- **Memory**: Maximum 2GB RAM per VM
- **Storage**: Maximum 50GB disk per VM
- **Network**: Isolated network per VM

### Security Configuration
- **User Isolation**: Each user gets separate storage directory
- **Network Isolation**: VMs in separate network namespace
- **Process Isolation**: Firecracker runs in dedicated cgroups
- **File Permissions**: Restrictive permissions on VM files

### VM Naming Convention
- **VM ID**: UUID4 format for uniqueness
- **TAP Device**: `fc-tap-{vm_id[:8]}`
- **Socket Path**: `/tmp/firecracker-{vm_id}.sock`
- **Log File**: `/var/log/firecracker/{vm_id}.log`

### Cleanup Strategy
- **Automatic**: Clean up orphaned resources every hour
- **On-Demand**: Immediate cleanup when VM is deleted
- **Graceful**: 30-second timeout for graceful shutdown
- **Force**: SIGKILL if graceful shutdown fails

## Security Checklist ✅

### VM Isolation Security
- [ ] VMs run in separate user namespaces (rootless Firecracker)
- [ ] SELinux/AppArmor profiles configured for Firecracker processes
- [ ] Network isolation between VMs (separate bridge networks)
- [ ] Resource limits enforced via cgroups (CPU, memory, I/O)
- [ ] No shared storage between VMs (individual root filesystems)
- [ ] Firecracker binary signature verification
- [ ] Secure VM configuration templates (no privileged access)
- [ ] Rate limiting on VM creation (max 5 VMs per user)
- [ ] VM escape monitoring and detection
- [ ] Automatic cleanup of orphaned resources

### Process Security
- [ ] Firecracker processes run as non-root user
- [ ] Process isolation with dedicated cgroups
- [ ] Secure communication via Unix domain sockets
- [ ] API socket permissions restricted to VM owner
- [ ] Log file access controls (no sensitive data exposure)
- [ ] Process monitoring for anomalous behavior
- [ ] Graceful shutdown procedures with timeouts
- [ ] Force-kill protection against runaway processes
- [ ] Resource usage monitoring and alerting
- [ ] Process group isolation to prevent escape

### Storage Security
- [ ] VM storage directories with restrictive permissions (700)
- [ ] Root filesystem images encrypted at rest
- [ ] Template images verified and scanned for malware
- [ ] Storage quota enforcement per user
- [ ] Secure deletion of VM storage on cleanup
- [ ] Backup encryption for VM snapshots
- [ ] Storage access auditing and logging
- [ ] Prevention of symlink attacks in storage paths
- [ ] Disk space monitoring and alerting
- [ ] Storage integrity verification

### Network Security
- [ ] VM network traffic isolated via bridge networks
- [ ] MAC address randomization for anonymity
- [ ] IP address allocation tracking and management
- [ ] Network traffic monitoring and logging
- [ ] Firewall rules for VM network access
- [ ] Prevention of IP spoofing and ARP poisoning
- [ ] Network resource usage monitoring
- [ ] DDoS protection for VM network interfaces
- [ ] Secure bridge configuration with minimal permissions
- [ ] Network namespace isolation

### Configuration Security
- [ ] VM configuration validation before launch
- [ ] Secure default configuration templates
- [ ] Configuration file permissions and access controls
- [ ] Input validation for all VM parameters
- [ ] Prevention of configuration injection attacks
- [ ] Audit logging for configuration changes
- [ ] Version control for configuration templates
- [ ] Regular security updates for base images
- [ ] Configuration backup and recovery procedures
- [ ] Compliance checking for security policies

## Testing Requirements

### VM Lifecycle Testing
- [ ] VM creation with various configurations
- [ ] VM startup and shutdown procedures
- [ ] Resource limit enforcement testing
- [ ] Network connectivity verification
- [ ] Storage allocation and cleanup
- [ ] Error handling and recovery

### Security Testing
- [ ] VM escape attempt detection
- [ ] Resource exhaustion attacks
- [ ] Network isolation verification
- [ ] Configuration injection attempts
- [ ] Privilege escalation testing
- [ ] Storage security validation

### Performance Testing
- [ ] VM startup time under load
- [ ] Concurrent VM creation stress test
- [ ] Resource utilization monitoring
- [ ] Network performance benchmarks
- [ ] Storage I/O performance testing
- [ ] Memory usage optimization

### Integration Testing
- [ ] Integration with authentication service
- [ ] Database state synchronization
- [ ] Health monitoring accuracy
- [ ] API endpoint functionality
- [ ] Error reporting and logging
- [ ] Cleanup automation verification

## Performance Targets

### VM Operations
- VM startup time < 5 seconds
- VM shutdown time < 10 seconds
- Network allocation < 1 second
- Storage creation < 3 seconds
- Health check latency < 100ms

### Scalability
- Support 100 concurrent VMs per host
- Handle 10 VM operations per second
- Network bandwidth 1Gbps per VM
- Storage IOPS 1000 per VM
- Memory overhead < 50MB per VM

## Monitoring & Alerting

### Key Metrics
- VM startup/shutdown times
- Resource utilization per VM
- Network traffic and errors
- Storage usage and IOPS
- Health check success rates
- Error rates and recovery times

### Alert Conditions
- VM startup failures > 5%
- Resource utilization > 90%
- Health check failures > 3 consecutive
- Storage usage > 80% of quota
- Network errors > 1% of traffic
- Orphaned resources detected

## Documentation Deliverables

### Technical Documentation
- [ ] Firecracker integration guide
- [ ] VM configuration reference
- [ ] Network topology documentation
- [ ] Storage management procedures
- [ ] Security architecture overview
- [ ] Performance tuning guide

### Operational Documentation
- [ ] VM troubleshooting guide
- [ ] Resource monitoring procedures
- [ ] Security incident response
- [ ] Backup and recovery procedures
- [ ] Capacity planning guidelines
- [ ] Maintenance procedures

## Next Steps

Upon successful completion of Session 3:
1. Core VM management operational
2. Security policies enforced
3. Resource monitoring active
4. Network isolation verified
5. Storage management functional
6. Proceed to Session 4: PTY/Terminal Connection Layer

## Risk Mitigation

### Technical Risks
1. **VM escape vulnerabilities**: Regular Firecracker updates, security monitoring
2. **Resource exhaustion**: Strict quotas, monitoring, auto-cleanup
3. **Network security**: Isolation testing, firewall rules, monitoring
4. **Storage corruption**: Checksums, backups, integrity verification
5. **Performance degradation**: Resource monitoring, optimization, scaling

### Operational Risks
1. **Service availability**: Health checks, automatic recovery, redundancy
2. **Data loss**: Backup procedures, snapshot management, verification
3. **Security incidents**: Monitoring, alerting, response procedures
4. **Capacity planning**: Usage monitoring, growth prediction, scaling
5. **Compliance issues**: Regular audits, policy enforcement, documentation

---

**Session 3 Success Criteria:**
- Firecracker VM management fully operational
- Security policies enforced and tested
- Resource limits and monitoring active
- Network isolation verified
- Performance targets met
- Ready for Session 4 terminal connection implementation