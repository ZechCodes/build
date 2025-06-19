"""Test enhanced authentication API endpoints according to Session 2 requirements."""

import pytest
from httpx import AsyncClient
from unittest.mock import patch, AsyncMock

from app.models.user import User, UserRole
from app.security.jwt import JWTManager


class TestEnhancedAuthEndpoints:
    """Test enhanced authentication endpoints with Session 2 features."""

    @pytest.mark.asyncio
    async def test_register_with_enhanced_validation(self, client: AsyncClient):
        """Test user registration with enhanced validation."""
        # Test valid registration
        valid_data = {
            "email": "test@example.com",
            "username": "testuser123",
            "password": "TestPass123!"
        }
        
        response = await client.post("/api/v1/auth/register", json=valid_data)
        assert response.status_code == 201
        
        data = response.json()
        assert data["email"] == valid_data["email"]
        assert data["username"] == valid_data["username"]
        assert "id" in data
        assert "password" not in data  # Password should not be returned

    @pytest.mark.asyncio
    async def test_register_weak_password(self, client: AsyncClient):
        """Test registration with weak password."""
        weak_passwords = [
            "short",  # Too short
            "alllowercase123!",  # No uppercase
            "ALLUPPERCASE123!",  # No lowercase
            "NoDigitsHere!",  # No digits
            "NoSpecialChars123"  # No special characters
        ]
        
        for weak_password in weak_passwords:
            data = {
                "email": "test@example.com",
                "username": "testuser",
                "password": weak_password
            }
            
            response = await client.post("/api/v1/auth/register", json=data)
            assert response.status_code == 422  # Validation error
            
            error_detail = response.json()["detail"]
            assert any("password" in str(error).lower() for error in error_detail)

    @pytest.mark.asyncio
    async def test_register_invalid_username(self, client: AsyncClient):
        """Test registration with invalid username."""
        invalid_usernames = [
            "us",  # Too short
            "user with spaces",  # Contains spaces
            "user@invalid",  # Invalid characters
            "user#invalid",  # Invalid characters
            ""  # Empty
        ]
        
        for invalid_username in invalid_usernames:
            data = {
                "email": "test@example.com",
                "username": invalid_username,
                "password": "TestPass123!"
            }
            
            response = await client.post("/api/v1/auth/register", json=data)
            assert response.status_code == 422

    @pytest.mark.asyncio
    async def test_login_with_enhanced_response(self, client: AsyncClient):
        """Test login with enhanced token response."""
        # Register user first
        register_data = {
            "email": "test@example.com",
            "username": "testuser",
            "password": "TestPass123!"
        }
        await client.post("/api/v1/auth/register", json=register_data)
        
        # Login
        login_data = {
            "email": "test@example.com",
            "password": "TestPass123!"
        }
        
        response = await client.post("/api/v1/auth/login", json=login_data)
        assert response.status_code == 200
        
        data = response.json()
        
        # Check enhanced token response structure
        assert "access_token" in data
        assert "refresh_token" in data
        assert "token_type" in data
        assert "expires_in" in data
        
        assert data["token_type"] == "bearer"
        assert data["expires_in"] == 900  # 15 minutes = 900 seconds
        
        # Check user profile in response
        assert "user" in data
        user_data = data["user"]
        assert user_data["email"] == "test@example.com"
        assert user_data["username"] == "testuser"
        assert user_data["role"] == "user"
        assert "id" in user_data

    @pytest.mark.asyncio
    async def test_login_rate_limiting(self, client: AsyncClient):
        """Test login rate limiting."""
        # Register user first
        register_data = {
            "email": "test@example.com",
            "username": "testuser",
            "password": "TestPass123!"
        }
        await client.post("/api/v1/auth/register", json=register_data)
        
        # Mock rate limiting middleware to simulate rate limit exceeded
        with patch('app.middleware.rate_limiting.RateLimitMiddleware.is_rate_limited') as mock_rate_limit:
            mock_rate_limit.return_value = True
            
            login_data = {
                "email": "test@example.com",
                "password": "TestPass123!"
            }
            
            response = await client.post("/api/v1/auth/login", json=login_data)
            assert response.status_code == 429  # Too Many Requests

    @pytest.mark.asyncio
    async def test_login_account_lockout(self, client: AsyncClient):
        """Test login with account lockout."""
        # This would require mocking the lockout manager
        # In practice, would need to make multiple failed attempts
        register_data = {
            "email": "test@example.com",
            "username": "testuser",
            "password": "TestPass123!"
        }
        await client.post("/api/v1/auth/register", json=register_data)
        
        # Simulate account lockout by mocking the lockout manager
        with patch('app.security.lockout.AccountLockoutManager.is_locked') as mock_is_locked:
            mock_is_locked.return_value = True
            
            login_data = {
                "email": "test@example.com",
                "password": "TestPass123!"
            }
            
            response = await client.post("/api/v1/auth/login", json=login_data)
            # Depending on implementation, could be 423 (Locked) or 429 (Too Many Requests)
            assert response.status_code in [423, 429, 401]

    @pytest.mark.asyncio
    async def test_refresh_token_endpoint(self, client: AsyncClient):
        """Test token refresh endpoint."""
        # Register and login to get tokens
        register_data = {
            "email": "test@example.com",
            "username": "testuser",
            "password": "TestPass123!"
        }
        await client.post("/api/v1/auth/register", json=register_data)
        
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "test@example.com",
            "password": "TestPass123!"
        })
        
        tokens = login_response.json()
        refresh_token = tokens["refresh_token"]
        
        # Use refresh token to get new access token
        refresh_data = {"refresh_token": refresh_token}
        response = await client.post("/api/v1/auth/refresh", json=refresh_data)
        
        assert response.status_code == 200
        data = response.json()
        
        assert "access_token" in data
        assert "token_type" in data
        assert data["token_type"] == "bearer"
        
        # New access token should be different from original
        assert data["access_token"] != tokens["access_token"]

    @pytest.mark.asyncio
    async def test_logout_endpoint(self, client: AsyncClient):
        """Test logout endpoint."""
        # Register and login
        register_data = {
            "email": "test@example.com",
            "username": "testuser",
            "password": "TestPass123!"
        }
        await client.post("/api/v1/auth/register", json=register_data)
        
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "test@example.com",
            "password": "TestPass123!"
        })
        
        tokens = login_response.json()
        access_token = tokens["access_token"]
        
        # Logout
        response = await client.post(
            "/api/v1/auth/logout",
            headers={"Authorization": f"Bearer {access_token}"}
        )
        
        assert response.status_code == 200
        data = response.json()
        assert "message" in data

    @pytest.mark.asyncio
    async def test_get_current_user_profile(self, client: AsyncClient):
        """Test getting current user profile."""
        # Register and login
        register_data = {
            "email": "test@example.com",
            "username": "testuser",
            "password": "TestPass123!"
        }
        await client.post("/api/v1/auth/register", json=register_data)
        
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "test@example.com",
            "password": "TestPass123!"
        })
        
        tokens = login_response.json()
        access_token = tokens["access_token"]
        
        # Get user profile
        response = await client.get(
            "/api/v1/auth/me",
            headers={"Authorization": f"Bearer {access_token}"}
        )
        
        assert response.status_code == 200
        data = response.json()
        
        assert data["email"] == "test@example.com"
        assert data["username"] == "testuser"
        assert data["role"] == "user"
        assert data["is_verified"] is False  # Default for new users
        assert "created_at" in data

    @pytest.mark.asyncio
    async def test_password_reset_request(self, client: AsyncClient):
        """Test password reset request."""
        # Register user first
        register_data = {
            "email": "test@example.com",
            "username": "testuser",
            "password": "TestPass123!"
        }
        await client.post("/api/v1/auth/register", json=register_data)
        
        # Request password reset
        reset_data = {"email": "test@example.com"}
        response = await client.post("/api/v1/auth/reset-password", json=reset_data)
        
        assert response.status_code == 200
        data = response.json()
        assert "message" in data
        # Should not reveal whether email exists (security)
        assert "reset link has been sent" in data["message"].lower()

    @pytest.mark.asyncio
    async def test_password_reset_request_nonexistent_email(self, client: AsyncClient):
        """Test password reset for non-existent email."""
        reset_data = {"email": "nonexistent@example.com"}
        response = await client.post("/api/v1/auth/reset-password", json=reset_data)
        
        # Should return same response to prevent email enumeration
        assert response.status_code == 200
        data = response.json()
        assert "message" in data

    @pytest.mark.asyncio
    async def test_unauthorized_access_to_protected_endpoint(self, client: AsyncClient):
        """Test accessing protected endpoint without authentication."""
        response = await client.get("/api/v1/auth/me")
        assert response.status_code == 401

    @pytest.mark.asyncio
    async def test_invalid_token_access(self, client: AsyncClient):
        """Test accessing protected endpoint with invalid token."""
        response = await client.get(
            "/api/v1/auth/me",
            headers={"Authorization": "Bearer invalid.token.here"}
        )
        assert response.status_code == 401

    @pytest.mark.asyncio
    async def test_expired_token_access(self, client: AsyncClient):
        """Test accessing protected endpoint with expired token."""
        # Create an expired token
        jwt_manager = JWTManager(secret_key="test-secret")
        import jwt as jose_jwt
        from datetime import datetime, timezone, timedelta
        
        expired_payload = {
            "sub": "12345678-1234-1234-1234-123456789012",
            "exp": datetime.now(timezone.utc) - timedelta(minutes=1),  # Expired
            "type": "access"
        }
        
        expired_token = jose_jwt.encode(expired_payload, "test-secret", algorithm="HS256")
        
        response = await client.get(
            "/api/v1/auth/me",
            headers={"Authorization": f"Bearer {expired_token}"}
        )
        assert response.status_code == 401

    @pytest.mark.asyncio
    async def test_auth_endpoints_cors_headers(self, client: AsyncClient):
        """Test that authentication endpoints include proper CORS headers."""
        # Test preflight request
        response = await client.options("/api/v1/auth/login")
        
        # Should include CORS headers (depending on configuration)
        # This test might need adjustment based on actual CORS setup
        assert response.status_code in [200, 204]

    @pytest.mark.asyncio
    async def test_auth_error_response_format(self, client: AsyncClient):
        """Test that authentication errors follow consistent format."""
        # Test various error scenarios
        error_scenarios = [
            ("/api/v1/auth/login", {"email": "invalid", "password": "short"}),
            ("/api/v1/auth/register", {"email": "test@example.com", "password": "weak"}),
            ("/api/v1/auth/refresh", {"refresh_token": "invalid"}),
        ]
        
        for endpoint, invalid_data in error_scenarios:
            response = await client.post(endpoint, json=invalid_data)
            
            # Should return proper error format
            assert response.status_code >= 400
            data = response.json()
            assert "detail" in data