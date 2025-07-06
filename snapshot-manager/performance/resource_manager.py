"""
Advanced Resource Management and Performance Optimization

Provides intelligent resource cleanup, memory optimization, and performance
monitoring for the VM snapshot system.
"""

import asyncio
import psutil
import time
import gc
from typing import Dict, Any, Optional, List, Tuple
from dataclasses import dataclass, asdict
from pathlib import Path
import structlog
import logfire

logger = structlog.get_logger()


@dataclass
class ResourceUsage:
    """System resource usage metrics."""
    cpu_percent: float
    memory_percent: float
    memory_available_mb: int
    disk_usage_percent: float
    disk_available_gb: float
    network_io_bytes: int
    timestamp: float


@dataclass
class PerformanceMetrics:
    """Performance metrics for snapshot operations."""
    operation_type: str
    duration_seconds: float
    data_size_bytes: int
    throughput_mbps: float
    cpu_usage_during: float
    memory_peak_mb: int
    success: bool
    error_message: Optional[str] = None


class ResourceManager:
    """
    Advanced resource management for VM snapshot operations.
    
    Provides:
    - Memory usage optimization
    - Automated cleanup processes
    - Performance monitoring
    - Resource quota enforcement
    - System health checks
    """
    
    def __init__(self, config: Optional[Dict[str, Any]] = None):
        """Initialize resource manager with configuration."""
        self.config = config or {}
        
        # Resource limits and thresholds
        self.max_memory_percent = self.config.get("max_memory_percent", 80)
        self.max_disk_percent = self.config.get("max_disk_percent", 85)
        self.max_concurrent_operations = self.config.get("max_concurrent_operations", 5)
        self.cleanup_interval_seconds = self.config.get("cleanup_interval", 300)  # 5 minutes
        
        # Performance tracking
        self.operation_metrics: List[PerformanceMetrics] = []
        self.resource_history: List[ResourceUsage] = []
        self.active_operations: Dict[str, Dict[str, Any]] = {}
        
        # Cleanup tracking
        self.last_cleanup = 0
        self.temp_files: set = set()
        self.cache_entries: Dict[str, Tuple[Any, float]] = {}  # value, timestamp
        
        # Performance optimization settings
        self.enable_compression = self.config.get("enable_compression", True)
        self.cache_max_size_mb = self.config.get("cache_max_size_mb", 100)
        self.background_cleanup_enabled = self.config.get("background_cleanup", True)
        
    async def initialize(self):
        """Initialize resource manager and start background tasks."""
        try:
            # Validate system resources
            await self._validate_system_resources()
            
            # Start background cleanup if enabled
            if self.background_cleanup_enabled:
                asyncio.create_task(self._background_cleanup_loop())
            
            # Start resource monitoring
            asyncio.create_task(self._resource_monitoring_loop())
            
            # Start backup encryption verification service
            asyncio.create_task(self._start_backup_verification())
            
            logger.info("Resource manager initialized",
                       max_memory_percent=self.max_memory_percent,
                       max_disk_percent=self.max_disk_percent,
                       max_concurrent_ops=self.max_concurrent_operations)
            
            logfire.info("Resource manager started",
                        config=self.config)
            
        except Exception as e:
            logger.error("Failed to initialize resource manager", error=str(e))
            logfire.error("Resource manager initialization failed", error=str(e))
            raise
    
    async def check_resource_availability(self, operation_type: str, 
                                        estimated_size_bytes: int = 0) -> bool:
        """
        Check if system has sufficient resources for operation.
        
        Args:
            operation_type: Type of operation (snapshot, restore, etc.)
            estimated_size_bytes: Estimated data size for the operation
            
        Returns:
            bool: True if resources are available
        """
        try:
            # Get current resource usage
            usage = await self._get_current_resource_usage()
            
            # Check memory availability
            estimated_memory_mb = estimated_size_bytes // (1024 * 1024) * 2  # Estimate 2x for processing
            if usage.memory_percent > self.max_memory_percent:
                logger.warning("Memory usage too high for operation",
                             current_percent=usage.memory_percent,
                             max_percent=self.max_memory_percent,
                             operation=operation_type)
                return False
            
            if estimated_memory_mb > usage.memory_available_mb:
                logger.warning("Insufficient memory for operation",
                             required_mb=estimated_memory_mb,
                             available_mb=usage.memory_available_mb,
                             operation=operation_type)
                return False
            
            # Check disk space
            if usage.disk_usage_percent > self.max_disk_percent:
                logger.warning("Disk usage too high for operation",
                             current_percent=usage.disk_usage_percent,
                             max_percent=self.max_disk_percent,
                             operation=operation_type)
                return False
            
            # Check concurrent operations limit
            if len(self.active_operations) >= self.max_concurrent_operations:
                logger.warning("Too many concurrent operations",
                             active_count=len(self.active_operations),
                             max_concurrent=self.max_concurrent_operations,
                             operation=operation_type)
                return False
            
            return True
            
        except Exception as e:
            logger.error("Failed to check resource availability", error=str(e))
            return False
    
    async def start_operation_tracking(self, operation_id: str, 
                                     operation_type: str, 
                                     estimated_size_bytes: int = 0) -> bool:
        """
        Start tracking a resource-intensive operation.
        
        Args:
            operation_id: Unique identifier for the operation
            operation_type: Type of operation
            estimated_size_bytes: Estimated data size
            
        Returns:
            bool: True if operation can proceed
        """
        try:
            # Check resource availability
            if not await self.check_resource_availability(operation_type, estimated_size_bytes):
                return False
            
            # Register operation
            self.active_operations[operation_id] = {
                "type": operation_type,
                "start_time": time.time(),
                "estimated_size": estimated_size_bytes,
                "start_cpu": psutil.cpu_percent(),
                "start_memory": psutil.virtual_memory().percent
            }
            
            logger.info("Operation tracking started",
                       operation_id=operation_id,
                       operation_type=operation_type,
                       estimated_size_mb=estimated_size_bytes // (1024 * 1024))
            
            return True
            
        except Exception as e:
            logger.error("Failed to start operation tracking", 
                        operation_id=operation_id, error=str(e))
            return False
    
    async def complete_operation_tracking(self, operation_id: str, 
                                        success: bool, data_size_bytes: int = 0,
                                        error_message: Optional[str] = None):
        """Complete tracking for an operation and record metrics."""
        try:
            if operation_id not in self.active_operations:
                logger.warning("Operation not found for completion tracking",
                             operation_id=operation_id)
                return
            
            operation = self.active_operations[operation_id]
            end_time = time.time()
            duration = end_time - operation["start_time"]
            
            # Calculate performance metrics
            throughput_mbps = 0
            if duration > 0 and data_size_bytes > 0:
                throughput_mbps = (data_size_bytes / (1024 * 1024)) / duration
            
            current_memory = psutil.virtual_memory()
            memory_peak_mb = int(current_memory.used / (1024 * 1024))
            
            # Create performance metrics
            metrics = PerformanceMetrics(
                operation_type=operation["type"],
                duration_seconds=duration,
                data_size_bytes=data_size_bytes,
                throughput_mbps=throughput_mbps,
                cpu_usage_during=psutil.cpu_percent() - operation["start_cpu"],
                memory_peak_mb=memory_peak_mb,
                success=success,
                error_message=error_message
            )
            
            # Store metrics
            self.operation_metrics.append(metrics)
            
            # Clean up old metrics (keep last 1000)
            if len(self.operation_metrics) > 1000:
                self.operation_metrics = self.operation_metrics[-1000:]
            
            # Remove from active operations
            del self.active_operations[operation_id]
            
            logger.info("Operation tracking completed",
                       operation_id=operation_id,
                       duration_seconds=duration,
                       throughput_mbps=throughput_mbps,
                       success=success)
            
            logfire.info("Operation performance recorded",
                        operation_id=operation_id,
                        operation_type=operation["type"],
                        duration_seconds=duration,
                        data_size_mb=data_size_bytes // (1024 * 1024),
                        throughput_mbps=throughput_mbps,
                        success=success)
            
        except Exception as e:
            logger.error("Failed to complete operation tracking",
                        operation_id=operation_id, error=str(e))
    
    async def optimize_memory_usage(self):
        """Optimize memory usage by clearing caches and running garbage collection."""
        try:
            initial_memory = psutil.virtual_memory().percent
            
            # Clear expired cache entries
            current_time = time.time()
            cache_ttl = 3600  # 1 hour TTL
            
            expired_keys = [
                key for key, (value, timestamp) in self.cache_entries.items()
                if current_time - timestamp > cache_ttl
            ]
            
            for key in expired_keys:
                del self.cache_entries[key]
            
            # Enforce cache size limit
            cache_size_mb = sum(
                len(str(value)) for value, _ in self.cache_entries.values()
            ) / (1024 * 1024)
            
            if cache_size_mb > self.cache_max_size_mb:
                # Remove oldest entries
                sorted_entries = sorted(
                    self.cache_entries.items(),
                    key=lambda x: x[1][1]  # Sort by timestamp
                )
                
                # Remove oldest 25% of entries
                remove_count = len(sorted_entries) // 4
                for key, _ in sorted_entries[:remove_count]:
                    del self.cache_entries[key]
            
            # Force garbage collection
            collected = gc.collect()
            
            final_memory = psutil.virtual_memory().percent
            memory_freed = initial_memory - final_memory
            
            logger.info("Memory optimization completed",
                       initial_memory_percent=initial_memory,
                       final_memory_percent=final_memory,
                       memory_freed_percent=memory_freed,
                       cache_entries_removed=len(expired_keys),
                       gc_objects_collected=collected)
            
            logfire.info("Memory optimization performed",
                        memory_freed_percent=memory_freed,
                        cache_size_mb=cache_size_mb,
                        gc_collected=collected)
            
        except Exception as e:
            logger.error("Memory optimization failed", error=str(e))
    
    async def cleanup_temp_files(self):
        """Clean up temporary files and directories."""
        try:
            cleaned_count = 0
            freed_bytes = 0
            
            # Clean up tracked temporary files
            for temp_file in list(self.temp_files):
                try:
                    file_path = Path(temp_file)
                    if file_path.exists():
                        file_size = file_path.stat().st_size
                        file_path.unlink()
                        freed_bytes += file_size
                        cleaned_count += 1
                    
                    self.temp_files.discard(temp_file)
                    
                except Exception as e:
                    logger.warning("Failed to clean temp file", 
                                 file=temp_file, error=str(e))
            
            # Clean up old snapshot temp files
            temp_dirs = ["/tmp", "/var/tmp"]
            for temp_dir in temp_dirs:
                temp_path = Path(temp_dir)
                if temp_path.exists():
                    # Find old snapshot files
                    for pattern in ["snap_*", "*snapshot*", "*firecracker*"]:
                        for old_file in temp_path.glob(pattern):
                            try:
                                # Remove files older than 1 hour
                                if time.time() - old_file.stat().st_mtime > 3600:
                                    file_size = old_file.stat().st_size
                                    old_file.unlink()
                                    freed_bytes += file_size
                                    cleaned_count += 1
                            except Exception as e:
                                logger.warning("Failed to clean old temp file",
                                             file=str(old_file), error=str(e))
            
            logger.info("Temporary file cleanup completed",
                       files_cleaned=cleaned_count,
                       bytes_freed=freed_bytes,
                       mb_freed=freed_bytes // (1024 * 1024))
            
        except Exception as e:
            logger.error("Temporary file cleanup failed", error=str(e))
    
    def register_temp_file(self, file_path: str):
        """Register a temporary file for cleanup."""
        self.temp_files.add(file_path)
    
    def cache_set(self, key: str, value: Any, ttl_seconds: int = 3600):
        """Set a value in the cache with TTL."""
        self.cache_entries[key] = (value, time.time())
    
    def cache_get(self, key: str) -> Optional[Any]:
        """Get a value from the cache if not expired."""
        if key in self.cache_entries:
            value, timestamp = self.cache_entries[key]
            if time.time() - timestamp < 3600:  # 1 hour TTL
                return value
            else:
                del self.cache_entries[key]
        return None
    
    def get_performance_report(self) -> Dict[str, Any]:
        """Generate comprehensive performance report."""
        try:
            if not self.operation_metrics:
                return {"error": "No performance data available"}
            
            # Calculate statistics
            successful_ops = [m for m in self.operation_metrics if m.success]
            failed_ops = [m for m in self.operation_metrics if not m.success]
            
            avg_duration = sum(m.duration_seconds for m in successful_ops) / len(successful_ops) if successful_ops else 0
            avg_throughput = sum(m.throughput_mbps for m in successful_ops) / len(successful_ops) if successful_ops else 0
            
            # Group by operation type
            ops_by_type = {}
            for metric in self.operation_metrics:
                op_type = metric.operation_type
                if op_type not in ops_by_type:
                    ops_by_type[op_type] = []
                ops_by_type[op_type].append(metric)
            
            type_stats = {}
            for op_type, metrics in ops_by_type.items():
                successful = [m for m in metrics if m.success]
                type_stats[op_type] = {
                    "total_operations": len(metrics),
                    "successful_operations": len(successful),
                    "success_rate": len(successful) / len(metrics) if metrics else 0,
                    "avg_duration_seconds": sum(m.duration_seconds for m in successful) / len(successful) if successful else 0,
                    "avg_throughput_mbps": sum(m.throughput_mbps for m in successful) / len(successful) if successful else 0
                }
            
            # Current system status
            current_usage = psutil.virtual_memory()
            disk_usage = psutil.disk_usage('/')
            
            return {
                "performance_summary": {
                    "total_operations": len(self.operation_metrics),
                    "successful_operations": len(successful_ops),
                    "failed_operations": len(failed_ops),
                    "success_rate": len(successful_ops) / len(self.operation_metrics),
                    "average_duration_seconds": avg_duration,
                    "average_throughput_mbps": avg_throughput
                },
                "operations_by_type": type_stats,
                "current_system_status": {
                    "memory_usage_percent": current_usage.percent,
                    "memory_available_mb": current_usage.available // (1024 * 1024),
                    "disk_usage_percent": (disk_usage.used / disk_usage.total) * 100,
                    "disk_available_gb": disk_usage.free // (1024 * 1024 * 1024),
                    "active_operations": len(self.active_operations),
                    "cache_entries": len(self.cache_entries)
                },
                "resource_limits": {
                    "max_memory_percent": self.max_memory_percent,
                    "max_disk_percent": self.max_disk_percent,
                    "max_concurrent_operations": self.max_concurrent_operations,
                    "cache_max_size_mb": self.cache_max_size_mb
                }
            }
            
        except Exception as e:
            logger.error("Failed to generate performance report", error=str(e))
            return {"error": f"Failed to generate report: {e}"}
    
    # Private methods
    
    async def _get_current_resource_usage(self) -> ResourceUsage:
        """Get current system resource usage."""
        cpu_percent = psutil.cpu_percent(interval=0.1)
        memory = psutil.virtual_memory()
        disk = psutil.disk_usage('/')
        network = psutil.net_io_counters()
        
        return ResourceUsage(
            cpu_percent=cpu_percent,
            memory_percent=memory.percent,
            memory_available_mb=memory.available // (1024 * 1024),
            disk_usage_percent=(disk.used / disk.total) * 100,
            disk_available_gb=disk.free // (1024 * 1024 * 1024),
            network_io_bytes=network.bytes_sent + network.bytes_recv,
            timestamp=time.time()
        )
    
    async def _validate_system_resources(self):
        """Validate that system has minimum required resources."""
        memory = psutil.virtual_memory()
        disk = psutil.disk_usage('/')
        
        # Minimum requirements
        min_memory_gb = 2
        min_disk_gb = 10
        
        if memory.total < min_memory_gb * 1024 * 1024 * 1024:
            raise RuntimeError(f"Insufficient memory: {memory.total // (1024**3)}GB < {min_memory_gb}GB required")
        
        if disk.free < min_disk_gb * 1024 * 1024 * 1024:
            raise RuntimeError(f"Insufficient disk space: {disk.free // (1024**3)}GB < {min_disk_gb}GB required")
        
        logger.info("System resource validation passed",
                   total_memory_gb=memory.total // (1024**3),
                   available_disk_gb=disk.free // (1024**3))
    
    async def _background_cleanup_loop(self):
        """Background loop for resource cleanup."""
        try:
            while True:
                await asyncio.sleep(self.cleanup_interval_seconds)
                
                try:
                    # Perform cleanup operations
                    await self.optimize_memory_usage()
                    await self.cleanup_temp_files()
                    
                    self.last_cleanup = time.time()
                    
                except Exception as e:
                    logger.error("Background cleanup failed", error=str(e))
                    
        except asyncio.CancelledError:
            logger.info("Background cleanup loop cancelled")
        except Exception as e:
            logger.error("Background cleanup loop failed", error=str(e))
    
    async def _resource_monitoring_loop(self):
        """Background loop for resource monitoring."""
        try:
            while True:
                await asyncio.sleep(60)  # Monitor every minute
                
                try:
                    usage = await self._get_current_resource_usage()
                    self.resource_history.append(usage)
                    
                    # Keep only last 24 hours of data (1440 minutes)
                    if len(self.resource_history) > 1440:
                        self.resource_history = self.resource_history[-1440:]
                    
                    # Alert on high resource usage
                    if usage.memory_percent > 90:
                        logger.warning("High memory usage detected",
                                     memory_percent=usage.memory_percent)
                        logfire.warning("High memory usage",
                                      memory_percent=usage.memory_percent)
                    
                    if usage.disk_usage_percent > 90:
                        logger.warning("High disk usage detected",
                                     disk_percent=usage.disk_usage_percent)
                        logfire.warning("High disk usage",
                                      disk_percent=usage.disk_usage_percent)
                    
                except Exception as e:
                    logger.error("Resource monitoring failed", error=str(e))
                    
        except asyncio.CancelledError:
            logger.info("Resource monitoring loop cancelled")
        except Exception as e:
            logger.error("Resource monitoring loop failed", error=str(e))
    
    async def _start_backup_verification(self):
        """Start automated backup encryption verification service."""
        try:
            # Import backup encryption verifier
            import sys
            from pathlib import Path
            
            # Add current directory to path for imports
            current_dir = Path(__file__).parent.parent
            if str(current_dir) not in sys.path:
                sys.path.insert(0, str(current_dir))
            
            from security.backup_encryption_verifier import BackupEncryptionVerifier
            
            # Initialize verification service with configuration
            verification_config = {
                "interval_hours": 24,  # Daily verification
                "sample_percentage": 5.0,  # 5% of snapshots
                "max_concurrent": 2,  # 2 concurrent verifications
                "timeout_seconds": 300,  # 5 minute timeout
                "enable_alerting": True,
                "min_sample_size": 1,
                "max_sample_size": 10
            }
            
            # Create verifier instance (storage and encryption services would be injected in production)
            self.backup_verifier = BackupEncryptionVerifier(
                storage_backend=None,  # Would be actual storage backend
                encryption_service=None,  # Would be actual encryption service
                verification_config=verification_config
            )
            
            # Start background verification service
            await self.backup_verifier.start_background_verification()
            
            logger.info("Backup encryption verification service started",
                       interval_hours=verification_config["interval_hours"],
                       sample_percentage=verification_config["sample_percentage"])
            
            logfire.info("Backup encryption verification service initialized",
                        config=verification_config)
            
        except Exception as e:
            logger.warning("Failed to start backup encryption verification",
                         error=str(e))
            # Don't fail resource manager initialization if backup verification fails
            logfire.warning("Backup encryption verification service failed to start",
                          error=str(e))