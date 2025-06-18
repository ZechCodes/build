"""Integration tests for database operations."""

import pytest
from sqlalchemy import select

from api.app.models.user import User
from api.app.services.auth import AuthService


@pytest.mark.integration
class TestDatabaseIntegration:
    """Test database integration with real PostgreSQL."""

    @pytest.mark.asyncio
    async def test_user_crud_operations(self, db_session, test_user_data):
        """Test user CRUD operations with real database."""
        # Create user
        user = await AuthService.create_user(db_session, test_user_data)
        assert user.id is not None
        assert user.email == test_user_data["email"]

        # Read user
        result = await db_session.execute(select(User).where(User.id == user.id))
        retrieved_user = result.scalar_one_or_none()
        assert retrieved_user is not None
        assert retrieved_user.email == test_user_data["email"]

        # Update user
        retrieved_user.full_name = "Updated Name"
        await db_session.commit()
        await db_session.refresh(retrieved_user)
        assert retrieved_user.full_name == "Updated Name"

        # Delete user
        await db_session.delete(retrieved_user)
        await db_session.commit()
        
        result = await db_session.execute(select(User).where(User.id == user.id))
        deleted_user = result.scalar_one_or_none()
        assert deleted_user is None

    @pytest.mark.asyncio
    async def test_user_authentication_with_database(self, db_session, test_user_data):
        """Test user authentication with real database."""
        # Create user
        user = await AuthService.create_user(db_session, test_user_data)
        
        # Authenticate with correct credentials
        authenticated_user = await AuthService.authenticate_user(
            db_session, 
            test_user_data["email"], 
            test_user_data["password"]
        )
        assert authenticated_user is not None
        assert authenticated_user.id == user.id

        # Authenticate with wrong credentials
        wrong_auth = await AuthService.authenticate_user(
            db_session, 
            test_user_data["email"], 
            "wrongpassword"
        )
        assert wrong_auth is None

    @pytest.mark.asyncio
    async def test_user_timestamps(self, db_session, test_user_data):
        """Test user timestamp fields."""
        user = await AuthService.create_user(db_session, test_user_data)
        
        assert user.created_at is not None
        assert user.updated_at is not None
        assert user.created_at == user.updated_at
        assert user.last_login is None

        # Update user to trigger updated_at change
        original_updated_at = user.updated_at
        user.full_name = "Updated Name"
        await db_session.commit()
        await db_session.refresh(user)
        
        assert user.updated_at > original_updated_at