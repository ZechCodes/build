"""
Optimized Snapshot Manager with Advanced Resource Management

Enhanced snapshot manager that integrates performance monitoring,
resource optimization, and intelligent scheduling for maximum efficiency.
"""

import asyncio
import hashlib
import json
import time
import uuid
from typing import Dict, Any, Optional, List
from dataclasses import dataclass, asdict
from enum import Enum
from pathlib import Path
import structlog
import logfire

# Import base snapshot manager and resource management
import sys
sys.path.insert(0, str(Path(__file__).parent.parent))

from core.snapshot_manager import (
    SnapshotManager, SnapshotMetadata, SnapshotState, SnapshotType
)
from performance.resource_manager import ResourceManager, PerformanceMetrics

logger = structlog.get_logger()


class OptimizationStrategy(Enum):
    """Snapshot optimization strategies."""
    BALANCED = "balanced"        # Balance between speed and compression
    SPEED = "speed"             # Prioritize speed over compression
    COMPRESSION = "compression"  # Prioritize compression over speed
    QUALITY = "quality"         # Maximum quality and integrity


@dataclass
class SnapshotOptimizationConfig:
    """Configuration for snapshot optimization."""
    strategy: OptimizationStrategy
    max_concurrent_snapshots: int
    compression_level: int
    memory_limit_mb: int
    enable_deduplication: bool
    background_optimization: bool


