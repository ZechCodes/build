# Session 7.1: Snapshot Manager Core

## Objective
Implement the core snapshot management system with Firecracker integration, providing VM state capture, restoration, and lifecycle management.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for snapshot operation monitoring and debugging
- **Session 2**: Integrates with authentication system for snapshot ownership validation
- **Session 3**: Directly interfaces with VM manager for snapshot operations
- **Session 5**: Uses database models for snapshot metadata storage
- **Session 6**: Coordinates with session manager for state synchronization

## Core Implementation

### Snapshot Manager System
**Location**: `snapshot-manager/core/snapshot_manager.py`

```python
# snapshot-manager/core/snapshot_manager.py
import asyncio
import hashlib
import json
import time
from typing import Dict, Any, Optional, List, BinaryIO
from dataclasses import dataclass, asdict
from enum import Enum
from pathlib import Path
import aiofiles
import structlog
import logfire

logger = structlog.get_logger()

class SnapshotState(Enum):
    CREATING = "creating"
    AVAILABLE = "available"
    RESTORING = "restoring"
    DELETING = "deleting"
    ERROR = "error"
    CORRUPTED = "corrupted"

class SnapshotType(Enum):
    MANUAL = "manual"
    AUTOMATIC = "automatic"
    SCHEDULED = "scheduled"

@dataclass
class SnapshotMetadata:
    snapshot_id: str
    vm_id: str
    user_id: str
    name: str
    description: str
    snapshot_type: SnapshotType
    state: SnapshotState
    created_at: float
    size_bytes: int
    compressed_size_bytes: int
    checksum: str
    storage_path: str
    parent_snapshot_id: Optional[str]
    version: int
    tags: List[str]
    vm_config: Dict[str, Any]
    is_encrypted: bool

class SnapshotManager:
    def __init__(self, vm_manager, storage_backend, encryption_service):
        self.vm_manager = vm_manager
        self.storage = storage_backend
        self.encryption = encryption_service
        self.snapshots: Dict[str, SnapshotMetadata] = {}
        self.active_operations: Dict[str, asyncio.Task] = {}
        self.max_snapshots_per_user = 50
        self.snapshot_timeout = 600  # 10 minutes
        
    async def initialize(self):
        """Initialize the snapshot manager"""
        # Load existing snapshots from database
        await self._load_existing_snapshots()
        
        # Clean up any stale snapshot operations
        await self._cleanup_stale_operations()
        
        logger.info("Snapshot manager initialized", 
                   total_snapshots=len(self.snapshots))
        
        # Log to Logfire
        logfire.info("Snapshot manager started",
                    snapshot_count=len(self.snapshots),
                    service="snapshot-manager")
    
    async def create_snapshot(self, vm_id: str, user_id: str, name: str,
                            description: str = "", snapshot_type: SnapshotType = SnapshotType.MANUAL,
                            tags: List[str] = None, encrypt: bool = True) -> str:
        """Create a new VM snapshot"""
        try:
            # Validate user quota
            await self._validate_user_quota(user_id)
            
            # Validate VM ownership
            vm = await self.vm_manager.get_vm(vm_id)
            if not vm or vm.user_id != user_id:
                raise PermissionError("VM not found or access denied")
            
            # Generate snapshot ID
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
                size_bytes=0,
                compressed_size_bytes=0,
                checksum="",
                storage_path="",
                parent_snapshot_id=None,
                version=await self._get_next_version(vm_id, user_id),
                tags=tags or [],
                vm_config=vm.get_config(),
                is_encrypted=encrypt
            )
            
            # Store metadata
            self.snapshots[snapshot_id] = metadata
            
            # Start snapshot creation task
            task = asyncio.create_task(
                self._create_snapshot_async(snapshot_id)
            )
            self.active_operations[snapshot_id] = task
            
            logger.info("Snapshot creation started", 
                       snapshot_id=snapshot_id,
                       vm_id=vm_id, user_id=user_id)
            
            # Log to Logfire with tracing
            with logfire.span("snapshot_creation_started") as span:
                span.set_attribute("snapshot_id", snapshot_id)
                span.set_attribute("vm_id", vm_id)
                span.set_attribute("user_id", user_id)
                span.set_attribute("snapshot_type", snapshot_type.value)
                
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
    
    async def _create_snapshot_async(self, snapshot_id: str):
        """Asynchronously create the snapshot"""
        metadata = self.snapshots[snapshot_id]
        
        try:
            # Pause VM if running
            vm_was_running = await self._pause_vm_if_running(metadata.vm_id)
            
            # Create Firecracker snapshot
            snapshot_data = await self._create_firecracker_snapshot(metadata.vm_id)
            
            # Calculate checksum
            checksum = hashlib.sha256(snapshot_data).hexdigest()
            
            # Encrypt if requested
            if metadata.is_encrypted:
                snapshot_data = await self.encryption.encrypt_data(
                    snapshot_data, f"snapshot:{snapshot_id}"
                )
            
            # Compress data
            compressed_data = await self._compress_snapshot_data(snapshot_data)
            
            # Store in backend
            storage_path = await self.storage.store_snapshot(
                snapshot_id, compressed_data
            )
            
            # Update metadata
            metadata.size_bytes = len(snapshot_data)
            metadata.compressed_size_bytes = len(compressed_data)
            metadata.checksum = checksum
            metadata.storage_path = storage_path
            metadata.state = SnapshotState.AVAILABLE
            
            # Resume VM if it was running
            if vm_was_running:
                await self._resume_vm(metadata.vm_id)
            
            # Persist metadata to database
            await self._persist_snapshot_metadata(metadata)
            
            logger.info("Snapshot created successfully", 
                       snapshot_id=snapshot_id,
                       size_bytes=metadata.size_bytes,
                       compressed_size=metadata.compressed_size_bytes,
                       compression_ratio=metadata.compressed_size_bytes / metadata.size_bytes)
            
            # Log completion to Logfire
            logfire.info("Snapshot creation completed",
                        snapshot_id=snapshot_id,
                        size_bytes=metadata.size_bytes,
                        compressed_size_bytes=metadata.compressed_size_bytes,
                        compression_ratio=metadata.compressed_size_bytes / metadata.size_bytes,
                        duration_seconds=time.time() - metadata.created_at)
            
        except Exception as e:
            # Mark as error
            metadata.state = SnapshotState.ERROR
            
            logger.error("Snapshot creation failed", 
                        snapshot_id=snapshot_id, error=str(e))
            logfire.error("Snapshot creation error",
                         snapshot_id=snapshot_id, error=str(e))
            
            # Try to resume VM if it was paused
            try:
                await self._resume_vm(metadata.vm_id)
            except Exception:
                pass  # Best effort
                
        finally:
            # Clean up active operation
            self.active_operations.pop(snapshot_id, None)
    
    async def restore_snapshot(self, snapshot_id: str, user_id: str, 
                             target_vm_id: Optional[str] = None) -> str:
        """Restore a snapshot to a VM"""
        try:
            # Validate snapshot ownership
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
            
            # Update state
            metadata.state = SnapshotState.RESTORING
            
            # Start restore task
            restore_task = asyncio.create_task(
                self._restore_snapshot_async(snapshot_id, target_vm_id)
            )
            self.active_operations[f"restore_{snapshot_id}"] = restore_task
            
            logger.info("Snapshot restore started", 
                       snapshot_id=snapshot_id,
                       target_vm_id=target_vm_id, user_id=user_id)
            
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
```

