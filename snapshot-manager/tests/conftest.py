"""
Test configuration and shared fixtures for snapshot manager tests.
"""

import sys
import os
from pathlib import Path

# Add the parent directory to Python path so we can import snapshot_manager modules
snapshot_manager_dir = Path(__file__).parent.parent
if str(snapshot_manager_dir) not in sys.path:
    sys.path.insert(0, str(snapshot_manager_dir))

# Also add the project root for cross-service imports
project_root = snapshot_manager_dir.parent
if str(project_root) not in sys.path:
    sys.path.insert(0, str(project_root))

import pytest
import asyncio
import structlog
import logfire
from unittest.mock import AsyncMock, MagicMock


# Configure test logging
structlog.configure(
    processors=[
        structlog.stdlib.filter_by_level,
        structlog.stdlib.add_logger_name,
        structlog.stdlib.add_log_level,
        structlog.stdlib.PositionalArgumentsFormatter(),
        structlog.processors.TimeStamper(fmt="iso"),
        structlog.dev.ConsoleRenderer()
    ],
    context_class=dict,
    logger_factory=structlog.stdlib.LoggerFactory(),
    wrapper_class=structlog.stdlib.BoundLogger,
    cache_logger_on_first_use=True,
)


# Mock logfire for testing
class MockLogfire:
    def info(self, *args, **kwargs):
        pass
    
    def error(self, *args, **kwargs):
        pass
    
    def span(self, name):
        return MockSpan()


class MockSpan:
    def __enter__(self):
        return self
    
    def __exit__(self, *args):
        pass
    
    def set_attribute(self, key, value):
        pass


# Replace logfire with mock for testing
logfire.info = MockLogfire().info
logfire.error = MockLogfire().error
logfire.span = MockLogfire().span


@pytest.fixture(scope="session")
def event_loop():
    """Create an instance of the default event loop for the test session."""
    policy = asyncio.get_event_loop_policy()
    loop = policy.new_event_loop()
    asyncio.set_event_loop(loop)
    yield loop
    loop.close()


@pytest.fixture
def mock_vm():
    """Mock VM object."""
    vm = MagicMock()
    vm.id = "vm123"
    vm.user_id = "user123"
    vm.get_config.return_value = {"cpu": 1, "memory": 512, "disk": "1GB"}
    return vm


@pytest.fixture
def mock_database():
    """Mock database for testing."""
    db = AsyncMock()
    db.save_snapshot_metadata = AsyncMock()
    db.get_snapshot_metadata = AsyncMock()
    db.delete_snapshot_metadata = AsyncMock()
    db.list_user_snapshots = AsyncMock(return_value=[])
    db.count_user_snapshots = AsyncMock(return_value=0)
    db.get_user_storage_usage = AsyncMock(return_value=0)
    return db


@pytest.fixture
def mock_storage_backend():
    """Mock storage backend for testing."""
    storage = AsyncMock()
    storage.store_snapshot = AsyncMock(return_value="test-storage-path")
    storage.retrieve_snapshot = AsyncMock(return_value=b"test-snapshot-data")
    storage.delete_snapshot = AsyncMock(return_value=True)
    storage.initialize = AsyncMock()
    return storage


@pytest.fixture
def mock_vm_manager():
    """Mock VM manager for testing."""
    vm_manager = AsyncMock()
    vm_manager.get_vm = AsyncMock()
    vm_manager.create_firecracker_snapshot = AsyncMock(return_value=b"mock_vm_data")
    vm_manager.restore_vm_from_snapshot = AsyncMock(return_value=True)
    vm_manager.verify_vm_ownership = AsyncMock(return_value=True)
    vm_manager.is_vm_running = AsyncMock(return_value=True)
    vm_manager.pause_vm = AsyncMock()
    vm_manager.resume_vm = AsyncMock()
    vm_manager.stop_vm = AsyncMock()
    vm_manager.start_vm = AsyncMock()
    vm_manager.get_vm_config = AsyncMock(return_value={"cpu": 1, "memory": 512})
    return vm_manager


@pytest.fixture
def mock_encryption_service():
    """Mock encryption service for testing."""
    encryption = AsyncMock()
    encryption.encrypt_data = AsyncMock(return_value=b"encrypted_data")
    encryption.decrypt_data = AsyncMock(return_value=b"decrypted_data")
    return encryption


@pytest.fixture
def sample_snapshot_metadata():
    """Sample snapshot metadata for testing."""
    from core.snapshot_manager import SnapshotMetadata, SnapshotState, SnapshotType
    return SnapshotMetadata(
        id="snap_test123",
        vm_id="vm_test123",
        user_id="user_test123",
        name="Test Snapshot",
        description="Test snapshot for unit tests",
        snapshot_type=SnapshotType.MANUAL,
        state=SnapshotState.AVAILABLE,
        size_bytes=1024 * 1024,  # 1MB
        compressed_size_bytes=512 * 1024,  # 512KB
        checksum_sha256="abc123def456",
        storage_path="s3://test-bucket/test-snapshot",
        created_at=1234567890.0,
        updated_at=1234567890.0,
        tags=["test", "unit"],
        vm_config={"cpu": 1, "memory": 512},
        restore_count=0,
        version=1
    )


# Test environment configuration
TEST_DATABASE_URL = "postgresql+asyncpg://postgres:dev_password@localhost:5434/build_dev"
TEST_MINIO_CONFIG = {
    "endpoint_url": "http://localhost:9000",
    "access_key": "minioadmin",
    "secret_key": "minioadmin123",
    "bucket_name": "snapshots-test",
    "enable_encryption": False  # Disable encryption for local testing
}

# Suppress logfire configuration warnings in tests
import os
os.environ.setdefault('LOGFIRE_IGNORE_NO_CONFIG', '1')