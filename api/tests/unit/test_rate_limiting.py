"""Test rate limiting middleware according to Session 2 requirements."""

import pytest
from unittest.mock import AsyncMock, MagicMock
from fastapi import FastAPI, HTTPException

from app.middleware.rate_limiting import RateLimitMiddleware


class TestRateLimitMiddleware:
    """Test rate limiting middleware functionality."""

    @pytest.fixture
    def mock_redis(self):
        """Create mock Redis client."""
        mock = AsyncMock()
        mock.get = AsyncMock(return_value=None)
        mock.incr = AsyncMock(return_value=1)
        mock.expire = AsyncMock()
        mock.ttl = AsyncMock(return_value=900)
        mock.delete = AsyncMock()
        return mock

    @pytest.fixture
    def middleware(self, mock_redis):
        """Create rate limiting middleware instance."""
        app = FastAPI()
        middleware = RateLimitMiddleware(app, redis_url="redis://localhost:6379")
        middleware.redis = mock_redis
        return middleware

    def test_rate_limit_configuration(self, middleware):
        """Test rate limit configuration."""
        assert "/auth/login" in middleware.rate_limits
        assert "/auth/register" in middleware.rate_limits
        assert "/auth/reset-password" in middleware.rate_limits
        
        # Check login limits
        login_limits = middleware.rate_limits["/auth/login"]
        assert login_limits["max_requests"] == 5
        assert login_limits["window_minutes"] == 15
        
        # Check register limits
        register_limits = middleware.rate_limits["/auth/register"]
        assert register_limits["max_requests"] == 3
        assert register_limits["window_minutes"] == 60
        
        # Check reset-password limits
        reset_limits = middleware.rate_limits["/auth/reset-password"]
        assert reset_limits["max_requests"] == 3
        assert reset_limits["window_minutes"] == 60

    @pytest.mark.asyncio
    async def test_is_rate_limited_no_previous_requests(self, middleware, mock_redis):
        """Test rate limiting check with no previous requests."""
        mock_redis.get.return_value = None
        
        is_limited = await middleware.is_rate_limited("192.168.1.1", "/auth/login")
        assert is_limited is False
        
        mock_redis.get.assert_called_with("rate_limit:192.168.1.1:/auth/login")

    @pytest.mark.asyncio
    async def test_is_rate_limited_below_threshold(self, middleware, mock_redis):
        """Test rate limiting check below threshold."""
        mock_redis.get.return_value = "3"  # 3 < 5 (login limit)
        
        is_limited = await middleware.is_rate_limited("192.168.1.1", "/auth/login")
        assert is_limited is False

    @pytest.mark.asyncio
    async def test_is_rate_limited_at_threshold(self, middleware, mock_redis):
        """Test rate limiting check at threshold."""
        mock_redis.get.return_value = "5"  # 5 >= 5 (login limit)
        
        is_limited = await middleware.is_rate_limited("192.168.1.1", "/auth/login")
        assert is_limited is True

    @pytest.mark.asyncio
    async def test_is_rate_limited_above_threshold(self, middleware, mock_redis):
        """Test rate limiting check above threshold."""
        mock_redis.get.return_value = "7"  # 7 > 5 (login limit)
        
        is_limited = await middleware.is_rate_limited("192.168.1.1", "/auth/login")
        assert is_limited is True

    @pytest.mark.asyncio
    async def test_record_request_first_request(self, middleware, mock_redis):
        """Test recording first request."""
        mock_redis.incr.return_value = 1
        
        await middleware.record_request("192.168.1.1", "/auth/login")
        
        mock_redis.incr.assert_called_with("rate_limit:192.168.1.1:/auth/login")
        mock_redis.expire.assert_called_with("rate_limit:192.168.1.1:/auth/login", 900)

    @pytest.mark.asyncio
    async def test_record_request_subsequent_request(self, middleware, mock_redis):
        """Test recording subsequent request."""
        mock_redis.incr.return_value = 3  # Not first request
        
        await middleware.record_request("192.168.1.1", "/auth/login")
        
        mock_redis.incr.assert_called_with("rate_limit:192.168.1.1:/auth/login")
        # Should not set expiration again
        mock_redis.expire.assert_not_called()

    def test_get_client_ip_with_forwarded_header(self, middleware):
        """Test client IP extraction with X-Forwarded-For header."""
        # Mock request with X-Forwarded-For header
        mock_request = MagicMock()
        mock_request.headers = {"x-forwarded-for": "192.168.1.1, 10.0.0.1"}
        mock_request.client.host = "127.0.0.1"
        
        client_ip = middleware.get_client_ip(mock_request)
        assert client_ip == "192.168.1.1"  # Should get first IP from forwarded header

    def test_get_client_ip_with_real_ip_header(self, middleware):
        """Test client IP extraction with X-Real-IP header."""
        mock_request = MagicMock()
        mock_request.headers = {"x-real-ip": "192.168.1.1"}
        mock_request.client.host = "127.0.0.1"
        
        client_ip = middleware.get_client_ip(mock_request)
        assert client_ip == "192.168.1.1"

    def test_get_client_ip_without_headers(self, middleware):
        """Test client IP extraction without special headers."""
        mock_request = MagicMock()
        mock_request.headers = {}
        mock_request.client.host = "127.0.0.1"
        
        client_ip = middleware.get_client_ip(mock_request)
        assert client_ip == "127.0.0.1"  # Should get IP from client.host

    @pytest.mark.asyncio
    async def test_get_remaining_requests_no_limit(self, middleware, mock_redis):
        """Test getting remaining requests for unlimited endpoint."""
        result = await middleware.get_remaining_requests("192.168.1.1", "/unlimited")
        
        assert result["remaining"] == -1
        assert result["reset_time"] is None
        mock_redis.get.assert_not_called()

    @pytest.mark.asyncio
    async def test_get_remaining_requests_no_usage(self, middleware, mock_redis):
        """Test getting remaining requests with no prior usage."""
        mock_redis.get.return_value = None
        
        result = await middleware.get_remaining_requests("192.168.1.1", "/auth/login")
        
        assert result["remaining"] == 5  # Full login limit
        assert result["reset_time"] is None

    @pytest.mark.asyncio
    async def test_get_remaining_requests_with_usage(self, middleware, mock_redis):
        """Test getting remaining requests with prior usage."""
        mock_redis.get.return_value = "3"
        mock_redis.ttl.return_value = 600  # 10 minutes remaining
        
        result = await middleware.get_remaining_requests("192.168.1.1", "/auth/login")
        
        assert result["remaining"] == 2  # 5 - 3 = 2
        assert result["current_count"] == 3
        assert result["max_requests"] == 5
        assert result["reset_time"] is not None

    @pytest.mark.asyncio
    async def test_reset_rate_limit_success(self, middleware, mock_redis):
        """Test successful rate limit reset."""
        success = await middleware.reset_rate_limit("192.168.1.1", "/auth/login")
        
        assert success is True
        mock_redis.delete.assert_called_with("rate_limit:192.168.1.1:/auth/login")

    @pytest.mark.asyncio
    async def test_reset_rate_limit_invalid_endpoint(self, middleware, mock_redis):
        """Test rate limit reset for invalid endpoint."""
        success = await middleware.reset_rate_limit("192.168.1.1", "/invalid")
        
        assert success is False
        mock_redis.delete.assert_not_called()

    @pytest.mark.asyncio
    async def test_redis_failure_handling(self, middleware, mock_redis):
        """Test handling of Redis failures."""
        mock_redis.get.side_effect = Exception("Redis connection failed")
        
        # Should not raise exception and return False (fail open)
        is_limited = await middleware.is_rate_limited("192.168.1.1", "/auth/login")
        assert is_limited is False