## TDD Implementation Cycle

### Test-Driven Development Process

1. **Red Phase**: Write failing snapshot tests
   ```bash
   # Create snapshot test file
   touch snapshot-manager/tests/test_snapshot_manager.py
   
   # Run failing test
   pytest snapshot-manager/tests/test_snapshot_manager.py::test_create_snapshot -v
   ```

2. **Green Phase**: Implement minimal snapshot functionality
   ```bash
   # Implement basic snapshot creation
   pytest snapshot-manager/tests/test_snapshot_manager.py::test_create_snapshot -v
   ```

3. **Refactor Phase**: Optimize snapshot operations
   ```bash
   # Add compression and encryption
   pytest snapshot-manager/tests/ -v
   ```

4. **Commit**: Commit snapshot functionality
   ```bash
   git add snapshot-manager/core/ snapshot-manager/tests/test_snapshot_manager.py
   git commit -m "feat: implement VM snapshot manager with Firecracker integration
   
   - Add SnapshotManager class with create/restore operations
   - Implement Firecracker snapshot capture with VM pause/resume
   - Add snapshot metadata management with versioning
   - Include compression and optional encryption for storage efficiency
   - Integrate with Logfire for snapshot operation monitoring
   
   Tests: Added comprehensive test suite for snapshot lifecycle
   Security: User ownership validation and encrypted storage support
   Integration: Connected to VM manager and storage backend"
   ```

### Core Test Cases

