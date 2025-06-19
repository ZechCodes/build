"""Test JWT token management system according to Session 2."""

import pytest
import secrets
from datetime import datetime, timedelta, timezone
from jose import jwt, JWTError
from fastapi import HTTPException

from app.security.jwt import JWTManager


class TestJWTManager:
    """Test JWT manager functionality."""

    @pytest.fixture
    def jwt_manager(self):
        """Create JWT manager instance for testing."""
        secret_key = secrets.token_urlsafe(32)
        return JWTManager(secret_key=secret_key, algorithm="HS256")

    def test_jwt_manager_initialization(self, jwt_manager):
        """Test JWT manager initialization."""
        assert jwt_manager.secret_key is not None
        assert jwt_manager.algorithm == "HS256"
        assert jwt_manager.access_token_expire_minutes == 15
        assert jwt_manager.refresh_token_expire_days == 7

    def test_create_access_token(self, jwt_manager):
        """Test access token creation."""
        user_data = {"sub": "12345", "email": "test@example.com"}
        token = jwt_manager.create_access_token(user_data)
        
        assert token is not None
        assert isinstance(token, str)
        
        # Decode and verify token contents
        payload = jwt.decode(token, jwt_manager.secret_key, algorithms=[jwt_manager.algorithm])
        assert payload["sub"] == "12345"
        assert payload["email"] == "test@example.com"
        assert payload["type"] == "access"
        assert "exp" in payload

    def test_create_refresh_token(self, jwt_manager):
        """Test refresh token creation."""
        user_data = {"sub": "12345", "email": "test@example.com"}
        token = jwt_manager.create_refresh_token(user_data)
        
        assert token is not None
        assert isinstance(token, str)
        
        # Decode and verify token contents
        payload = jwt.decode(token, jwt_manager.secret_key, algorithms=[jwt_manager.algorithm])
        assert payload["sub"] == "12345"
        assert payload["email"] == "test@example.com"
        assert payload["type"] == "refresh"
        assert "exp" in payload
        assert "jti" in payload  # JWT ID for token tracking

    def test_verify_access_token_valid(self, jwt_manager):
        """Test verification of valid access token."""
        user_data = {"sub": "12345", "email": "test@example.com"}
        token = jwt_manager.create_access_token(user_data)
        
        payload = jwt_manager.verify_token(token, token_type="access")
        assert payload["sub"] == "12345"
        assert payload["email"] == "test@example.com"
        assert payload["type"] == "access"

    def test_verify_refresh_token_valid(self, jwt_manager):
        """Test verification of valid refresh token."""
        user_data = {"sub": "12345", "email": "test@example.com"}
        token = jwt_manager.create_refresh_token(user_data)
        
        payload = jwt_manager.verify_token(token, token_type="refresh")
        assert payload["sub"] == "12345"
        assert payload["email"] == "test@example.com"
        assert payload["type"] == "refresh"

    def test_verify_token_invalid_signature(self, jwt_manager):
        """Test verification with invalid signature."""
        # Create token with different secret
        wrong_secret = secrets.token_urlsafe(32)
        user_data = {"sub": "12345", "exp": datetime.now(timezone.utc) + timedelta(minutes=15)}
        invalid_token = jwt.encode(user_data, wrong_secret, algorithm="HS256")
        
        with pytest.raises(HTTPException) as exc_info:
            jwt_manager.verify_token(invalid_token)
        assert exc_info.value.status_code == 401

    def test_verify_token_expired(self, jwt_manager):
        """Test verification of expired token."""
        # Create expired token
        user_data = {
            "sub": "12345", 
            "exp": datetime.now(timezone.utc) - timedelta(minutes=1),  # Expired 1 minute ago
            "type": "access"
        }
        expired_token = jwt.encode(user_data, jwt_manager.secret_key, algorithm=jwt_manager.algorithm)
        
        with pytest.raises(HTTPException) as exc_info:
            jwt_manager.verify_token(expired_token)
        assert exc_info.value.status_code == 401

    def test_verify_token_wrong_type(self, jwt_manager):
        """Test verification with wrong token type."""
        user_data = {"sub": "12345", "email": "test@example.com"}
        refresh_token = jwt_manager.create_refresh_token(user_data)
        
        # Try to verify refresh token as access token
        with pytest.raises(HTTPException) as exc_info:
            jwt_manager.verify_token(refresh_token, token_type="access")
        assert exc_info.value.status_code == 401
        assert "Invalid token type" in str(exc_info.value.detail)

    def test_verify_token_malformed(self, jwt_manager):
        """Test verification of malformed token."""
        malformed_token = "not.a.valid.jwt.token"
        
        with pytest.raises(HTTPException) as exc_info:
            jwt_manager.verify_token(malformed_token)
        assert exc_info.value.status_code == 401

    def test_token_expiration_times(self, jwt_manager):
        """Test that tokens have correct expiration times."""
        user_data = {"sub": "12345"}
        
        # Test access token expiration
        access_token = jwt_manager.create_access_token(user_data)
        access_payload = jwt.decode(access_token, jwt_manager.secret_key, algorithms=[jwt_manager.algorithm])
        access_exp = datetime.fromtimestamp(access_payload["exp"], tz=timezone.utc)
        expected_access_exp = datetime.now(timezone.utc) + timedelta(minutes=15)
        assert abs((access_exp - expected_access_exp).total_seconds()) < 60  # Within 1 minute
        
        # Test refresh token expiration
        refresh_token = jwt_manager.create_refresh_token(user_data)
        refresh_payload = jwt.decode(refresh_token, jwt_manager.secret_key, algorithms=[jwt_manager.algorithm])
        refresh_exp = datetime.fromtimestamp(refresh_payload["exp"], tz=timezone.utc)
        expected_refresh_exp = datetime.now(timezone.utc) + timedelta(days=7)
        assert abs((refresh_exp - expected_refresh_exp).total_seconds()) < 3600  # Within 1 hour

    def test_refresh_token_has_jti(self, jwt_manager):
        """Test that refresh tokens include JWT ID for tracking."""
        user_data = {"sub": "12345"}
        refresh_token = jwt_manager.create_refresh_token(user_data)
        
        payload = jwt.decode(refresh_token, jwt_manager.secret_key, algorithms=[jwt_manager.algorithm])
        assert "jti" in payload
        assert len(payload["jti"]) > 0

    def test_token_default_algorithm(self):
        """Test that default algorithm is HS256."""
        secret_key = secrets.token_urlsafe(32)
        manager = JWTManager(secret_key=secret_key)
        assert manager.algorithm == "HS256"

    def test_custom_algorithm(self):
        """Test JWT manager with custom algorithm."""
        secret_key = secrets.token_urlsafe(32)
        manager = JWTManager(secret_key=secret_key, algorithm="HS512")
        assert manager.algorithm == "HS512"