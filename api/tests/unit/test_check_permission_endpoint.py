"""Tests for the POST /auth/check-permission endpoint."""

import pytest
from httpx import AsyncClient
from sqlalchemy.ext.asyncio import AsyncSession

from app.services.auth import AuthService
from app.schemas.auth import UserCreate
from app.models.user import User, UserRole


class TestCheckPermissionEndpoint:
    """Test the POST /auth/check-permission endpoint."""

    @pytest.mark.asyncio
    async def test_check_valid_permission_granted(self, client: AsyncClient, db_session: AsyncSession):
        """Test checking a permission that user has."""
        # Create user
        user_create = UserCreate(
            email="checkperm@example.com",
            username="checkpermuser",
            password="CheckPermPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "checkperm@example.com",
            "password": "CheckPermPassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Check permission user has (vm:create)
        response = await client.post(
            "/api/v1/auth/check-permission",
            headers={"Authorization": f"Bearer {access_token}"},
            json={"permission": "vm:create"}
        )
        
        assert response.status_code == 200
        data = response.json()
        
        assert data["permission"] == "vm:create"
        assert data["granted"] is True
        assert data["role"] == "user"

    @pytest.mark.asyncio
    async def test_check_valid_permission_denied(self, client: AsyncClient, db_session: AsyncSession):
        """Test checking a permission that user doesn't have."""
        # Create user
        user_create = UserCreate(
            email="checkpermdenied@example.com",
            username="checkpermdenieduser",
            password="CheckPermDeniedPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "checkpermdenied@example.com",
            "password": "CheckPermDeniedPassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Check permission user doesn't have (system:admin)
        response = await client.post(
            "/api/v1/auth/check-permission",
            headers={"Authorization": f"Bearer {access_token}"},
            json={"permission": "system:admin"}
        )
        
        assert response.status_code == 200
        data = response.json()
        
        assert data["permission"] == "system:admin"
        assert data["granted"] is False
        assert data["role"] == "user"

    @pytest.mark.asyncio
    async def test_check_permission_admin_has_all(self, client: AsyncClient, db_session: AsyncSession):
        """Test that admin has all permissions."""
        # Create admin user
        user_create = UserCreate(
            email="checkpermadmin@example.com",
            username="checkpermadminuser",
            password="CheckPermAdminPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Update user to admin role
        user.role = UserRole.ADMIN
        await db_session.commit()
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "checkpermadmin@example.com",
            "password": "CheckPermAdminPassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Check system:admin permission (only admins have this)
        response = await client.post(
            "/api/v1/auth/check-permission",
            headers={"Authorization": f"Bearer {access_token}"},
            json={"permission": "system:admin"}
        )
        
        assert response.status_code == 200
        data = response.json()
        
        assert data["permission"] == "system:admin"
        assert data["granted"] is True
        assert data["role"] == "admin"

    @pytest.mark.asyncio
    async def test_check_invalid_permission_name(self, client: AsyncClient, db_session: AsyncSession):
        """Test checking an invalid permission name."""
        # Create user
        user_create = UserCreate(
            email="checkinvalid@example.com",
            username="checkinvaliduser",
            password="CheckInvalidPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "checkinvalid@example.com",
            "password": "CheckInvalidPassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Check invalid permission
        response = await client.post(
            "/api/v1/auth/check-permission",
            headers={"Authorization": f"Bearer {access_token}"},
            json={"permission": "invalid:permission"}
        )
        
        assert response.status_code == 400
        assert "invalid permission name" in response.text.lower()

    @pytest.mark.asyncio
    async def test_check_permission_missing_permission_field(self, client: AsyncClient, db_session: AsyncSession):
        """Test checking permission without permission field."""
        # Create user
        user_create = UserCreate(
            email="checkmissing@example.com",
            username="checkmissinguser",
            password="CheckMissingPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "checkmissing@example.com",
            "password": "CheckMissingPassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Check permission without permission field
        response = await client.post(
            "/api/v1/auth/check-permission",
            headers={"Authorization": f"Bearer {access_token}"},
            json={}
        )
        
        assert response.status_code == 400
        assert "permission name required" in response.text.lower()

    @pytest.mark.asyncio
    async def test_check_permission_requires_authentication(self, client: AsyncClient):
        """Test that check-permission endpoint requires authentication."""
        response = await client.post(
            "/api/v1/auth/check-permission",
            json={"permission": "vm:create"}
        )
        
        assert response.status_code == 401

    @pytest.mark.asyncio
    async def test_check_permission_with_invalid_token(self, client: AsyncClient):
        """Test check-permission endpoint with invalid token."""
        response = await client.post(
            "/api/v1/auth/check-permission",
            headers={"Authorization": "Bearer invalid_token"},
            json={"permission": "vm:create"}
        )
        
        assert response.status_code == 401