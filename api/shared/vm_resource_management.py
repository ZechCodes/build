"""
Admin-Configurable VM Resource Management System

Provides comprehensive resource limit management with:
- Per-user and per-tier resource allocation
- Real-time resource usage monitoring
- Automatic enforcement through cgroups
- Usage alerts and quota management
- Dynamic resource scaling
- Fair resource distribution
"""

import os
import asyncio
import subprocess
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Any, Union
from enum import Enum
from pathlib import Path
import json

from pydantic import BaseModel, Field, validator
import structlog


class ResourceType(str, Enum):
    """Types of resources that can be limited."""
    CPU_CORES = "cpu_cores"
    MEMORY_MB = "memory_mb"
    DISK_GB = "disk_gb"
    NETWORK_BANDWIDTH_MBPS = "network_bandwidth_mbps"
    MAX_PROCESSES = "max_processes"
    MAX_OPEN_FILES = "max_open_files"
    MAX_VMS = "max_vms"
    IOPS = "iops"
    GPU_COUNT = "gpu_count"


class ResourceTier(str, Enum):
    """Resource allocation tiers."""
    FREE = "free"
    BASIC = "basic"
    STANDARD = "standard"
    PREMIUM = "premium"
    ENTERPRISE = "enterprise"
    CUSTOM = "custom"


class ResourceStatus(str, Enum):
    """Resource allocation status."""
    AVAILABLE = "available"
    ALLOCATED = "allocated"
    EXCEEDED = "exceeded"
    SUSPENDED = "suspended"


class ResourceLimit(BaseModel):
    """Individual resource limit configuration."""
    resource_type: ResourceType
    soft_limit: float = Field(..., description="Soft limit (warnings)")
    hard_limit: float = Field(..., description="Hard limit (enforcement)")
    burst_limit: Optional[float] = Field(None, description="Temporary burst limit")
    unit: str = Field(..., description="Unit of measurement")
    
    @validator('hard_limit')
    def hard_limit_must_be_greater_than_soft(cls, v, values):
        if 'soft_limit' in values and v <= values['soft_limit']:
            raise ValueError('Hard limit must be greater than soft limit')
        return v


class ResourceProfile(BaseModel):
    """Complete resource profile for a user or tier."""
    profile_id: str = Field(..., description="Unique profile identifier")
    name: str = Field(..., description="Profile name")
    tier: ResourceTier = Field(..., description="Resource tier")
    limits: Dict[ResourceType, ResourceLimit] = Field(..., description="Resource limits")
    created_at: datetime = Field(default_factory=datetime.utcnow)
    updated_at: datetime = Field(default_factory=datetime.utcnow)
    is_active: bool = Field(default=True)
    priority: int = Field(default=100, description="Scheduling priority")


class ResourceUsage(BaseModel):
    """Current resource usage for a user/VM."""
    user_id: str
    vm_id: Optional[str] = None
    resource_type: ResourceType
    current_usage: float
    allocated_limit: float
    usage_percentage: float
    timestamp: datetime = Field(default_factory=datetime.utcnow)
    
    @validator('usage_percentage', pre=True, always=True)
    def calculate_usage_percentage(cls, v, values):
        if 'current_usage' in values and 'allocated_limit' in values:
            if values['allocated_limit'] > 0:
                return (values['current_usage'] / values['allocated_limit']) * 100
        return 0.0


class ResourceQuota(BaseModel):
    """Resource quota management."""
    user_id: str
    profile_id: str
    allocated_resources: Dict[ResourceType, float]
    used_resources: Dict[ResourceType, float]
    quota_period_start: datetime
    quota_period_end: datetime
    is_suspended: bool = False
    suspension_reason: Optional[str] = None


