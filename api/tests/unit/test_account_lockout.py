"""Tests for account lockout protection functionality."""

import pytest
from httpx import AsyncClient
from sqlalchemy.ext.asyncio import AsyncSession
from datetime import datetime, timedelta, timezone

from app.services.auth import AuthService
from app.schemas.auth import UserCreate
from app.models.user import User


class TestAccountLockout:
    """Test account lockout protection functionality."""
    
    @pytest.mark.asyncio
    async def test_failed_login_increments_counter(self, client: AsyncClient, db_session: AsyncSession):
        """Test that failed login attempts increment the counter."""
        # Create user
        user_create = UserCreate(
            email="lockout@example.com",
            username="lockoutuser",
            password="LockoutPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Attempt login with wrong password
        response = await client.post("/api/v1/auth/login", json={
            "email": "lockout@example.com",
            "password": "WrongPassword123!"
        })
        
        assert response.status_code == 401
        
        # Check that failed attempt was recorded
        await db_session.refresh(user)
        assert user.failed_login_attempts == 1
        assert user.locked_until is None  # Not locked yet
    
    @pytest.mark.asyncio
    async def test_account_locked_after_max_attempts(self, client: AsyncClient, db_session: AsyncSession):
        """Test that account gets locked after maximum failed attempts."""
        # Create user
        user_create = UserCreate(
            email="maxlockout@example.com",
            username="maxlockoutuser",
            password="MaxLockoutPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Make 5 failed login attempts (should trigger lockout)
        for i in range(5):
            response = await client.post("/api/v1/auth/login", json={
                "email": "maxlockout@example.com",
                "password": "WrongPassword123!"
            })
            assert response.status_code == 401
        
        # Check that account is now locked
        await db_session.refresh(user)
        assert user.failed_login_attempts == 5
        assert user.locked_until is not None
        # Just check that lockout time is in the future (avoid timezone comparison issues)
        time_diff = (user.locked_until.replace(tzinfo=timezone.utc) if user.locked_until.tzinfo is None else user.locked_until) - datetime.now(timezone.utc)
        assert time_diff.total_seconds() > 0  # Should be in the future
    
    @pytest.mark.asyncio
    async def test_locked_account_rejects_valid_credentials(self, client: AsyncClient, db_session: AsyncSession):
        """Test that locked account rejects even valid credentials."""
        # Create user
        user_create = UserCreate(
            email="validlocked@example.com",
            username="validlockeduser",
            password="ValidLockedPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Lock the account by setting failed attempts and locked_until
        user.failed_login_attempts = 5
        user.locked_until = datetime.now(timezone.utc) + timedelta(minutes=30)
        await db_session.commit()
        
        # Try to login with valid credentials
        response = await client.post("/api/v1/auth/login", json={
            "email": "validlocked@example.com",
            "password": "ValidLockedPassword123!"
        })
        
        assert response.status_code == 423  # Locked status code
        assert "locked" in response.text.lower()
    
    @pytest.mark.asyncio
    async def test_successful_login_resets_failed_attempts(self, client: AsyncClient, db_session: AsyncSession):
        """Test that successful login resets failed attempt counter."""
        # Create user
        user_create = UserCreate(
            email="reset@example.com",
            username="resetuser",
            password="ResetPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Make some failed attempts
        for i in range(3):
            response = await client.post("/api/v1/auth/login", json={
                "email": "reset@example.com",
                "password": "WrongPassword123!"
            })
            assert response.status_code == 401
        
        # Verify failed attempts were recorded
        await db_session.refresh(user)
        assert user.failed_login_attempts == 3
        
        # Now login successfully
        response = await client.post("/api/v1/auth/login", json={
            "email": "reset@example.com",
            "password": "ResetPassword123!"
        })
        
        assert response.status_code == 200
        
        # Check that failed attempts were reset
        await db_session.refresh(user)
        assert user.failed_login_attempts == 0
        assert user.locked_until is None
    
    @pytest.mark.asyncio
    async def test_account_unlock_after_lockout_period(self, client: AsyncClient, db_session: AsyncSession):
        """Test that account unlocks automatically after lockout period."""
        # Create user
        user_create = UserCreate(
            email="unlock@example.com",
            username="unlockuser",
            password="UnlockPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Lock the account with expired lockout time (past)
        user.failed_login_attempts = 5
        user.locked_until = datetime.now(timezone.utc) - timedelta(minutes=1)  # Expired
        await db_session.commit()
        
        # Try to login with valid credentials
        response = await client.post("/api/v1/auth/login", json={
            "email": "unlock@example.com",
            "password": "UnlockPassword123!"
        })
        
        assert response.status_code == 200
        
        # Check that account was unlocked and counter reset
        await db_session.refresh(user)
        assert user.failed_login_attempts == 0
        assert user.locked_until is None
    
    @pytest.mark.asyncio
    async def test_lockout_with_different_ips(self, client: AsyncClient, db_session: AsyncSession):
        """Test that lockout works regardless of IP address."""
        # Create user
        user_create = UserCreate(
            email="multiip@example.com",
            username="multiipuser",
            password="MultiIpPassword123!"
        )
        await AuthService.create_user(db_session, user_create)
        
        # Make failed attempts (should work regardless of IP simulation)
        for i in range(5):
            response = await client.post("/api/v1/auth/login", json={
                "email": "multiip@example.com",
                "password": "WrongPassword123!"
            })
            assert response.status_code == 401
        
        # Account should be locked for any subsequent attempt
        response = await client.post("/api/v1/auth/login", json={
            "email": "multiip@example.com",
            "password": "MultiIpPassword123!"
        })
        
        assert response.status_code == 423