```python
# snapshot-manager/tests/test_snapshot_manager.py
import pytest
import asyncio
from unittest.mock import AsyncMock, MagicMock
from snapshot_manager.core.snapshot_manager import SnapshotManager, SnapshotState, SnapshotType

@pytest.fixture
async def vm_manager_mock():
    """Mock VM manager"""
    vm_manager = AsyncMock()
    vm_mock = MagicMock()
    vm_mock.user_id = "user123"
    vm_mock.get_config.return_value = {"cpu": 1, "memory": 512}
    vm_manager.get_vm = AsyncMock(return_value=vm_mock)
    return vm_manager

@pytest.fixture
async def storage_mock():
    """Mock storage backend"""
    storage = AsyncMock()
    storage.store_snapshot = AsyncMock(return_value="s3://bucket/snapshot123")
    storage.retrieve_snapshot = AsyncMock(return_value=b"snapshot_data")
    return storage

@pytest.fixture
async def encryption_mock():
    """Mock encryption service"""
    encryption = AsyncMock()
    encryption.encrypt_data = AsyncMock(return_value=b"encrypted_data")
    encryption.decrypt_data = AsyncMock(return_value=b"decrypted_data")
    return encryption

@pytest.fixture
async def snapshot_manager(vm_manager_mock, storage_mock, encryption_mock):
    """SnapshotManager instance with mocked dependencies"""
    manager = SnapshotManager(vm_manager_mock, storage_mock, encryption_mock)
    await manager.initialize()
    return manager

class TestSnapshotManager:
    async def test_create_snapshot_success(self, snapshot_manager):
        """Test successful snapshot creation"""
        # Arrange
        vm_id = "vm123"
        user_id = "user123"
        name = "test-snapshot"
        
        # Act
        snapshot_id = await snapshot_manager.create_snapshot(
            vm_id, user_id, name
        )
        
        # Assert
        assert snapshot_id is not None
        assert snapshot_id in snapshot_manager.snapshots
        snapshot = snapshot_manager.snapshots[snapshot_id]
        assert snapshot.vm_id == vm_id
        assert snapshot.user_id == user_id
        assert snapshot.name == name
        assert snapshot.state == SnapshotState.CREATING
    
    async def test_create_snapshot_unauthorized_vm(self, snapshot_manager):
        """Test snapshot creation with unauthorized VM"""
        # Arrange
        vm_id = "vm123"
        user_id = "different_user"
        name = "test-snapshot"
        
        # Mock VM with different owner
        vm_mock = MagicMock()
        vm_mock.user_id = "other_user"
        snapshot_manager.vm_manager.get_vm.return_value = vm_mock
        
        # Act & Assert
        with pytest.raises(PermissionError, match="access denied"):
            await snapshot_manager.create_snapshot(vm_id, user_id, name)
    
    async def test_restore_snapshot_success(self, snapshot_manager):
        """Test successful snapshot restoration"""
        # Arrange
        # First create a snapshot
        vm_id = "vm123"
        user_id = "user123"
        snapshot_id = await snapshot_manager.create_snapshot(
            vm_id, user_id, "test-snapshot"
        )
        
        # Mark as available for restore
        snapshot_manager.snapshots[snapshot_id].state = SnapshotState.AVAILABLE
        
        # Act
        restored_vm_id = await snapshot_manager.restore_snapshot(
            snapshot_id, user_id
        )
        
        # Assert
        assert restored_vm_id == vm_id
        snapshot = snapshot_manager.snapshots[snapshot_id]
        assert snapshot.state == SnapshotState.RESTORING
```

## Security Checklist for Snapshot Management

### Snapshot Access Control
- [ ] Snapshot ownership validation on all operations
- [ ] Cross-user snapshot access prevention with strict authorization
- [ ] VM ownership verification before snapshot creation
- [ ] Target VM ownership validation during restoration
- [ ] Snapshot enumeration protection with user-scoped queries
- [ ] Rate limiting on snapshot operations (5 snapshots per user per hour)
- [ ] Snapshot quota enforcement per user (50 snapshots maximum)
- [ ] Audit logging for all snapshot lifecycle operations
- [ ] Administrative snapshot access controls with elevated permissions
- [ ] Snapshot sharing permission controls for collaborative scenarios

### Data Protection
- [ ] Optional snapshot encryption with user-controlled keys
- [ ] Snapshot data integrity verification with checksums
- [ ] Secure snapshot storage with access controls
- [ ] Snapshot metadata protection in database
- [ ] Sensitive data filtering from snapshot logs
- [ ] Secure deletion of snapshot data with proper cleanup
- [ ] Backup encryption for snapshot archives
- [ ] Protection against snapshot data tampering
- [ ] Encryption key management with rotation policies
- [ ] Data retention policy enforcement with automatic cleanup

