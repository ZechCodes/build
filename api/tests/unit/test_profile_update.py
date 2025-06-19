"""Tests for user profile update functionality."""

import pytest
from httpx import AsyncClient
from sqlalchemy.ext.asyncio import AsyncSession

from app.services.auth import AuthService
from app.schemas.auth import UserCreate
from app.models.user import User


class TestProfileUpdate:
    """Test user profile update functionality."""
    
    @pytest.mark.asyncio
    async def test_update_profile_with_valid_data(self, client: AsyncClient, db_session: AsyncSession):
        """Test updating user profile with valid data."""
        # Create user
        user_create = UserCreate(
            email="update@example.com",
            username="updateuser",
            password="UpdatePassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "update@example.com",
            "password": "UpdatePassword123!"
        })
        token = login_response.json()["access_token"]
        
        # Update profile
        update_data = {
            "username": "newusername",
            "email": "newemail@example.com"
        }
        
        response = await client.put(
            "/api/v1/auth/me",
            json=update_data,
            headers={"Authorization": f"Bearer {token}"}
        )
        
        assert response.status_code == 200
        profile = response.json()
        assert profile["username"] == "newusername"
        assert profile["email"] == "newemail@example.com"
        assert profile["id"] == str(user.id)
    
    @pytest.mark.asyncio
    async def test_update_profile_with_invalid_username(self, client: AsyncClient, db_session: AsyncSession):
        """Test updating profile with invalid username format."""
        # Create user
        user_create = UserCreate(
            email="invalid@example.com",
            username="invaliduser",
            password="InvalidPassword123!"
        )
        await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "invalid@example.com",
            "password": "InvalidPassword123!"
        })
        token = login_response.json()["access_token"]
        
        # Try to update with invalid username
        update_data = {
            "username": "ab"  # Too short
        }
        
        response = await client.put(
            "/api/v1/auth/me",
            json=update_data,
            headers={"Authorization": f"Bearer {token}"}
        )
        
        assert response.status_code == 422
        assert "at least 3 characters" in response.text
    
    @pytest.mark.asyncio
    async def test_update_profile_with_duplicate_email(self, client: AsyncClient, db_session: AsyncSession):
        """Test updating profile with email that already exists."""
        # Create first user
        user_create1 = UserCreate(
            email="first@example.com",
            username="firstuser",
            password="FirstPassword123!"
        )
        await AuthService.create_user(db_session, user_create1)
        
        # Create second user
        user_create2 = UserCreate(
            email="second@example.com",
            username="seconduser",
            password="SecondPassword123!"
        )
        await AuthService.create_user(db_session, user_create2)
        
        # Login as second user
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "second@example.com",
            "password": "SecondPassword123!"
        })
        token = login_response.json()["access_token"]
        
        # Try to update with first user's email
        update_data = {
            "email": "first@example.com"
        }
        
        response = await client.put(
            "/api/v1/auth/me",
            json=update_data,
            headers={"Authorization": f"Bearer {token}"}
        )
        
        assert response.status_code == 409
        assert "already exists" in response.text.lower()
    
    @pytest.mark.asyncio
    async def test_update_profile_without_auth(self, client: AsyncClient):
        """Test updating profile without authentication."""
        update_data = {
            "username": "newusername"
        }
        
        response = await client.put("/api/v1/auth/me", json=update_data)
        
        assert response.status_code == 401
    
    @pytest.mark.asyncio
    async def test_update_profile_with_partial_data(self, client: AsyncClient, db_session: AsyncSession):
        """Test updating profile with only some fields."""
        # Create user
        user_create = UserCreate(
            email="partial@example.com",
            username="partialuser",
            password="PartialPassword123!"
        )
        await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "partial@example.com",
            "password": "PartialPassword123!"
        })
        token = login_response.json()["access_token"]
        
        # Update only username
        update_data = {
            "username": "newpartialuser"
        }
        
        response = await client.put(
            "/api/v1/auth/me",
            json=update_data,
            headers={"Authorization": f"Bearer {token}"}
        )
        
        assert response.status_code == 200
        profile = response.json()
        assert profile["username"] == "newpartialuser"
        assert profile["email"] == "partial@example.com"  # Unchanged