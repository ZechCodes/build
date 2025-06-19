"""Tests for password reset functionality."""

import pytest
from httpx import AsyncClient
from sqlalchemy.ext.asyncio import AsyncSession
from unittest.mock import patch
import uuid

from app.services.auth import AuthService
from app.schemas.auth import UserCreate
from app.models.user import User


class TestPasswordReset:
    """Test password reset functionality."""
    
    @pytest.mark.asyncio
    async def test_request_password_reset_existing_user(self, client: AsyncClient, db_session: AsyncSession):
        """Test requesting password reset for existing user."""
        # Create user
        user_create = UserCreate(
            email="reset@example.com",
            username="resetuser",
            password="ResetPassword123!"
        )
        await AuthService.create_user(db_session, user_create)
        
        # Request password reset
        response = await client.post("/api/v1/auth/request-password-reset", json={
            "email": "reset@example.com"
        })
        
        assert response.status_code == 200
        assert "reset link has been sent" in response.json()["message"].lower()
    
    @pytest.mark.asyncio
    async def test_request_password_reset_nonexistent_user(self, client: AsyncClient, db_session: AsyncSession):
        """Test requesting password reset for non-existent user returns same response."""
        # Request password reset for non-existent user
        response = await client.post("/api/v1/auth/request-password-reset", json={
            "email": "nonexistent@example.com"
        })
        
        # Should return same response to prevent email enumeration
        assert response.status_code == 200
        assert "reset link has been sent" in response.json()["message"].lower()
    
    @pytest.mark.asyncio
    async def test_request_password_reset_inactive_user(self, client: AsyncClient, db_session: AsyncSession):
        """Test requesting password reset for inactive user."""
        # Create user and deactivate
        user_create = UserCreate(
            email="inactive@example.com",
            username="inactiveuser",
            password="InactivePassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        user.is_active = False
        await db_session.commit()
        
        # Request password reset
        response = await client.post("/api/v1/auth/request-password-reset", json={
            "email": "inactive@example.com"
        })
        
        # Should return same response to prevent user enumeration
        assert response.status_code == 200
        assert "reset link has been sent" in response.json()["message"].lower()
    
    @pytest.mark.asyncio
    async def test_reset_password_with_valid_token(self, client: AsyncClient, db_session: AsyncSession):
        """Test resetting password with valid token."""
        # Create user
        user_create = UserCreate(
            email="validreset@example.com",
            username="validresetuser",
            password="ValidResetPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Mock Redis for both request and reset operations to use the same token
        test_token = "consistent_test_token"
        with patch('app.services.auth.get_redis') as mock_redis:
            mock_redis_instance = mock_redis.return_value.__aenter__.return_value
            mock_redis_instance.set.return_value = None  # For storing the token
            mock_redis_instance.get.return_value = test_token  # For retrieving the token
            mock_redis_instance.delete.return_value = None
            mock_redis_instance.keys.return_value = []
            
            # Request password reset
            await client.post("/api/v1/auth/request-password-reset", json={
                "email": "validreset@example.com"
            })
            
            # Reset password with the same token
            response = await client.post("/api/v1/auth/reset-password", json={
                "user_id": str(user.id),
                "reset_token": test_token,
                "new_password": "NewPassword123!"
            })
            
            assert response.status_code == 200
            assert "reset successfully" in response.json()["message"].lower()
    
    @pytest.mark.asyncio
    async def test_reset_password_with_invalid_token(self, client: AsyncClient, db_session: AsyncSession):
        """Test resetting password with invalid token."""
        # Create user
        user_create = UserCreate(
            email="invalidtoken@example.com",
            username="invalidtokenuser",
            password="InvalidTokenPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Try to reset with invalid token
        response = await client.post("/api/v1/auth/reset-password", json={
            "user_id": str(user.id),
            "reset_token": "invalid_token",
            "new_password": "NewPassword123!"
        })
        
        assert response.status_code == 503  # Service unavailable when Redis is down
        assert "temporarily unavailable" in response.text.lower()
    
    @pytest.mark.asyncio
    async def test_reset_password_with_weak_password(self, client: AsyncClient, db_session: AsyncSession):
        """Test resetting password with weak password."""
        # Create user
        user_create = UserCreate(
            email="weakpass@example.com",
            username="weakpassuser",
            password="WeakPassPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Mock valid reset token
        with patch('app.services.auth.get_redis') as mock_redis:
            mock_redis_instance = mock_redis.return_value.__aenter__.return_value
            mock_redis_instance.get.return_value = "valid_reset_token"
            mock_redis_instance.delete.return_value = None
            mock_redis_instance.keys.return_value = []
            
            # Try to reset with weak password
            response = await client.post("/api/v1/auth/reset-password", json={
                "user_id": str(user.id),
                "reset_token": "valid_reset_token",
                "new_password": "weak"  # Too weak
            })
            
            assert response.status_code == 422
            assert "password" in response.text.lower()
    
    @pytest.mark.asyncio
    async def test_reset_password_nonexistent_user(self, client: AsyncClient, db_session: AsyncSession):
        """Test resetting password for non-existent user."""
        fake_user_id = str(uuid.uuid4())
        
        # Try to reset password for non-existent user
        response = await client.post("/api/v1/auth/reset-password", json={
            "user_id": fake_user_id,
            "reset_token": "some_token",
            "new_password": "NewPassword123!"
        })
        
        assert response.status_code == 404
        assert "not found" in response.text.lower()
    
    @pytest.mark.asyncio
    async def test_password_reset_invalidates_existing_tokens(self, client: AsyncClient, db_session: AsyncSession):
        """Test that password reset invalidates existing refresh tokens."""
        # Create user and login to get tokens
        user_create = UserCreate(
            email="invalidate@example.com",
            username="invalidateuser",
            password="InvalidatePassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login to get refresh token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "invalidate@example.com",
            "password": "InvalidatePassword123!"
        })
        old_refresh_token = login_response.json()["refresh_token"]
        
        # Mock reset token and reset password
        with patch('app.services.auth.get_redis') as mock_redis:
            mock_redis_instance = mock_redis.return_value.__aenter__.return_value
            mock_redis_instance.get.return_value = "reset_token"
            mock_redis_instance.delete.return_value = None
            mock_redis_instance.keys.return_value = []
            
            # Reset password
            response = await client.post("/api/v1/auth/reset-password", json={
                "user_id": str(user.id),
                "reset_token": "reset_token",
                "new_password": "NewPassword456!"
            })
            
            assert response.status_code == 200
        
        # Try to use old refresh token (should fail)
        response = await client.post("/api/v1/auth/refresh", json={
            "refresh_token": old_refresh_token
        })
        
        assert response.status_code == 401
    
    @pytest.mark.asyncio
    async def test_login_with_new_password_after_reset(self, client: AsyncClient, db_session: AsyncSession):
        """Test that user can login with new password after reset."""
        # Create user
        user_create = UserCreate(
            email="newpass@example.com",
            username="newpassuser",
            password="OldPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Mock reset token and reset password
        with patch('app.services.auth.get_redis') as mock_redis:
            mock_redis_instance = mock_redis.return_value.__aenter__.return_value
            mock_redis_instance.get.return_value = "reset_token"
            mock_redis_instance.delete.return_value = None
            mock_redis_instance.keys.return_value = []
            
            # Reset password
            reset_response = await client.post("/api/v1/auth/reset-password", json={
                "user_id": str(user.id),
                "reset_token": "reset_token",
                "new_password": "NewPassword123!"
            })
            
            assert reset_response.status_code == 200
        
        # Try to login with old password (should fail)
        old_login_response = await client.post("/api/v1/auth/login", json={
            "email": "newpass@example.com",
            "password": "OldPassword123!"
        })
        assert old_login_response.status_code == 401
        
        # Login with new password (should succeed)
        new_login_response = await client.post("/api/v1/auth/login", json={
            "email": "newpass@example.com",
            "password": "NewPassword123!"
        })
        assert new_login_response.status_code == 200
        assert "access_token" in new_login_response.json()