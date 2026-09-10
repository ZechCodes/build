# Session 10.4: Storage Optimization & Management

## Objective
Implement comprehensive storage optimization system for terminal recordings, providing efficient storage management, compression strategies, deduplication, and intelligent lifecycle management to minimize storage costs while maintaining fast access.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for storage performance monitoring and optimization analytics
- **Session 7**: Integrates with existing storage backend infrastructure and MinIO
- **Session 10.1**: Optimizes storage for recordings created by the recording engine
- **Session 10.2**: Manages storage for privacy-filtered recording content
- **Session 10.3**: Provides optimized storage access for playback system

## Core Implementation

### Storage Optimization Engine
**Location**: `recording-manager/storage/optimization_engine.py`

```python
# recording-manager/storage/optimization_engine.py
import asyncio
import hashlib
import time
import gzip
import zstd
import lz4.frame
import json
from typing import Dict, Any, Optional, List, Set, Tuple
from dataclasses import dataclass, asdict
from enum import Enum
from pathlib import Path
import structlog
import logfire
from concurrent.futures import ThreadPoolExecutor, ProcessPoolExecutor

logger = structlog.get_logger()

class CompressionType(Enum):
    NONE = "none"
    GZIP = "gzip"
    ZSTD = "zstd"
    LZ4 = "lz4"
    BROTLI = "brotli"

class StorageTier(Enum):
    HOT = "hot"      # Frequently accessed, fast SSD
    WARM = "warm"    # Occasionally accessed, standard storage
    COLD = "cold"    # Rarely accessed, cheap storage
    ARCHIVE = "archive"  # Long-term archive, slowest/cheapest

class OptimizationStrategy(Enum):
    SIZE_OPTIMAL = "size_optimal"
    SPEED_OPTIMAL = "speed_optimal"
    BALANCED = "balanced"
    COST_OPTIMAL = "cost_optimal"

@dataclass
class StorageMetrics:
    recording_id: str
    original_size: int
    compressed_size: int
    compression_ratio: float
    compression_type: CompressionType
    storage_tier: StorageTier
    access_count: int
    last_accessed: float
    created_at: float
    checksum: str
    deduplication_group: Optional[str]
    chunks_count: int
    average_chunk_size: int

@dataclass
class OptimizationJob:
    id: str
    recording_id: str
    current_compression: CompressionType
    target_compression: CompressionType
    current_tier: StorageTier
    target_tier: StorageTier
    priority: int
    estimated_savings: int
    created_at: float
    started_at: Optional[float]
    completed_at: Optional[float]
    status: str
    error_message: Optional[str]

@dataclass
class DeduplicationCandidate:
    recording_id: str
    content_hash: str
    size: int
    similar_recordings: List[str]
    potential_savings: int
    similarity_score: float

class RecordingStorageOptimizer:
    def __init__(self, storage_backend, database):
        self.storage = storage_backend
        self.db = database
        
        # Configuration
        self.compression_config = {
            CompressionType.GZIP: {"level": 6, "threads": 1},
            CompressionType.ZSTD: {"level": 3, "threads": 4},
            CompressionType.LZ4: {"level": 1, "threads": 2},
        }
        
        # Storage tier thresholds
        self.tier_thresholds = {
            "hot_to_warm_days": 7,
            "warm_to_cold_days": 30,
            "cold_to_archive_days": 90,
            "access_count_hot": 10,
            "access_count_warm": 5
        }
        
        # Thread pools for different operations
        self.compression_pool = ProcessPoolExecutor(max_workers=4)
        self.io_pool = ThreadPoolExecutor(max_workers=8)
        
        # Optimization queues
        self.optimization_queue: asyncio.Queue = asyncio.Queue(maxsize=1000)
        self.active_jobs: Dict[str, OptimizationJob] = {}
        
        # Deduplication tracking
        self.content_hashes: Dict[str, List[str]] = {}
        self.chunk_cache: Dict[str, bytes] = {}
        
        # Statistics
        self.optimization_stats = {
            "total_savings_bytes": 0,
            "total_recordings_optimized": 0,
            "compression_jobs_completed": 0,
            "deduplication_savings": 0,
            "tier_movements": 0
        }
        
        # Start optimization workers
        self.workers = []
        for i in range(3):
            worker = asyncio.create_task(self._optimization_worker(f"worker_{i}"))
            self.workers.append(worker)
        
        # Start maintenance tasks
        self.maintenance_task = asyncio.create_task(self._maintenance_loop())

    async def optimize_recording(self, recording_id: str, strategy: OptimizationStrategy = OptimizationStrategy.BALANCED) -> str:
        """Queue recording for optimization"""
        try:
            # Get current storage metrics
            metrics = await self._get_storage_metrics(recording_id)
            if not metrics:
                raise ValueError("Recording not found")
            
            # Determine optimal compression and tier
            target_compression, target_tier = await self._determine_optimization_targets(
                metrics, strategy
            )
            
            # Calculate potential savings
            estimated_savings = await self._estimate_savings(
                metrics, target_compression, target_tier
            )
            
            # Create optimization job
            job_id = self._generate_job_id(recording_id)
            job = OptimizationJob(
                id=job_id,
                recording_id=recording_id,
                current_compression=metrics.compression_type,
                target_compression=target_compression,
                current_tier=metrics.storage_tier,
                target_tier=target_tier,
                priority=self._calculate_priority(metrics, estimated_savings),
                estimated_savings=estimated_savings,
                created_at=time.time(),
                started_at=None,
                completed_at=None,
                status="queued",
                error_message=None
            )
            
            # Queue job
            await self.optimization_queue.put(job)
            self.active_jobs[job_id] = job
            
            logfire.info("Recording optimization queued",
                       recording_id=recording_id,
                       job_id=job_id,
                       strategy=strategy.value,
                       estimated_savings=estimated_savings)
            
            return job_id
            
        except Exception as e:
            logger.error("Failed to queue recording optimization",
                        recording_id=recording_id,
                        error=str(e))
            raise

    async def compress_recording(self, recording_id: str, 
                               compression_type: CompressionType,
                               content: bytes = None) -> Tuple[bytes, float]:
        """Compress recording with specified algorithm"""
        try:
            # Load content if not provided
            if content is None:
                content = await self._load_recording_content(recording_id)
            
            start_time = time.time()
            
            # Compress based on type
            if compression_type == CompressionType.GZIP:
                compressed = await asyncio.get_event_loop().run_in_executor(
                    self.compression_pool,
                    self._compress_gzip,
                    content
                )
            elif compression_type == CompressionType.ZSTD:
                compressed = await asyncio.get_event_loop().run_in_executor(
                    self.compression_pool,
                    self._compress_zstd,
                    content
                )
            elif compression_type == CompressionType.LZ4:
                compressed = await asyncio.get_event_loop().run_in_executor(
                    self.compression_pool,
                    self._compress_lz4,
                    content
                )
            else:
                compressed = content
            
            compression_time = time.time() - start_time
            compression_ratio = len(compressed) / len(content) if content else 0
            
            logfire.info("Recording compression completed",
                       recording_id=recording_id,
                       compression_type=compression_type.value,
                       original_size=len(content),
                       compressed_size=len(compressed),
                       compression_ratio=compression_ratio,
                       compression_time_seconds=compression_time)
            
            return compressed, compression_ratio
            
        except Exception as e:
            logger.error("Failed to compress recording",
                        recording_id=recording_id,
                        compression_type=compression_type.value,
                        error=str(e))
            raise

    def _compress_gzip(self, content: bytes) -> bytes:
        """Compress using gzip"""
        config = self.compression_config[CompressionType.GZIP]
        return gzip.compress(content, compresslevel=config["level"])

    def _compress_zstd(self, content: bytes) -> bytes:
        """Compress using zstd"""
        config = self.compression_config[CompressionType.ZSTD]
        return zstd.compress(content, level=config["level"], threads=config["threads"])

    def _compress_lz4(self, content: bytes) -> bytes:
        """Compress using lz4"""
        config = self.compression_config[CompressionType.LZ4]
        return lz4.frame.compress(content, compression_level=config["level"])

    async def deduplicate_recordings(self, recording_ids: List[str] = None) -> Dict[str, Any]:
        """Find and deduplicate similar recordings"""
        try:
            if recording_ids is None:
                recording_ids = await self._get_all_recording_ids()
            
            # Calculate content hashes for similarity detection
            candidates = await self._find_deduplication_candidates(recording_ids)
            
            total_savings = 0
            deduplicated_count = 0
            
            for candidate in candidates:
                if candidate.potential_savings > 1024 * 1024:  # Only deduplicate if >1MB savings
                    savings = await self._perform_deduplication(candidate)
                    total_savings += savings
                    deduplicated_count += 1
            
            # Update statistics
            self.optimization_stats["deduplication_savings"] += total_savings
            
            logfire.info("Deduplication completed",
                       recordings_processed=len(recording_ids),
                       deduplicated_count=deduplicated_count,
                       total_savings_bytes=total_savings)
            
            return {
                "recordings_processed": len(recording_ids),
                "candidates_found": len(candidates),
                "deduplicated_count": deduplicated_count,
                "total_savings_bytes": total_savings
            }
            
        except Exception as e:
            logger.error("Failed to deduplicate recordings", error=str(e))
            raise

    async def move_to_storage_tier(self, recording_id: str, target_tier: StorageTier) -> bool:
        """Move recording to different storage tier"""
        try:
            # Get current storage info
            metrics = await self._get_storage_metrics(recording_id)
            if not metrics:
                return False
            
            if metrics.storage_tier == target_tier:
                return True  # Already in target tier
            
            # Load recording content
            content = await self._load_recording_content(recording_id)
            
            # Store in new tier
            new_storage_path = await self._store_in_tier(recording_id, content, target_tier)
            
            # Update metadata
            await self._update_storage_metrics(recording_id, {
                "storage_tier": target_tier,
                "storage_path": new_storage_path
            })
            
            # Remove from old tier (async cleanup)
            asyncio.create_task(self._cleanup_old_storage(metrics.recording_id, metrics.storage_tier))
            
            # Update statistics
            self.optimization_stats["tier_movements"] += 1
            
            logfire.info("Recording moved to new storage tier",
                       recording_id=recording_id,
                       from_tier=metrics.storage_tier.value,
                       to_tier=target_tier.value)
            
            return True
            
        except Exception as e:
            logger.error("Failed to move recording to storage tier",
                        recording_id=recording_id,
                        target_tier=target_tier.value,
                        error=str(e))
            return False

    async def _optimization_worker(self, worker_name: str):
        """Background worker for processing optimization jobs"""
        while True:
            try:
                # Get job from queue
                job = await self.optimization_queue.get()
                
                if job.id not in self.active_jobs:
                    continue
                
                # Update job status
                job.started_at = time.time()
                job.status = "running"
                
                logfire.info("Starting optimization job",
                           worker=worker_name,
                           job_id=job.id,
                           recording_id=job.recording_id)
                
                try:
                    # Perform optimization
                    await self._execute_optimization_job(job)
                    
                    # Mark as completed
                    job.completed_at = time.time()
                    job.status = "completed"
                    
                    # Update statistics
                    self.optimization_stats["total_recordings_optimized"] += 1
                    if job.estimated_savings > 0:
                        self.optimization_stats["total_savings_bytes"] += job.estimated_savings
                    
                    logfire.info("Optimization job completed",
                               worker=worker_name,
                               job_id=job.id,
                               recording_id=job.recording_id,
                               execution_time_seconds=job.completed_at - job.started_at,
                               savings_bytes=job.estimated_savings)
                
                except Exception as e:
                    # Mark as failed
                    job.status = "failed"
                    job.error_message = str(e)
                    
                    logger.error("Optimization job failed",
                               worker=worker_name,
                               job_id=job.id,
                               recording_id=job.recording_id,
                               error=str(e))
                
                finally:
                    # Remove from active jobs
                    self.active_jobs.pop(job.id, None)
                    
                    # Store job result
                    await self._store_optimization_job_result(job)
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Optimization worker error",
                           worker=worker_name,
                           error=str(e))

    async def _execute_optimization_job(self, job: OptimizationJob):
        """Execute optimization job"""
        # Load current recording
        content = await self._load_recording_content(job.recording_id)
        
        # Apply compression optimization
        if job.target_compression != job.current_compression:
            compressed_content, compression_ratio = await self.compress_recording(
                job.recording_id, job.target_compression, content
            )
            
            # Store compressed version
            await self._store_compressed_recording(
                job.recording_id, compressed_content, job.target_compression
            )
            
            self.optimization_stats["compression_jobs_completed"] += 1
        
        # Apply storage tier optimization
        if job.target_tier != job.current_tier:
            await self.move_to_storage_tier(job.recording_id, job.target_tier)

    async def _maintenance_loop(self):
        """Periodic maintenance for storage optimization"""
        while True:
            try:
                await asyncio.sleep(3600)  # Run every hour
                
                # Analyze storage usage and patterns
                await self._analyze_storage_patterns()
                
                # Auto-optimize based on access patterns
                await self._auto_optimize_by_access_patterns()
                
                # Cleanup old optimization jobs
                await self._cleanup_old_optimization_jobs()
                
                # Run garbage collection on chunk cache
                await self._cleanup_chunk_cache()
                
                logfire.info("Storage maintenance completed",
                           active_jobs=len(self.active_jobs),
                           optimization_stats=self.optimization_stats)
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Storage maintenance error", error=str(e))

    async def _analyze_storage_patterns(self):
        """Analyze storage usage patterns for optimization opportunities"""
        try:
            # Get all recordings with metrics
            all_metrics = await self._get_all_storage_metrics()
            
            # Analyze access patterns
            hot_recordings = []
            cold_recordings = []
            
            current_time = time.time()
            
            for metrics in all_metrics:
                days_since_access = (current_time - metrics.last_accessed) / 86400
                
                # Identify hot recordings (frequently accessed, recently accessed)
                if (metrics.access_count >= self.tier_thresholds["access_count_hot"] and
                    days_since_access <= self.tier_thresholds["hot_to_warm_days"]):
                    
                    if metrics.storage_tier != StorageTier.HOT:
                        hot_recordings.append(metrics.recording_id)
                
                # Identify cold recordings (infrequently accessed, old)
                elif (metrics.access_count <= self.tier_thresholds["access_count_warm"] and
                      days_since_access >= self.tier_thresholds["warm_to_cold_days"]):
                    
                    if metrics.storage_tier in [StorageTier.HOT, StorageTier.WARM]:
                        cold_recordings.append(metrics.recording_id)
            
            # Queue optimization jobs for tier changes
            for recording_id in hot_recordings:
                await self.optimize_recording(recording_id, OptimizationStrategy.SPEED_OPTIMAL)
            
            for recording_id in cold_recordings:
                await self.optimize_recording(recording_id, OptimizationStrategy.COST_OPTIMAL)
            
            logfire.info("Storage pattern analysis completed",
                       hot_candidates=len(hot_recordings),
                       cold_candidates=len(cold_recordings))
            
        except Exception as e:
            logger.error("Failed to analyze storage patterns", error=str(e))

    async def get_optimization_statistics(self) -> Dict[str, Any]:
        """Get storage optimization statistics"""
        try:
            # Get current storage distribution
            storage_distribution = await self._get_storage_distribution()
            
            # Get compression statistics
            compression_stats = await self._get_compression_statistics()
            
            # Calculate total storage used
            total_storage = sum(
                tier_info["total_size"] for tier_info in storage_distribution.values()
            )
            
            return {
                "optimization_stats": self.optimization_stats.copy(),
                "storage_distribution": storage_distribution,
                "compression_stats": compression_stats,
                "total_storage_bytes": total_storage,
                "active_optimization_jobs": len(self.active_jobs),
                "queue_size": self.optimization_queue.qsize(),
                "cache_size": len(self.chunk_cache),
                "deduplication_groups": len(self.content_hashes)
            }
            
        except Exception as e:
            logger.error("Failed to get optimization statistics", error=str(e))
            return {}

    async def force_optimization_all_recordings(self, strategy: OptimizationStrategy = OptimizationStrategy.BALANCED) -> str:
        """Force optimization of all recordings"""
        try:
            all_recording_ids = await self._get_all_recording_ids()
            
            optimization_job_ids = []
            for recording_id in all_recording_ids:
                try:
                    job_id = await self.optimize_recording(recording_id, strategy)
                    optimization_job_ids.append(job_id)
                except Exception as e:
                    logger.warning("Failed to queue optimization for recording",
                                 recording_id=recording_id,
                                 error=str(e))
            
            logfire.info("Bulk optimization queued",
                       total_recordings=len(all_recording_ids),
                       queued_jobs=len(optimization_job_ids),
                       strategy=strategy.value)
            
            return f"Queued {len(optimization_job_ids)} optimization jobs"
            
        except Exception as e:
            logger.error("Failed to queue bulk optimization", error=str(e))
            raise

    # Helper methods and database operations
    async def _determine_optimization_targets(self, metrics: StorageMetrics, 
                                            strategy: OptimizationStrategy) -> Tuple[CompressionType, StorageTier]:
        """Determine optimal compression and storage tier"""
        if strategy == OptimizationStrategy.SIZE_OPTIMAL:
            return CompressionType.ZSTD, StorageTier.COLD
        elif strategy == OptimizationStrategy.SPEED_OPTIMAL:
            return CompressionType.LZ4, StorageTier.HOT
        elif strategy == OptimizationStrategy.COST_OPTIMAL:
            return CompressionType.GZIP, StorageTier.ARCHIVE
        else:  # BALANCED
            # Choose based on access patterns
            current_time = time.time()
            days_since_access = (current_time - metrics.last_accessed) / 86400
            
            if days_since_access <= 7 and metrics.access_count >= 10:
                return CompressionType.LZ4, StorageTier.HOT
            elif days_since_access <= 30:
                return CompressionType.GZIP, StorageTier.WARM
            else:
                return CompressionType.ZSTD, StorageTier.COLD

    def _generate_job_id(self, recording_id: str) -> str:
        """Generate optimization job ID"""
        import secrets
        timestamp = str(int(time.time() * 1000))
        random_suffix = secrets.token_hex(8)
        return f"opt_{recording_id}_{timestamp}_{random_suffix}"

    def _calculate_priority(self, metrics: StorageMetrics, estimated_savings: int) -> int:
        """Calculate optimization job priority"""
        # Higher priority for larger potential savings and more accessed recordings
        size_factor = min(metrics.original_size / (1024 * 1024), 100)  # MB, capped at 100
        access_factor = min(metrics.access_count, 50)  # Capped at 50
        savings_factor = min(estimated_savings / (1024 * 1024), 100)  # MB, capped at 100
        
        return int(size_factor + access_factor + savings_factor)

    # Database operations (implement based on your database choice)
    async def _get_storage_metrics(self, recording_id: str) -> Optional[StorageMetrics]:
        """Get storage metrics for recording"""
        # Implementation depends on database backend
        pass
    
    async def _update_storage_metrics(self, recording_id: str, updates: Dict[str, Any]) -> None:
        """Update storage metrics in database"""
        # Implementation depends on database backend
        pass
    
    async def _get_all_recording_ids(self) -> List[str]:
        """Get all recording IDs from database"""
        # Implementation depends on database backend
        pass
    
    async def _store_optimization_job_result(self, job: OptimizationJob) -> None:
        """Store optimization job result in database"""
        # Implementation depends on database backend
        pass
```

