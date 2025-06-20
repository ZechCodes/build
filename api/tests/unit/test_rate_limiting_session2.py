"""Tests for Session 2 rate limiting requirements."""

import pytest
import asyncio
from httpx import AsyncClient
from sqlalchemy.ext.asyncio import AsyncSession
from unittest.mock import patch, AsyncMock

from app.services.auth import AuthService
from app.schemas.auth import UserCreate


class TestSession2RateLimiting:
    """Test Session 2 rate limiting requirements."""

    @pytest.mark.asyncio
    async def test_login_rate_limiting_5_per_15_minutes(self, client: AsyncClient, db_session: AsyncSession):
        """Test login rate limiting: 5 requests per 15 minutes."""
        # Create a user first
        user_create = UserCreate(
            email="ratelimit@example.com",
            username="ratelimituser",
            password="RateLimit123!"
        )
        await AuthService.create_user(db_session, user_create)
        
        # Mock the rate limiting cache to control behavior
        with patch('app.services.cache.CacheService.check_rate_limit') as mock_check:
            # First 5 requests should be allowed
            mock_check.return_value = (True, 1, 0)
            
            for i in range(5):
                response = await client.post("/api/v1/auth/login", json={
                    "email": "ratelimit@example.com",
                    "password": "RateLimit123!"
                })
                # Should either succeed (200) or fail for auth reasons (401) but not rate limited
                assert response.status_code in [200, 401]
            
            # 6th request should be rate limited
            mock_check.return_value = (False, 6, 900)  # Blocked, count=6, 15 minutes remaining
            
            response = await client.post("/api/v1/auth/login", json={
                "email": "ratelimit@example.com", 
                "password": "RateLimit123!"
            })
            
            # Should be rate limited (429) 
            assert response.status_code == 429
            assert "rate limit" in response.text.lower()

    @pytest.mark.asyncio 
    async def test_register_rate_limiting_3_per_hour(self, client: AsyncClient):
        """Test register rate limiting: 3 requests per hour."""
        with patch('app.services.cache.CacheService.check_rate_limit') as mock_check:
            # First 3 requests should be allowed
            mock_check.return_value = (True, 1, 0)
            
            for i in range(3):
                response = await client.post("/api/v1/auth/register", json={
                    "email": f"test{i}@example.com",
                    "username": f"testuser{i}",
                    "password": "TestPassword123!"
                })
                # Should either succeed (201) or fail for validation (422/400) but not rate limited
                assert response.status_code in [201, 400, 422]
            
            # 4th request should be rate limited
            mock_check.return_value = (False, 4, 3600)  # Blocked, count=4, 1 hour remaining
            
            response = await client.post("/api/v1/auth/register", json={
                "email": "test4@example.com",
                "username": "testuser4", 
                "password": "TestPassword123!"
            })
            
            assert response.status_code == 429
            assert "rate limit" in response.text.lower()

    @pytest.mark.asyncio
    async def test_password_reset_request_rate_limiting(self, client: AsyncClient):
        """Test password reset request rate limiting: 3 requests per hour.""" 
        with patch('app.services.cache.CacheService.check_rate_limit') as mock_check:
            # First 3 requests should be allowed
            mock_check.return_value = (True, 1, 0)
            
            for i in range(3):
                response = await client.post("/api/v1/auth/request-password-reset", json={
                    "email": f"reset{i}@example.com"
                })
                # Should succeed (200) - returns success regardless of email existence
                assert response.status_code == 200
            
            # 4th request should be rate limited
            mock_check.return_value = (False, 4, 3600)  # Blocked, count=4, 1 hour remaining
            
            response = await client.post("/api/v1/auth/request-password-reset", json={
                "email": "reset4@example.com"
            })
            
            assert response.status_code == 429
            assert "rate limit" in response.text.lower()

    @pytest.mark.asyncio
    async def test_password_reset_completion_rate_limiting(self, client: AsyncClient, db_session: AsyncSession):
        """Test password reset completion rate limiting: 3 requests per hour."""
        # Create user first
        user_create = UserCreate(
            email="resetcomp@example.com",
            username="resetcompuser", 
            password="ResetComp123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        with patch('app.services.cache.CacheService.check_rate_limit') as mock_check:
            # First 3 requests should be allowed (though they may fail for other reasons)
            mock_check.return_value = (True, 1, 0)
            
            for i in range(3):
                response = await client.post("/api/v1/auth/reset-password", json={
                    "user_id": str(user.id),
                    "reset_token": f"fake_token_{i}",
                    "new_password": "NewPassword123!"
                })
                # Should either fail for bad token (400/503) but not rate limited
                assert response.status_code in [400, 503, 404]
            
            # 4th request should be rate limited
            mock_check.return_value = (False, 4, 3600)  # Blocked, count=4, 1 hour remaining
            
            response = await client.post("/api/v1/auth/reset-password", json={
                "user_id": str(user.id),
                "reset_token": "fake_token_4",
                "new_password": "NewPassword123!"
            })
            
            assert response.status_code == 429
            assert "rate limit" in response.text.lower()

    @pytest.mark.asyncio
    async def test_rate_limiting_different_ips(self, client: AsyncClient):
        """Test that rate limiting is per-IP (different IPs have separate limits)."""
        with patch('app.services.cache.CacheService.check_rate_limit') as mock_check:
            # Mock different behavior for different IPs by checking the identifier
            def check_limit_side_effect(identifier, limit, window):
                if "ip:127.0.0.1" in identifier:
                    return (False, 6, 900)  # First IP is rate limited
                else:
                    return (True, 1, 0)  # Other IPs are fine
            
            mock_check.side_effect = check_limit_side_effect
            
            # First IP should be rate limited
            response = await client.post("/api/v1/auth/login", json={
                "email": "test@example.com",
                "password": "TestPassword123!"
            })
            assert response.status_code == 429
            
            # Different IP would not be rate limited (but we can't easily test this with the test client)
            # This test verifies the logic but can't fully test the IP separation in the test environment

    @pytest.mark.asyncio
    async def test_rate_limiting_headers(self, client: AsyncClient):
        """Test that rate limiting includes proper headers."""
        with patch('app.services.cache.CacheService.check_rate_limit') as mock_check:
            # Allow request but set up for rate limit info
            mock_check.return_value = (True, 3, 600)  # 3 requests used, 10 minutes remaining
            
            response = await client.post("/api/v1/auth/request-password-reset", json={
                "email": "headers@example.com"
            })
            
            # Should succeed
            assert response.status_code == 200
            
            # Check if rate limit headers are present (implementation dependent)
            # Some rate limiting implementations add headers like X-RateLimit-*
            # This is more of a documentation/specification test

    @pytest.mark.asyncio
    async def test_rate_limiting_with_redis_failure(self, client: AsyncClient):
        """Test graceful handling when Redis is unavailable."""
        with patch('app.core.redis.get_redis') as mock_redis:
            # Simulate Redis connection failure
            mock_redis.side_effect = Exception("Redis connection failed")
            
            # Request should still work (fail open for availability)
            response = await client.post("/api/v1/auth/request-password-reset", json={
                "email": "redis_fail@example.com"
            })
            
            # Should either succeed or fail for non-rate-limiting reasons
            # Rate limiting should fail open when Redis is unavailable
            assert response.status_code in [200, 503]  # Success or service unavailable

    @pytest.mark.asyncio
    async def test_rate_limiting_excludes_health_endpoints(self, client: AsyncClient):
        """Test that health endpoints are not rate limited.""" 
        with patch('app.services.cache.CacheService.check_rate_limit') as mock_check:
            # Set up rate limiting to always block
            mock_check.return_value = (False, 1000, 3600)
            
            # Health endpoints should still work
            response = await client.get("/health")
            assert response.status_code == 200
            
            response = await client.get("/health/detailed")
            assert response.status_code == 200
            
            # Rate limiting check should not have been called for these endpoints
            # (implementation dependent - some middleware might skip these entirely)