"""
Automated schedule manager for VM snapshot operations.

Provides comprehensive scheduling capabilities with cron-like functionality,
retention policies, and robust error handling.
"""

import asyncio
import time
import secrets
import json
from typing import Dict, List, Optional, Any, Callable
from datetime import datetime, timedelta
from dataclasses import asdict
import structlog
import logfire

from .schedule_models import ScheduleConfig, SchedulePolicy, ScheduleType, RetentionPolicy

logger = structlog.get_logger()


class ScheduleManager:
    """
    Comprehensive schedule manager for automated VM snapshots.
    
    Provides cron-like scheduling with retention policies, error handling,
    and comprehensive monitoring integration.
    """
    
    def __init__(self, snapshot_manager, database, policy: SchedulePolicy = None):
        """Initialize schedule manager."""
        self.snapshot_manager = snapshot_manager
        self.database = database
        self.policy = policy or SchedulePolicy()
        
        # Runtime state
        self.schedules: Dict[str, ScheduleConfig] = {}
        self.running_jobs: Dict[str, asyncio.Task] = {}
        self.worker_tasks: List[asyncio.Task] = []
        self.is_running = False
        self.metrics = {
            'schedules_executed': 0,
            'snapshots_created': 0,
            'snapshots_cleaned': 0,
            'jobs_failed': 0,
            'last_health_check': 0
        }
    
    async def start(self):
        """Start the schedule manager and worker tasks."""
        if self.is_running:
            logger.warning("Schedule manager already running")
            return
        
        try:
            # Load existing schedules
            await self._load_schedules()
            
            # Start worker tasks
            self.is_running = True
            for i in range(self.policy.worker_count):
                task = asyncio.create_task(self._worker_loop(f"worker-{i}"))
                self.worker_tasks.append(task)
            
            # Start health check task
            health_task = asyncio.create_task(self._health_check_loop())
            self.worker_tasks.append(health_task)
            
            logger.info("Schedule manager started",
                       workers=self.policy.worker_count,
                       schedules_loaded=len(self.schedules))
            
            logfire.info("Schedule manager started",
                        workers=self.policy.worker_count,
                        schedules_count=len(self.schedules),
                        service="snapshot-scheduler")
            
        except Exception as e:
            logger.error("Failed to start schedule manager", error=str(e))
            logfire.error("Schedule manager startup failed", error=str(e))
            raise
    
    async def stop(self):
        """Stop the schedule manager and all worker tasks."""
        if not self.is_running:
            return
        
        logger.info("Stopping schedule manager")
        self.is_running = False
        
        # Cancel all running jobs
        for job_id, task in self.running_jobs.items():
            if not task.done():
                task.cancel()
                logger.debug("Cancelled running job", job_id=job_id)
        
        # Wait for jobs to complete with timeout
        if self.running_jobs:
            try:
                await asyncio.wait_for(
                    asyncio.gather(*self.running_jobs.values(), return_exceptions=True),
                    timeout=30.0
                )
            except asyncio.TimeoutError:
                logger.warning("Some jobs did not complete within timeout")
        
        # Cancel worker tasks
        for task in self.worker_tasks:
            if not task.done():
                task.cancel()
        
        # Wait for workers to stop
        if self.worker_tasks:
            await asyncio.gather(*self.worker_tasks, return_exceptions=True)
        
        self.worker_tasks.clear()
        self.running_jobs.clear()
        
        logger.info("Schedule manager stopped")
        logfire.info("Schedule manager stopped", service="snapshot-scheduler")
    
    async def create_schedule(self, vm_id: str, user_id: str, name: str,
                            schedule_type: ScheduleType, **kwargs) -> str:
        """
        Create a new snapshot schedule.
        
        Args:
            vm_id: VM to schedule snapshots for
            user_id: User creating the schedule
            name: Schedule name
            schedule_type: Type of schedule
            **kwargs: Additional schedule parameters
            
        Returns:
            str: Schedule ID
        """
        # Validate user limits
        user_schedules = [s for s in self.schedules.values() if s.user_id == user_id]
        if len(user_schedules) >= self.policy.max_schedules_per_user:
            raise ValueError(f"User has reached maximum schedules limit ({self.policy.max_schedules_per_user})")
        
        vm_schedules = [s for s in self.schedules.values() if s.vm_id == vm_id]
        if len(vm_schedules) >= self.policy.max_schedules_per_vm:
            raise ValueError(f"VM has reached maximum schedules limit ({self.policy.max_schedules_per_vm})")
        
        # Generate schedule ID
        schedule_id = f"sched_{secrets.token_hex(16)}"
        
        # Create schedule configuration
        current_time = time.time()
        schedule = ScheduleConfig(
            schedule_id=schedule_id,
            vm_id=vm_id,
            user_id=user_id,
            name=name,
            description=kwargs.get('description', ''),
            schedule_type=schedule_type,
            enabled=kwargs.get('enabled', True),
            interval_hours=kwargs.get('interval_hours'),
            daily_time=kwargs.get('daily_time'),
            weekly_days=kwargs.get('weekly_days'),
            monthly_day=kwargs.get('monthly_day'),
            retention_policy=kwargs.get('retention_policy', RetentionPolicy.COUNT_BASED),
            retention_count=kwargs.get('retention_count', 7),
            retention_days=kwargs.get('retention_days'),
            created_at=current_time,
            updated_at=current_time,
            tags=kwargs.get('tags', {}),
            max_concurrent_snapshots=kwargs.get('max_concurrent_snapshots', 1),
            snapshot_name_template=kwargs.get('snapshot_name_template', "{vm_id}_{timestamp}")
        )
        
        # Validate schedule configuration
        self._validate_schedule(schedule)
        
        # Calculate next run time
        schedule.next_run_at = self._calculate_next_run(schedule)
        
        # Store schedule
        self.schedules[schedule_id] = schedule
        await self._persist_schedule(schedule)
        
        logger.info("Schedule created",
                   schedule_id=schedule_id,
                   vm_id=vm_id,
                   user_id=user_id,
                   schedule_type=schedule_type.value,
                   next_run=datetime.fromtimestamp(schedule.next_run_at).isoformat())
        
        logfire.info("Snapshot schedule created",
                    schedule_id=schedule_id,
                    vm_id=vm_id,
                    user_id=user_id,
                    schedule_type=schedule_type.value,
                    next_run_timestamp=schedule.next_run_at)
        
        return schedule_id
    
    async def update_schedule(self, schedule_id: str, user_id: str, **kwargs) -> bool:
        """
        Update an existing schedule.
        
        Args:
            schedule_id: Schedule to update
            user_id: User making the update
            **kwargs: Fields to update
            
        Returns:
            bool: True if updated successfully
        """
        if schedule_id not in self.schedules:
            raise ValueError("Schedule not found")
        
        schedule = self.schedules[schedule_id]
        if schedule.user_id != user_id:
            raise ValueError("Access denied - not schedule owner")
        
        # Update fields
        updated = False
        for field, value in kwargs.items():
            if hasattr(schedule, field) and getattr(schedule, field) != value:
                setattr(schedule, field, value)
                updated = True
        
        if updated:
            schedule.updated_at = time.time()
            
            # Recalculate next run if timing changed
            if any(field in kwargs for field in ['schedule_type', 'interval_hours', 'daily_time', 'weekly_days', 'monthly_day']):
                schedule.next_run_at = self._calculate_next_run(schedule)
            
            self._validate_schedule(schedule)
            await self._persist_schedule(schedule)
            
            logger.info("Schedule updated", schedule_id=schedule_id, user_id=user_id)
            logfire.info("Schedule updated", schedule_id=schedule_id, user_id=user_id)
        
        return updated
    
    async def delete_schedule(self, schedule_id: str, user_id: str) -> bool:
        """
        Delete a schedule.
        
        Args:
            schedule_id: Schedule to delete
            user_id: User requesting deletion
            
        Returns:
            bool: True if deleted successfully
        """
        if schedule_id not in self.schedules:
            raise ValueError("Schedule not found")
        
        schedule = self.schedules[schedule_id]
        if schedule.user_id != user_id:
            raise ValueError("Access denied - not schedule owner")
        
        # Cancel any running job for this schedule
        if schedule_id in self.running_jobs:
            self.running_jobs[schedule_id].cancel()
            del self.running_jobs[schedule_id]
        
        # Remove from memory and storage
        del self.schedules[schedule_id]
        await self._delete_schedule_from_storage(schedule_id)
        
        logger.info("Schedule deleted", schedule_id=schedule_id, user_id=user_id)
        logfire.info("Schedule deleted", schedule_id=schedule_id, user_id=user_id)
        
        return True
    
    async def get_user_schedules(self, user_id: str) -> List[ScheduleConfig]:
        """Get all schedules for a user."""
        return [s for s in self.schedules.values() if s.user_id == user_id]
    
    async def get_schedule(self, schedule_id: str, user_id: str) -> Optional[ScheduleConfig]:
        """Get a specific schedule."""
        schedule = self.schedules.get(schedule_id)
        if schedule and schedule.user_id == user_id:
            return schedule
        return None
    
    def get_metrics(self) -> Dict[str, Any]:
        """Get scheduler metrics."""
        return {
            'active_schedules': len([s for s in self.schedules.values() if s.enabled]),
            'total_schedules': len(self.schedules),
            'running_jobs': len(self.running_jobs),
            'worker_count': len(self.worker_tasks),
            'is_running': self.is_running,
            **self.metrics
        }
    
    # Private methods
    
    async def _worker_loop(self, worker_id: str):
        """Main worker loop for processing scheduled jobs."""
        logger.debug("Worker started", worker_id=worker_id)
        
        while self.is_running:
            try:
                # Find schedules ready to run
                ready_schedules = []
                current_time = time.time()
                
                for schedule in self.schedules.values():
                    if (schedule.enabled and 
                        schedule.next_run_at and 
                        schedule.next_run_at <= current_time and
                        schedule.schedule_id not in self.running_jobs):
                        ready_schedules.append(schedule)
                
                # Process ready schedules in batches
                for schedule in ready_schedules[:self.policy.batch_size]:
                    if len(self.running_jobs) >= self.policy.max_concurrent_jobs:
                        break
                    
                    # Start job
                    task = asyncio.create_task(self._execute_schedule_job(schedule))
                    self.running_jobs[schedule.schedule_id] = task
                
                # Clean up completed jobs
                completed_jobs = []
                for schedule_id, task in self.running_jobs.items():
                    if task.done():
                        completed_jobs.append(schedule_id)
                        try:
                            await task  # Retrieve any exceptions
                        except Exception as e:
                            logger.error("Job failed", schedule_id=schedule_id, error=str(e))
                            self.metrics['jobs_failed'] += 1
                
                for schedule_id in completed_jobs:
                    del self.running_jobs[schedule_id]
                
                # Sleep before next iteration
                await asyncio.sleep(10)  # Check every 10 seconds
                
            except Exception as e:
                logger.error("Worker loop error", worker_id=worker_id, error=str(e))
                await asyncio.sleep(30)  # Longer sleep on error
    
    async def _execute_schedule_job(self, schedule: ScheduleConfig):
        """Execute a scheduled snapshot job."""
        job_start = time.time()
        
        try:
            logger.info("Executing scheduled snapshot",
                       schedule_id=schedule.schedule_id,
                       vm_id=schedule.vm_id,
                       user_id=schedule.user_id)
            
            # Generate snapshot name from template
            timestamp = datetime.fromtimestamp(job_start).strftime("%Y%m%d_%H%M%S")
            snapshot_name = schedule.snapshot_name_template.format(
                vm_id=schedule.vm_id,
                timestamp=timestamp,
                schedule_name=schedule.name
            )
            
            # Create snapshot
            snapshot_id = await self.snapshot_manager.create_snapshot(
                vm_id=schedule.vm_id,
                user_id=schedule.user_id,
                name=snapshot_name,
                description=f"Automated snapshot from schedule: {schedule.name}",
                snapshot_type="scheduled",
                tags=schedule.tags or {}
            )
            
            # Update schedule statistics
            schedule.last_run_at = job_start
            schedule.run_count += 1
            schedule.next_run_at = self._calculate_next_run(schedule)
            schedule.updated_at = time.time()
            
            # Persist updated schedule
            await self._persist_schedule(schedule)
            
            # Apply retention policy
            await self._apply_retention_policy(schedule)
            
            # Update metrics
            self.metrics['schedules_executed'] += 1
            self.metrics['snapshots_created'] += 1
            
            duration_ms = (time.time() - job_start) * 1000
            
            logger.info("Scheduled snapshot completed",
                       schedule_id=schedule.schedule_id,
                       snapshot_id=snapshot_id,
                       duration_ms=duration_ms,
                       next_run=datetime.fromtimestamp(schedule.next_run_at).isoformat())
            
            logfire.info("Scheduled snapshot completed",
                        schedule_id=schedule.schedule_id,
                        snapshot_id=snapshot_id,
                        vm_id=schedule.vm_id,
                        user_id=schedule.user_id,
                        duration_ms=duration_ms,
                        next_run_timestamp=schedule.next_run_at)
            
        except Exception as e:
            # Update failure count
            schedule.failed_count += 1
            schedule.updated_at = time.time()
            await self._persist_schedule(schedule)
            
            logger.error("Scheduled snapshot failed",
                        schedule_id=schedule.schedule_id,
                        vm_id=schedule.vm_id,
                        error=str(e),
                        failed_count=schedule.failed_count)
            
            logfire.error("Scheduled snapshot failed",
                         schedule_id=schedule.schedule_id,
                         vm_id=schedule.vm_id,
                         user_id=schedule.user_id,
                         error=str(e),
                         failed_count=schedule.failed_count)
            
            # Disable schedule if too many failures
            if schedule.failed_count >= self.policy.retry_attempts:
                schedule.enabled = False
                await self._persist_schedule(schedule)
                
                logger.warning("Schedule disabled due to repeated failures",
                             schedule_id=schedule.schedule_id,
                             failed_count=schedule.failed_count)
            
            raise
    
    async def _apply_retention_policy(self, schedule: ScheduleConfig):
        """Apply retention policy to clean up old snapshots."""
        try:
            # Get all automated snapshots for this VM and schedule
            all_snapshots = await self.snapshot_manager.list_user_snapshots(schedule.user_id)
            schedule_snapshots = [
                s for s in all_snapshots 
                if (s.vm_id == schedule.vm_id and 
                    s.snapshot_type == "scheduled" and
                    schedule.name in (s.name or ""))
            ]
            
            # Sort by creation time (newest first)
            schedule_snapshots.sort(key=lambda x: x.created_at, reverse=True)
            
            snapshots_to_delete = []
            
            if schedule.retention_policy == RetentionPolicy.COUNT_BASED:
                # Keep only the N most recent snapshots
                if schedule.retention_count and len(schedule_snapshots) > schedule.retention_count:
                    snapshots_to_delete = schedule_snapshots[schedule.retention_count:]
            
            elif schedule.retention_policy == RetentionPolicy.TIME_BASED:
                # Delete snapshots older than N days
                if schedule.retention_days:
                    cutoff_time = time.time() - (schedule.retention_days * 24 * 3600)
                    snapshots_to_delete = [
                        s for s in schedule_snapshots 
                        if s.created_at < cutoff_time
                    ]
            
            elif schedule.retention_policy == RetentionPolicy.HYBRID:
                # Combination of count and time
                if schedule.retention_count and len(schedule_snapshots) > schedule.retention_count:
                    snapshots_to_delete.extend(schedule_snapshots[schedule.retention_count:])
                
                if schedule.retention_days:
                    cutoff_time = time.time() - (schedule.retention_days * 24 * 3600)
                    time_based_deletes = [
                        s for s in schedule_snapshots 
                        if s.created_at < cutoff_time and s not in snapshots_to_delete
                    ]
                    snapshots_to_delete.extend(time_based_deletes)
            
            # Delete old snapshots
            for snapshot in snapshots_to_delete:
                try:
                    await self.snapshot_manager.delete_snapshot(
                        snapshot.snapshot_id, 
                        schedule.user_id
                    )
                    self.metrics['snapshots_cleaned'] += 1
                    
                    logger.debug("Cleaned up old snapshot",
                               snapshot_id=snapshot.snapshot_id,
                               schedule_id=schedule.schedule_id)
                
                except Exception as e:
                    logger.warning("Failed to clean up snapshot",
                                 snapshot_id=snapshot.snapshot_id,
                                 error=str(e))
            
            if snapshots_to_delete:
                logger.info("Applied retention policy",
                           schedule_id=schedule.schedule_id,
                           cleaned_snapshots=len(snapshots_to_delete),
                           retention_policy=schedule.retention_policy.value)
            
        except Exception as e:
            logger.error("Failed to apply retention policy",
                        schedule_id=schedule.schedule_id,
                        error=str(e))
    
    def _calculate_next_run(self, schedule: ScheduleConfig) -> float:
        """Calculate the next run time for a schedule."""
        current_time = time.time()
        current_dt = datetime.fromtimestamp(current_time)
        
        if schedule.schedule_type == ScheduleType.INTERVAL:
            # Simple interval-based scheduling
            interval_seconds = schedule.interval_hours * 3600
            return current_time + interval_seconds
        
        elif schedule.schedule_type == ScheduleType.DAILY:
            # Daily at specific time
            if not schedule.daily_time:
                raise ValueError("Daily schedule requires daily_time")
            
            hour, minute = map(int, schedule.daily_time.split(':'))
            next_run = current_dt.replace(hour=hour, minute=minute, second=0, microsecond=0)
            
            # If time has passed today, schedule for tomorrow
            if next_run <= current_dt:
                next_run += timedelta(days=1)
            
            return next_run.timestamp()
        
        elif schedule.schedule_type == ScheduleType.WEEKLY:
            # Weekly on specific days
            if not schedule.weekly_days:
                raise ValueError("Weekly schedule requires weekly_days")
            
            # Find next occurrence
            days_ahead = None
            current_weekday = current_dt.weekday()
            
            for target_day in sorted(schedule.weekly_days):
                days_until = (target_day - current_weekday) % 7
                if days_until == 0 and current_dt.hour >= 12:  # Default to noon
                    days_until = 7  # Next week
                if days_ahead is None or days_until < days_ahead:
                    days_ahead = days_until
            
            if days_ahead is None:
                days_ahead = 7  # Default to next week
            
            next_run = current_dt + timedelta(days=days_ahead)
            next_run = next_run.replace(hour=12, minute=0, second=0, microsecond=0)
            
            return next_run.timestamp()
        
        elif schedule.schedule_type == ScheduleType.MONTHLY:
            # Monthly on specific day
            if not schedule.monthly_day:
                raise ValueError("Monthly schedule requires monthly_day")
            
            # Try current month first
            try:
                next_run = current_dt.replace(day=schedule.monthly_day, hour=12, minute=0, second=0, microsecond=0)
                if next_run <= current_dt:
                    # Next month
                    if current_dt.month == 12:
                        next_run = next_run.replace(year=current_dt.year + 1, month=1)
                    else:
                        next_run = next_run.replace(month=current_dt.month + 1)
            except ValueError:
                # Day doesn't exist in current month, try next month
                if current_dt.month == 12:
                    next_run = datetime(current_dt.year + 1, 1, min(schedule.monthly_day, 31), 12, 0, 0)
                else:
                    next_run = datetime(current_dt.year, current_dt.month + 1, min(schedule.monthly_day, 31), 12, 0, 0)
            
            return next_run.timestamp()
        
        else:
            raise ValueError(f"Unsupported schedule type: {schedule.schedule_type}")
    
    def _validate_schedule(self, schedule: ScheduleConfig):
        """Validate schedule configuration."""
        if schedule.schedule_type == ScheduleType.INTERVAL:
            if not schedule.interval_hours or schedule.interval_hours < 1:
                raise ValueError("Interval schedules require interval_hours >= 1")
        
        elif schedule.schedule_type == ScheduleType.DAILY:
            if not schedule.daily_time:
                raise ValueError("Daily schedules require daily_time")
            try:
                hour, minute = map(int, schedule.daily_time.split(':'))
                if not (0 <= hour <= 23 and 0 <= minute <= 59):
                    raise ValueError("Invalid time format")
            except ValueError:
                raise ValueError("daily_time must be in HH:MM format")
        
        elif schedule.schedule_type == ScheduleType.WEEKLY:
            if not schedule.weekly_days:
                raise ValueError("Weekly schedules require weekly_days")
            if not all(0 <= day <= 6 for day in schedule.weekly_days):
                raise ValueError("weekly_days must be 0-6 (Monday=0, Sunday=6)")
        
        elif schedule.schedule_type == ScheduleType.MONTHLY:
            if not schedule.monthly_day:
                raise ValueError("Monthly schedules require monthly_day")
            if not (1 <= schedule.monthly_day <= 31):
                raise ValueError("monthly_day must be 1-31")
        
        # Validate retention policy
        if schedule.retention_policy == RetentionPolicy.COUNT_BASED:
            if not schedule.retention_count or schedule.retention_count < 1:
                raise ValueError("Count-based retention requires retention_count >= 1")
        
        elif schedule.retention_policy == RetentionPolicy.TIME_BASED:
            if not schedule.retention_days or schedule.retention_days < 1:
                raise ValueError("Time-based retention requires retention_days >= 1")
        
        elif schedule.retention_policy == RetentionPolicy.HYBRID:
            if ((not schedule.retention_count or schedule.retention_count < 1) and
                (not schedule.retention_days or schedule.retention_days < 1)):
                raise ValueError("Hybrid retention requires either retention_count or retention_days")
    
    async def _health_check_loop(self):
        """Periodic health check and maintenance."""
        while self.is_running:
            try:
                current_time = time.time()
                
                # Update health check metric
                self.metrics['last_health_check'] = current_time
                
                # Log health status
                if self.policy.enable_metrics:
                    metrics = self.get_metrics()
                    logger.debug("Scheduler health check", **metrics)
                    
                    logfire.info("Scheduler health check",
                                active_schedules=metrics['active_schedules'],
                                running_jobs=metrics['running_jobs'],
                                schedules_executed=metrics['schedules_executed'],
                                service="snapshot-scheduler")
                
                # Cleanup orphaned schedules if enabled
                if self.policy.cleanup_orphaned_schedules:
                    await self._cleanup_orphaned_schedules()
                
                # Sleep until next health check
                await asyncio.sleep(self.policy.health_check_interval_minutes * 60)
                
            except Exception as e:
                logger.error("Health check failed", error=str(e))
                await asyncio.sleep(60)  # Retry in 1 minute
    
    async def _cleanup_orphaned_schedules(self):
        """Clean up orphaned or very old schedules."""
        try:
            current_time = time.time()
            max_age_seconds = self.policy.max_schedule_age_days * 24 * 3600
            
            orphaned_schedules = []
            for schedule in self.schedules.values():
                # Mark as orphaned if very old and never run
                if (schedule.created_at < current_time - max_age_seconds and
                    schedule.run_count == 0 and
                    schedule.failed_count > self.policy.retry_attempts):
                    orphaned_schedules.append(schedule.schedule_id)
            
            for schedule_id in orphaned_schedules:
                schedule = self.schedules[schedule_id]
                logger.info("Cleaning up orphaned schedule",
                           schedule_id=schedule_id,
                           age_days=(current_time - schedule.created_at) / (24 * 3600),
                           failed_count=schedule.failed_count)
                
                del self.schedules[schedule_id]
                await self._delete_schedule_from_storage(schedule_id)
            
        except Exception as e:
            logger.error("Failed to cleanup orphaned schedules", error=str(e))
    
    async def _load_schedules(self):
        """Load schedules from persistent storage."""
        # In a real implementation, this would load from database
        # For now, we'll use a simple in-memory approach
        logger.info("Loading schedules from storage")
        pass
    
    async def _persist_schedule(self, schedule: ScheduleConfig):
        """Persist schedule to storage."""
        # In a real implementation, this would save to database
        logger.debug("Persisting schedule", schedule_id=schedule.schedule_id)
        pass
    
    async def _delete_schedule_from_storage(self, schedule_id: str):
        """Delete schedule from storage."""
        # In a real implementation, this would delete from database
        logger.debug("Deleting schedule from storage", schedule_id=schedule_id)
        pass