### Storage Lifecycle Manager
**Location**: `recording-manager/storage/lifecycle_manager.py`

```python
# recording-manager/storage/lifecycle_manager.py
import asyncio
import time
from typing import Dict, Any, List, Optional
from dataclasses import dataclass
from enum import Enum
import structlog
import logfire

logger = structlog.get_logger()

class LifecycleAction(Enum):
    RETAIN = "retain"
    COMPRESS = "compress"
    MOVE_TIER = "move_tier" 
    ARCHIVE = "archive"
    DELETE = "delete"

@dataclass
class LifecycleRule:
    id: str
    name: str
    user_id: Optional[str]  # None for global rules
    conditions: Dict[str, Any]
    action: LifecycleAction
    action_params: Dict[str, Any]
    is_enabled: bool
    priority: int
    created_at: float
    last_applied: float

class RecordingLifecycleManager:
    def __init__(self, storage_optimizer, database):
        self.optimizer = storage_optimizer
        self.db = database
        
        # Default lifecycle rules
        self.default_rules = [
            LifecycleRule(
                id="default_compress_old",
                name="Compress recordings older than 30 days",
                user_id=None,
                conditions={"age_days": 30, "access_count_max": 5},
                action=LifecycleAction.COMPRESS,
                action_params={"compression_type": "zstd"},
                is_enabled=True,
                priority=10,
                created_at=time.time(),
                last_applied=0
            ),
            LifecycleRule(
                id="default_archive_inactive",
                name="Archive recordings inactive for 90 days",
                user_id=None,
                conditions={"last_accessed_days": 90, "access_count_max": 2},
                action=LifecycleAction.ARCHIVE,
                action_params={},
                is_enabled=True,
                priority=20,
                created_at=time.time(),
                last_applied=0
            ),
            LifecycleRule(
                id="default_delete_expired",
                name="Delete recordings older than 1 year with no access",
                user_id=None,
                conditions={"age_days": 365, "access_count_max": 0},
                action=LifecycleAction.DELETE,
                action_params={},
                is_enabled=False,  # Disabled by default
                priority=30,
                created_at=time.time(),
                last_applied=0
            )
        ]
        
        # Lifecycle task
        self.lifecycle_task = asyncio.create_task(self._lifecycle_loop())

    async def apply_lifecycle_rules(self, recording_ids: List[str] = None) -> Dict[str, Any]:
        """Apply lifecycle rules to recordings"""
        try:
            if recording_ids is None:
                recording_ids = await self._get_all_recording_ids()
            
            # Get all active rules
            rules = await self._get_active_lifecycle_rules()
            
            actions_taken = {action.value: 0 for action in LifecycleAction}
            processed_recordings = 0
            
            for recording_id in recording_ids:
                try:
                    # Get recording metadata
                    metadata = await self._get_recording_metadata(recording_id)
                    if not metadata:
                        continue
                    
                    # Check each rule
                    for rule in rules:
                        if await self._should_apply_rule(rule, metadata):
                            success = await self._apply_lifecycle_action(
                                recording_id, rule.action, rule.action_params
                            )
                            
                            if success:
                                actions_taken[rule.action.value] += 1
                                break  # Only apply first matching rule
                    
                    processed_recordings += 1
                    
                except Exception as e:
                    logger.error("Failed to apply lifecycle rules to recording",
                               recording_id=recording_id,
                               error=str(e))
            
            logfire.info("Lifecycle rules applied",
                       processed_recordings=processed_recordings,
                       actions_taken=actions_taken)
            
            return {
                "processed_recordings": processed_recordings,
                "actions_taken": actions_taken
            }
            
        except Exception as e:
            logger.error("Failed to apply lifecycle rules", error=str(e))
            raise

    async def _lifecycle_loop(self):
        """Periodic lifecycle management"""
        while True:
            try:
                await asyncio.sleep(24 * 3600)  # Run daily
                
                # Apply lifecycle rules
                await self.apply_lifecycle_rules()
                
                # Update rule last_applied timestamps
                await self._update_rule_timestamps()
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Lifecycle loop error", error=str(e))

    async def _should_apply_rule(self, rule: LifecycleRule, metadata) -> bool:
        """Check if lifecycle rule should be applied to recording"""
        current_time = time.time()
        
        # Check age condition
        if "age_days" in rule.conditions:
            age_days = (current_time - metadata.created_at) / 86400
            if age_days < rule.conditions["age_days"]:
                return False
        
        # Check last accessed condition  
        if "last_accessed_days" in rule.conditions:
            last_accessed_days = (current_time - metadata.last_accessed) / 86400
            if last_accessed_days < rule.conditions["last_accessed_days"]:
                return False
        
        # Check access count condition
        if "access_count_max" in rule.conditions:
            if metadata.access_count > rule.conditions["access_count_max"]:
                return False
        
        # Check user-specific conditions
        if rule.user_id and rule.user_id != metadata.user_id:
            return False
        
        return True

    async def _apply_lifecycle_action(self, recording_id: str, 
                                    action: LifecycleAction, 
                                    params: Dict[str, Any]) -> bool:
        """Apply lifecycle action to recording"""
        try:
            if action == LifecycleAction.COMPRESS:
                compression_type = params.get("compression_type", "gzip")
                job_id = await self.optimizer.optimize_recording(
                    recording_id, 
                    strategy="size_optimal"
                )
                return job_id is not None
                
            elif action == LifecycleAction.MOVE_TIER:
                target_tier = params.get("target_tier", "cold")
                return await self.optimizer.move_to_storage_tier(
                    recording_id, target_tier
                )
                
            elif action == LifecycleAction.ARCHIVE:
                return await self.optimizer.move_to_storage_tier(
                    recording_id, "archive"
                )
                
            elif action == LifecycleAction.DELETE:
                # Implement safe deletion with confirmation
                return await self._safe_delete_recording(recording_id)
                
            return False
            
        except Exception as e:
            logger.error("Failed to apply lifecycle action",
                        recording_id=recording_id,
                        action=action.value,
                        error=str(e))
            return False

    # Database operations
    async def _get_active_lifecycle_rules(self) -> List[LifecycleRule]:
        """Get active lifecycle rules from database"""
        # Implementation depends on database backend
        return [rule for rule in self.default_rules if rule.is_enabled]
    
    async def _get_recording_metadata(self, recording_id: str):
        """Get recording metadata from database"""
        # Implementation depends on database backend
        pass
    
    async def _safe_delete_recording(self, recording_id: str) -> bool:
        """Safely delete recording with proper cleanup"""
        # Implementation depends on storage backend
        pass
```

