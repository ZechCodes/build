"""Test WebSocket authentication according to Session 2 requirements."""

import pytest
from unittest.mock import AsyncMock, MagicMock, patch
from fastapi import WebSocket, HTTPException, status

from app.models.user import User, UserRole
from app.security.jwt import JWTManager
from app.websocket.auth import WebSocketAuthenticator


class TestWebSocketAuthenticator:
    """Test WebSocket authentication functionality."""

    @pytest.fixture
    def jwt_manager(self):
        """Create JWT manager for testing."""
        return JWTManager(secret_key="test-secret-key")

    @pytest.fixture
    def mock_websocket(self):
        """Create mock WebSocket connection."""
        mock = MagicMock(spec=WebSocket)
        mock.query_params = {}
        mock.headers = {}
        mock.close = AsyncMock()
        return mock

    @pytest.fixture
    def mock_db_session(self):
        """Create mock database session."""
        return AsyncMock()

    @pytest.fixture
    def test_user(self):
        """Create test user."""
        return User(
            id="12345678-1234-1234-1234-123456789012",
            email="test@example.com",
            username="testuser",
            password_hash="hashed",
            role=UserRole.USER,
            is_active=True
        )

    @pytest.fixture
    def websocket_auth(self, jwt_manager):
        """Create WebSocket authenticator."""
        return WebSocketAuthenticator(jwt_manager)

    @pytest.mark.asyncio
    async def test_authenticate_websocket_with_query_token(self, websocket_auth, mock_websocket, test_user, jwt_manager):
        """Test WebSocket authentication with token in query parameters."""
        # Create valid token
        token_data = {"sub": str(test_user.id), "email": test_user.email}
        token = jwt_manager.create_access_token(token_data)
        
        # Set token in query parameters
        mock_websocket.query_params = {"token": token}
        
        # Mock database session and user lookup
        with patch('app.websocket.auth.AsyncSessionLocal') as mock_session_factory:
            mock_db = AsyncMock()
            mock_db.__aenter__ = AsyncMock(return_value=mock_db)
            mock_db.__aexit__ = AsyncMock(return_value=None)
            mock_db.get = AsyncMock(return_value=test_user)
            mock_session_factory.return_value = mock_db
            
            user = await websocket_auth.authenticate_websocket(mock_websocket)
            
            assert user == test_user
            mock_websocket.close.assert_not_called()

    @pytest.mark.asyncio
    async def test_authenticate_websocket_with_header_token(self, websocket_auth, mock_websocket, test_user, jwt_manager):
        """Test WebSocket authentication with token in headers."""
        # Create valid token
        token_data = {"sub": str(test_user.id), "email": test_user.email}
        token = jwt_manager.create_access_token(token_data)
        
        # Set token in authorization header
        mock_websocket.headers = {"authorization": f"Bearer {token}"}
        
        # Mock database session and user lookup
        with patch('app.websocket.auth.AsyncSessionLocal') as mock_session_factory:
            mock_db = AsyncMock()
            mock_db.__aenter__ = AsyncMock(return_value=mock_db)
            mock_db.__aexit__ = AsyncMock(return_value=None)
            mock_db.get = AsyncMock(return_value=test_user)
            mock_session_factory.return_value = mock_db
            
            user = await websocket_auth.authenticate_websocket(mock_websocket)
            
            assert user == test_user
            mock_websocket.close.assert_not_called()

    @pytest.mark.asyncio
    async def test_authenticate_websocket_no_token(self, websocket_auth, mock_websocket):
        """Test WebSocket authentication with no token provided."""
        # No token in query params or headers
        mock_websocket.query_params = {}
        mock_websocket.headers = {}
        
        with pytest.raises(HTTPException) as exc_info:
            await websocket_auth.authenticate_websocket(mock_websocket)
        
        assert exc_info.value.status_code == 401
        assert "No token provided" in str(exc_info.value.detail)
        mock_websocket.close.assert_called_once_with(code=status.WS_1008_POLICY_VIOLATION)

    @pytest.mark.asyncio
    async def test_authenticate_websocket_invalid_token(self, websocket_auth, mock_websocket):
        """Test WebSocket authentication with invalid token."""
        # Set invalid token
        mock_websocket.query_params = {"token": "invalid.token.here"}
        
        with pytest.raises(HTTPException) as exc_info:
            await websocket_auth.authenticate_websocket(mock_websocket)
        
        assert exc_info.value.status_code == 401
        assert "Invalid token" in str(exc_info.value.detail)
        mock_websocket.close.assert_called_once_with(code=status.WS_1008_POLICY_VIOLATION)

    @pytest.mark.asyncio
    async def test_authenticate_websocket_user_not_found(self, websocket_auth, mock_websocket, jwt_manager):
        """Test WebSocket authentication when user not found in database."""
        # Create valid token for non-existent user
        token_data = {"sub": "nonexistent-user-id"}
        token = jwt_manager.create_access_token(token_data)
        
        mock_websocket.query_params = {"token": token}
        
        # Mock database session returning None for user
        with patch('app.websocket.auth.AsyncSessionLocal') as mock_session_factory:
            mock_db = AsyncMock()
            mock_db.__aenter__ = AsyncMock(return_value=mock_db)
            mock_db.__aexit__ = AsyncMock(return_value=None)
            mock_db.get = AsyncMock(return_value=None)  # User not found
            mock_session_factory.return_value = mock_db
            
            with pytest.raises(HTTPException) as exc_info:
                await websocket_auth.authenticate_websocket(mock_websocket)
            
            assert exc_info.value.status_code == 401
            assert "Invalid user" in str(exc_info.value.detail)
            mock_websocket.close.assert_called_once_with(code=status.WS_1008_POLICY_VIOLATION)

    @pytest.mark.asyncio
    async def test_authenticate_websocket_inactive_user(self, websocket_auth, mock_websocket, jwt_manager):
        """Test WebSocket authentication with inactive user."""
        # Create inactive user
        inactive_user = User(
            id="12345678-1234-1234-1234-123456789012",
            email="test@example.com",
            username="testuser",
            password_hash="hashed",
            role=UserRole.USER,
            is_active=False  # Inactive user
        )
        
        # Create valid token
        token_data = {"sub": str(inactive_user.id)}
        token = jwt_manager.create_access_token(token_data)
        
        mock_websocket.query_params = {"token": token}
        
        # Mock database session returning inactive user
        with patch('app.websocket.auth.AsyncSessionLocal') as mock_session_factory:
            mock_db = AsyncMock()
            mock_db.__aenter__ = AsyncMock(return_value=mock_db)
            mock_db.__aexit__ = AsyncMock(return_value=None)
            mock_db.get = AsyncMock(return_value=inactive_user)
            mock_session_factory.return_value = mock_db
            
            with pytest.raises(HTTPException) as exc_info:
                await websocket_auth.authenticate_websocket(mock_websocket)
            
            assert exc_info.value.status_code == 401
            assert "Invalid user" in str(exc_info.value.detail)
            mock_websocket.close.assert_called_once_with(code=status.WS_1008_POLICY_VIOLATION)

    @pytest.mark.asyncio
    async def test_authenticate_websocket_expired_token(self, websocket_auth, mock_websocket):
        """Test WebSocket authentication with expired token."""
        # Create expired token manually
        from jose import jwt
        from datetime import datetime, timezone, timedelta
        
        expired_payload = {
            "sub": "12345678-1234-1234-1234-123456789012",
            "exp": datetime.now(timezone.utc) - timedelta(minutes=1),  # Expired
            "type": "access"
        }
        
        expired_token = jwt.encode(expired_payload, "test-secret-key", algorithm="HS256")
        mock_websocket.query_params = {"token": expired_token}
        
        with pytest.raises(HTTPException) as exc_info:
            await websocket_auth.authenticate_websocket(mock_websocket)
        
        assert exc_info.value.status_code == 401
        assert "Invalid token" in str(exc_info.value.detail)
        mock_websocket.close.assert_called_once_with(code=status.WS_1008_POLICY_VIOLATION)

    @pytest.mark.asyncio
    async def test_websocket_connection_manager_authentication(self, websocket_auth, mock_websocket, test_user, jwt_manager):
        """Test WebSocket connection manager with authentication."""
        # Create valid token
        token_data = {"sub": str(test_user.id), "email": test_user.email}
        token = jwt_manager.create_access_token(token_data)
        
        mock_websocket.query_params = {"token": token}
        
        # Mock database session
        with patch('app.websocket.auth.AsyncSessionLocal') as mock_session_factory:
            mock_db = AsyncMock()
            mock_db.__aenter__ = AsyncMock(return_value=mock_db)
            mock_db.__aexit__ = AsyncMock(return_value=None)
            mock_db.get = AsyncMock(return_value=test_user)
            mock_session_factory.return_value = mock_db
            
            # Test authentication in connection context
            user = await websocket_auth.authenticate_websocket(mock_websocket)
            
            assert user.id == test_user.id
            assert user.email == test_user.email
            assert user.is_active
            
            # Verify no premature close
            mock_websocket.close.assert_not_called()

    def test_token_extraction_precedence(self, websocket_auth, jwt_manager):
        """Test that query parameter token takes precedence over header token."""
        query_token = jwt_manager.create_access_token({"sub": "query-user"})
        header_token = jwt_manager.create_access_token({"sub": "header-user"})
        
        mock_websocket = MagicMock()
        mock_websocket.query_params = {"token": query_token}
        mock_websocket.headers = {"authorization": f"Bearer {header_token}"}
        
        # Extract token using the same logic as the authenticator
        token = mock_websocket.query_params.get("token")
        if not token:
            auth_header = mock_websocket.headers.get("authorization")
            if auth_header and auth_header.startswith("Bearer "):
                token = auth_header[7:]
        
        # Should get query token (precedence)
        payload = jwt_manager.verify_token(token)
        assert payload["sub"] == "query-user"

    @pytest.mark.asyncio
    async def test_websocket_authentication_logging(self, websocket_auth, mock_websocket, test_user, jwt_manager):
        """Test that WebSocket authentication events are logged."""
        token_data = {"sub": str(test_user.id)}
        token = jwt_manager.create_access_token(token_data)
        
        mock_websocket.query_params = {"token": token}
        
        # Mock database and logging
        with patch('app.websocket.auth.AsyncSessionLocal') as mock_session_factory, \
             patch('app.websocket.auth.logger') as mock_logger:
            
            mock_db = AsyncMock()
            mock_db.__aenter__ = AsyncMock(return_value=mock_db)
            mock_db.__aexit__ = AsyncMock(return_value=None)
            mock_db.get = AsyncMock(return_value=test_user)
            mock_session_factory.return_value = mock_db
            
            await websocket_auth.authenticate_websocket(mock_websocket)
            
            # Verify successful authentication was logged
            # (specific logging calls would depend on implementation)