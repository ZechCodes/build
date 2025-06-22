"""
Comprehensive test suite for automated snapshot scheduling.
Tests schedule creation, execution, retention policies, and error handling.
"""

import pytest
import asyncio
import time
from unittest.mock import AsyncMock, MagicMock, patch
from datetime import datetime, timedelta

# Import scheduler components
import sys
from pathlib import Path
current_dir = Path(__file__).parent
parent_dir = current_dir.parent
sys.path.insert(0, str(parent_dir))

from scheduler.schedule_manager import ScheduleManager
from scheduler.schedule_models import ScheduleConfig, SchedulePolicy, ScheduleType, RetentionPolicy


@pytest.fixture
def mock_snapshot_manager():
    """Mock snapshot manager for testing."""
    manager = AsyncMock()
    manager.create_snapshot = AsyncMock(return_value="snap_scheduled_123")
    manager.delete_snapshot = AsyncMock(return_value=True)
    manager.list_user_snapshots = AsyncMock(return_value=[])
    return manager


@pytest.fixture
def mock_database():
    """Mock database for testing."""
    return AsyncMock()


@pytest.fixture
def schedule_policy():
    """Test policy for scheduling."""
    return SchedulePolicy(
        max_concurrent_jobs=5,
        worker_count=2,
        batch_size=3,
        health_check_interval_minutes=1,
        max_schedules_per_user=5,
        max_schedules_per_vm=2
    )


@pytest.fixture
def schedule_manager(mock_snapshot_manager, mock_database, schedule_policy):
    """Schedule manager instance for testing."""
    return ScheduleManager(
        snapshot_manager=mock_snapshot_manager,
        database=mock_database,
        policy=schedule_policy
    )


