"""Test enhanced User model with roles and security features."""

import pytest
from datetime import datetime, timezone
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.user import User, UserRole
from app.models.base import Base


class TestUserModelEnhanced:
    """Test enhanced user model functionality."""

    @pytest.mark.asyncio
    async def test_user_model_with_role(self, db_session: AsyncSession):
        """Test user creation with role."""
        user = User(
            email="test@example.com",
            username="testuser",
            password_hash="hashed_password",
            role=UserRole.USER
        )
        
        db_session.add(user)
        await db_session.commit()
        await db_session.refresh(user)
        
        assert user.role == UserRole.USER
        assert user.failed_login_attempts == 0
        assert user.locked_until is None
        assert user.is_active is True
        assert user.is_verified is False

    @pytest.mark.asyncio
    async def test_user_model_admin_role(self, db_session: AsyncSession):
        """Test user creation with admin role."""
        user = User(
            email="admin@example.com",
            username="admin",
            password_hash="hashed_password",
            role=UserRole.ADMIN
        )
        
        db_session.add(user)
        await db_session.commit()
        await db_session.refresh(user)
        
        assert user.role == UserRole.ADMIN

    @pytest.mark.asyncio
    async def test_user_failed_login_tracking(self, db_session: AsyncSession):
        """Test failed login attempt tracking."""
        user = User(
            email="test@example.com",
            username="testuser",
            password_hash="hashed_password",
            failed_login_attempts=3
        )
        
        db_session.add(user)
        await db_session.commit()
        await db_session.refresh(user)
        
        assert user.failed_login_attempts == 3

    @pytest.mark.asyncio
    async def test_user_lockout_functionality(self, db_session: AsyncSession):
        """Test user account lockout."""
        lockout_time = datetime.now(timezone.utc)
        user = User(
            email="test@example.com",
            username="testuser",
            password_hash="hashed_password",
            failed_login_attempts=5,
            locked_until=lockout_time
        )
        
        db_session.add(user)
        await db_session.commit()
        await db_session.refresh(user)
        
        # Compare datetime ignoring timezone (SQLite test db loses timezone)
        assert user.locked_until.replace(tzinfo=timezone.utc) == lockout_time
        assert user.failed_login_attempts == 5

    def test_user_role_enum_values(self):
        """Test UserRole enum values."""
        assert UserRole.USER.value == "user"
        assert UserRole.ADMIN.value == "admin"
        assert UserRole.MODERATOR.value == "moderator"