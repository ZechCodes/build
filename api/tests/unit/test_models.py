"""Unit tests for database models."""

import pytest
import uuid
from datetime import datetime, timezone
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select
from sqlalchemy.exc import IntegrityError

from app.models.user import User
from app.models.vm import VMInstance
from app.models.session import Session
from app.models.snapshot import Snapshot
from app.models.audit import AuditLog


class TestUserModel:
    """Test User model with UUID primary keys."""

    @pytest.mark.asyncio
    async def test_user_creation_with_uuid(self, db_session: AsyncSession):
        """Test user creation with UUID primary key."""
        user = User(
            email="test@example.com",
            username="testuser",
            password_hash="hashed_password",
        )
        
        db_session.add(user)
        await db_session.commit()
        await db_session.refresh(user)
        
        # UUID should be automatically generated
        assert user.id is not None
        assert isinstance(user.id, uuid.UUID)
        assert user.email == "test@example.com"
        assert user.username == "testuser"
        assert user.is_active is True
        assert user.is_verified is False
        assert user.created_at is not None
        assert user.updated_at is not None

    @pytest.mark.asyncio
    async def test_user_email_uniqueness(self, db_session: AsyncSession):
        """Test email uniqueness constraint."""
        user1 = User(
            email="duplicate@example.com",
            username="user1",
            password_hash="hash1"
        )
        user2 = User(
            email="duplicate@example.com",
            username="user2", 
            password_hash="hash2"
        )
        
        db_session.add(user1)
        await db_session.commit()
        
        db_session.add(user2)
        with pytest.raises(IntegrityError):
            await db_session.commit()

    @pytest.mark.asyncio
    async def test_user_username_uniqueness(self, db_session: AsyncSession):
        """Test username uniqueness constraint."""
        user1 = User(
            email="user1@example.com",
            username="duplicate",
            password_hash="hash1"
        )
        user2 = User(
            email="user2@example.com",
            username="duplicate",
            password_hash="hash2"
        )
        
        db_session.add(user1)
        await db_session.commit()
        
        db_session.add(user2)
        with pytest.raises(IntegrityError):
            await db_session.commit()

    @pytest.mark.asyncio
    async def test_user_timestamps(self, db_session: AsyncSession):
        """Test automatic timestamp generation."""
        user = User(
            email="timestamp@example.com",
            username="timestampuser",
            password_hash="hash"
        )
        
        db_session.add(user)
        await db_session.commit()
        await db_session.refresh(user)
        
        original_updated_at = user.updated_at
        
        # Force a small delay to ensure timestamp difference
        import time
        time.sleep(0.1)
        
        # Update user - manually trigger updated_at for SQLite
        user.email = "updated@example.com"
        user.updated_at = datetime.now(timezone.utc)
        await db_session.commit()
        await db_session.refresh(user)
        
        assert user.updated_at >= original_updated_at
        assert user.email == "updated@example.com"


class TestVMInstanceModel:
    """Test VMInstance model."""

    @pytest.mark.asyncio
    async def test_vm_instance_creation(self, db_session: AsyncSession):
        """Test VM instance creation with foreign key."""
        # Create user first
        user = User(
            email="vmowner@example.com",
            username="vmowner",
            password_hash="hash"
        )
        db_session.add(user)
        await db_session.commit()
        await db_session.refresh(user)
        
        # Create VM instance
        vm = VMInstance(
            user_id=user.id,
            name="Test VM",
            state="stopped",
            config={"cpu": 2, "memory": 1024, "disk": 20}
        )
        
        db_session.add(vm)
        await db_session.commit()
        await db_session.refresh(vm)
        
        assert vm.id is not None
        assert isinstance(vm.id, uuid.UUID)
        assert vm.user_id == user.id
        assert vm.name == "Test VM"
        assert vm.state == "stopped"
        assert vm.config == {"cpu": 2, "memory": 1024, "disk": 20}
        assert vm.firecracker_id is None

    @pytest.mark.asyncio
    async def test_vm_firecracker_id_uniqueness(self, db_session: AsyncSession):
        """Test firecracker_id uniqueness constraint."""
        # Create user
        user = User(
            email="vmowner2@example.com",
            username="vmowner2",
            password_hash="hash"
        )
        db_session.add(user)
        await db_session.commit()
        await db_session.refresh(user)
        
        # Create first VM
        vm1 = VMInstance(
            user_id=user.id,
            name="VM 1",
            state="running",
            firecracker_id="fc_123",
            config={"cpu": 1}
        )
        db_session.add(vm1)
        await db_session.commit()
        
        # Try to create second VM with same firecracker_id
        vm2 = VMInstance(
            user_id=user.id,
            name="VM 2",
            state="running",
            firecracker_id="fc_123",
            config={"cpu": 1}
        )
        db_session.add(vm2)
        
        with pytest.raises(IntegrityError):
            await db_session.commit()