class TestScheduleManager:
    """Test schedule manager core functionality."""
    
    async def test_manager_startup_and_shutdown(self, schedule_manager):
        """Test schedule manager startup and shutdown."""
        assert not schedule_manager.is_running
        assert len(schedule_manager.worker_tasks) == 0
        
        # Start manager
        await schedule_manager.start()
        
        assert schedule_manager.is_running
        assert len(schedule_manager.worker_tasks) == 3  # 2 workers + 1 health check
        
        # Stop manager
        await schedule_manager.stop()
        
        assert not schedule_manager.is_running
        assert len(schedule_manager.worker_tasks) == 0
    
    async def test_create_interval_schedule(self, schedule_manager):
        """Test creating an interval-based schedule."""
        schedule_id = await schedule_manager.create_schedule(
            vm_id="vm_user123_test",
            user_id="user123",
            name="hourly-backup",
            schedule_type=ScheduleType.INTERVAL,
            description="Hourly automated snapshots",
            interval_hours=2,
            retention_policy=RetentionPolicy.COUNT_BASED,
            retention_count=5
        )
        
        assert schedule_id.startswith("sched_")
        assert schedule_id in schedule_manager.schedules
        
        schedule = schedule_manager.schedules[schedule_id]
        assert schedule.vm_id == "vm_user123_test"
        assert schedule.user_id == "user123"
        assert schedule.name == "hourly-backup"
        assert schedule.schedule_type == ScheduleType.INTERVAL
        assert schedule.interval_hours == 2
        assert schedule.enabled is True
        assert schedule.next_run_at is not None
    
    async def test_create_daily_schedule(self, schedule_manager):
        """Test creating a daily schedule."""
        schedule_id = await schedule_manager.create_schedule(
            vm_id="vm_user123_test",
            user_id="user123",
            name="daily-backup",
            schedule_type=ScheduleType.DAILY,
            daily_time="02:30",
            retention_policy=RetentionPolicy.TIME_BASED,
            retention_days=30
        )
        
        schedule = schedule_manager.schedules[schedule_id]
        assert schedule.schedule_type == ScheduleType.DAILY
        assert schedule.daily_time == "02:30"
        assert schedule.retention_policy == RetentionPolicy.TIME_BASED
        assert schedule.retention_days == 30
    
    async def test_create_weekly_schedule(self, schedule_manager):
        """Test creating a weekly schedule."""
        schedule_id = await schedule_manager.create_schedule(
            vm_id="vm_user123_test",
            user_id="user123",
            name="weekly-backup",
            schedule_type=ScheduleType.WEEKLY,
            weekly_days=[1, 3, 5],  # Tuesday, Thursday, Saturday
            retention_policy=RetentionPolicy.HYBRID,
            retention_count=10,
            retention_days=90
        )
        
        schedule = schedule_manager.schedules[schedule_id]
        assert schedule.schedule_type == ScheduleType.WEEKLY
        assert schedule.weekly_days == [1, 3, 5]
        assert schedule.retention_policy == RetentionPolicy.HYBRID
    
    async def test_create_monthly_schedule(self, schedule_manager):
        """Test creating a monthly schedule."""
        schedule_id = await schedule_manager.create_schedule(
            vm_id="vm_user123_test",
            user_id="user123",
            name="monthly-backup",
            schedule_type=ScheduleType.MONTHLY,
            monthly_day=15,
            retention_count=12
        )
        
        schedule = schedule_manager.schedules[schedule_id]
        assert schedule.schedule_type == ScheduleType.MONTHLY
        assert schedule.monthly_day == 15
    
    async def test_schedule_validation(self, schedule_manager):
        """Test schedule validation errors."""
        # Test interval without hours
        with pytest.raises(ValueError, match="interval_hours"):
            await schedule_manager.create_schedule(
                vm_id="vm_test",
                user_id="user123",
                name="invalid-interval",
                schedule_type=ScheduleType.INTERVAL
            )
        
        # Test daily without time
        with pytest.raises(ValueError, match="daily_time"):
            await schedule_manager.create_schedule(
                vm_id="vm_test",
                user_id="user123",
                name="invalid-daily",
                schedule_type=ScheduleType.DAILY
            )
        
        # Test weekly without days
        with pytest.raises(ValueError, match="weekly_days"):
            await schedule_manager.create_schedule(
                vm_id="vm_test",
                user_id="user123",
                name="invalid-weekly",
                schedule_type=ScheduleType.WEEKLY
            )
        
        # Test monthly without day
        with pytest.raises(ValueError, match="monthly_day"):
            await schedule_manager.create_schedule(
                vm_id="vm_test",
                user_id="user123",
                name="invalid-monthly",
                schedule_type=ScheduleType.MONTHLY
            )
    
    async def test_user_schedule_limits(self, schedule_manager):
        """Test user schedule limits."""
        user_id = "user123"
        vm_id = "vm_user123_test"
        
        # Create schedules up to limit
        for i in range(schedule_manager.policy.max_schedules_per_user):
            await schedule_manager.create_schedule(
                vm_id=f"{vm_id}_{i}",
                user_id=user_id,
                name=f"schedule-{i}",
                schedule_type=ScheduleType.INTERVAL,
                interval_hours=1
            )
        
        # Next schedule should fail
        with pytest.raises(ValueError, match="maximum schedules limit"):
            await schedule_manager.create_schedule(
                vm_id="vm_user123_extra",
                user_id=user_id,
                name="over-limit",
                schedule_type=ScheduleType.INTERVAL,
                interval_hours=1
            )
    
    async def test_vm_schedule_limits(self, schedule_manager):
        """Test VM schedule limits."""
        vm_id = "vm_user123_test"
        
        # Create schedules up to VM limit
        for i in range(schedule_manager.policy.max_schedules_per_vm):
            await schedule_manager.create_schedule(
                vm_id=vm_id,
                user_id=f"user{i}",
                name=f"schedule-{i}",
                schedule_type=ScheduleType.INTERVAL,
                interval_hours=1
            )
        
        # Next schedule should fail
        with pytest.raises(ValueError, match="maximum schedules limit"):
            await schedule_manager.create_schedule(
                vm_id=vm_id,
                user_id="userextra",
                name="over-limit",
                schedule_type=ScheduleType.INTERVAL,
                interval_hours=1
            )
    
    async def test_update_schedule(self, schedule_manager):
        """Test updating an existing schedule."""
        # Create schedule
        schedule_id = await schedule_manager.create_schedule(
            vm_id="vm_user123_test",
            user_id="user123",
            name="test-schedule",
            schedule_type=ScheduleType.INTERVAL,
            interval_hours=2
        )
        
        original_updated_at = schedule_manager.schedules[schedule_id].updated_at
        
        # Update schedule
        updated = await schedule_manager.update_schedule(
            schedule_id=schedule_id,
            user_id="user123",
            interval_hours=4,
            enabled=False
        )
        
        assert updated is True
        schedule = schedule_manager.schedules[schedule_id]
        assert schedule.interval_hours == 4
        assert schedule.enabled is False
        assert schedule.updated_at > original_updated_at
    
    async def test_update_schedule_unauthorized(self, schedule_manager):
        """Test updating schedule with wrong user."""
        # Create schedule
        schedule_id = await schedule_manager.create_schedule(
            vm_id="vm_user123_test",
            user_id="user123",
            name="test-schedule",
            schedule_type=ScheduleType.INTERVAL,
            interval_hours=2
        )
        
        # Try to update with different user
        with pytest.raises(ValueError, match="Access denied"):
            await schedule_manager.update_schedule(
                schedule_id=schedule_id,
                user_id="user456",
                interval_hours=4
            )
    
    async def test_delete_schedule(self, schedule_manager):
        """Test deleting a schedule."""
        # Create schedule
        schedule_id = await schedule_manager.create_schedule(
            vm_id="vm_user123_test",
            user_id="user123",
            name="test-schedule",
            schedule_type=ScheduleType.INTERVAL,
            interval_hours=2
        )
        
        assert schedule_id in schedule_manager.schedules
        
        # Delete schedule
        deleted = await schedule_manager.delete_schedule(schedule_id, "user123")
        
        assert deleted is True
        assert schedule_id not in schedule_manager.schedules
    
    async def test_delete_schedule_unauthorized(self, schedule_manager):
        """Test deleting schedule with wrong user."""
        # Create schedule
        schedule_id = await schedule_manager.create_schedule(
            vm_id="vm_user123_test",
            user_id="user123",
            name="test-schedule",
            schedule_type=ScheduleType.INTERVAL,
            interval_hours=2
        )
        
        # Try to delete with different user
        with pytest.raises(ValueError, match="Access denied"):
            await schedule_manager.delete_schedule(schedule_id, "user456")
    
    async def test_get_user_schedules(self, schedule_manager):
        """Test getting user's schedules."""
        user_id = "user123"
        
        # Create multiple schedules
        schedule_ids = []
        for i in range(3):
            schedule_id = await schedule_manager.create_schedule(
                vm_id=f"vm_user123_test_{i}",
                user_id=user_id,
                name=f"schedule-{i}",
                schedule_type=ScheduleType.INTERVAL,
                interval_hours=i + 1
            )
            schedule_ids.append(schedule_id)
        
        # Create schedule for different user
        await schedule_manager.create_schedule(
            vm_id="vm_user456_test",
            user_id="user456",
            name="other-schedule",
            schedule_type=ScheduleType.INTERVAL,
            interval_hours=1
        )
        
        # Get user schedules
        user_schedules = await schedule_manager.get_user_schedules(user_id)
        
        assert len(user_schedules) == 3
        assert all(s.user_id == user_id for s in user_schedules)
        assert all(s.schedule_id in schedule_ids for s in user_schedules)