### VM Security During Snapshots
- [ ] Secure VM pause/resume operations during snapshot creation
- [ ] Memory content protection during snapshot capture
- [ ] Process isolation during snapshot operations
- [ ] Secure VM state preservation with integrity checks
- [ ] Protection against VM state corruption during snapshots
- [ ] Secure temporary file handling during snapshot processing
- [ ] VM configuration validation during restore operations
- [ ] Protection against malicious snapshot injection
- [ ] VM security context preservation across snapshot operations
- [ ] Secure VM migration with snapshot-based restoration

## Performance Requirements

### Snapshot Operations
- Snapshot creation time < 60 seconds for 1GB VM
- Snapshot restoration time < 30 seconds
- Compression ratio > 50% for typical VM states
- Concurrent snapshot operations (5 per user max)
- Memory usage < 200MB during snapshot operations
- CPU usage optimization during compression/encryption

### Storage Performance
- Snapshot upload speed > 50 MB/s to storage backend
- Snapshot download speed > 100 MB/s from storage
- Storage space efficiency with deduplication
- Metadata query response time < 100ms
- Snapshot listing performance < 500ms for 1000 snapshots
- Storage quota checking < 50ms per operation

## Integration Testing

### VM Manager Integration
```python
async def test_vm_manager_integration(snapshot_manager, vm_manager):
    """Test integration with VM manager"""
    vm_id = "vm123"
    user_id = "user123"
    
    # Create snapshot
    snapshot_id = await snapshot_manager.create_snapshot(
        vm_id, user_id, "integration-test"
    )
    
    # Verify VM manager interactions
    vm_manager.get_vm.assert_called_with(vm_id)
    # Additional VM manager method calls would be verified here
```

### Storage Backend Integration
```python
async def test_storage_integration(snapshot_manager, storage_backend):
    """Test integration with storage backend"""
    # Test snapshot storage operations
    pass
```

### Database Integration
```python
async def test_database_integration(snapshot_manager, db_session):
    """Test snapshot metadata persistence"""
    # Test database operations for snapshot metadata
    pass
```

## Error Handling and Recovery

### Snapshot Creation Failures
- VM pause/resume error handling with state recovery
- Storage upload failure with retry mechanisms
- Compression/encryption error handling
- Timeout handling for long-running operations
- Partial snapshot cleanup on failures
- User notification for failed operations

### Snapshot Restoration Failures
- VM state validation before restoration
- Storage download error handling with retries
- Decompression/decryption error handling
- VM configuration compatibility checks
- Rollback mechanisms for failed restorations
- Corruption detection and recovery procedures

## Firecracker Integration

### Snapshot Capture
```python
async def _create_firecracker_snapshot(self, vm_id: str) -> bytes:
    """Create Firecracker snapshot"""
    try:
        # Pause VM
        await self.vm_manager.firecracker_client.pause_vm(vm_id)
        
        # Create snapshot
        snapshot_data = await self.vm_manager.firecracker_client.create_snapshot(
            vm_id, include_memory=True
        )
        
        return snapshot_data
        
    except Exception as e:
        logger.error("Firecracker snapshot creation failed", 
                    vm_id=vm_id, error=str(e))
        raise
```

### Snapshot Restoration
```python
async def _restore_firecracker_snapshot(self, vm_id: str, snapshot_data: bytes):
    """Restore Firecracker snapshot"""
    try:
        # Stop current VM if running
        await self.vm_manager.firecracker_client.stop_vm(vm_id)
        
        # Restore from snapshot
        await self.vm_manager.firecracker_client.restore_snapshot(
            vm_id, snapshot_data
        )
        
        logger.info("Firecracker snapshot restored", vm_id=vm_id)
        
    except Exception as e:
        logger.error("Firecracker snapshot restoration failed", 
                    vm_id=vm_id, error=str(e))
        raise
```

## Next Implementation Steps

1. **Complete Firecracker integration** with snapshot capture and restoration
2. **Implement compression algorithms** for storage optimization
3. **Add encryption support** for sensitive VM data protection
4. **Create quota management** with user limits and enforcement
5. **Add versioning system** for snapshot history tracking
6. **Implement deduplication** for storage space optimization
7. **Create comprehensive test suite** with integration scenarios

## Commit Guidelines

Each commit should include:
- **Feature implementation** with comprehensive error handling
- **Security validation** for snapshot access and data protection
- **Test coverage** for snapshot operations (>80%)
- **Integration verification** with VM manager and storage
- **Performance optimization** for snapshot operations
- **Documentation updates** for API changes and procedures