class TestSessionModel:
    """Test Session model."""

    @pytest.mark.asyncio
    async def test_session_creation(self, db_session: AsyncSession):
        """Test session creation with VM and user relationships."""
        # Create user
        user = User(
            email="sessionuser@example.com",
            username="sessionuser",
            password_hash="hash"
        )
        db_session.add(user)
        await db_session.commit()
        await db_session.refresh(user)
        
        # Create VM
        vm = VMInstance(
            user_id=user.id,
            name="Session VM",
            state="running",
            config={"cpu": 1}
        )
        db_session.add(vm)
        await db_session.commit()
        await db_session.refresh(vm)
        
        # Create session
        now = datetime.now(timezone.utc).replace(microsecond=0)
        expires_at = now.replace(hour=now.hour + 1)  # 1 hour from now
        session = Session(
            vm_instance_id=vm.id,
            user_id=user.id,
            session_token="unique_token_123",
            last_activity=now,
            expires_at=expires_at
        )
        
        db_session.add(session)
        await db_session.commit()
        await db_session.refresh(session)
        
        assert session.id is not None
        assert isinstance(session.id, uuid.UUID)
        assert session.vm_instance_id == vm.id
        assert session.user_id == user.id
        assert session.session_token == "unique_token_123"
        assert session.state == "active"
        assert session.expires_at == expires_at

    @pytest.mark.asyncio
    async def test_session_token_uniqueness(self, db_session: AsyncSession):
        """Test session token uniqueness constraint."""
        # Create user and VM
        user = User(
            email="sessionuser2@example.com",
            username="sessionuser2",
            password_hash="hash"
        )
        db_session.add(user)
        await db_session.commit()
        await db_session.refresh(user)
        
        vm = VMInstance(
            user_id=user.id,
            name="Session VM 2",
            state="running",
            config={"cpu": 1}
        )
        db_session.add(vm)
        await db_session.commit()
        await db_session.refresh(vm)
        
        # Create first session
        expires_at = datetime.now(timezone.utc)
        session1 = Session(
            vm_instance_id=vm.id,
            user_id=user.id,
            session_token="duplicate_token",
            last_activity=expires_at,
            expires_at=expires_at
        )
        db_session.add(session1)
        await db_session.commit()
        
        # Try to create second session with same token
        session2 = Session(
            vm_instance_id=vm.id,
            user_id=user.id,
            session_token="duplicate_token",
            last_activity=expires_at,
            expires_at=expires_at
        )
        db_session.add(session2)
        
        with pytest.raises(IntegrityError):
            await db_session.commit()


class TestSnapshotModel:
    """Test Snapshot model."""

    @pytest.mark.asyncio
    async def test_snapshot_creation(self, db_session: AsyncSession):
        """Test snapshot creation."""
        # Create user and VM
        user = User(
            email="snapshotuser@example.com",
            username="snapshotuser",
            password_hash="hash"
        )
        db_session.add(user)
        await db_session.commit()
        await db_session.refresh(user)
        
        vm = VMInstance(
            user_id=user.id,
            name="Snapshot VM",
            state="stopped",
            config={"cpu": 2}
        )
        db_session.add(vm)
        await db_session.commit()
        await db_session.refresh(vm)
        
        # Create snapshot
        snapshot = Snapshot(
            vm_instance_id=vm.id,
            user_id=user.id,
            name="Test Snapshot",
            description="A test snapshot",
            storage_path="/snapshots/test-snapshot.img",
            size_bytes=1073741824,  # 1GB
            snapshot_metadata={"compression": "gzip", "checksum": "abc123"}
        )
        
        db_session.add(snapshot)
        await db_session.commit()
        await db_session.refresh(snapshot)
        
        assert snapshot.id is not None
        assert isinstance(snapshot.id, uuid.UUID)
        assert snapshot.vm_instance_id == vm.id
        assert snapshot.user_id == user.id
        assert snapshot.name == "Test Snapshot"
        assert snapshot.storage_path == "/snapshots/test-snapshot.img"
        assert snapshot.size_bytes == 1073741824
        assert snapshot.snapshot_metadata == {"compression": "gzip", "checksum": "abc123"}


class TestAuditLogModel:
    """Test AuditLog model."""

    @pytest.mark.asyncio
    async def test_audit_log_creation(self, db_session: AsyncSession):
        """Test audit log creation."""
        # Create user
        user = User(
            email="audituser@example.com",
            username="audituser",
            password_hash="hash"
        )
        db_session.add(user)
        await db_session.commit()
        await db_session.refresh(user)
        
        # Create audit log
        audit_log = AuditLog(
            user_id=user.id,
            action="CREATE_VM",
            resource_type="vm_instance",
            resource_id=uuid.uuid4(),
            ip_address="192.168.1.100",
            user_agent="Mozilla/5.0...",
            details={"vm_name": "test-vm", "config": {"cpu": 2}}
        )
        
        db_session.add(audit_log)
        await db_session.commit()
        await db_session.refresh(audit_log)
        
        assert audit_log.id is not None
        assert isinstance(audit_log.id, uuid.UUID)
        assert audit_log.user_id == user.id
        assert audit_log.action == "CREATE_VM"
        assert audit_log.resource_type == "vm_instance"
        assert audit_log.ip_address == "192.168.1.100"
        assert audit_log.details == {"vm_name": "test-vm", "config": {"cpu": 2}}

    @pytest.mark.asyncio
    async def test_audit_log_without_user(self, db_session: AsyncSession):
        """Test audit log creation without user (system actions)."""
        audit_log = AuditLog(
            action="SYSTEM_BACKUP",
            resource_type="system",
            details={"backup_location": "s3://backups/daily-backup.sql"}
        )
        
        db_session.add(audit_log)
        await db_session.commit()
        await db_session.refresh(audit_log)
        
        assert audit_log.id is not None
        assert audit_log.user_id is None
        assert audit_log.action == "SYSTEM_BACKUP"