class VMResourceConfig:
    """VM resource configuration and enforcement."""
    
    def __init__(self):
        self.logger = structlog.get_logger()
        self.cgroup_base_path = Path("/sys/fs/cgroup")
        self.vm_cgroup_path = self.cgroup_base_path / "buildplatform"
        
        # Default resource profiles
        self.default_profiles = self._create_default_profiles()
        
        # Resource monitoring
        self.resource_usage_cache: Dict[str, Dict[ResourceType, float]] = {}
        
    def _create_default_profiles(self) -> Dict[ResourceTier, ResourceProfile]:
        """Create default resource profiles for each tier."""
        profiles = {}
        
        # Free tier
        profiles[ResourceTier.FREE] = ResourceProfile(
            profile_id="free_default",
            name="Free Tier",
            tier=ResourceTier.FREE,
            limits={
                ResourceType.CPU_CORES: ResourceLimit(
                    resource_type=ResourceType.CPU_CORES,
                    soft_limit=1.0,
                    hard_limit=1.0,
                    unit="cores"
                ),
                ResourceType.MEMORY_MB: ResourceLimit(
                    resource_type=ResourceType.MEMORY_MB,
                    soft_limit=1024,
                    hard_limit=1536,
                    unit="MB"
                ),
                ResourceType.DISK_GB: ResourceLimit(
                    resource_type=ResourceType.DISK_GB,
                    soft_limit=5,
                    hard_limit=10,
                    unit="GB"
                ),
                ResourceType.MAX_VMS: ResourceLimit(
                    resource_type=ResourceType.MAX_VMS,
                    soft_limit=1,
                    hard_limit=1,
                    unit="count"
                ),
                ResourceType.NETWORK_BANDWIDTH_MBPS: ResourceLimit(
                    resource_type=ResourceType.NETWORK_BANDWIDTH_MBPS,
                    soft_limit=10,
                    hard_limit=25,
                    unit="Mbps"
                )
            }
        )
        
        # Basic tier
        profiles[ResourceTier.BASIC] = ResourceProfile(
            profile_id="basic_default",
            name="Basic Tier",
            tier=ResourceTier.BASIC,
            limits={
                ResourceType.CPU_CORES: ResourceLimit(
                    resource_type=ResourceType.CPU_CORES,
                    soft_limit=2.0,
                    hard_limit=2.0,
                    burst_limit=4.0,
                    unit="cores"
                ),
                ResourceType.MEMORY_MB: ResourceLimit(
                    resource_type=ResourceType.MEMORY_MB,
                    soft_limit=2048,
                    hard_limit=4096,
                    unit="MB"
                ),
                ResourceType.DISK_GB: ResourceLimit(
                    resource_type=ResourceType.DISK_GB,
                    soft_limit=20,
                    hard_limit=50,
                    unit="GB"
                ),
                ResourceType.MAX_VMS: ResourceLimit(
                    resource_type=ResourceType.MAX_VMS,
                    soft_limit=2,
                    hard_limit=3,
                    unit="count"
                ),
                ResourceType.NETWORK_BANDWIDTH_MBPS: ResourceLimit(
                    resource_type=ResourceType.NETWORK_BANDWIDTH_MBPS,
                    soft_limit=50,
                    hard_limit=100,
                    unit="Mbps"
                )
            }
        )
        
        # Standard tier
        profiles[ResourceTier.STANDARD] = ResourceProfile(
            profile_id="standard_default",
            name="Standard Tier",
            tier=ResourceTier.STANDARD,
            limits={
                ResourceType.CPU_CORES: ResourceLimit(
                    resource_type=ResourceType.CPU_CORES,
                    soft_limit=4.0,
                    hard_limit=8.0,
                    burst_limit=12.0,
                    unit="cores"
                ),
                ResourceType.MEMORY_MB: ResourceLimit(
                    resource_type=ResourceType.MEMORY_MB,
                    soft_limit=8192,
                    hard_limit=16384,
                    unit="MB"
                ),
                ResourceType.DISK_GB: ResourceLimit(
                    resource_type=ResourceType.DISK_GB,
                    soft_limit=100,
                    hard_limit=250,
                    unit="GB"
                ),
                ResourceType.MAX_VMS: ResourceLimit(
                    resource_type=ResourceType.MAX_VMS,
                    soft_limit=5,
                    hard_limit=10,
                    unit="count"
                ),
                ResourceType.NETWORK_BANDWIDTH_MBPS: ResourceLimit(
                    resource_type=ResourceType.NETWORK_BANDWIDTH_MBPS,
                    soft_limit=200,
                    hard_limit=500,
                    unit="Mbps"
                )
            }
        )
        
        # Premium tier
        profiles[ResourceTier.PREMIUM] = ResourceProfile(
            profile_id="premium_default",
            name="Premium Tier",
            tier=ResourceTier.PREMIUM,
            limits={
                ResourceType.CPU_CORES: ResourceLimit(
                    resource_type=ResourceType.CPU_CORES,
                    soft_limit=8.0,
                    hard_limit=16.0,
                    burst_limit=24.0,
                    unit="cores"
                ),
                ResourceType.MEMORY_MB: ResourceLimit(
                    resource_type=ResourceType.MEMORY_MB,
                    soft_limit=16384,
                    hard_limit=32768,
                    unit="MB"
                ),
                ResourceType.DISK_GB: ResourceLimit(
                    resource_type=ResourceType.DISK_GB,
                    soft_limit=500,
                    hard_limit=1000,
                    unit="GB"
                ),
                ResourceType.MAX_VMS: ResourceLimit(
                    resource_type=ResourceType.MAX_VMS,
                    soft_limit=10,
                    hard_limit=25,
                    unit="count"
                ),
                ResourceType.NETWORK_BANDWIDTH_MBPS: ResourceLimit(
                    resource_type=ResourceType.NETWORK_BANDWIDTH_MBPS,
                    soft_limit=1000,
                    hard_limit=2000,
                    unit="Mbps"
                )
            }
        )
        
        return profiles
    
    async def get_user_limits(self, user_id: str, tier: ResourceTier = None) -> ResourceProfile:
        """Get resource limits for a user."""
        # In production, this would query the database
        # For now, return default profile for tier
        if tier and tier in self.default_profiles:
            return self.default_profiles[tier]
        
        # Default to free tier
        return self.default_profiles[ResourceTier.FREE]
    
    async def create_custom_profile(
        self,
        user_id: str,
        profile_name: str,
        limits: Dict[ResourceType, ResourceLimit]
    ) -> ResourceProfile:
        """Create a custom resource profile."""
        profile = ResourceProfile(
            profile_id=f"custom_{user_id}_{int(datetime.utcnow().timestamp())}",
            name=profile_name,
            tier=ResourceTier.CUSTOM,
            limits=limits
        )
        
        # In production, save to database
        self.logger.info(
            "Created custom resource profile",
            user_id=user_id,
            profile_id=profile.profile_id
        )
        
        return profile
    
    async def enforce_vm_limits(self, vm_id: str, user_id: str, limits: ResourceProfile):
        """Enforce resource limits on a VM using cgroups."""
        try:
            # Create cgroup for VM if it doesn't exist
            vm_cgroup = self.vm_cgroup_path / vm_id
            vm_cgroup.mkdir(parents=True, exist_ok=True)
            
            # Apply CPU limits
            if ResourceType.CPU_CORES in limits.limits:
                cpu_limit = limits.limits[ResourceType.CPU_CORES]
                await self._apply_cpu_limit(vm_cgroup, cpu_limit.hard_limit)
            
            # Apply memory limits
            if ResourceType.MEMORY_MB in limits.limits:
                memory_limit = limits.limits[ResourceType.MEMORY_MB]
                await self._apply_memory_limit(vm_cgroup, memory_limit.hard_limit)
            
            # Apply I/O limits
            if ResourceType.IOPS in limits.limits:
                iops_limit = limits.limits[ResourceType.IOPS]
                await self._apply_io_limit(vm_cgroup, iops_limit.hard_limit)
            
            # Apply network limits
            if ResourceType.NETWORK_BANDWIDTH_MBPS in limits.limits:
                bandwidth_limit = limits.limits[ResourceType.NETWORK_BANDWIDTH_MBPS]
                await self._apply_network_limit(vm_id, bandwidth_limit.hard_limit)
            
            self.logger.info(
                "Applied resource limits to VM",
                vm_id=vm_id,
                user_id=user_id,
                limits=limits.model_dump()
            )
            
        except Exception as e:
            self.logger.error(
                "Failed to apply resource limits",
                vm_id=vm_id,
                user_id=user_id,
                error=str(e)
            )
            raise
    
    async def _apply_cpu_limit(self, cgroup_path: Path, cpu_cores: float):
        """Apply CPU limit using cgroups v2."""
        # CPU quota (microseconds per 100ms period)
        quota = int(cpu_cores * 100000)
        period = 100000
        
        cpu_max_file = cgroup_path / "cpu.max"
        if cpu_max_file.exists():
            cpu_max_file.write_text(f"{quota} {period}")
    
    async def _apply_memory_limit(self, cgroup_path: Path, memory_mb: float):
        """Apply memory limit using cgroups v2."""
        memory_bytes = int(memory_mb * 1024 * 1024)
        
        memory_max_file = cgroup_path / "memory.max"
        if memory_max_file.exists():
            memory_max_file.write_text(str(memory_bytes))
    
    async def _apply_io_limit(self, cgroup_path: Path, iops: float):
        """Apply I/O limits using cgroups v2."""
        # This would implement I/O bandwidth and IOPS limits
        # using io.max interface in cgroups v2
        pass
    
    async def _apply_network_limit(self, vm_id: str, bandwidth_mbps: float):
        """Apply network bandwidth limits using tc (traffic control)."""
        try:
            # This would use tc to set up traffic shaping
            # Example: tc qdisc add dev veth0 root handle 1: htb default 12
            # tc class add dev veth0 parent 1: classid 1:1 htb rate {bandwidth_mbps}mbit
            pass
        except Exception as e:
            self.logger.error("Failed to apply network limits", vm_id=vm_id, error=str(e))
    
    async def monitor_vm_resources(self, vm_id: str) -> Dict[ResourceType, float]:
        """Monitor current resource usage for a VM."""
        usage = {}
        vm_cgroup = self.vm_cgroup_path / vm_id
        
        try:
            # Get CPU usage
            cpu_stat_file = vm_cgroup / "cpu.stat"
            if cpu_stat_file.exists():
                cpu_stats = cpu_stat_file.read_text()
                # Parse CPU usage from cgroup stats
                usage[ResourceType.CPU_CORES] = self._parse_cpu_usage(cpu_stats)
            
            # Get memory usage
            memory_current_file = vm_cgroup / "memory.current"
            if memory_current_file.exists():
                memory_bytes = int(memory_current_file.read_text().strip())
                usage[ResourceType.MEMORY_MB] = memory_bytes / (1024 * 1024)
            
            # Get I/O usage
            io_stat_file = vm_cgroup / "io.stat"
            if io_stat_file.exists():
                io_stats = io_stat_file.read_text()
                usage[ResourceType.IOPS] = self._parse_io_usage(io_stats)
            
            # Cache the usage data
            self.resource_usage_cache[vm_id] = usage
            
        except Exception as e:
            self.logger.error(
                "Failed to monitor VM resources",
                vm_id=vm_id,
                error=str(e)
            )
        
        return usage
    
    def _parse_cpu_usage(self, cpu_stats: str) -> float:
        """Parse CPU usage from cgroup stats."""
        # This would parse the actual CPU usage from the stats file
        # For now, return a placeholder
        return 0.0
    
    def _parse_io_usage(self, io_stats: str) -> float:
        """Parse I/O usage from cgroup stats."""
        # This would parse the actual I/O usage from the stats file
        # For now, return a placeholder
        return 0.0
    
    async def check_resource_quotas(self, user_id: str) -> Dict[ResourceType, ResourceStatus]:
        """Check if user is within resource quotas."""
        status = {}
        
        # Get user's resource profile
        profile = await self.get_user_limits(user_id)
        
        # Get current usage across all user's VMs
        current_usage = await self._get_user_total_usage(user_id)
        
        for resource_type, limit in profile.limits.items():
            used = current_usage.get(resource_type, 0)
            
            if used >= limit.hard_limit:
                status[resource_type] = ResourceStatus.EXCEEDED
            elif used >= limit.soft_limit:
                status[resource_type] = ResourceStatus.ALLOCATED
            else:
                status[resource_type] = ResourceStatus.AVAILABLE
        
        return status
    
    async def _get_user_total_usage(self, user_id: str) -> Dict[ResourceType, float]:
        """Get total resource usage across all user's VMs."""
        # This would query all VMs for the user and sum up usage
        # For now, return placeholder
        return {
            ResourceType.CPU_CORES: 0.0,
            ResourceType.MEMORY_MB: 0.0,
            ResourceType.DISK_GB: 0.0,
            ResourceType.MAX_VMS: 0,
        }
    
    async def can_allocate_resources(
        self,
        user_id: str,
        requested_resources: Dict[ResourceType, float]
    ) -> Tuple[bool, Dict[ResourceType, str]]:
        """Check if user can allocate additional resources."""
        profile = await self.get_user_limits(user_id)
        current_usage = await self._get_user_total_usage(user_id)
        
        can_allocate = True
        reasons = {}
        
        for resource_type, requested in requested_resources.items():
            if resource_type not in profile.limits:
                continue
            
            limit = profile.limits[resource_type]
            current = current_usage.get(resource_type, 0)
            
            if current + requested > limit.hard_limit:
                can_allocate = False
                reasons[resource_type] = (
                    f"Would exceed hard limit: {current + requested} > {limit.hard_limit}"
                )
            elif current + requested > limit.soft_limit:
                reasons[resource_type] = (
                    f"Would exceed soft limit: {current + requested} > {limit.soft_limit}"
                )
        
        return can_allocate, reasons
    
    async def release_vm_resources(self, vm_id: str):
        """Release resources when VM is destroyed."""
        try:
            # Remove cgroup
            vm_cgroup = self.vm_cgroup_path / vm_id
            if vm_cgroup.exists():
                # Move all processes out of cgroup first
                await self._move_processes_out(vm_cgroup)
                
                # Remove cgroup directory
                vm_cgroup.rmdir()
            
            # Remove from usage cache
            self.resource_usage_cache.pop(vm_id, None)
            
            self.logger.info("Released VM resources", vm_id=vm_id)
            
        except Exception as e:
            self.logger.error(
                "Failed to release VM resources",
                vm_id=vm_id,
                error=str(e)
            )
    
    async def _move_processes_out(self, cgroup_path: Path):
        """Move all processes out of a cgroup before removing it."""
        procs_file = cgroup_path / "cgroup.procs"
        if procs_file.exists():
            pids = procs_file.read_text().strip().split('\n')
            for pid in pids:
                if pid:
                    try:
                        # Move process to root cgroup
                        root_procs = self.cgroup_base_path / "cgroup.procs"
                        root_procs.write_text(pid)
                    except Exception:
                        pass  # Process might have exited


