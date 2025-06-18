"""Integration tests for API endpoints."""

import pytest
import httpx


@pytest.mark.integration
class TestAPIIntegration:
    """Test API integration with real services."""

    @pytest.mark.asyncio
    async def test_health_endpoints(self, api_client: httpx.AsyncClient):
        """Test health endpoints with real database."""
        # Basic health check
        response = await api_client.get("/health")
        assert response.status_code == 200
        data = response.json()
        assert data["status"] == "healthy"

        # Detailed health check
        response = await api_client.get("/api/v1/health/detailed")
        assert response.status_code == 200
        data = response.json()
        assert data["status"] == "healthy"
        assert "components" in data
        assert data["components"]["database"] == "healthy"

    @pytest.mark.asyncio
    async def test_user_registration_and_authentication_flow(
        self, api_client: httpx.AsyncClient, test_user_data
    ):
        """Test complete user registration and authentication flow."""
        # Register user
        response = await api_client.post("/api/v1/auth/register", json=test_user_data)
        assert response.status_code == 200
        user = response.json()
        assert user["email"] == test_user_data["email"]

        # Login
        login_data = {
            "email": test_user_data["email"],
            "password": test_user_data["password"]
        }
        response = await api_client.post("/api/v1/auth/login", json=login_data)
        assert response.status_code == 200
        tokens = response.json()
        assert "access_token" in tokens
        assert "refresh_token" in tokens

        # Access protected endpoint
        headers = {"Authorization": f"Bearer {tokens['access_token']}"}
        response = await api_client.get("/api/v1/users/me", headers=headers)
        assert response.status_code == 200
        profile = response.json()
        assert profile["email"] == test_user_data["email"]

    @pytest.mark.asyncio
    async def test_token_refresh(self, api_client: httpx.AsyncClient, authenticated_user):
        """Test token refresh functionality."""
        refresh_data = {"refresh_token": authenticated_user["refresh_token"]}
        response = await api_client.post("/api/v1/auth/refresh", json=refresh_data)
        
        assert response.status_code == 200
        data = response.json()
        assert "access_token" in data
        assert data["token_type"] == "bearer"

    @pytest.mark.asyncio
    async def test_user_profile_update(self, api_client: httpx.AsyncClient, authenticated_user):
        """Test user profile update."""
        headers = {"Authorization": f"Bearer {authenticated_user['access_token']}"}
        update_data = {"full_name": "Updated Integration User"}
        
        response = await api_client.put(
            "/api/v1/users/me", 
            headers=headers, 
            json=update_data
        )
        
        assert response.status_code == 200
        updated_user = response.json()
        assert updated_user["full_name"] == update_data["full_name"]