## TDD Implementation Cycle

### Red Phase: Storage Optimization Test Creation
```python
# recording-manager/tests/test_storage_optimization.py
import pytest
from recording_manager.storage.optimization_engine import RecordingStorageOptimizer, CompressionType

@pytest.mark.asyncio
async def test_storage_optimizer_initialization():
    """Test storage optimizer initializes correctly"""
    # This test should initially fail (Red phase)
    optimizer = RecordingStorageOptimizer(None, None)
    assert False, "Storage optimizer initialization not implemented yet"

@pytest.mark.asyncio
async def test_compression_optimization():
    """Test recording compression optimization"""
    # This test should initially fail (Red phase)
    assert False, "Compression optimization not implemented yet"

@pytest.mark.asyncio
async def test_storage_tier_management():
    """Test storage tier movement and management"""
    # This test should initially fail (Red phase)
    assert False, "Storage tier management not implemented yet"
```

### Green Phase: Storage Optimization Implementation
```python
# Implement storage optimization features to make tests pass
# This involves adding compression algorithms, tier management, and optimization logic
```

### Refactor Phase: Storage Optimization Enhancement
```python
# Optimize storage system for performance and efficiency
# Add advanced compression strategies and intelligent tier management
# Enhance deduplication and lifecycle management
```

## Security Checklist ✅