class ResourceAlertManager:
    """Manage resource usage alerts and notifications."""
    
    def __init__(self, resource_config: VMResourceConfig):
        self.resource_config = resource_config
        self.logger = structlog.get_logger()
        self.alert_thresholds = {
            "warning": 80.0,  # 80% of soft limit
            "critical": 95.0  # 95% of soft limit
        }
    
    async def check_and_send_alerts(self, user_id: str):
        """Check resource usage and send alerts if needed."""
        profile = await self.resource_config.get_user_limits(user_id)
        current_usage = await self.resource_config._get_user_total_usage(user_id)
        
        for resource_type, limit in profile.limits.items():
            used = current_usage.get(resource_type, 0)
            usage_percentage = (used / limit.soft_limit) * 100
            
            if usage_percentage >= self.alert_thresholds["critical"]:
                await self._send_alert(user_id, resource_type, "critical", usage_percentage)
            elif usage_percentage >= self.alert_thresholds["warning"]:
                await self._send_alert(user_id, resource_type, "warning", usage_percentage)
    
    async def _send_alert(
        self,
        user_id: str,
        resource_type: ResourceType,
        level: str,
        usage_percentage: float
    ):
        """Send resource usage alert."""
        self.logger.warning(
            "Resource usage alert",
            user_id=user_id,
            resource_type=resource_type.value,
            level=level,
            usage_percentage=usage_percentage
        )
        
        # In production, this would send actual notifications
        # (email, webhook, etc.)


