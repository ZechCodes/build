"""
Models for snapshot scheduling configuration and policies.
"""

from dataclasses import dataclass
from typing import Optional, Dict, Any, List
from enum import Enum
from datetime import datetime, timedelta


class ScheduleType(Enum):
    """Types of snapshot schedules."""
    INTERVAL = "interval"      # Every N hours/days
    DAILY = "daily"           # Daily at specific time
    WEEKLY = "weekly"         # Weekly on specific days
    MONTHLY = "monthly"       # Monthly on specific date


class RetentionPolicy(Enum):
    """Snapshot retention policies."""
    COUNT_BASED = "count"     # Keep N most recent snapshots
    TIME_BASED = "time"       # Keep snapshots for N days
    HYBRID = "hybrid"         # Combination of count and time


@dataclass
class ScheduleConfig:
    """Configuration for automated snapshot scheduling."""
    schedule_id: str
    vm_id: str
    user_id: str
    name: str
    description: str
    schedule_type: ScheduleType
    enabled: bool
    created_at: float
    updated_at: float
    
    # Schedule timing
    interval_hours: Optional[int] = None      # For INTERVAL type
    daily_time: Optional[str] = None          # For DAILY type (HH:MM format)
    weekly_days: Optional[List[int]] = None   # For WEEKLY type (0=Monday, 6=Sunday)
    monthly_day: Optional[int] = None         # For MONTHLY type (1-31)
    
    # Retention policy
    retention_policy: RetentionPolicy = RetentionPolicy.COUNT_BASED
    retention_count: Optional[int] = None     # Number of snapshots to keep
    retention_days: Optional[int] = None      # Days to keep snapshots
    
    # Schedule metadata
    last_run_at: Optional[float] = None
    next_run_at: Optional[float] = None
    run_count: int = 0
    failed_count: int = 0
    
    # Advanced options
    tags: Optional[Dict[str, str]] = None
    max_concurrent_snapshots: int = 1
    snapshot_name_template: str = "{vm_id}_{timestamp}"
    
    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary for serialization."""
        return {
            'schedule_id': self.schedule_id,
            'vm_id': self.vm_id,
            'user_id': self.user_id,
            'name': self.name,
            'description': self.description,
            'schedule_type': self.schedule_type.value,
            'enabled': self.enabled,
            'interval_hours': self.interval_hours,
            'daily_time': self.daily_time,
            'weekly_days': self.weekly_days,
            'monthly_day': self.monthly_day,
            'retention_policy': self.retention_policy.value,
            'retention_count': self.retention_count,
            'retention_days': self.retention_days,
            'created_at': self.created_at,
            'updated_at': self.updated_at,
            'last_run_at': self.last_run_at,
            'next_run_at': self.next_run_at,
            'run_count': self.run_count,
            'failed_count': self.failed_count,
            'tags': self.tags,
            'max_concurrent_snapshots': self.max_concurrent_snapshots,
            'snapshot_name_template': self.snapshot_name_template
        }


@dataclass
class SchedulePolicy:
    """Policy for snapshot scheduling behavior."""
    max_concurrent_jobs: int = 10
    retry_attempts: int = 3
    retry_delay_minutes: int = 5
    timeout_minutes: int = 60
    cleanup_orphaned_schedules: bool = True
    max_schedule_age_days: int = 365
    enable_metrics: bool = True
    
    # Resource limits
    max_snapshots_per_user: int = 50
    max_schedules_per_user: int = 10
    max_schedules_per_vm: int = 3
    
    # Performance tuning
    batch_size: int = 5
    worker_count: int = 3
    health_check_interval_minutes: int = 5