### Storage Access Control
- [ ] Storage optimization operation authorization validation
- [ ] Cross-user storage access prevention
- [ ] Recording ownership verification for optimization operations
- [ ] Storage path manipulation prevention
- [ ] Unauthorized storage tier access prevention
- [ ] Storage backend authentication and authorization
- [ ] Storage operation audit logging
- [ ] Optimization job ownership validation
- [ ] Storage enumeration protection
- [ ] Administrative storage operation protection

### Data Protection During Optimization
- [ ] Data integrity validation during compression
- [ ] Secure temporary storage during optimization
- [ ] Memory protection for optimization operations
- [ ] Checksum verification for optimized data
- [ ] Secure data transfer between storage tiers
- [ ] Protection against data corruption during optimization
- [ ] Backup verification before destructive operations
- [ ] Rollback capability for failed optimizations
- [ ] Data recovery procedures for optimization failures
- [ ] Secure cleanup of temporary optimization data

### Compression Security
- [ ] Compression algorithm security validation
- [ ] Protection against compression bombs
- [ ] Resource limits for compression operations
- [ ] Secure compression library usage
- [ ] Memory usage monitoring during compression
- [ ] CPU usage limits for compression jobs
- [ ] Compression output validation
- [ ] Protection against malicious compressed data
- [ ] Secure decompression with limits
- [ ] Compression job isolation and sandboxing