class ResourceManagementAPI:
    """API for resource management operations."""
    
    def __init__(self):
        self.resource_config = VMResourceConfig()
        self.alert_manager = ResourceAlertManager(self.resource_config)
    
    async def create_vm_with_limits(
        self,
        vm_id: str,
        user_id: str,
        requested_resources: Dict[ResourceType, float]
    ) -> bool:
        """Create VM with resource limits."""
        # Check if user can allocate resources
        can_allocate, reasons = await self.resource_config.can_allocate_resources(
            user_id, requested_resources
        )
        
        if not can_allocate:
            raise ValueError(f"Cannot allocate resources: {reasons}")
        
        # Get user's resource profile
        profile = await self.resource_config.get_user_limits(user_id)
        
        # Apply limits to VM
        await self.resource_config.enforce_vm_limits(vm_id, user_id, profile)
        
        return True
    
    async def update_user_limits(
        self,
        user_id: str,
        new_limits: Dict[ResourceType, ResourceLimit]
    ) -> ResourceProfile:
        """Update resource limits for a user."""
        profile = await self.resource_config.create_custom_profile(
            user_id, f"Custom profile for {user_id}", new_limits
        )
        
        # In production, save to database and apply to existing VMs
        
        return profile
    
    async def get_resource_usage_report(self, user_id: str) -> Dict[str, Any]:
        """Get comprehensive resource usage report."""
        profile = await self.resource_config.get_user_limits(user_id)
        current_usage = await self.resource_config._get_user_total_usage(user_id)
        quota_status = await self.resource_config.check_resource_quotas(user_id)
        
        return {
            "user_id": user_id,
            "profile": profile.model_dump(),
            "current_usage": current_usage,
            "quota_status": {k.value: v.value for k, v in quota_status.items()},
            "timestamp": datetime.utcnow().isoformat()
        }


