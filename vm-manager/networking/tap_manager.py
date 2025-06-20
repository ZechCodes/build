"""TAP device management for VM networking with security validation."""

import subprocess
import ipaddress
import re
import asyncio
from typing import Optional, Dict, Set
from pathlib import Path

import structlog

from config.settings import VMManagerSettings, settings

logger = structlog.get_logger(__name__)


class NetworkError(Exception):
    """Exception raised when network operations fail."""
    pass


class TAPDeviceManager:
    """Manages TAP devices for VM networking with security controls."""
    
    def __init__(
        self, 
        bridge_name: str = "fc-bridge", 
        subnet: str = "10.0.100.0/20",
        settings: Optional[VMManagerSettings] = None
    ):
        from config.settings import settings as default_settings
        self.settings = settings or default_settings
        self.bridge_name = self._validate_bridge_name(bridge_name)
        self.subnet = ipaddress.IPv4Network(subnet)
        self.allocated_ips: Set[str] = set()
        self._ip_lock = asyncio.Lock()
        
        # Setup bridge network
        try:
            self._setup_bridge()
        except Exception as e:
            logger.error("Failed to setup bridge network", error=str(e))
            raise NetworkError(f"Failed to setup bridge: {e}")
    
    def _validate_bridge_name(self, bridge_name: str) -> str:
        """Validate bridge name for security."""
        if not bridge_name:
            raise NetworkError("Bridge name cannot be empty")
        
        if not re.match(r'^[a-zA-Z0-9_-]+$', bridge_name):
            raise NetworkError(f"Invalid bridge name: {bridge_name}")
        
        if len(bridge_name) > 15:  # Linux interface name limit
            raise NetworkError(f"Bridge name too long: {bridge_name}")
        
        if ".." in bridge_name or "/" in bridge_name:
            raise NetworkError(f"Invalid bridge name: {bridge_name}")
        
        return bridge_name
    
    def _setup_bridge(self):
        """Setup the bridge interface for VM networking."""
        try:
            # Create bridge if it doesn't exist (ignore if exists)
            subprocess.run([
                "ip", "link", "add", "name", self.bridge_name, "type", "bridge"
            ], check=False)
            
            # Configure bridge IP (first host in subnet)
            bridge_ip = str(list(self.subnet.hosts())[0])
            subprocess.run([
                "ip", "addr", "add", f"{bridge_ip}/{self.subnet.prefixlen}",
                "dev", self.bridge_name
            ], check=False)
            
            # Bring bridge up
            subprocess.run([
                "ip", "link", "set", "dev", self.bridge_name, "up"
            ], check=True)
            
            logger.info(
                "Bridge configured successfully",
                bridge=self.bridge_name,
                ip=bridge_ip,
                subnet=str(self.subnet)
            )
            
        except subprocess.CalledProcessError as e:
            logger.error("Failed to setup bridge", bridge=self.bridge_name, error=str(e))
            raise NetworkError(f"Failed to setup bridge: {e}")
    
    async def create_tap_device(self, vm_id: str) -> Optional[Dict[str, str]]:
        """Create a TAP device for a VM with proper validation."""
        
        try:
            self._validate_vm_id(vm_id)
            
            # Generate TAP device name (limited to 8 chars from VM ID for interface name limits)
            tap_name = f"fc-tap-{vm_id[:8]}"
            self._validate_tap_name(tap_name)
            
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
            
            # Allocate IP address (thread-safe)
            async with self._ip_lock:
                vm_ip = self._allocate_ip()
            
            logger.info(
                "TAP device created successfully",
                vm_id=vm_id,
                tap_name=tap_name,
                ip=vm_ip,
                bridge=self.bridge_name
            )
            
            return {
                "tap_device": tap_name,
                "ip_address": vm_ip,
                "bridge": self.bridge_name
            }
            
        except subprocess.CalledProcessError as e:
            logger.error("Failed to create TAP device", vm_id=vm_id, error=str(e))
            # Attempt cleanup on failure
            await self.cleanup_tap_device(tap_name)
            return None
        except Exception as e:
            logger.error("Unexpected error creating TAP device", vm_id=vm_id, error=str(e))
            return None
    
    async def cleanup_tap_device(self, tap_name: str) -> bool:
        """Clean up a TAP device with validation."""
        
        try:
            self._validate_tap_name(tap_name)
            
            subprocess.run([
                "ip", "link", "delete", "dev", tap_name
            ], check=True)
            
            logger.info("TAP device cleaned up successfully", tap_name=tap_name)
            return True
            
        except subprocess.CalledProcessError as e:
            logger.error("Failed to cleanup TAP device", tap_name=tap_name, error=str(e))
            return False
        except Exception as e:
            logger.error("Unexpected error cleaning up TAP device", tap_name=tap_name, error=str(e))
            return False
    
    def _allocate_ip(self) -> str:
        """Allocate an available IP address from the subnet."""
        # Skip the first IP (reserved for bridge)
        available_hosts = list(self.subnet.hosts())[1:]
        
        for ip in available_hosts:
            ip_str = str(ip)
            if ip_str not in self.allocated_ips:
                self.allocated_ips.add(ip_str)
                logger.debug("IP address allocated", ip=ip_str)
                return ip_str
        
        raise NetworkError("No available IP addresses in subnet")
    
    def release_ip(self, ip_address: str):
        """Release an allocated IP address."""
        self.allocated_ips.discard(ip_address)
        logger.debug("IP address released", ip=ip_address)
    
    def _validate_vm_id(self, vm_id: str):
        """Validate VM ID for security."""
        if not vm_id:
            raise NetworkError("VM ID cannot be empty")
        
        if len(vm_id) < 8:
            raise NetworkError(f"VM ID too short: {vm_id}")
        
        if len(vm_id) > 100:
            raise NetworkError(f"VM ID too long: {vm_id}")
        
        # Check for dangerous characters
        if not re.match(r'^[a-zA-Z0-9_-]+$', vm_id):
            raise NetworkError(f"Invalid VM ID: {vm_id}")
        
        # Check for path traversal
        if ".." in vm_id or "/" in vm_id or "\\" in vm_id:
            raise NetworkError(f"Invalid VM ID: {vm_id}")
    
    def _validate_tap_name(self, tap_name: str):
        """Validate TAP device name for security."""
        if not tap_name:
            raise NetworkError("TAP device name cannot be empty")
        
        if len(tap_name) > 15:  # Linux interface name limit
            raise NetworkError(f"TAP device name too long: {tap_name}")
        
        # Check for dangerous characters
        if not re.match(r'^[a-zA-Z0-9_-]+$', tap_name):
            raise NetworkError(f"Invalid TAP device name: {tap_name}")
        
        # Check for path traversal
        if ".." in tap_name or "/" in tap_name or "\\" in tap_name:
            raise NetworkError(f"Invalid TAP device name: {tap_name}")
    
    def get_allocated_ips(self) -> Set[str]:
        """Get currently allocated IP addresses."""
        return self.allocated_ips.copy()
    
    def get_available_ip_count(self) -> int:
        """Get count of available IP addresses."""
        total_hosts = len(list(self.subnet.hosts())) - 1  # Minus bridge IP
        return total_hosts - len(self.allocated_ips)
    
    def get_network_info(self) -> Dict[str, str]:
        """Get network configuration information."""
        bridge_ip = str(list(self.subnet.hosts())[0])
        return {
            "bridge_name": self.bridge_name,
            "bridge_ip": bridge_ip,
            "subnet": str(self.subnet),
            "available_ips": str(self.get_available_ip_count()),
            "allocated_ips": str(len(self.allocated_ips))
        }