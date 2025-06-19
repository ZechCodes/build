"""Test account lockout protection according to Session 2 requirements."""

import pytest
from datetime import datetime, timedelta, timezone
from unittest.mock import AsyncMock

from app.models.user import User
from app.security.lockout import AccountLockoutManager


class TestAccountLockoutManager:
    """Test account lockout protection functionality."""

    @pytest.fixture
    def lockout_manager(self):
        """Create account lockout manager with Session 2 settings."""
        # Session 2 requirements: 5 failed attempts = 30 minute lockout
        return AccountLockoutManager(max_attempts=5, lockout_duration_minutes=30)

    @pytest.fixture
    def test_user(self):
        """Create test user for lockout testing."""
        return User(
            email="test@example.com",
            username="testuser", 
            password_hash="hashed_password",
            failed_login_attempts=0,
            locked_until=None
        )

    @pytest.fixture
    def mock_db(self):
        """Create mock database session."""
        mock = AsyncMock()
        mock.commit = AsyncMock()
        return mock

    @pytest.mark.asyncio
    async def test_lockout_manager_initialization(self, lockout_manager):
        """Test lockout manager initialization."""
        assert lockout_manager.max_attempts == 5
        assert lockout_manager.lockout_duration == timedelta(minutes=30)

    @pytest.mark.asyncio
    async def test_record_failed_attempt_first_failure(self, lockout_manager, test_user, mock_db):
        """Test recording first failed attempt."""
        assert test_user.failed_login_attempts == 0
        assert test_user.locked_until is None

        await lockout_manager.record_failed_attempt(test_user, mock_db)

        assert test_user.failed_login_attempts == 1
        assert test_user.locked_until is None  # Not locked yet
        mock_db.commit.assert_called_once()

    @pytest.mark.asyncio
    async def test_record_failed_attempt_multiple_failures(self, lockout_manager, test_user, mock_db):
        """Test recording multiple failed attempts below threshold."""
        # Record 4 failed attempts (below 5 threshold)
        for i in range(4):
            await lockout_manager.record_failed_attempt(test_user, mock_db)

        assert test_user.failed_login_attempts == 4
        assert test_user.locked_until is None  # Still not locked
        assert mock_db.commit.call_count == 4

    @pytest.mark.asyncio
    async def test_record_failed_attempt_triggers_lockout(self, lockout_manager, test_user, mock_db):
        """Test that 5th failed attempt triggers lockout."""
        # Record 4 failed attempts first
        test_user.failed_login_attempts = 4

        # 5th attempt should trigger lockout
        await lockout_manager.record_failed_attempt(test_user, mock_db)

        assert test_user.failed_login_attempts == 5
        assert test_user.locked_until is not None
        
        # Check lockout duration is approximately 30 minutes
        expected_unlock = datetime.now(timezone.utc) + timedelta(minutes=30)
        time_diff = abs((test_user.locked_until - expected_unlock).total_seconds())
        assert time_diff < 60  # Within 1 minute

    @pytest.mark.asyncio
    async def test_record_failed_attempt_beyond_lockout(self, lockout_manager, test_user, mock_db):
        """Test recording failed attempts beyond lockout threshold."""
        # Set user to already locked state
        test_user.failed_login_attempts = 7
        test_user.locked_until = datetime.now(timezone.utc) + timedelta(minutes=30)

        # Additional failed attempt
        await lockout_manager.record_failed_attempt(test_user, mock_db)

        assert test_user.failed_login_attempts == 8
        # Lockout time should be updated
        assert test_user.locked_until is not None

    @pytest.mark.asyncio
    async def test_reset_failed_attempts(self, lockout_manager, test_user, mock_db):
        """Test resetting failed login attempts."""
        # Set user with failed attempts and lockout
        test_user.failed_login_attempts = 5
        test_user.locked_until = datetime.now(timezone.utc) + timedelta(minutes=30)

        await lockout_manager.reset_failed_attempts(test_user, mock_db)

        assert test_user.failed_login_attempts == 0
        assert test_user.locked_until is None
        mock_db.commit.assert_called_once()

    @pytest.mark.asyncio
    async def test_is_locked_not_locked(self, lockout_manager, test_user):
        """Test is_locked check for non-locked user."""
        assert not lockout_manager.is_locked(test_user)

    @pytest.mark.asyncio
    async def test_is_locked_currently_locked(self, lockout_manager, test_user):
        """Test is_locked check for currently locked user."""
        # Set lockout time in the future
        test_user.locked_until = datetime.now(timezone.utc) + timedelta(minutes=15)
        
        assert lockout_manager.is_locked(test_user)

    @pytest.mark.asyncio
    async def test_is_locked_lockout_expired(self, lockout_manager, test_user):
        """Test is_locked check for user with expired lockout."""
        # Set lockout time in the past
        test_user.locked_until = datetime.now(timezone.utc) - timedelta(minutes=15)
        
        assert not lockout_manager.is_locked(test_user)

    @pytest.mark.asyncio
    async def test_is_locked_no_lockout_time(self, lockout_manager, test_user):
        """Test is_locked check for user with no lockout time."""
        test_user.locked_until = None
        
        assert not lockout_manager.is_locked(test_user)

    @pytest.mark.asyncio
    async def test_lockout_duration_configuration(self):
        """Test different lockout duration configurations."""
        # 15 minute lockout
        manager_15 = AccountLockoutManager(max_attempts=3, lockout_duration_minutes=15)
        assert manager_15.lockout_duration == timedelta(minutes=15)
        
        # 60 minute lockout
        manager_60 = AccountLockoutManager(max_attempts=5, lockout_duration_minutes=60)
        assert manager_60.lockout_duration == timedelta(minutes=60)

    @pytest.mark.asyncio
    async def test_different_max_attempts(self):
        """Test different max attempts configurations."""
        # 3 attempts before lockout
        manager_3 = AccountLockoutManager(max_attempts=3, lockout_duration_minutes=30)
        assert manager_3.max_attempts == 3
        
        # 10 attempts before lockout
        manager_10 = AccountLockoutManager(max_attempts=10, lockout_duration_minutes=30)
        assert manager_10.max_attempts == 10

    @pytest.mark.asyncio
    async def test_get_lockout_remaining_time(self, lockout_manager, test_user):
        """Test getting remaining lockout time."""
        # Set lockout time 15 minutes in the future
        lockout_time = datetime.now(timezone.utc) + timedelta(minutes=15)
        test_user.locked_until = lockout_time

        remaining = lockout_manager.get_lockout_remaining_time(test_user)
        
        assert remaining is not None
        # Should be approximately 15 minutes (within 1 minute tolerance)
        assert abs(remaining.total_seconds() - 900) < 60

    @pytest.mark.asyncio
    async def test_get_lockout_remaining_time_expired(self, lockout_manager, test_user):
        """Test getting remaining time for expired lockout."""
        # Set lockout time in the past
        test_user.locked_until = datetime.now(timezone.utc) - timedelta(minutes=15)

        remaining = lockout_manager.get_lockout_remaining_time(test_user)
        
        assert remaining is None or remaining.total_seconds() <= 0

    @pytest.mark.asyncio
    async def test_get_lockout_remaining_time_no_lockout(self, lockout_manager, test_user):
        """Test getting remaining time when not locked."""
        test_user.locked_until = None

        remaining = lockout_manager.get_lockout_remaining_time(test_user)
        
        assert remaining is None

    @pytest.mark.asyncio
    async def test_can_attempt_login(self, lockout_manager, test_user):
        """Test checking if user can attempt login."""
        # User not locked
        assert lockout_manager.can_attempt_login(test_user)

        # User locked
        test_user.locked_until = datetime.now(timezone.utc) + timedelta(minutes=15)
        assert not lockout_manager.can_attempt_login(test_user)

        # Lockout expired
        test_user.locked_until = datetime.now(timezone.utc) - timedelta(minutes=15)
        assert lockout_manager.can_attempt_login(test_user)

    @pytest.mark.asyncio
    async def test_auto_unlock_expired_lockout(self, lockout_manager, test_user, mock_db):
        """Test automatic unlock for expired lockouts."""
        # Set expired lockout
        test_user.failed_login_attempts = 5
        test_user.locked_until = datetime.now(timezone.utc) - timedelta(minutes=15)

        # Auto-unlock should reset the lockout
        unlocked = await lockout_manager.auto_unlock_expired(test_user, mock_db)

        assert unlocked is True
        assert test_user.failed_login_attempts == 0
        assert test_user.locked_until is None

    @pytest.mark.asyncio
    async def test_auto_unlock_still_locked(self, lockout_manager, test_user, mock_db):
        """Test auto-unlock when lockout is still active."""
        # Set active lockout
        test_user.failed_login_attempts = 5
        test_user.locked_until = datetime.now(timezone.utc) + timedelta(minutes=15)

        # Should not unlock
        unlocked = await lockout_manager.auto_unlock_expired(test_user, mock_db)

        assert unlocked is False
        assert test_user.failed_login_attempts == 5
        assert test_user.locked_until is not None