# Global resource management instance
resource_manager = ResourceManagementAPI()


# Configuration
class ResourceManagementConfig:
    """Resource management configuration."""
    
    # Cgroup settings
    CGROUP_VERSION = 2
    CGROUP_BASE_PATH = "/sys/fs/cgroup"
    BUILDPLATFORM_CGROUP = "buildplatform"
    
    # Monitoring
    MONITORING_INTERVAL = 30  # seconds
    ALERT_CHECK_INTERVAL = 300  # 5 minutes
    
    # Default limits
    DEFAULT_CPU_CORES = 2.0
    DEFAULT_MEMORY_MB = 4096
    DEFAULT_DISK_GB = 20
    DEFAULT_NETWORK_MBPS = 100
    
    # Safety margins
    SYSTEM_RESERVE_CPU_PERCENTAGE = 10
    SYSTEM_RESERVE_MEMORY_PERCENTAGE = 10
    
    @classmethod
    def from_env(cls):
        """Load configuration from environment."""
        return cls(
            MONITORING_INTERVAL=int(os.getenv("RESOURCE_MONITORING_INTERVAL", "30")),
            DEFAULT_CPU_CORES=float(os.getenv("DEFAULT_CPU_CORES", "2.0")),
            DEFAULT_MEMORY_MB=int(os.getenv("DEFAULT_MEMORY_MB", "4096"))
        )


# Example admin interface functions
async def admin_create_resource_tier(
    tier_name: str,
    limits: Dict[ResourceType, ResourceLimit]
) -> ResourceProfile:
    """Admin function to create new resource tier."""
    return await resource_manager.resource_config.create_custom_profile(
        "admin", tier_name, limits
    )


async def admin_get_global_usage() -> Dict[str, Any]:
    """Admin function to get global resource usage."""
    # This would aggregate usage across all users
    return {
        "total_users": 0,
        "total_vms": 0,
        "total_cpu_cores": 0,
        "total_memory_gb": 0,
        "timestamp": datetime.utcnow().isoformat()
    }


if __name__ == "__main__":
    # Example usage
    async def example_resource_management():
        # Create VM with resource limits
        await resource_manager.create_vm_with_limits(
            "vm-123",
            "user-456",
            {
                ResourceType.CPU_CORES: 2.0,
                ResourceType.MEMORY_MB: 4096,
                ResourceType.DISK_GB: 50
            }
        )
        
        # Get usage report
        report = await resource_manager.get_resource_usage_report("user-456")
        print(f"Resource usage report: {report}")
    
    asyncio.run(example_resource_management())