class TestScheduleExecution:
    """Test schedule execution and job processing."""
    
    async def test_schedule_execution(self, schedule_manager, mock_snapshot_manager):
        """Test basic schedule execution."""
        # Create schedule with immediate execution
        schedule_id = await schedule_manager.create_schedule(
            vm_id="vm_user123_test",
            user_id="user123",
            name="immediate-test",
            schedule_type=ScheduleType.INTERVAL,
            interval_hours=1
        )
        
        # Manually set next run to now for immediate execution
        schedule = schedule_manager.schedules[schedule_id]
        schedule.next_run_at = time.time() - 1  # 1 second ago
        
        # Start manager
        await schedule_manager.start()
        
        # Wait for job execution
        await asyncio.sleep(2)
        
        # Verify snapshot was created
        mock_snapshot_manager.create_snapshot.assert_called_once()
        
        # Verify schedule was updated
        assert schedule.run_count == 1
        assert schedule.last_run_at is not None
        
        await schedule_manager.stop()
    
    async def test_schedule_name_templating(self, schedule_manager, mock_snapshot_manager):
        """Test snapshot name templating."""
        # Create schedule with custom name template
        schedule_id = await schedule_manager.create_schedule(
            vm_id="vm_user123_test",
            user_id="user123",
            name="template-test",
            schedule_type=ScheduleType.INTERVAL,
            interval_hours=1,
            snapshot_name_template="backup_{vm_id}_{schedule_name}_{timestamp}"
        )
        
        # Execute schedule manually
        schedule = schedule_manager.schedules[schedule_id]
        await schedule_manager._execute_schedule_job(schedule)
        
        # Verify snapshot name follows template
        call_args = mock_snapshot_manager.create_snapshot.call_args
        snapshot_name = call_args[1]['name']
        
        assert "backup_vm_user123_test_template-test_" in snapshot_name
        assert len(snapshot_name.split('_')) >= 4
    
    async def test_schedule_failure_handling(self, schedule_manager, mock_snapshot_manager):
        """Test schedule failure handling and retry logic."""
        # Mock snapshot creation to fail
        mock_snapshot_manager.create_snapshot.side_effect = Exception("Snapshot creation failed")
        
        # Create schedule
        schedule_id = await schedule_manager.create_schedule(
            vm_id="vm_user123_test",
            user_id="user123",
            name="failure-test",
            schedule_type=ScheduleType.INTERVAL,
            interval_hours=1
        )
        
        schedule = schedule_manager.schedules[schedule_id]
        
        # Execute schedule multiple times to test failure handling
        for i in range(schedule_manager.policy.retry_attempts + 1):
            try:
                await schedule_manager._execute_schedule_job(schedule)
            except Exception:
                pass  # Expected to fail
        
        # Verify failure count incremented
        assert schedule.failed_count == schedule_manager.policy.retry_attempts + 1
        
        # Verify schedule was disabled after max failures
        assert schedule.enabled is False