class OptimizedSnapshotManager(SnapshotManager):
    """
    Enhanced snapshot manager with advanced performance optimization.
    
    Features:
    - Intelligent resource management
    - Performance monitoring and optimization
    - Adaptive compression strategies
    - Memory-aware processing
    - Background optimization tasks
    """
    
    def __init__(self, vm_manager, storage_backend, encryption_service, 
                 database, resource_manager: Optional[ResourceManager] = None,
                 optimization_config: Optional[SnapshotOptimizationConfig] = None):
        """
        Initialize optimized snapshot manager.
        
        Args:
            vm_manager: VM management service
            storage_backend: Storage backend for snapshots
            encryption_service: Encryption service
            database: Database connection
            resource_manager: Resource management service
            optimization_config: Optimization configuration
        """
        super().__init__(vm_manager, storage_backend, encryption_service, database)
        
        # Resource management
        self.resource_manager = resource_manager or ResourceManager()
        
        # Optimization configuration
        self.optimization_config = optimization_config or SnapshotOptimizationConfig(
            strategy=OptimizationStrategy.BALANCED,
            max_concurrent_snapshots=3,
            compression_level=6,
            memory_limit_mb=1024,
            enable_deduplication=True,
            background_optimization=True
        )
        
        # Performance tracking
        self.performance_metrics: List[PerformanceMetrics] = []
        self.optimization_queue: asyncio.Queue = asyncio.Queue()
        
        # Adaptive settings based on system performance
        self.adaptive_compression_level = self.optimization_config.compression_level
        self.adaptive_memory_limit = self.optimization_config.memory_limit_mb
        
        # Background task references
        self.background_tasks: List[asyncio.Task] = []
        
    async def initialize(self):
        """Initialize optimized snapshot manager with resource management."""
        try:
            # Initialize base snapshot manager
            await super().initialize()
            
            # Initialize resource manager
            await self.resource_manager.initialize()
            
            # Start background optimization if enabled
            if self.optimization_config.background_optimization:
                optimization_task = asyncio.create_task(
                    self._background_optimization_loop()
                )
                self.background_tasks.append(optimization_task)
                
                # Start performance monitoring
                monitoring_task = asyncio.create_task(
                    self._performance_monitoring_loop()
                )
                self.background_tasks.append(monitoring_task)
            
            logger.info("Optimized snapshot manager initialized",
                       strategy=self.optimization_config.strategy.value,
                       max_concurrent=self.optimization_config.max_concurrent_snapshots,
                       resource_management=True)
            
            logfire.info("Optimized snapshot manager ready",
                        optimization_strategy=self.optimization_config.strategy.value,
                        resource_management_enabled=True)
            
        except Exception as e:
            logger.error("Failed to initialize optimized snapshot manager", error=str(e))
            logfire.error("Optimized snapshot manager initialization failed", error=str(e))
            raise
    
    async def create_snapshot_optimized(self, vm_id: str, user_id: str, name: str,
                                      description: str = "", 
                                      snapshot_type: SnapshotType = SnapshotType.MANUAL,
                                      tags: List[str] = None, encrypt: bool = True,
                                      priority: str = "normal") -> str:
        """
        Create an optimized snapshot with intelligent resource management.
        
        Args:
            vm_id: ID of the VM to snapshot
            user_id: ID of the user creating the snapshot
            name: Human-readable name for the snapshot
            description: Optional description
            snapshot_type: Type of snapshot
            tags: Optional list of tags
            encrypt: Whether to encrypt the snapshot data
            priority: Operation priority (low, normal, high)
            
        Returns:
            str: Unique snapshot ID
        """
        operation_id = f"snapshot_{uuid.uuid4().hex[:8]}"
        
        try:
            # Pre-flight resource check
            vm_data_estimate = await self._estimate_vm_size(vm_id)
            
            # Check resource availability with resource manager
            if not await self.resource_manager.check_resource_availability(
                "snapshot_creation", vm_data_estimate
            ):
                raise RuntimeError("Insufficient system resources for snapshot creation")
            
            # Start resource tracking
            if not await self.resource_manager.start_operation_tracking(
                operation_id, "snapshot_creation", vm_data_estimate
            ):
                raise RuntimeError("Unable to start resource tracking - system overloaded")
            
            logger.info("Starting optimized snapshot creation",
                       operation_id=operation_id,
                       vm_id=vm_id,
                       user_id=user_id,
                       estimated_size_mb=vm_data_estimate // (1024 * 1024),
                       priority=priority)
            
            # Choose optimization strategy based on priority and system state
            optimization_strategy = self._choose_optimization_strategy(priority)
            
            # Create snapshot with optimizations
            start_time = time.time()
            snapshot_id = await self._create_optimized_snapshot_internal(
                vm_id, user_id, name, description, snapshot_type, 
                tags, encrypt, optimization_strategy, operation_id
            )
            duration = time.time() - start_time
            
            # Complete resource tracking
            await self.resource_manager.complete_operation_tracking(
                operation_id, success=True, data_size_bytes=vm_data_estimate
            )
            
            # Record performance metrics
            await self._record_performance_metrics(
                "snapshot_creation", duration, vm_data_estimate, True
            )
            
            # Queue for background optimization if applicable
            if self.optimization_config.background_optimization:
                await self.optimization_queue.put({
                    "action": "optimize_snapshot",
                    "snapshot_id": snapshot_id,
                    "created_at": time.time()
                })
            
            logger.info("Optimized snapshot creation completed",
                       operation_id=operation_id,
                       snapshot_id=snapshot_id,
                       duration_seconds=duration,
                       optimization_strategy=optimization_strategy.value)
            
            logfire.info("Optimized snapshot created",
                        operation_id=operation_id,
                        snapshot_id=snapshot_id,
                        vm_id=vm_id,
                        user_id=user_id,
                        duration_seconds=duration,
                        data_size_mb=vm_data_estimate // (1024 * 1024))
            
            return snapshot_id
            
        except Exception as e:
            # Complete resource tracking with error
            await self.resource_manager.complete_operation_tracking(
                operation_id, success=False, error_message=str(e)
            )
            
            # Record failed performance metrics
            await self._record_performance_metrics(
                "snapshot_creation", 0, 0, False, str(e)
            )
            
            logger.error("Optimized snapshot creation failed",
                        operation_id=operation_id,
                        vm_id=vm_id,
                        error=str(e))
            
            logfire.error("Optimized snapshot creation failed",
                         operation_id=operation_id,
                         vm_id=vm_id,
                         error=str(e))
            raise
    
    async def restore_snapshot_optimized(self, snapshot_id: str, user_id: str,
                                       target_vm_id: Optional[str] = None,
                                       priority: str = "normal") -> str:
        """
        Restore a snapshot with optimization and resource management.
        
        Args:
            snapshot_id: ID of the snapshot to restore
            user_id: ID of the user requesting restore
            target_vm_id: Optional target VM ID (default: original VM)
            priority: Operation priority
            
        Returns:
            str: ID of the restored VM
        """
        operation_id = f"restore_{uuid.uuid4().hex[:8]}"
        
        try:
            # Validate snapshot access
            if snapshot_id not in self.snapshots:
                raise ValueError(f"Snapshot {snapshot_id} not found")
            
            metadata = self.snapshots[snapshot_id]
            if metadata.user_id != user_id:
                raise PermissionError(f"Access denied to snapshot {snapshot_id}")
            
            # Estimate resource requirements
            restore_size_estimate = metadata.size_bytes
            
            # Check resource availability
            if not await self.resource_manager.check_resource_availability(
                "snapshot_restore", restore_size_estimate
            ):
                raise RuntimeError("Insufficient system resources for snapshot restore")
            
            # Start resource tracking
            if not await self.resource_manager.start_operation_tracking(
                operation_id, "snapshot_restore", restore_size_estimate
            ):
                raise RuntimeError("Unable to start resource tracking - system overloaded")
            
            logger.info("Starting optimized snapshot restore",
                       operation_id=operation_id,
                       snapshot_id=snapshot_id,
                       user_id=user_id,
                       target_vm_id=target_vm_id,
                       priority=priority)
            
            # Choose optimization strategy
            optimization_strategy = self._choose_optimization_strategy(priority)
            
            # Perform optimized restore
            start_time = time.time()
            restored_vm_id = await self._restore_optimized_snapshot_internal(
                snapshot_id, user_id, target_vm_id, optimization_strategy, operation_id
            )
            duration = time.time() - start_time
            
            # Complete resource tracking
            await self.resource_manager.complete_operation_tracking(
                operation_id, success=True, data_size_bytes=restore_size_estimate
            )
            
            # Record performance metrics
            await self._record_performance_metrics(
                "snapshot_restore", duration, restore_size_estimate, True
            )
            
            logger.info("Optimized snapshot restore completed",
                       operation_id=operation_id,
                       snapshot_id=snapshot_id,
                       restored_vm_id=restored_vm_id,
                       duration_seconds=duration)
            
            logfire.info("Optimized snapshot restored",
                        operation_id=operation_id,
                        snapshot_id=snapshot_id,
                        restored_vm_id=restored_vm_id,
                        duration_seconds=duration,
                        data_size_mb=restore_size_estimate // (1024 * 1024))
            
            return restored_vm_id
            
        except Exception as e:
            # Complete resource tracking with error
            await self.resource_manager.complete_operation_tracking(
                operation_id, success=False, error_message=str(e)
            )
            
            # Record failed performance metrics
            await self._record_performance_metrics(
                "snapshot_restore", 0, 0, False, str(e)
            )
            
            logger.error("Optimized snapshot restore failed",
                        operation_id=operation_id,
                        snapshot_id=snapshot_id,
                        error=str(e))
            
            logfire.error("Optimized snapshot restore failed",
                         operation_id=operation_id,
                         snapshot_id=snapshot_id,
                         error=str(e))
            raise
    
    def get_optimization_report(self) -> Dict[str, Any]:
        """Generate comprehensive optimization and performance report."""
        try:
            # Get resource manager performance report
            resource_report = self.resource_manager.get_performance_report()
            
            # Calculate snapshot-specific metrics
            snapshot_metrics = self._calculate_snapshot_metrics()
            
            # Get adaptive settings
            adaptive_settings = {
                "current_compression_level": self.adaptive_compression_level,
                "current_memory_limit_mb": self.adaptive_memory_limit,
                "optimization_strategy": self.optimization_config.strategy.value,
                "background_optimization_enabled": self.optimization_config.background_optimization
            }
            
            # Combine reports
            optimization_report = {
                "timestamp": time.time(),
                "resource_management": resource_report,
                "snapshot_performance": snapshot_metrics,
                "adaptive_settings": adaptive_settings,
                "optimization_config": {
                    "strategy": self.optimization_config.strategy.value,
                    "max_concurrent_snapshots": self.optimization_config.max_concurrent_snapshots,
                    "compression_level": self.optimization_config.compression_level,
                    "memory_limit_mb": self.optimization_config.memory_limit_mb,
                    "enable_deduplication": self.optimization_config.enable_deduplication
                },
                "background_tasks": {
                    "active_tasks": len([t for t in self.background_tasks if not t.done()]),
                    "completed_tasks": len([t for t in self.background_tasks if t.done()]),
                    "optimization_queue_size": self.optimization_queue.qsize()
                }
            }
            
            return optimization_report
            
        except Exception as e:
            logger.error("Failed to generate optimization report", error=str(e))
            return {"error": f"Failed to generate report: {e}"}
    
    async def shutdown(self):
        """Gracefully shutdown optimized snapshot manager."""
        try:
            logger.info("Shutting down optimized snapshot manager")
            
            # Cancel background tasks
            for task in self.background_tasks:
                if not task.done():
                    task.cancel()
                    try:
                        await task
                    except asyncio.CancelledError:
                        pass
            
            # Clear optimization queue
            while not self.optimization_queue.empty():
                try:
                    self.optimization_queue.get_nowait()
                    # Only call task_done() if there are pending tasks
                    # In practice, this should match get() calls from background workers
                except asyncio.QueueEmpty:
                    break
            
            logger.info("Optimized snapshot manager shutdown completed")
            
        except Exception as e:
            logger.error("Error during optimized snapshot manager shutdown", error=str(e))
    
    # Private implementation methods
    
    async def _estimate_vm_size(self, vm_id: str) -> int:
        """Estimate VM size for resource planning."""
        try:
            # Try to get VM configuration to estimate size
            vm = await self.vm_manager.get_vm(vm_id)
            if vm and hasattr(vm, 'get_config'):
                config = vm.get_config()
                memory_mb = config.get('memory', 512)
                # Estimate: memory + 50% overhead for disk state
                estimated_bytes = int(memory_mb * 1024 * 1024 * 1.5)
                return estimated_bytes
            
            # Default estimate: 1GB
            return 1024 * 1024 * 1024
            
        except Exception as e:
            logger.warning("Failed to estimate VM size", vm_id=vm_id, error=str(e))
            return 1024 * 1024 * 1024  # 1GB default
    
    def _choose_optimization_strategy(self, priority: str) -> OptimizationStrategy:
        """Choose optimization strategy based on priority and system state."""
        # Get current system performance
        try:
            # Direct strategy selection for explicit strategy names
            if priority == "speed":
                return OptimizationStrategy.SPEED
            if priority == "compression":
                return OptimizationStrategy.COMPRESSION
            if priority == "balanced":
                return OptimizationStrategy.BALANCED
            
            # High priority operations prefer speed
            if priority == "high":
                return OptimizationStrategy.SPEED
            
            # Low priority operations can use maximum compression
            if priority == "low":
                return OptimizationStrategy.COMPRESSION
            
            # Normal priority uses configured strategy or adaptive choice
            if self.optimization_config.strategy != OptimizationStrategy.BALANCED:
                return self.optimization_config.strategy
            
            # For balanced strategy, adapt based on system performance
            # This is a simplified adaptive algorithm - could be enhanced
            recent_metrics = self.performance_metrics[-10:] if self.performance_metrics else []
            
            if recent_metrics:
                avg_duration = sum(m.duration_seconds for m in recent_metrics) / len(recent_metrics)
                avg_throughput = sum(m.throughput_mbps for m in recent_metrics) / len(recent_metrics)
                
                # If recent operations were slow, prioritize speed
                if avg_duration > 60 or avg_throughput < 10:
                    return OptimizationStrategy.SPEED
                
                # If system is performing well, prioritize compression
                if avg_duration < 30 and avg_throughput > 50:
                    return OptimizationStrategy.COMPRESSION
            
            return OptimizationStrategy.BALANCED
            
        except Exception as e:
            logger.warning("Failed to choose optimization strategy", error=str(e))
            return OptimizationStrategy.BALANCED
    
    async def _create_optimized_snapshot_internal(self, vm_id: str, user_id: str, 
                                                name: str, description: str,
                                                snapshot_type: SnapshotType,
                                                tags: List[str], encrypt: bool,
                                                optimization_strategy: OptimizationStrategy,
                                                operation_id: str) -> str:
        """Internal optimized snapshot creation logic."""
        try:
            # Adjust settings based on optimization strategy
            original_compression = self.compression_enabled
            
            if optimization_strategy == OptimizationStrategy.SPEED:
                self.compression_enabled = False
            elif optimization_strategy == OptimizationStrategy.COMPRESSION:
                self.compression_enabled = True
                # Could increase compression level here
            
            # Use the base snapshot creation with optimizations
            snapshot_id = await super().create_snapshot(
                vm_id, user_id, name, description, snapshot_type, tags, encrypt
            )
            
            # Restore original settings
            self.compression_enabled = original_compression
            
            return snapshot_id
            
        except Exception as e:
            logger.error("Optimized snapshot creation internal error", 
                        operation_id=operation_id, error=str(e))
            raise
    
    async def _restore_optimized_snapshot_internal(self, snapshot_id: str, user_id: str,
                                                 target_vm_id: Optional[str],
                                                 optimization_strategy: OptimizationStrategy,
                                                 operation_id: str) -> str:
        """Internal optimized snapshot restore logic."""
        try:
            # Use the base snapshot restore with optimizations
            # The target_vm_id logic would need to be implemented in base class
            # For now, restore to original VM
            metadata = self.snapshots[snapshot_id]
            restored_vm_id = await super().restore_snapshot(snapshot_id, user_id)
            
            return restored_vm_id
            
        except Exception as e:
            logger.error("Optimized snapshot restore internal error",
                        operation_id=operation_id, error=str(e))
            raise
    
    async def _record_performance_metrics(self, operation_type: str, duration: float,
                                        data_size: int, success: bool,
                                        error_message: Optional[str] = None):
        """Record performance metrics for analysis."""
        try:
            throughput_mbps = 0
            if duration > 0 and data_size > 0:
                throughput_mbps = (data_size / (1024 * 1024)) / duration
            
            metrics = PerformanceMetrics(
                operation_type=operation_type,
                duration_seconds=duration,
                data_size_bytes=data_size,
                throughput_mbps=throughput_mbps,
                cpu_usage_during=0,  # Would need to track during operation
                memory_peak_mb=0,    # Would need to track during operation
                success=success,
                error_message=error_message
            )
            
            self.performance_metrics.append(metrics)
            
            # Keep only last 1000 metrics
            if len(self.performance_metrics) > 1000:
                self.performance_metrics = self.performance_metrics[-1000:]
                
        except Exception as e:
            logger.warning("Failed to record performance metrics", error=str(e))
    
    def _calculate_snapshot_metrics(self) -> Dict[str, Any]:
        """Calculate snapshot-specific performance metrics."""
        try:
            if not self.performance_metrics:
                return {"no_data": True}
            
            # Separate by operation type
            creation_metrics = [m for m in self.performance_metrics if m.operation_type == "snapshot_creation"]
            restore_metrics = [m for m in self.performance_metrics if m.operation_type == "snapshot_restore"]
            
            def calculate_stats(metrics_list):
                if not metrics_list:
                    return {"count": 0}
                
                successful = [m for m in metrics_list if m.success]
                return {
                    "count": len(metrics_list),
                    "success_count": len(successful),
                    "success_rate": len(successful) / len(metrics_list),
                    "avg_duration_seconds": sum(m.duration_seconds for m in successful) / len(successful) if successful else 0,
                    "avg_throughput_mbps": sum(m.throughput_mbps for m in successful) / len(successful) if successful else 0,
                    "max_duration_seconds": max(m.duration_seconds for m in metrics_list),
                    "min_duration_seconds": min(m.duration_seconds for m in metrics_list)
                }
            
            return {
                "snapshot_creation": calculate_stats(creation_metrics),
                "snapshot_restore": calculate_stats(restore_metrics),
                "total_operations": len(self.performance_metrics)
            }
            
        except Exception as e:
            logger.error("Failed to calculate snapshot metrics", error=str(e))
            return {"error": str(e)}
    
    async def _background_optimization_loop(self):
        """Background loop for optimization tasks."""
        try:
            logger.info("Starting background optimization loop")
            
            while True:
                try:
                    # Wait for optimization tasks
                    optimization_task = await asyncio.wait_for(
                        self.optimization_queue.get(), timeout=300  # 5 minutes
                    )
                    
                    # Process optimization task
                    await self._process_optimization_task(optimization_task)
                    
                except asyncio.TimeoutError:
                    # Periodic maintenance tasks
                    await self._perform_periodic_optimization()
                    
                except Exception as e:
                    logger.error("Background optimization task failed", error=str(e))
                    await asyncio.sleep(10)  # Wait before retrying
                    
        except asyncio.CancelledError:
            logger.info("Background optimization loop cancelled")
        except Exception as e:
            logger.error("Background optimization loop failed", error=str(e))
    
    async def _performance_monitoring_loop(self):
        """Background loop for performance monitoring and adaptation."""
        try:
            logger.info("Starting performance monitoring loop")
            
            while True:
                await asyncio.sleep(300)  # Check every 5 minutes
                
                try:
                    # Analyze recent performance and adjust settings
                    await self._adapt_settings_based_on_performance()
                    
                except Exception as e:
                    logger.error("Performance monitoring failed", error=str(e))
                    
        except asyncio.CancelledError:
            logger.info("Performance monitoring loop cancelled")
        except Exception as e:
            logger.error("Performance monitoring loop failed", error=str(e))
    
    async def _process_optimization_task(self, task: Dict[str, Any]):
        """Process a background optimization task."""
        try:
            action = task.get("action")
            
            if action == "optimize_snapshot":
                snapshot_id = task.get("snapshot_id")
                if snapshot_id in self.snapshots:
                    # Could implement post-creation optimization here
                    # e.g., better compression, deduplication, etc.
                    logger.debug("Processing snapshot optimization", snapshot_id=snapshot_id)
                    
        except Exception as e:
            logger.error("Failed to process optimization task", task=task, error=str(e))
    
    async def _perform_periodic_optimization(self):
        """Perform periodic optimization tasks."""
        try:
            # Trigger resource manager optimization
            await self.resource_manager.optimize_memory_usage()
            await self.resource_manager.cleanup_temp_files()
            
            logger.debug("Periodic optimization completed")
            
        except Exception as e:
            logger.error("Periodic optimization failed", error=str(e))
    
    async def _adapt_settings_based_on_performance(self):
        """Adapt settings based on recent performance metrics."""
        try:
            if len(self.performance_metrics) < 10:
                return  # Not enough data
            
            recent_metrics = self.performance_metrics[-20:]  # Last 20 operations
            
            # Calculate average performance
            successful_metrics = [m for m in recent_metrics if m.success]
            if not successful_metrics:
                return
            
            avg_duration = sum(m.duration_seconds for m in successful_metrics) / len(successful_metrics)
            avg_throughput = sum(m.throughput_mbps for m in successful_metrics) / len(successful_metrics)
            
            # Adapt compression level based on performance
            if avg_duration > 120:  # Operations taking too long
                if self.adaptive_compression_level > 1:
                    self.adaptive_compression_level -= 1
                    logger.info("Reduced compression level for better performance",
                               new_level=self.adaptive_compression_level)
            elif avg_duration < 30 and avg_throughput > 50:  # Good performance
                if self.adaptive_compression_level < 9:
                    self.adaptive_compression_level += 1
                    logger.info("Increased compression level for better storage efficiency",
                               new_level=self.adaptive_compression_level)
            
        except Exception as e:
            logger.error("Failed to adapt settings based on performance", error=str(e))