"""Tests for the GET /auth/permissions endpoint."""

import pytest
from httpx import AsyncClient
from sqlalchemy.ext.asyncio import AsyncSession

from app.services.auth import AuthService
from app.schemas.auth import UserCreate
from app.models.user import User, UserRole


class TestPermissionsEndpoint:
    """Test the GET /auth/permissions endpoint."""

    @pytest.mark.asyncio
    async def test_get_user_permissions(self, client: AsyncClient, db_session: AsyncSession):
        """Test getting permissions for regular user."""
        # Create user
        user_create = UserCreate(
            email="permissions@example.com",
            username="permissionsuser",
            password="PermissionsPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "permissions@example.com",
            "password": "PermissionsPassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Get permissions
        response = await client.get(
            "/api/v1/auth/permissions",
            headers={"Authorization": f"Bearer {access_token}"}
        )
        
        assert response.status_code == 200
        data = response.json()
        
        # Verify response structure
        assert "permissions" in data
        assert "role" in data
        assert data["role"] == "user"
        
        # Verify user has expected permissions
        permissions = data["permissions"]
        expected_user_permissions = [
            "vm:create", "vm:delete", "vm:modify", "vm:view",
            "session:create", "session:view", "session:terminate",
            "snapshot:create", "snapshot:delete", "snapshot:restore", "snapshot:view"
        ]
        
        for perm in expected_user_permissions:
            assert perm in permissions
        
        # User should not have admin permissions
        assert "user:manage" not in permissions
        assert "system:admin" not in permissions

    @pytest.mark.asyncio
    async def test_get_moderator_permissions(self, client: AsyncClient, db_session: AsyncSession):
        """Test getting permissions for moderator."""
        # Create moderator user
        user_create = UserCreate(
            email="moderator@example.com",
            username="moderatoruser",
            password="ModeratorPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Update user to moderator role
        user.role = UserRole.MODERATOR
        await db_session.commit()
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "moderator@example.com",
            "password": "ModeratorPassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Get permissions
        response = await client.get(
            "/api/v1/auth/permissions",
            headers={"Authorization": f"Bearer {access_token}"}
        )
        
        assert response.status_code == 200
        data = response.json()
        
        # Verify response structure
        assert data["role"] == "moderator"
        permissions = data["permissions"]
        
        # Moderator should have all user permissions plus user:manage
        expected_moderator_permissions = [
            "vm:create", "vm:delete", "vm:modify", "vm:view",
            "session:create", "session:view", "session:terminate",
            "snapshot:create", "snapshot:delete", "snapshot:restore", "snapshot:view",
            "user:manage"
        ]
        
        for perm in expected_moderator_permissions:
            assert perm in permissions
        
        # Moderator should not have system:admin
        assert "system:admin" not in permissions

    @pytest.mark.asyncio
    async def test_get_admin_permissions(self, client: AsyncClient, db_session: AsyncSession):
        """Test getting permissions for admin."""
        # Create admin user
        user_create = UserCreate(
            email="admin@example.com",
            username="adminuser",
            password="AdminPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Update user to admin role
        user.role = UserRole.ADMIN
        await db_session.commit()
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "admin@example.com",
            "password": "AdminPassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Get permissions
        response = await client.get(
            "/api/v1/auth/permissions",
            headers={"Authorization": f"Bearer {access_token}"}
        )
        
        assert response.status_code == 200
        data = response.json()
        
        # Verify response structure
        assert data["role"] == "admin"
        permissions = data["permissions"]
        
        # Admin should have all permissions
        expected_admin_permissions = [
            "vm:create", "vm:delete", "vm:modify", "vm:view",
            "session:create", "session:view", "session:terminate",
            "snapshot:create", "snapshot:delete", "snapshot:restore", "snapshot:view",
            "user:manage", "system:admin"
        ]
        
        for perm in expected_admin_permissions:
            assert perm in permissions

    @pytest.mark.asyncio
    async def test_get_permissions_requires_authentication(self, client: AsyncClient):
        """Test that permissions endpoint requires authentication."""
        response = await client.get("/api/v1/auth/permissions")
        
        assert response.status_code == 401

    @pytest.mark.asyncio
    async def test_get_permissions_with_invalid_token(self, client: AsyncClient):
        """Test permissions endpoint with invalid token."""
        response = await client.get(
            "/api/v1/auth/permissions",
            headers={"Authorization": "Bearer invalid_token"}
        )
        
        assert response.status_code == 401