class TestRetentionPolicies:
    """Test snapshot retention policies."""
    
    def create_mock_snapshots(self, count: int, vm_id: str, schedule_name: str):
        """Create mock snapshots for testing retention."""
        snapshots = []
        current_time = time.time()
        
        for i in range(count):
            snapshot = MagicMock()
            snapshot.snapshot_id = f"snap_{i}"
            snapshot.vm_id = vm_id
            snapshot.name = f"{schedule_name}_backup_{i}"
            snapshot.snapshot_type = "scheduled"
            snapshot.created_at = current_time - (i * 24 * 3600)  # One per day, going back
            snapshots.append(snapshot)
        
        return snapshots
    
    async def test_count_based_retention(self, schedule_manager, mock_snapshot_manager):
        """Test count-based retention policy."""
        # Create schedule with count-based retention
        schedule_id = await schedule_manager.create_schedule(
            vm_id="vm_user123_test",
            user_id="user123",
            name="count-retention-test",
            schedule_type=ScheduleType.INTERVAL,
            interval_hours=1,
            retention_policy=RetentionPolicy.COUNT_BASED,
            retention_count=3
        )
        
        # Mock existing snapshots (5 snapshots, should keep 3)
        mock_snapshots = self.create_mock_snapshots(5, "vm_user123_test", "count-retention-test")
        mock_snapshot_manager.list_user_snapshots.return_value = mock_snapshots
        
        schedule = schedule_manager.schedules[schedule_id]
        
        # Apply retention policy
        await schedule_manager._apply_retention_policy(schedule)
        
        # Verify 2 oldest snapshots were deleted (keep 3 most recent)
        assert mock_snapshot_manager.delete_snapshot.call_count == 2
        
        # Verify correct snapshots were deleted (oldest ones)
        deleted_snapshot_ids = [
            call[0][0] for call in mock_snapshot_manager.delete_snapshot.call_args_list
        ]
        assert "snap_3" in deleted_snapshot_ids
        assert "snap_4" in deleted_snapshot_ids
    
    async def test_time_based_retention(self, schedule_manager, mock_snapshot_manager):
        """Test time-based retention policy."""
        # Create schedule with time-based retention (keep 2 days)
        schedule_id = await schedule_manager.create_schedule(
            vm_id="vm_user123_test",
            user_id="user123",
            name="time-retention-test",
            schedule_type=ScheduleType.INTERVAL,
            interval_hours=1,
            retention_policy=RetentionPolicy.TIME_BASED,
            retention_days=2
        )
        
        # Mock existing snapshots (5 snapshots, 3 older than 2 days)
        mock_snapshots = self.create_mock_snapshots(5, "vm_user123_test", "time-retention-test")
        mock_snapshot_manager.list_user_snapshots.return_value = mock_snapshots
        
        schedule = schedule_manager.schedules[schedule_id]
        
        # Apply retention policy
        await schedule_manager._apply_retention_policy(schedule)
        
        # Verify 3 old snapshots were deleted (older than 2 days)
        assert mock_snapshot_manager.delete_snapshot.call_count == 3
    
    async def test_hybrid_retention(self, schedule_manager, mock_snapshot_manager):
        """Test hybrid retention policy."""
        # Create schedule with hybrid retention
        schedule_id = await schedule_manager.create_schedule(
            vm_id="vm_user123_test",
            user_id="user123",
            name="hybrid-retention-test",
            schedule_type=ScheduleType.INTERVAL,
            interval_hours=1,
            retention_policy=RetentionPolicy.HYBRID,
            retention_count=3,
            retention_days=2
        )
        
        # Mock existing snapshots
        mock_snapshots = self.create_mock_snapshots(6, "vm_user123_test", "hybrid-retention-test")
        mock_snapshot_manager.list_user_snapshots.return_value = mock_snapshots
        
        schedule = schedule_manager.schedules[schedule_id]
        
        # Apply retention policy
        await schedule_manager._apply_retention_policy(schedule)
        
        # Should delete based on both count and time constraints
        # Exact number depends on overlap between policies
        assert mock_snapshot_manager.delete_snapshot.call_count >= 3


