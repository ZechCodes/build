"""
Unit tests for SessionStateManager
"""
import pytest
import asyncio
import time
from unittest.mock import AsyncMock, MagicMock

import sys
import os

# Add the session-manager directory to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '../..'))

from core.state_manager import SessionStateManager, SessionState, SessionContext


class TestSessionStateManager:
    """Test suite for SessionStateManager"""
    
    async def test_create_session_success(self, redis_mock, sample_session_data):
        """Test successful session creation"""
        # Arrange
        state_manager = SessionStateManager(redis_mock)
        await state_manager.initialize()
        
        # Act
        session_id = await state_manager.create_session(
            sample_session_data["user_id"],
            sample_session_data["vm_id"],
            sample_session_data["terminal_size"],
            sample_session_data["environment_vars"]
        )
        
        # Assert
        assert session_id is not None
        assert len(session_id) > 10  # Should be a meaningful session ID
        assert session_id in state_manager.sessions
        session = state_manager.sessions[session_id]
        assert session.user_id == sample_session_data["user_id"]
        assert session.vm_id == sample_session_data["vm_id"]
        assert session.state == SessionState.INITIALIZING
        
        # Verify Redis persistence was called
        redis_mock.pipeline.assert_called()
        pipe_mock = redis_mock.pipeline.return_value
        pipe_mock.hset.assert_called()
        pipe_mock.execute.assert_called()
    
    async def test_update_session_state(self, state_manager_with_cleanup, sample_session_data):
        """Test session state updates"""
        # Arrange
        state_manager = state_manager_with_cleanup
        
        # Create a session first
        session_id = await state_manager.create_session(
            sample_session_data["user_id"],
            sample_session_data["vm_id"],
            sample_session_data["terminal_size"],
            sample_session_data["environment_vars"]
        )
        
        # Verify initial state
        session = state_manager.sessions[session_id]
        assert session.state == SessionState.INITIALIZING
        
        # Act - Update state to ACTIVE
        result = await state_manager.update_session_state(session_id, SessionState.ACTIVE)
        
        # Assert
        assert result is True
        updated_session = state_manager.sessions[session_id]
        assert updated_session.state == SessionState.ACTIVE
        # Timestamp should be updated (greater than or equal since operations are fast)
        assert updated_session.last_activity >= session.last_activity
    
    async def test_get_session_by_id(self, redis_mock, sample_session_data):
        """Test session retrieval by ID"""
        pytest.skip("SessionStateManager not implemented yet - Red phase of TDD")
        
        # This test will be implemented after basic CRUD operations pass
    
    async def test_delete_session(self, redis_mock, sample_session_data):
        """Test session deletion"""
        pytest.skip("SessionStateManager not implemented yet - Red phase of TDD")
        
        # This test will be implemented after basic CRUD operations pass
    
    async def test_session_ownership_validation(self, state_manager_with_cleanup, sample_session_data):
        """Test that session ownership is validated"""
        # Arrange
        state_manager = state_manager_with_cleanup
        
        # Create a session for user_456
        session_id = await state_manager.create_session(
            sample_session_data["user_id"],  # user_456
            sample_session_data["vm_id"],
            sample_session_data["terminal_size"],
            sample_session_data["environment_vars"]
        )
        
        # Verify session was created for user_456
        session = await state_manager.get_session(session_id)
        assert session is not None
        assert session.user_id == "user_456"
        
        # Act - Try to get sessions for a different user
        different_user_sessions = await state_manager.get_user_sessions("user_999")
        
        # Assert - Different user should not see the session
        assert len(different_user_sessions) == 0
        
        # Verify correct user can see the session
        correct_user_sessions = await state_manager.get_user_sessions("user_456")
        assert len(correct_user_sessions) == 1
        assert correct_user_sessions[0].session_id == session_id
    
    async def test_redis_persistence(self, redis_mock, sample_session_data):
        """Test that session data is persisted to Redis"""
        pytest.skip("SessionStateManager not implemented yet - Red phase of TDD")
        
        # This test will verify Redis operations are called correctly
    
    async def test_create_session_invalid_inputs(self, state_manager_with_cleanup):
        """Test session creation with invalid inputs"""
        state_manager = state_manager_with_cleanup
        
        # Test empty user_id
        with pytest.raises(ValueError, match="User ID and VM ID are required"):
            await state_manager.create_session("", "vm_123")
        
        # Test empty vm_id  
        with pytest.raises(ValueError, match="User ID and VM ID are required"):
            await state_manager.create_session("user_123", "")
        
        # Test None user_id
        with pytest.raises(ValueError, match="User ID and VM ID are required"):
            await state_manager.create_session(None, "vm_123")
    
    async def test_update_nonexistent_session(self, state_manager_with_cleanup):
        """Test updating a session that doesn't exist"""
        state_manager = state_manager_with_cleanup
        
        # Try to update a session that doesn't exist
        result = await state_manager.update_session_state("nonexistent_session", SessionState.ACTIVE)
        
        # Should return False
        assert result is False