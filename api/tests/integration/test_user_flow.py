"""Integration tests for complete user flows."""

import pytest
from httpx import AsyncClient


class TestUserFlow:
    """Test complete user flows."""

    @pytest.mark.asyncio
    async def test_complete_user_registration_and_login_flow(self, client: AsyncClient):
        """Test complete user registration and login flow."""
        user_data = {
            "email": "integration@example.com",
            "password": "integrationtest123",
            "full_name": "Integration Test User"
        }

        # 1. Register user
        register_response = await client.post("/api/v1/auth/register", json=user_data)
        assert register_response.status_code == 200
        user = register_response.json()
        assert user["email"] == user_data["email"]

        # 2. Login
        login_data = {
            "email": user_data["email"],
            "password": user_data["password"]
        }
        login_response = await client.post("/api/v1/auth/login", json=login_data)
        assert login_response.status_code == 200
        tokens = login_response.json()
        access_token = tokens["access_token"]

        # 3. Access protected endpoint
        headers = {"Authorization": f"Bearer {access_token}"}
        profile_response = await client.get("/api/v1/users/me", headers=headers)
        assert profile_response.status_code == 200
        profile = profile_response.json()
        assert profile["email"] == user_data["email"]

        # 4. Update profile
        update_data = {"full_name": "Updated Integration User"}
        update_response = await client.put(
            "/api/v1/users/me", 
            headers=headers, 
            json=update_data
        )
        assert update_response.status_code == 200
        updated_profile = update_response.json()
        assert updated_profile["full_name"] == update_data["full_name"]

    @pytest.mark.asyncio
    async def test_unauthorized_access(self, client: AsyncClient):
        """Test unauthorized access to protected endpoints."""
        # Try to access protected endpoint without token
        response = await client.get("/api/v1/users/me")
        assert response.status_code == 401

        # Try to access with invalid token
        headers = {"Authorization": "Bearer invalid_token"}
        response = await client.get("/api/v1/users/me", headers=headers)
        assert response.status_code == 401

    @pytest.mark.asyncio
    async def test_duplicate_email_registration(self, client: AsyncClient):
        """Test registration with duplicate email."""
        user_data = {
            "email": "duplicate@example.com",
            "password": "password123",
            "full_name": "First User"
        }

        # Register first user
        response1 = await client.post("/api/v1/auth/register", json=user_data)
        assert response1.status_code == 200

        # Try to register second user with same email
        user_data["full_name"] = "Second User"
        response2 = await client.post("/api/v1/auth/register", json=user_data)
        assert response2.status_code == 400
        assert "already registered" in response2.json()["detail"]