class TestScheduleCalculation:
    """Test schedule calculation algorithms."""
    
    def test_interval_calculation(self, schedule_manager):
        """Test interval-based next run calculation."""
        from scheduler.schedule_models import ScheduleConfig
        
        schedule = ScheduleConfig(
            schedule_id="test",
            vm_id="vm_test",
            user_id="user_test",
            name="test",
            description="",
            schedule_type=ScheduleType.INTERVAL,
            enabled=True,
            interval_hours=2,
            created_at=time.time(),
            updated_at=time.time()
        )
        
        current_time = time.time()
        next_run = schedule_manager._calculate_next_run(schedule)
        
        # Should be approximately 2 hours from now
        expected_time = current_time + (2 * 3600)
        assert abs(next_run - expected_time) < 60  # Within 1 minute tolerance
    
    def test_daily_calculation(self, schedule_manager):
        """Test daily schedule calculation."""
        from scheduler.schedule_models import ScheduleConfig
        
        schedule = ScheduleConfig(
            schedule_id="test",
            vm_id="vm_test",
            user_id="user_test",
            name="test",
            description="",
            schedule_type=ScheduleType.DAILY,
            enabled=True,
            daily_time="14:30",  # 2:30 PM
            created_at=time.time(),
            updated_at=time.time()
        )
        
        next_run = schedule_manager._calculate_next_run(schedule)
        next_run_dt = datetime.fromtimestamp(next_run)
        
        # Should be at 14:30
        assert next_run_dt.hour == 14
        assert next_run_dt.minute == 30
        assert next_run_dt.second == 0
    
    def test_weekly_calculation(self, schedule_manager):
        """Test weekly schedule calculation."""
        from scheduler.schedule_models import ScheduleConfig
        
        schedule = ScheduleConfig(
            schedule_id="test",
            vm_id="vm_test",
            user_id="user_test",
            name="test",
            description="",
            schedule_type=ScheduleType.WEEKLY,
            enabled=True,
            weekly_days=[1, 3, 5],  # Tuesday, Thursday, Saturday
            created_at=time.time(),
            updated_at=time.time()
        )
        
        next_run = schedule_manager._calculate_next_run(schedule)
        next_run_dt = datetime.fromtimestamp(next_run)
        
        # Should be on one of the specified days
        assert next_run_dt.weekday() in [1, 3, 5]
    
    def test_monthly_calculation(self, schedule_manager):
        """Test monthly schedule calculation."""
        from scheduler.schedule_models import ScheduleConfig
        
        schedule = ScheduleConfig(
            schedule_id="test",
            vm_id="vm_test",
            user_id="user_test",
            name="test",
            description="",
            schedule_type=ScheduleType.MONTHLY,
            enabled=True,
            monthly_day=15,
            created_at=time.time(),
            updated_at=time.time()
        )
        
        next_run = schedule_manager._calculate_next_run(schedule)
        next_run_dt = datetime.fromtimestamp(next_run)
        
        # Should be on the 15th of the month
        assert next_run_dt.day == 15


class TestScheduleMetrics:
    """Test schedule metrics and monitoring."""
    
    async def test_metrics_collection(self, schedule_manager):
        """Test metrics collection during operation."""
        # Create some schedules
        for i in range(3):
            await schedule_manager.create_schedule(
                vm_id=f"vm_user123_test_{i}",
                user_id="user123",
                name=f"schedule-{i}",
                schedule_type=ScheduleType.INTERVAL,
                interval_hours=1,
                enabled=(i % 2 == 0)  # Enable every other schedule
            )
        
        metrics = schedule_manager.get_metrics()
        
        assert metrics['total_schedules'] == 3
        assert metrics['active_schedules'] == 2  # 2 enabled
        assert metrics['running_jobs'] == 0
        assert metrics['is_running'] is False
        assert 'schedules_executed' in metrics
        assert 'snapshots_created' in metrics


if __name__ == "__main__":
    pytest.main([__file__, "-v"])