### Lifecycle Management Security
- [ ] Lifecycle rule authorization validation
- [ ] Safe deletion with proper verification
- [ ] Archive operation security validation
- [ ] Lifecycle action audit logging
- [ ] Protection against unauthorized rule modification
- [ ] User consent for destructive lifecycle actions
- [ ] Backup verification before lifecycle actions
- [ ] Recovery procedures for lifecycle mistakes
- [ ] Lifecycle rule injection prevention
- [ ] Administrative lifecycle operation protection

### Deduplication Security
- [ ] Content hash security and collision prevention
- [ ] Deduplication operation authorization
- [ ] Cross-user deduplication isolation
- [ ] Secure chunk storage and retrieval
- [ ] Deduplication metadata integrity protection
- [ ] Protection against hash collision attacks
- [ ] Secure similarity detection algorithms
- [ ] Deduplication audit logging and monitoring
- [ ] Recovery procedures for deduplication errors
- [ ] Protection against deduplication-based attacks

## Performance Requirements

### Optimization Performance
- Compression job processing speed > 10 MB/s
- Storage tier movement < 30 seconds for 100MB recording
- Deduplication analysis < 5 minutes for 1000 recordings
- Lifecycle rule evaluation < 10 seconds for all recordings
- Optimization queue processing latency < 1 second
- Storage metrics calculation < 500ms per recording

