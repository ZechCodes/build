"""Test enhanced authentication schemas."""

import pytest
from pydantic import ValidationError

from app.schemas.auth import (
    UserCreate, UserLogin, TokenResponse, 
    UserProfile, PasswordResetRequest, PasswordReset
)


class TestUserCreateSchema:
    """Test UserCreate schema validation."""

    def test_valid_user_create(self):
        """Test valid user creation data."""
        data = {
            "email": "test@example.com",
            "username": "testuser",
            "password": "TestPass123!"
        }
        user_create = UserCreate(**data)
        assert user_create.email == data["email"]
        assert user_create.username == data["username"]
        assert user_create.password == data["password"]

    def test_password_strength_validation(self):
        """Test password strength requirements."""
        # Too short
        with pytest.raises(ValidationError, match="at least 8 characters"):
            UserCreate(email="test@example.com", password="short")
        
        # No uppercase
        with pytest.raises(ValidationError, match="uppercase letter"):
            UserCreate(email="test@example.com", password="testpass123!")
        
        # No lowercase
        with pytest.raises(ValidationError, match="lowercase letter"):
            UserCreate(email="test@example.com", password="TESTPASS123!")
        
        # No digit
        with pytest.raises(ValidationError, match="digit"):
            UserCreate(email="test@example.com", password="TestPass!")
        
        # No special character
        with pytest.raises(ValidationError, match="special character"):
            UserCreate(email="test@example.com", password="TestPass123")

    def test_username_validation(self):
        """Test username validation rules."""
        # Valid usernames
        valid_usernames = ["user123", "test_user", "user-name", "a1b2c3"]
        for username in valid_usernames:
            user = UserCreate(
                email="test@example.com", 
                username=username, 
                password="TestPass123!"
            )
            assert user.username == username
        
        # Invalid usernames
        invalid_usernames = ["user@name", "user name", "user#123", ""]
        for username in invalid_usernames:
            with pytest.raises(ValidationError):
                UserCreate(
                    email="test@example.com", 
                    username=username, 
                    password="TestPass123!"
                )

    def test_email_validation(self):
        """Test email validation."""
        # Valid email
        user = UserCreate(
            email="test@example.com",
            password="TestPass123!"
        )
        assert user.email == "test@example.com"
        
        # Invalid email
        with pytest.raises(ValidationError):
            UserCreate(email="invalid-email", password="TestPass123!")


class TestUserLoginSchema:
    """Test UserLogin schema validation."""

    def test_valid_login(self):
        """Test valid login data."""
        data = {"email": "test@example.com", "password": "password123"}
        login = UserLogin(**data)
        assert login.email == data["email"]
        assert login.password == data["password"]

    def test_invalid_email(self):
        """Test invalid email format."""
        with pytest.raises(ValidationError):
            UserLogin(email="invalid-email", password="password123")


class TestTokenResponseSchema:
    """Test TokenResponse schema."""

    def test_valid_token_response(self):
        """Test valid token response."""
        data = {
            "access_token": "access_token_value",
            "refresh_token": "refresh_token_value",
            "token_type": "bearer",
            "expires_in": 900
        }
        token_response = TokenResponse(**data)
        assert token_response.access_token == data["access_token"]
        assert token_response.refresh_token == data["refresh_token"]
        assert token_response.token_type == data["token_type"]
        assert token_response.expires_in == data["expires_in"]

    def test_default_token_type(self):
        """Test default token type."""
        data = {
            "access_token": "access_token_value",
            "refresh_token": "refresh_token_value",
            "expires_in": 900
        }
        token_response = TokenResponse(**data)
        assert token_response.token_type == "bearer"


class TestUserProfileSchema:
    """Test UserProfile schema."""

    def test_valid_user_profile(self):
        """Test valid user profile data."""
        from datetime import datetime
        
        data = {
            "id": "12345678-1234-1234-1234-123456789012",
            "email": "test@example.com",
            "username": "testuser",
            "role": "user",
            "is_verified": True,
            "created_at": datetime.now()
        }
        profile = UserProfile(**data)
        assert profile.id == data["id"]
        assert profile.email == data["email"]
        assert profile.username == data["username"]
        assert profile.role == data["role"]
        assert profile.is_verified == data["is_verified"]


class TestPasswordResetSchemas:
    """Test password reset schemas."""

    def test_password_reset_request(self):
        """Test password reset request schema."""
        data = {"email": "test@example.com"}
        request = PasswordResetRequest(**data)
        assert request.email == data["email"]

    def test_password_reset(self):
        """Test password reset schema."""
        data = {
            "user_id": "12345678-1234-1234-1234-123456789012",
            "reset_token": "reset_token_value",
            "new_password": "NewPass123!"
        }
        reset = PasswordReset(**data)
        assert reset.user_id == data["user_id"]
        assert reset.reset_token == data["reset_token"]
        assert reset.new_password == data["new_password"]

    def test_password_reset_weak_password(self):
        """Test password reset with weak password."""
        data = {
            "user_id": "12345678-1234-1234-1234-123456789012",
            "reset_token": "reset_token_value",
            "new_password": "weak"
        }
        with pytest.raises(ValidationError, match="at least 8 characters"):
            PasswordReset(**data)