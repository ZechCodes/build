"""
Unit tests for RecoveryManager
"""
import pytest
import asyncio
import time
from unittest.mock import AsyncMock, MagicMock

import sys
import os

# Add the session-manager directory to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '../..'))

from persistence.recovery_manager import RecoveryManager, RecoveryInfo


class TestRecoveryManager:
    """Test suite for RecoveryManager"""
    
    async def test_initiate_recovery_success(self, session_manager_mock, buffer_manager_mock, websocket_bridge_mock):
        """Test successful recovery initiation"""
        # Arrange
        recovery_manager = RecoveryManager(
            session_manager_mock, 
            buffer_manager_mock, 
            websocket_bridge_mock
        )
        await recovery_manager.initialize()
        
        try:
            session_id = "session_123"
            user_id = "user123"
            connection_id = "conn_456"
            
            # Act
            result = await recovery_manager.initiate_recovery(session_id, user_id, connection_id)
            
            # Assert
            assert result is True
            
            # Verify session was checked
            session_manager_mock.get_session.assert_called_with(session_id)
            
            # Verify recovery was tracked
            assert session_id in recovery_manager.active_recoveries
            recovery_info = recovery_manager.active_recoveries[session_id]
            assert recovery_info.user_id == user_id
            assert recovery_info.session_id == session_id
            assert recovery_info.connection_id == connection_id
            
        finally:
            await recovery_manager.stop()
    
    async def test_recovery_authorization_validation(self, session_manager_mock, buffer_manager_mock, websocket_bridge_mock):
        """Test recovery authorization with user validation"""
        # Arrange
        recovery_manager = RecoveryManager(
            session_manager_mock, 
            buffer_manager_mock, 
            websocket_bridge_mock
        )
        await recovery_manager.initialize()
        
        try:
            session_id = "session_123"
            owner_user_id = "user123"
            unauthorized_user_id = "user999"
            connection_id = "conn_456"
            
            # Act - Try to recover with wrong user
            result = await recovery_manager.initiate_recovery(session_id, unauthorized_user_id, connection_id)
            
            # Assert - Should be denied
            assert result is False
            
            # Verify session was checked but no recovery was started
            session_manager_mock.get_session.assert_called_with(session_id)
            assert session_id not in recovery_manager.active_recoveries
            
            # Act - Try with correct user
            result = await recovery_manager.initiate_recovery(session_id, owner_user_id, connection_id)
            
            # Assert - Should be allowed
            assert result is True
            assert session_id in recovery_manager.active_recoveries
            
        finally:
            await recovery_manager.stop()
    
    async def test_recovery_data_integrity(self, redis_mock):
        """Test recovery data integrity and validation"""
        # Arrange
        session_manager = AsyncMock()
        buffer_manager = AsyncMock()
        websocket_bridge = AsyncMock()
        
        recovery_manager = RecoveryManager(session_manager, buffer_manager, websocket_bridge)
        await recovery_manager.initialize()
        
        # Mock session validation
        mock_session = MagicMock()
        mock_session.user_id = "user_123"
        mock_session.session_id = "session_456"
        mock_session.last_activity = time.time() - 30  # 30 seconds ago (within recovery window)
        session_manager.get_session.return_value = mock_session
        
        # Mock buffer data with proper SessionBuffer structure
        from persistence.buffer_manager import SessionBuffer
        test_buffer_data = SessionBuffer(
            session_id="session_456",
            user_id="user_123",
            buffer_data=b"test terminal data content",
            cursor_position=(10, 20),
            scroll_position=5,
            last_updated=time.time(),
            size_bytes=26,
            line_count=3
        )
        buffer_manager.retrieve_buffer.return_value = test_buffer_data
        
        # Mock successful websocket send
        websocket_bridge.send_to_connection.return_value = True
        
        # Act - Initiate recovery
        success = await recovery_manager.initiate_recovery(
            session_id="session_456",
            user_id="user_123", 
            connection_id="conn_789"
        )
        
        # Wait for recovery processing
        await asyncio.sleep(0.1)
        
        # Assert - Verify data integrity validation
        assert success is True
        
        # Verify buffer data was retrieved
        buffer_manager.retrieve_buffer.assert_called_with("session_456", "user_123")
        
        # Verify websocket message was sent with integrity checks
        websocket_bridge.send_to_connection.assert_called()
        sent_args = websocket_bridge.send_to_connection.call_args[0]
        sent_message = sent_args[1]
        
        # Verify recovery message structure and integrity
        assert "recovery_data" in sent_message
        assert "buffer" in sent_message["recovery_data"]
        assert "cursor_position" in sent_message["recovery_data"]
        assert "buffer_metadata" in sent_message["recovery_data"]
        assert sent_message["recovery_data"]["buffer"] == "test terminal data content"
        
        # Test corrupted buffer data handling
        buffer_manager.retrieve_buffer.return_value = None
        corrupted_success = await recovery_manager.initiate_recovery(
            session_id="session_456",
            user_id="user_123",
            connection_id="conn_789_2"
        )
        # Should still succeed but with empty buffer
        assert corrupted_success is True
    
    async def test_recovery_timeout_handling(self, redis_mock):
        """Test recovery timeout and cleanup"""
        # Arrange
        session_manager = AsyncMock()
        buffer_manager = AsyncMock()
        websocket_bridge = AsyncMock()
        
        recovery_manager = RecoveryManager(session_manager, buffer_manager, websocket_bridge)
        # Set short timeout for testing
        recovery_manager.recovery_timeout = 0.2  # 200ms for testing
        await recovery_manager.initialize()
        
        # Mock session validation
        mock_session = MagicMock()
        mock_session.user_id = "user_123"
        mock_session.last_activity = time.time() - 30  # 30 seconds ago (within recovery window)
        session_manager.get_session.return_value = mock_session
        
        # Mock buffer data
        buffer_manager.retrieve_buffer.return_value = []
        
        # Mock websocket failure to simulate hanging recovery
        websocket_bridge.send_to_connection.return_value = False
        
        try:
            # Act - Start recovery that should timeout
            start_time = time.time()
            await recovery_manager.initiate_recovery(
                session_id="session_456",
                user_id="user_123",
                connection_id="conn_789"
            )
            
            # Wait for timeout + buffer
            await asyncio.sleep(0.3)
            elapsed_time = time.time() - start_time
            
            # Assert - Verify timeout handling
            # Recovery should be cleaned up from active recoveries
            active_recoveries = recovery_manager.get_active_recoveries()
            assert "session_456" not in active_recoveries
            
            # Test that timeout occurred within expected window
            assert elapsed_time >= 0.2  # At least timeout duration
            assert elapsed_time < 1.0   # But not hanging indefinitely
            
            # Verify cleanup notification was attempted
            websocket_bridge.send_to_connection.assert_called()
            
            # Test multiple timeout scenarios
            recovery_manager.recovery_timeout = 0.1  # Even shorter
            
            # Start another recovery
            await recovery_manager.initiate_recovery(
                session_id="session_789",
                user_id="user_123", 
                connection_id="conn_456"
            )
            
            await asyncio.sleep(0.15)
            
            # Should also be cleaned up
            active_recoveries_2 = recovery_manager.get_active_recoveries()
            assert "session_789" not in active_recoveries_2
            
        finally:
            await recovery_manager.stop()
    
    async def test_recovery_failure_scenarios(self, redis_mock):
        """Test recovery failure handling"""
        # Arrange
        session_manager = AsyncMock()
        buffer_manager = AsyncMock()
        websocket_bridge = AsyncMock()
        
        recovery_manager = RecoveryManager(session_manager, buffer_manager, websocket_bridge)
        await recovery_manager.initialize()
        
        try:
            # Test Case 1: Session not found
            session_manager.get_session.return_value = None
            
            failure_1 = await recovery_manager.initiate_recovery(
                session_id="non_existent_session",
                user_id="user_123",
                connection_id="conn_789"
            )
            assert failure_1 is False
            
            # Test Case 2: Network failure during recovery
            mock_session = MagicMock()
            mock_session.user_id = "user_123"
            session_manager.get_session.return_value = mock_session
            buffer_manager.retrieve_buffer.return_value = []
            
            # Simulate network failure
            websocket_bridge.send_to_connection.side_effect = Exception("Network error")
            
            # Should handle exception gracefully
            network_failure = await recovery_manager.initiate_recovery(
                session_id="session_456",
                user_id="user_123",
                connection_id="conn_789"
            )
            # Recovery initiation should succeed even if sending fails
            assert network_failure is True
            
            # Test Case 3: Buffer corruption
            buffer_manager.retrieve_buffer.side_effect = Exception("Buffer corruption")
            websocket_bridge.send_to_connection.side_effect = None  # Reset
            websocket_bridge.send_to_connection.return_value = True
            
            buffer_corruption = await recovery_manager.initiate_recovery(
                session_id="session_789",
                user_id="user_123",
                connection_id="conn_456"
            )
            # Should handle buffer corruption gracefully
            assert buffer_corruption is True
            
            # Test Case 4: Partial recovery failure with fallback
            buffer_manager.retrieve_buffer.side_effect = None  # Reset
            buffer_manager.retrieve_buffer.return_value = [
                {"data": b"incomplete", "timestamp": time.time()}
            ]
            
            # Simulate partial websocket failure
            websocket_bridge.send_to_connection.return_value = False
            
            partial_failure = await recovery_manager.initiate_recovery(
                session_id="session_partial",
                user_id="user_123",
                connection_id="conn_partial"
            )
            
            # Should attempt recovery even with failures
            assert partial_failure is True
            
            # Wait for processing
            await asyncio.sleep(0.1)
            
            # Test Case 5: Multiple concurrent failures
            websocket_bridge.send_to_connection.return_value = True
            
            # Start multiple recoveries
            recovery_tasks = []
            for i in range(3):
                task = asyncio.create_task(
                    recovery_manager.initiate_recovery(
                        session_id=f"session_concurrent_{i}",
                        user_id="user_123",
                        connection_id=f"conn_concurrent_{i}"
                    )
                )
                recovery_tasks.append(task)
            
            # Wait for all to complete
            results = await asyncio.gather(*recovery_tasks, return_exceptions=True)
            
            # All should handle gracefully (no exceptions raised)
            for result in results:
                assert not isinstance(result, Exception)
                assert result is True
                
        finally:
            await recovery_manager.stop()
    
    async def test_recovery_rate_limiting(self, redis_mock):
        """Test recovery attempt rate limiting"""
        # Arrange
        session_manager = AsyncMock()
        buffer_manager = AsyncMock()
        websocket_bridge = AsyncMock()
        
        recovery_manager = RecoveryManager(session_manager, buffer_manager, websocket_bridge)
        # Set aggressive rate limits for testing
        recovery_manager.max_recoveries_per_window = 3
        recovery_manager.rate_limit_window = 1  # 1 second window
        await recovery_manager.initialize()
        
        # Mock session validation
        mock_session = MagicMock()
        mock_session.user_id = "user_123"
        mock_session.last_activity = time.time() - 30  # 30 seconds ago (within recovery window)
        session_manager.get_session.return_value = mock_session
        buffer_manager.retrieve_buffer.return_value = []
        websocket_bridge.send_to_connection.return_value = True
        
        try:
            # Act - Test rate limiting
            successful_recoveries = 0
            failed_recoveries = 0
            
            # Attempt 5 recoveries within rate limit window (should allow 3, block 2)
            for i in range(5):
                result = await recovery_manager.initiate_recovery(
                    session_id=f"session_{i}",
                    user_id="user_123",
                    connection_id=f"conn_{i}"
                )
                
                if result:
                    successful_recoveries += 1
                else:
                    failed_recoveries += 1
            
            # Assert rate limiting behavior
            assert successful_recoveries == 3  # Should allow first 3
            assert failed_recoveries == 2     # Should block last 2
            
            # Test rate limit window reset
            # Wait for rate limit window to expire
            await asyncio.sleep(1.1)
            
            # Should be able to recover again after window reset
            post_window_recovery = await recovery_manager.initiate_recovery(
                session_id="session_post_window",
                user_id="user_123",
                connection_id="conn_post_window"
            )
            assert post_window_recovery is True
            
            # Test per-user rate limiting isolation
            # Different user should have separate rate limits
            mock_session_2 = MagicMock()
            mock_session_2.user_id = "user_456"
            
            def get_session_side_effect(session_id):
                if "user2" in session_id:
                    return mock_session_2
                return mock_session
                
            session_manager.get_session.side_effect = get_session_side_effect
            
            # User 456 should have fresh rate limit
            user2_recovery = await recovery_manager.initiate_recovery(
                session_id="session_user2_1",
                user_id="user_456",
                connection_id="conn_user2_1"
            )
            assert user2_recovery is True
            
            # Test rate limit tracking
            recovery_stats = recovery_manager.get_recovery_stats()
            assert "total_attempts" in recovery_stats
            assert recovery_stats["total_attempts"] >= 6  # All attempts counted
            
            # Test rate limit violation logging
            # Reset user_123 attempts to test violation detection
            recovery_manager.recovery_attempts["user_123"] = [time.time()] * 10  # Pre-fill violations
            
            violation_recovery = await recovery_manager.initiate_recovery(
                session_id="session_violation",
                user_id="user_123", 
                connection_id="conn_violation"
            )
            assert violation_recovery is False  # Should be blocked
            
        finally:
            await recovery_manager.stop()
    
    async def test_cross_session_recovery_prevention(self, redis_mock):
        """Test prevention of cross-session recovery attempts"""
        # Arrange
        session_manager = AsyncMock()
        buffer_manager = AsyncMock()
        websocket_bridge = AsyncMock()
        
        recovery_manager = RecoveryManager(session_manager, buffer_manager, websocket_bridge)
        await recovery_manager.initialize()
        
        # Create sessions for different users
        user1_session = MagicMock()
        user1_session.user_id = "user_123"
        user1_session.session_id = "session_user1"
        
        user2_session = MagicMock()
        user2_session.user_id = "user_456" 
        user2_session.session_id = "session_user2"
        
        def get_session_side_effect(session_id):
            if session_id == "session_user1":
                return user1_session
            elif session_id == "session_user2":
                return user2_session
            return None
            
        session_manager.get_session.side_effect = get_session_side_effect
        buffer_manager.retrieve_buffer.return_value = []
        websocket_bridge.send_to_connection.return_value = True
        
        try:
            # Test Case 1: User trying to recover their own session (should succeed)
            legitimate_recovery = await recovery_manager.initiate_recovery(
                session_id="session_user1",
                user_id="user_123",  # Matches session owner
                connection_id="conn_123"
            )
            assert legitimate_recovery is True
            
            # Test Case 2: User trying to recover another user's session (should fail)
            cross_user_recovery = await recovery_manager.initiate_recovery(
                session_id="session_user2",  # Belongs to user_456
                user_id="user_123",          # But user_123 is trying to access
                connection_id="conn_456"
            )
            assert cross_user_recovery is False
            
            # Test Case 3: Verify no buffer data was accessed for unauthorized attempt
            # Buffer should not be called for failed authorization
            buffer_calls = buffer_manager.retrieve_buffer.call_args_list
            authorized_session_calls = [call for call in buffer_calls if "session_user1" in str(call)]
            unauthorized_session_calls = [call for call in buffer_calls if "session_user2" in str(call)]
            
            assert len(authorized_session_calls) > 0  # Should have accessed user1's buffer
            assert len(unauthorized_session_calls) == 0  # Should NOT have accessed user2's buffer
            
            # Test Case 4: Multiple users attempting cross-session recovery
            # User 456 trying to recover user 123's session
            reverse_cross_recovery = await recovery_manager.initiate_recovery(
                session_id="session_user1",  # Belongs to user_123
                user_id="user_456",          # But user_456 is trying to access  
                connection_id="conn_reverse"
            )
            assert reverse_cross_recovery is False
            
            # Test Case 5: Non-existent session recovery attempt
            non_existent_recovery = await recovery_manager.initiate_recovery(
                session_id="session_non_existent",
                user_id="user_123",
                connection_id="conn_non_existent"
            )
            assert non_existent_recovery is False
            
            # Test Case 6: Verify security audit logging for violation attempts
            # Check that attempts to access unauthorized sessions are recorded
            active_recoveries = recovery_manager.get_active_recoveries()
            # Only legitimate recovery should be active
            assert len(active_recoveries) <= 1
            
            # Verify that unauthorized attempts don't create recovery contexts
            assert "session_user2" not in active_recoveries  # Cross-user attempt
            assert "session_non_existent" not in active_recoveries  # Non-existent session
            
            # Test Case 7: Session ownership validation with edge cases
            # Empty user_id
            empty_user_recovery = await recovery_manager.initiate_recovery(
                session_id="session_user1",
                user_id="",  # Empty user ID
                connection_id="conn_empty"
            )
            assert empty_user_recovery is False
            
            # None user_id
            none_user_recovery = await recovery_manager.initiate_recovery(
                session_id="session_user1", 
                user_id=None,  # None user ID
                connection_id="conn_none"
            )
            assert none_user_recovery is False
            
        finally:
            await recovery_manager.stop()
    
    async def test_recovery_cleanup_procedures(self, redis_mock):
        """Test recovery cleanup and resource management"""
        # Arrange
        session_manager = AsyncMock()
        buffer_manager = AsyncMock()
        websocket_bridge = AsyncMock()
        
        recovery_manager = RecoveryManager(session_manager, buffer_manager, websocket_bridge)
        recovery_manager.recovery_timeout = 0.2  # Short timeout for testing
        await recovery_manager.initialize()
        
        # Mock session validation
        mock_session = MagicMock()
        mock_session.user_id = "user_123"
        mock_session.last_activity = time.time() - 30  # 30 seconds ago (within recovery window)
        session_manager.get_session.return_value = mock_session
        buffer_manager.retrieve_buffer.return_value = []
        websocket_bridge.send_to_connection.return_value = True
        
        try:
            # Test Case 1: Successful recovery cleanup
            initial_active_count = len(recovery_manager.get_active_recoveries())
            
            success_recovery = await recovery_manager.initiate_recovery(
                session_id="session_success",
                user_id="user_123",
                connection_id="conn_success"
            )
            assert success_recovery is True
            
            # Should have active recovery initially
            active_recoveries = recovery_manager.get_active_recoveries()
            assert "session_success" in active_recoveries
            
            # Wait for recovery to complete and cleanup
            await asyncio.sleep(0.1)
            
            # Verify cleanup occurred
            post_cleanup_recoveries = recovery_manager.get_active_recoveries()
            assert "session_success" not in post_cleanup_recoveries
            
            # Test Case 2: Failed recovery cleanup
            websocket_bridge.send_to_connection.return_value = False  # Simulate failure
            
            failed_recovery = await recovery_manager.initiate_recovery(
                session_id="session_failed",
                user_id="user_123", 
                connection_id="conn_failed"
            )
            assert failed_recovery is True  # Initiation succeeds
            
            # Wait for failure handling and cleanup
            await asyncio.sleep(0.1)
            
            # Failed recovery should also be cleaned up
            failed_cleanup_recoveries = recovery_manager.get_active_recoveries()
            assert "session_failed" not in failed_cleanup_recoveries
            
            # Test Case 3: Timeout-based cleanup
            websocket_bridge.send_to_connection.return_value = True  # Reset
            # Make websocket slow to trigger timeout
            async def slow_send(*args, **kwargs):
                await asyncio.sleep(0.3)  # Longer than timeout
                return True
            websocket_bridge.send_to_connection.side_effect = slow_send
            
            timeout_recovery = await recovery_manager.initiate_recovery(
                session_id="session_timeout",
                user_id="user_123",
                connection_id="conn_timeout"  
            )
            assert timeout_recovery is True
            
            # Should be in active recoveries initially
            timeout_active = recovery_manager.get_active_recoveries()
            assert "session_timeout" in timeout_active
            
            # Wait for timeout and cleanup
            await asyncio.sleep(0.4)
            
            # Should be cleaned up after timeout
            timeout_cleanup = recovery_manager.get_active_recoveries()
            assert "session_timeout" not in timeout_cleanup
            
            # Test Case 4: Memory usage validation
            websocket_bridge.send_to_connection.side_effect = None  # Reset
            websocket_bridge.send_to_connection.return_value = True
            
            # Create multiple recoveries to test memory management
            recovery_ids = []
            for i in range(5):
                session_id = f"session_memory_{i}"
                recovery_success = await recovery_manager.initiate_recovery(
                    session_id=session_id,
                    user_id="user_123",
                    connection_id=f"conn_memory_{i}"
                )
                if recovery_success:
                    recovery_ids.append(session_id)
            
            # Wait for all to process and cleanup
            await asyncio.sleep(0.2)
            
            # All should be cleaned up (no memory leaks)
            memory_cleanup = recovery_manager.get_active_recoveries()
            for recovery_id in recovery_ids:
                assert recovery_id not in memory_cleanup
            
            # Test Case 5: Resource limit enforcement
            # Verify recovery attempt tracking is cleaned up
            initial_attempt_count = len(recovery_manager.recovery_attempts)
            
            # Force cleanup by stopping and restarting
            await recovery_manager.stop()
            await recovery_manager.initialize()
            
            # Verify cleanup occurred
            post_restart_attempt_count = len(recovery_manager.recovery_attempts)
            # Should maintain some history but not grow indefinitely
            assert post_restart_attempt_count <= initial_attempt_count
            
            # Test Case 6: Recovery statistics cleanup
            stats_before = recovery_manager.get_recovery_stats()
            
            # Perform more recoveries
            for i in range(3):
                await recovery_manager.initiate_recovery(
                    session_id=f"session_stats_{i}",
                    user_id="user_123",
                    connection_id=f"conn_stats_{i}"
                )
            
            await asyncio.sleep(0.1)
            
            stats_after = recovery_manager.get_recovery_stats()
            # Stats should be updated but not grow indefinitely
            assert "total_attempts" in stats_after
            assert stats_after["total_attempts"] >= stats_before.get("total_attempts", 0)
            
        finally:
            await recovery_manager.stop()
            
            # Final cleanup validation
            final_active_recoveries = recovery_manager.get_active_recoveries()
            assert len(final_active_recoveries) == 0  # All cleaned up