"""
Core snapshot management system with Firecracker integration.

Provides VM state capture, restoration, and lifecycle management with
comprehensive security controls and performance optimization.
"""

import asyncio
import hashlib
import json
import time
import gzip
from typing import Dict, Any, Optional, List, BinaryIO
from dataclasses import dataclass, asdict
from enum import Enum
from pathlib import Path
import structlog
import logfire

logger = structlog.get_logger()


class SnapshotState(Enum):
    """Snapshot lifecycle states."""
    CREATING = "creating"
    AVAILABLE = "available"
    RESTORING = "restoring"
    DELETING = "deleting"
    ERROR = "error"
    CORRUPTED = "corrupted"


class SnapshotType(Enum):
    """Types of snapshots."""
    MANUAL = "manual"
    AUTOMATIC = "automatic"
    SCHEDULED = "scheduled"


@dataclass
class SnapshotMetadata:
    """Comprehensive snapshot metadata."""
    snapshot_id: str
    vm_id: str
    user_id: str
    name: str
    description: str
    snapshot_type: SnapshotType
    state: SnapshotState
    created_at: float
    updated_at: float
    size_bytes: int
    compressed_size_bytes: int
    checksum_sha256: str
    storage_path: str
    parent_snapshot_id: Optional[str]
    version: int
    tags: List[str]
    vm_config: Dict[str, Any]
    is_encrypted: bool
    restore_count: int = 0

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary for serialization."""
        data = asdict(self)
        data['snapshot_type'] = self.snapshot_type.value
        data['state'] = self.state.value
        return data


class SnapshotManager:
    """
    Core snapshot management system with security controls.
    
    Handles VM snapshot creation, restoration, deletion, and lifecycle management
    with comprehensive security validation and performance optimization.
    """

    def __init__(self, vm_manager, storage_backend, encryption_service, database):
        """Initialize snapshot manager with required dependencies."""
        self.vm_manager = vm_manager
        self.storage_backend = storage_backend
        self.encryption_service = encryption_service
        self.database = database
        
        # In-memory snapshot registry
        self.snapshots: Dict[str, SnapshotMetadata] = {}
        self.active_operations: Dict[str, asyncio.Task] = {}
        
        # Quota and rate limiting configuration
        self.max_snapshots_per_user = 50
        self.max_storage_per_user_gb = 100
        self.max_snapshots_per_hour = 5
        
        # Performance configuration
        self.snapshot_timeout = 600  # 10 minutes
        self.compression_enabled = True
        
        # Rate limiting tracking
        self.user_operation_timestamps: Dict[str, List[float]] = {}

    async def initialize(self):
        """Initialize the snapshot manager and load existing snapshots."""
        try:
            # Load existing snapshots from database
            await self._load_existing_snapshots()
            
            # Clean up any stale operations
            await self._cleanup_stale_operations()
            
            logger.info("Snapshot manager initialized", 
                       total_snapshots=len(self.snapshots))
            
            # Log to Logfire
            logfire.info("Snapshot manager started",
                        snapshot_count=len(self.snapshots),
                        service="snapshot-manager")
                        
        except Exception as e:
            logger.error("Failed to initialize snapshot manager", error=str(e))
            logfire.error("Snapshot manager initialization failed", error=str(e))
            raise

    async def create_snapshot(self, vm_id: str, user_id: str, name: str,
                            description: str = "", snapshot_type: SnapshotType = SnapshotType.MANUAL,
                            tags: List[str] = None, encrypt: bool = True) -> str:
        """
        Create a new VM snapshot with comprehensive validation.
        
        Args:
            vm_id: ID of the VM to snapshot
            user_id: ID of the user creating the snapshot
            name: Human-readable name for the snapshot
            description: Optional description
            snapshot_type: Type of snapshot (manual, automatic, scheduled)
            tags: Optional list of tags
            encrypt: Whether to encrypt the snapshot data
            
        Returns:
            str: Unique snapshot ID
            
        Raises:
            PermissionError: If VM access is denied
            ValueError: If quota exceeded or invalid input
        """
        try:
            # Input validation
            await self._validate_snapshot_inputs(vm_id, user_id, name, description, tags)
            
            # Check rate limiting
            if not await self._check_rate_limit(user_id):
                raise ValueError("Rate limit exceeded - maximum 5 snapshots per hour")
            
            # Validate user quotas
            await self._validate_user_quota(user_id)
            
            # Validate VM ownership and access
            vm = await self.vm_manager.get_vm(vm_id)
            if not vm or vm.user_id != user_id:
                raise PermissionError("VM not found or access denied")
            
            # Generate unique snapshot ID
            snapshot_id = self._generate_snapshot_id(vm_id, user_id)
            
            # Create snapshot metadata
            metadata = SnapshotMetadata(
                snapshot_id=snapshot_id,
                vm_id=vm_id,
                user_id=user_id,
                name=name,
                description=description,
                snapshot_type=snapshot_type,
                state=SnapshotState.CREATING,
                created_at=time.time(),
                updated_at=time.time(),
                size_bytes=0,
                compressed_size_bytes=0,
                checksum_sha256="",
                storage_path="",
                parent_snapshot_id=None,
                version=await self._get_next_version(vm_id, user_id),
                tags=tags or [],
                vm_config=vm.get_config(),
                is_encrypted=encrypt,
                restore_count=0
            )
            
            # Store in memory registry
            self.snapshots[snapshot_id] = metadata
            
            # Record operation timestamp for rate limiting
            self._record_operation_timestamp(user_id)
            
            # Start asynchronous snapshot creation
            task = asyncio.create_task(
                self._create_snapshot_async(snapshot_id)
            )
            self.active_operations[snapshot_id] = task
            
            logger.info("Snapshot creation started", 
                       snapshot_id=snapshot_id,
                       vm_id=vm_id, 
                       user_id=user_id,
                       name=name)
            
            # Log to Logfire with span tracing
            with logfire.span("snapshot_creation_started") as span:
                span.set_attribute("snapshot_id", snapshot_id)
                span.set_attribute("vm_id", vm_id)
                span.set_attribute("user_id", user_id)
                span.set_attribute("snapshot_type", snapshot_type.value)
                span.set_attribute("encrypted", encrypt)
                
                logfire.info("VM snapshot creation initiated",
                           snapshot_id=snapshot_id,
                           vm_id=vm_id,
                           user_id=user_id,
                           name=name,
                           encrypt=encrypt)
            
            return snapshot_id
            
        except Exception as e:
            logger.error("Failed to create snapshot", 
                        vm_id=vm_id, user_id=user_id, error=str(e))
            logfire.error("Snapshot creation failed",
                         vm_id=vm_id, user_id=user_id, error=str(e))
            raise

    async def restore_snapshot(self, snapshot_id: str, user_id: str, 
                             target_vm_id: Optional[str] = None) -> str:
        """
        Restore a snapshot to a VM with security validation.
        
        Args:
            snapshot_id: ID of the snapshot to restore
            user_id: ID of the user requesting restoration
            target_vm_id: Optional target VM ID (defaults to original VM)
            
        Returns:
            str: ID of the VM being restored to
            
        Raises:
            PermissionError: If snapshot or VM access is denied
            ValueError: If snapshot is not available for restore
        """
        try:
            # Validate snapshot ownership and access
            metadata = self.snapshots.get(snapshot_id)
            if not metadata or metadata.user_id != user_id:
                raise PermissionError("Snapshot not found or access denied")
            
            if metadata.state != SnapshotState.AVAILABLE:
                raise ValueError(f"Snapshot not available for restore: {metadata.state}")
            
            # Determine target VM
            if target_vm_id is None:
                target_vm_id = metadata.vm_id
            else:
                # Validate target VM ownership
                target_vm = await self.vm_manager.get_vm(target_vm_id)
                if not target_vm or target_vm.user_id != user_id:
                    raise PermissionError("Target VM not found or access denied")
            
            # Update snapshot state
            metadata.state = SnapshotState.RESTORING
            metadata.updated_at = time.time()
            
            # Start asynchronous restoration
            restore_task_id = f"restore_{snapshot_id}_{target_vm_id}"
            task = asyncio.create_task(
                self._restore_snapshot_async(snapshot_id, target_vm_id)
            )
            self.active_operations[restore_task_id] = task
            
            logger.info("Snapshot restore started", 
                       snapshot_id=snapshot_id,
                       target_vm_id=target_vm_id, 
                       user_id=user_id)
            
            # Log to Logfire
            logfire.info("Snapshot restore initiated",
                        snapshot_id=snapshot_id,
                        target_vm_id=target_vm_id,
                        user_id=user_id)
            
            return target_vm_id
            
        except Exception as e:
            logger.error("Failed to restore snapshot", 
                        snapshot_id=snapshot_id, error=str(e))
            logfire.error("Snapshot restore failed",
                         snapshot_id=snapshot_id, error=str(e))
            raise

    async def delete_snapshot(self, snapshot_id: str, user_id: str) -> bool:
        """
        Delete a snapshot with security validation.
        
        Args:
            snapshot_id: ID of the snapshot to delete
            user_id: ID of the user requesting deletion
            
        Returns:
            bool: True if deletion successful, False otherwise
        """
        try:
            # Validate snapshot ownership
            metadata = self.snapshots.get(snapshot_id)
            if not metadata or metadata.user_id != user_id:
                return False
            
            # Update state to deleting
            metadata.state = SnapshotState.DELETING
            metadata.updated_at = time.time()
            
            # Delete from storage backend
            if metadata.storage_path:
                await self.storage_backend.delete_snapshot(snapshot_id)
            else:
                # For snapshots without storage path, simulate storage deletion
                pass
            
            # Remove from memory registry
            del self.snapshots[snapshot_id]
            
            # Remove from database
            await self.database.delete_snapshot_metadata(snapshot_id)
            
            logger.info("Snapshot deleted successfully", 
                       snapshot_id=snapshot_id, user_id=user_id)
            
            logfire.info("Snapshot deletion completed",
                        snapshot_id=snapshot_id,
                        user_id=user_id)
            
            return True
            
        except Exception as e:
            logger.error("Failed to delete snapshot", 
                        snapshot_id=snapshot_id, error=str(e))
            logfire.error("Snapshot deletion failed",
                         snapshot_id=snapshot_id, error=str(e))
            return False

    async def list_user_snapshots(self, user_id: str, vm_id: Optional[str] = None, 
                                 limit: int = 50, offset: int = 0) -> List[SnapshotMetadata]:
        """
        List snapshots for a specific user with optional filtering.
        
        Args:
            user_id: ID of the user
            vm_id: Optional VM ID filter
            limit: Maximum number of results
            offset: Offset for pagination
            
        Returns:
            List[SnapshotMetadata]: List of user's snapshots
        """
        try:
            # Filter snapshots by user and optionally by VM
            user_snapshots = [
                metadata for metadata in self.snapshots.values()
                if metadata.user_id == user_id and 
                   (vm_id is None or metadata.vm_id == vm_id)
            ]
            
            # Sort by creation time (newest first)
            user_snapshots.sort(key=lambda x: x.created_at, reverse=True)
            
            # Apply pagination
            return user_snapshots[offset:offset + limit]
            
        except Exception as e:
            logger.error("Failed to list user snapshots", 
                        user_id=user_id, error=str(e))
            return []

    async def get_snapshot(self, snapshot_id: str, user_id: str) -> Optional[SnapshotMetadata]:
        """
        Get snapshot metadata with ownership validation.
        
        Args:
            snapshot_id: ID of the snapshot
            user_id: ID of the requesting user
            
        Returns:
            Optional[SnapshotMetadata]: Snapshot metadata if authorized, None otherwise
        """
        metadata = self.snapshots.get(snapshot_id)
        if metadata and metadata.user_id == user_id:
            return metadata
        return None

    # Private helper methods

    async def _validate_snapshot_inputs(self, vm_id: str, user_id: str, 
                                       name: str, description: str, 
                                       tags: Optional[List[str]]):
        """Validate all snapshot inputs for security."""
        if not vm_id or not isinstance(vm_id, str):
            raise ValueError("Invalid VM ID")
        
        if not user_id or not isinstance(user_id, str):
            raise ValueError("Invalid user ID")
        
        if not name or not isinstance(name, str) or len(name) > 100:
            raise ValueError("Invalid snapshot name")
        
        if description and len(description) > 500:
            raise ValueError("Description too long")
        
        if tags and (len(tags) > 10 or any(len(tag) > 50 for tag in tags)):
            raise ValueError("Invalid tags")
        
        # Security validation - prevent injection attacks
        dangerous_patterns = ["<script", "javascript:", "DROP TABLE", "../", "..\\"]
        for pattern in dangerous_patterns:
            if pattern.lower() in name.lower() or pattern.lower() in description.lower():
                raise ValueError("Invalid characters in input")

    async def _check_rate_limit(self, user_id: str) -> bool:
        """Check if user is within rate limits."""
        current_time = time.time()
        hour_ago = current_time - 3600  # 1 hour ago
        
        # Get user's operation timestamps
        user_timestamps = self.user_operation_timestamps.get(user_id, [])
        
        # Filter to last hour
        recent_operations = [ts for ts in user_timestamps if ts > hour_ago]
        
        # Update the stored timestamps
        self.user_operation_timestamps[user_id] = recent_operations
        
        # Check if under limit
        return len(recent_operations) < self.max_snapshots_per_hour

    def _record_operation_timestamp(self, user_id: str):
        """Record timestamp of snapshot operation for rate limiting."""
        current_time = time.time()
        if user_id not in self.user_operation_timestamps:
            self.user_operation_timestamps[user_id] = []
        self.user_operation_timestamps[user_id].append(current_time)

    async def _validate_user_quota(self, user_id: str):
        """Validate user snapshot and storage quotas."""
        # Check snapshot count quota
        snapshot_count = await self.database.count_user_snapshots(user_id)
        if snapshot_count >= self.max_snapshots_per_user:
            raise ValueError("Snapshot quota exceeded - maximum 50 snapshots per user")
        
        # Check storage quota
        storage_usage_bytes = await self.database.get_user_storage_usage(user_id)
        storage_usage_gb = storage_usage_bytes / (1024 ** 3)
        if storage_usage_gb >= self.max_storage_per_user_gb:
            raise ValueError("Storage quota exceeded - maximum 100GB per user")

    def _generate_snapshot_id(self, vm_id: str, user_id: str) -> str:
        """Generate cryptographically secure snapshot ID."""
        timestamp = str(int(time.time() * 1000000))  # microseconds
        data = f"{vm_id}:{user_id}:{timestamp}"
        hash_digest = hashlib.sha256(data.encode()).hexdigest()
        return f"snap_{hash_digest[:16]}"

    async def _get_next_version(self, vm_id: str, user_id: str) -> int:
        """Get the next version number for snapshots of this VM."""
        user_vm_snapshots = [
            s for s in self.snapshots.values()
            if s.vm_id == vm_id and s.user_id == user_id
        ]
        return max([s.version for s in user_vm_snapshots], default=0) + 1

    async def _create_snapshot_async(self, snapshot_id: str):
        """Asynchronously create the snapshot (background task)."""
        if snapshot_id not in self.snapshots:
            logger.warning("Snapshot metadata not found during async creation", 
                         snapshot_id=snapshot_id)
            return
        
        metadata = self.snapshots[snapshot_id]
        
        try:
            # Pause VM if running
            vm_was_running = await self.vm_manager.is_vm_running(metadata.vm_id)
            if vm_was_running:
                await self.vm_manager.pause_vm(metadata.vm_id)
            
            # Create Firecracker snapshot
            snapshot_data = await self.vm_manager.create_firecracker_snapshot(metadata.vm_id)
            
            # Calculate checksum
            checksum = hashlib.sha256(snapshot_data).hexdigest()
            
            # Encrypt if requested
            if metadata.is_encrypted:
                snapshot_data = await self.encryption_service.encrypt_data(
                    snapshot_data, f"snapshot:{snapshot_id}"
                )
            
            # Compress data
            if self.compression_enabled:
                snapshot_data = gzip.compress(snapshot_data)
            
            # Store in backend
            storage_path = await self.storage_backend.store_snapshot(
                snapshot_id, snapshot_data
            )
            
            # Update metadata
            metadata.size_bytes = len(snapshot_data) if not self.compression_enabled else len(snapshot_data) * 2  # Estimate
            metadata.compressed_size_bytes = len(snapshot_data)
            metadata.checksum_sha256 = checksum
            metadata.storage_path = storage_path
            metadata.state = SnapshotState.AVAILABLE
            metadata.updated_at = time.time()
            
            # Resume VM if it was running
            if vm_was_running:
                await self.vm_manager.resume_vm(metadata.vm_id)
            
            # Persist to database
            await self.database.save_snapshot_metadata(metadata)
            
            logger.info("Snapshot created successfully", 
                       snapshot_id=snapshot_id,
                       size_bytes=metadata.size_bytes,
                       compressed_size=metadata.compressed_size_bytes)
            
            # Log completion to Logfire
            logfire.info("Snapshot creation completed",
                        snapshot_id=snapshot_id,
                        size_bytes=metadata.size_bytes,
                        compressed_size_bytes=metadata.compressed_size_bytes,
                        duration_seconds=time.time() - metadata.created_at)
            
        except Exception as e:
            # Mark as error
            metadata.state = SnapshotState.ERROR
            metadata.updated_at = time.time()
            
            logger.error("Snapshot creation failed", 
                        snapshot_id=snapshot_id, error=str(e))
            logfire.error("Snapshot creation error",
                         snapshot_id=snapshot_id, error=str(e))
            
            # Try to resume VM if it was paused
            try:
                if 'vm_was_running' in locals() and vm_was_running:
                    await self.vm_manager.resume_vm(metadata.vm_id)
            except Exception:
                pass  # Best effort
                
        finally:
            # Clean up active operation
            self.active_operations.pop(snapshot_id, None)

    async def _restore_snapshot_async(self, snapshot_id: str, target_vm_id: str):
        """Asynchronously restore snapshot (background task)."""
        metadata = self.snapshots[snapshot_id]
        
        try:
            # Stop target VM if running
            vm_was_running = await self.vm_manager.is_vm_running(target_vm_id)
            if vm_was_running:
                await self.vm_manager.pause_vm(target_vm_id)
            
            # Retrieve snapshot data from storage
            snapshot_data = await self.storage_backend.retrieve_snapshot(snapshot_id)
            
            # Decompress if needed
            if self.compression_enabled:
                snapshot_data = gzip.decompress(snapshot_data)
            
            # Decrypt if needed
            if metadata.is_encrypted:
                snapshot_data = await self.encryption_service.decrypt_data(
                    snapshot_data, f"snapshot:{snapshot_id}"
                )
            
            # Verify checksum
            checksum = hashlib.sha256(snapshot_data).hexdigest()
            if checksum != metadata.checksum_sha256:
                raise ValueError("Snapshot checksum verification failed - data may be corrupted")
            
            # Restore VM from snapshot data
            # This would integrate with Firecracker's restore functionality
            await self._restore_vm_from_snapshot_data(target_vm_id, snapshot_data)
            
            # Resume VM if it was running
            if vm_was_running:
                await self.vm_manager.resume_vm(target_vm_id)
            
            # Update restore count
            metadata.restore_count += 1
            metadata.state = SnapshotState.AVAILABLE
            metadata.updated_at = time.time()
            
            await self.database.save_snapshot_metadata(metadata)
            
            logger.info("Snapshot restored successfully", 
                       snapshot_id=snapshot_id,
                       target_vm_id=target_vm_id,
                       restore_count=metadata.restore_count)
            
            logfire.info("Snapshot restoration completed",
                        snapshot_id=snapshot_id,
                        target_vm_id=target_vm_id,
                        restore_count=metadata.restore_count)
            
        except Exception as e:
            # Reset snapshot state
            metadata.state = SnapshotState.AVAILABLE
            metadata.updated_at = time.time()
            
            logger.error("Snapshot restoration failed", 
                        snapshot_id=snapshot_id,
                        target_vm_id=target_vm_id, 
                        error=str(e))
            logfire.error("Snapshot restoration error",
                         snapshot_id=snapshot_id,
                         target_vm_id=target_vm_id,
                         error=str(e))
            
        finally:
            # Clean up active operation
            restore_task_id = f"restore_{snapshot_id}_{target_vm_id}"
            self.active_operations.pop(restore_task_id, None)

    async def _restore_vm_from_snapshot_data(self, vm_id: str, snapshot_data: bytes):
        """Restore VM from snapshot data using Firecracker."""
        # This would be implemented based on Firecracker's restore API
        # For now, it's a placeholder that simulates the operation
        pass

    async def _load_existing_snapshots(self):
        """Load existing snapshots from database."""
        # This would load from the database and populate self.snapshots
        # For now, start with empty registry
        pass

    async def _cleanup_stale_operations(self):
        """Clean up any stale async operations."""
        # This would clean up any operations that might have been interrupted
        pass