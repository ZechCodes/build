"""Unit tests for authentication."""

import pytest
from httpx import AsyncClient

from app.services.auth import AuthService
from app.core.security import verify_password, get_password_hash, create_access_token, verify_token
from app.schemas.auth import UserCreate


class TestAuthService:
    """Test authentication service."""

    @pytest.mark.asyncio
    async def test_create_user(self, db_session, test_user_data):
        """Test user creation."""
        user_create = UserCreate(**test_user_data)
        user = await AuthService.create_user(db_session, user_create)
        
        assert user.email == test_user_data["email"]
        assert user.username == test_user_data["username"]
        assert user.is_active is True
        assert verify_password(test_user_data["password"], user.password_hash)

    @pytest.mark.asyncio
    async def test_authenticate_user_success(self, db_session, test_user_data):
        """Test successful user authentication."""
        # Create user first
        user_create = UserCreate(**test_user_data)
        await AuthService.create_user(db_session, user_create)
        
        # Authenticate
        user = await AuthService.authenticate_user(
            db_session, 
            test_user_data["email"], 
            test_user_data["password"]
        )
        
        assert user is not None
        assert user.email == test_user_data["email"]

    @pytest.mark.asyncio
    async def test_authenticate_user_invalid_password(self, db_session, test_user_data):
        """Test authentication with invalid password."""
        # Create user first
        user_create = UserCreate(**test_user_data)
        await AuthService.create_user(db_session, user_create)
        
        # Try to authenticate with wrong password
        user = await AuthService.authenticate_user(
            db_session, 
            test_user_data["email"], 
            "wrongpassword"
        )
        
        assert user is None

    @pytest.mark.asyncio
    async def test_authenticate_user_not_found(self, db_session):
        """Test authentication with non-existent user."""
        user = await AuthService.authenticate_user(
            db_session, 
            "nonexistent@example.com", 
            "password"
        )
        
        assert user is None


class TestAuthEndpoints:
    """Test authentication endpoints."""

    @pytest.mark.asyncio
    async def test_register_user(self, client: AsyncClient, test_user_data):
        """Test user registration endpoint."""
        response = await client.post("/api/v1/auth/register", json=test_user_data)
        
        assert response.status_code == 201
        data = response.json()
        assert data["email"] == test_user_data["email"]
        assert data["username"] == test_user_data["username"]
        assert "id" in data

    @pytest.mark.asyncio
    async def test_login_success(self, client: AsyncClient, test_user_data):
        """Test successful login."""
        # Register user first
        await client.post("/api/v1/auth/register", json=test_user_data)
        
        # Login
        login_data = {
            "email": test_user_data["email"],
            "password": test_user_data["password"]
        }
        response = await client.post("/api/v1/auth/login", json=login_data)
        
        assert response.status_code == 200
        data = response.json()
        assert "access_token" in data
        assert "refresh_token" in data
        assert data["token_type"] == "bearer"

    @pytest.mark.asyncio
    async def test_login_invalid_credentials(self, client: AsyncClient, test_user_data):
        """Test login with invalid credentials."""
        # Register user first
        await client.post("/api/v1/auth/register", json=test_user_data)
        
        # Try login with wrong password
        login_data = {
            "email": test_user_data["email"],
            "password": "wrongpassword"
        }
        response = await client.post("/api/v1/auth/login", json=login_data)
        
        assert response.status_code == 401
        data = response.json()
        assert "detail" in data


class TestSecurityUtils:
    """Test security utility functions."""

    def test_password_hashing(self):
        """Test password hashing and verification."""
        password = "testpassword123"
        hashed = get_password_hash(password)
        
        assert hashed != password
        assert verify_password(password, hashed) is True
        assert verify_password("wrongpassword", hashed) is False

    def test_jwt_token_creation_and_verification(self):
        """Test JWT token creation and verification."""
        user_id = "123"
        token = create_access_token(subject=user_id)
        
        assert token is not None
        verified_user_id = verify_token(token)
        assert verified_user_id == user_id

    def test_jwt_token_invalid(self):
        """Test invalid JWT token verification."""
        invalid_token = "invalid.token.here"
        verified_user_id = verify_token(invalid_token)
        assert verified_user_id is None