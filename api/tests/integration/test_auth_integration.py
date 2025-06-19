"""Integration tests for authentication system."""

import pytest
from httpx import AsyncClient
from sqlalchemy.ext.asyncio import AsyncSession

from app.main import app
from app.core.database import get_db
from app.services.auth import AuthService
from app.schemas.auth import UserCreate


class TestAuthIntegration:
    """Test authentication integration."""
    
    @pytest.mark.asyncio
    async def test_user_registration_flow(self, client: AsyncClient, db_session: AsyncSession):
        """Test complete user registration flow."""
        user_data = {
            "email": "test@example.com",
            "username": "testuser",
            "password": "TestPassword123!"
        }
        
        # Register user
        response = await client.post("/api/v1/auth/register", json=user_data)
        assert response.status_code == 201
        
        user_response = response.json()
        assert user_response["email"] == user_data["email"]
        assert user_response["username"] == user_data["username"]
        assert user_response["is_active"] is True
        assert user_response["is_verified"] is False
        assert "id" in user_response
    
    @pytest.mark.asyncio
    async def test_user_login_flow(self, client: AsyncClient, db_session: AsyncSession):
        """Test complete user login flow."""
        # Create user first
        user_create = UserCreate(
            email="login@example.com",
            username="loginuser",
            password="LoginPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login
        login_data = {
            "email": "login@example.com",
            "password": "LoginPassword123!"
        }
        
        response = await client.post("/api/v1/auth/login", json=login_data)
        assert response.status_code == 200
        
        login_response = response.json()
        assert "access_token" in login_response
        assert "refresh_token" in login_response
        assert login_response["token_type"] == "bearer"
        assert login_response["user"]["email"] == user.email
    
    @pytest.mark.asyncio
    async def test_token_refresh_flow(self, client: AsyncClient, db_session: AsyncSession):
        """Test token refresh flow."""
        # Create user and login
        user_create = UserCreate(
            email="refresh@example.com",
            username="refreshuser",
            password="RefreshPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        login_data = {
            "email": "refresh@example.com",
            "password": "RefreshPassword123!"
        }
        
        login_response = await client.post("/api/v1/auth/login", json=login_data)
        assert login_response.status_code == 200
        
        refresh_token = login_response.json()["refresh_token"]
        
        # Refresh token
        refresh_data = {"refresh_token": refresh_token}
        response = await client.post("/api/v1/auth/refresh", json=refresh_data)
        assert response.status_code == 200
        
        refresh_response = response.json()
        assert "access_token" in refresh_response
        assert refresh_response["token_type"] == "bearer"
    
    @pytest.mark.asyncio
    async def test_logout_flow(self, client: AsyncClient, db_session: AsyncSession):
        """Test logout flow."""
        # Create user and login
        user_create = UserCreate(
            email="logout@example.com",
            username="logoutuser",
            password="LogoutPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        login_data = {
            "email": "logout@example.com",
            "password": "LogoutPassword123!"
        }
        
        login_response = await client.post("/api/v1/auth/login", json=login_data)
        assert login_response.status_code == 200
        
        refresh_token = login_response.json()["refresh_token"]
        
        # Logout
        logout_data = {"refresh_token": refresh_token}
        response = await client.post("/api/v1/auth/logout", json=logout_data)
        assert response.status_code == 200
        
        logout_response = response.json()
        assert "message" in logout_response
    
    @pytest.mark.asyncio
    async def test_invalid_login_attempts(self, client: AsyncClient, db_session: AsyncSession):
        """Test invalid login attempts and lockout mechanism."""
        # Create user
        user_create = UserCreate(
            email="lockout@example.com",
            username="lockoutuser",
            password="LockoutPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Make multiple failed login attempts
        login_data = {
            "email": "lockout@example.com",
            "password": "WrongPassword123!"
        }
        
        for i in range(5):
            response = await client.post("/api/v1/auth/login", json=login_data)
            assert response.status_code == 401
        
        # Next attempt should result in account lockout
        response = await client.post("/api/v1/auth/login", json=login_data)
        assert response.status_code == 429  # Too Many Requests
        
        # Even correct password should be locked
        correct_login_data = {
            "email": "lockout@example.com",
            "password": "LockoutPassword123!"
        }
        
        response = await client.post("/api/v1/auth/login", json=correct_login_data)
        assert response.status_code == 429
    
    @pytest.mark.asyncio
    async def test_password_validation(self, client: AsyncClient):
        """Test password validation during registration."""
        weak_passwords = [
            "short",  # Too short
            "alllowercase123!",  # No uppercase
            "ALLUPPERCASE123!",  # No lowercase
            "NoNumbers!",  # No numbers
            "NoSpecialChars123",  # No special characters
        ]
        
        for i, password in enumerate(weak_passwords):
            user_data = {
                "email": f"weak{i}@example.com",
                "username": f"weakuser{i}",
                "password": password
            }
            
            response = await client.post("/api/v1/auth/register", json=user_data)
            assert response.status_code == 422  # Validation Error
    
    @pytest.mark.asyncio
    async def test_duplicate_email_registration(self, client: AsyncClient, db_session: AsyncSession):
        """Test duplicate email registration prevention."""
        # Create first user
        user_create = UserCreate(
            email="duplicate@example.com",
            username="firstuser",
            password="FirstPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Try to register with same email
        user_data = {
            "email": "duplicate@example.com",
            "username": "seconduser",
            "password": "SecondPassword123!"
        }
        
        response = await client.post("/api/v1/auth/register", json=user_data)
        assert response.status_code == 400
        assert "already registered" in response.json()["detail"]
    
    @pytest.mark.asyncio
    async def test_invalid_token_refresh(self, client: AsyncClient):
        """Test refresh with invalid token."""
        invalid_tokens = [
            "invalid_token",
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.invalid.signature",
            "",  # Empty token
        ]
        
        for token in invalid_tokens:
            refresh_data = {"refresh_token": token}
            response = await client.post("/api/v1/auth/refresh", json=refresh_data)
            assert response.status_code == 401