### Resource Efficiency
- Memory usage < 500MB for optimization operations
- CPU usage < 80% during peak optimization
- Disk I/O efficiency > 90% of storage backend capacity
- Network bandwidth utilization < 70% during tier movement
- Thread pool utilization > 85%
- Cache hit ratio > 90% for frequently accessed data

### Scalability Requirements
- Support 10000+ recordings optimization simultaneously
- Handle 100+ concurrent optimization jobs
- Scale to 10TB+ total storage optimization
- Support 1000+ users with individual optimization preferences
- Process 1M+ lifecycle rule evaluations per day
- Manage 100000+ deduplication candidates

## Commit Instructions

After implementing the storage optimization system:

```bash
git add recording-manager/storage/
git commit -m "Add comprehensive storage optimization and lifecycle management

- Implement RecordingStorageOptimizer with multi-algorithm compression
- Add intelligent storage tier management (hot/warm/cold/archive)
- Implement deduplication system with content similarity detection
- Add RecordingLifecycleManager with configurable rules
- Include optimization job queue with priority-based processing
- Add comprehensive storage analytics and statistics
- Implement automated maintenance and pattern analysis
- Add resource-efficient compression with multiple algorithms
- Include storage cost optimization with intelligent tier placement
- Add TDD cycle with Red-Green-Refactor for optimization features
- Ensure >85% storage optimization test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete storage optimization test suite:

```bash
# Run all storage optimization tests
pytest recording-manager/tests/test_storage_optimization.py -v --timeout=600

# Run specific optimization test categories
pytest recording-manager/tests/storage/ -k "compression" -v
pytest recording-manager/tests/storage/ -k "deduplication" -v
pytest recording-manager/tests/storage/ -k "lifecycle" -v

# Run storage optimization performance tests
pytest recording-manager/tests/storage/performance/ -v

# Run storage optimization integration tests
pytest recording-manager/tests/storage/integration/ -v
```

Validate storage optimization test coverage:
```bash
pytest recording-manager/tests/storage/ --cov=recording_manager.storage --cov-report=html --cov-fail-under=85
```

## Integration Testing

Test storage optimization integration with other components:
```bash
# Test integration with Session 7 (Storage Backend)
pytest recording-manager/tests/integration/test_optimization_storage_integration.py -v

# Test integration with Session 10.1 (Recording Engine)
pytest recording-manager/tests/integration/test_optimization_recording_integration.py -v

# Test optimization with playback system
pytest recording-manager/tests/integration/test_optimization_playback_integration.py -v
```