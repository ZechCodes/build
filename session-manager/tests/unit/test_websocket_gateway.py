"""
Unit tests for WebSocket Gateway IP/UA tracking and security
"""
import pytest
import asyncio
import time
from unittest.mock import AsyncMock, MagicMock

import sys
import os

# Add the session-manager directory to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '../..'))

from websocket.gateway import WebSocketGateway, ConnectionInfo


class TestWebSocketGatewaySecurity:
    """Test suite for WebSocket Gateway security features"""
    
    def test_extract_client_ip_forwarded_for(self):
        """Test IP extraction from X-Forwarded-For header"""
        # Arrange
        gateway = WebSocketGateway(None, None, "test_secret")
        websocket_mock = MagicMock()
        websocket_mock.headers = {
            "x-forwarded-for": "192.168.1.100, 10.0.0.1",
            "user-agent": "Mozilla/5.0"
        }
        
        # Act
        ip = gateway._extract_client_ip(websocket_mock)
        
        # Assert
        assert ip == "192.168.1.100"
    
    def test_extract_client_ip_real_ip(self):
        """Test IP extraction from X-Real-IP header"""
        # Arrange
        gateway = WebSocketGateway(None, None, "test_secret")
        websocket_mock = MagicMock()
        websocket_mock.headers = {
            "x-real-ip": "203.0.113.195",
            "user-agent": "Mozilla/5.0"
        }
        
        # Act
        ip = gateway._extract_client_ip(websocket_mock)
        
        # Assert
        assert ip == "203.0.113.195"
    
    def test_extract_client_ip_cloudflare(self):
        """Test IP extraction from CloudFlare header"""
        # Arrange
        gateway = WebSocketGateway(None, None, "test_secret")
        websocket_mock = MagicMock()
        websocket_mock.headers = {
            "cf-connecting-ip": "198.51.100.42",
            "user-agent": "Mozilla/5.0"
        }
        
        # Act
        ip = gateway._extract_client_ip(websocket_mock)
        
        # Assert
        assert ip == "198.51.100.42"
    
    def test_extract_client_ip_fallback(self):
        """Test IP extraction fallback to client info"""
        # Arrange
        gateway = WebSocketGateway(None, None, "test_secret")
        websocket_mock = MagicMock()
        websocket_mock.headers = {}
        websocket_mock.client = MagicMock()
        websocket_mock.client.host = "172.16.0.50"
        
        # Act
        ip = gateway._extract_client_ip(websocket_mock)
        
        # Assert
        assert ip == "172.16.0.50"
    
    def test_extract_client_ip_unknown(self):
        """Test IP extraction returns unknown when no source available"""
        # Arrange
        gateway = WebSocketGateway(None, None, "test_secret")
        websocket_mock = MagicMock()
        websocket_mock.headers = {}
        websocket_mock.client = None
        
        # Act
        ip = gateway._extract_client_ip(websocket_mock)
        
        # Assert
        assert ip == "unknown"
    
    def test_extract_user_agent_success(self):
        """Test User-Agent extraction"""
        # Arrange
        gateway = WebSocketGateway(None, None, "test_secret")
        websocket_mock = MagicMock()
        user_agent = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/91.0.4472.124"
        websocket_mock.headers = {"user-agent": user_agent}
        
        # Act
        extracted_ua = gateway._extract_user_agent(websocket_mock)
        
        # Assert
        assert extracted_ua == user_agent
    
    def test_extract_user_agent_truncation(self):
        """Test User-Agent truncation for very long strings"""
        # Arrange
        gateway = WebSocketGateway(None, None, "test_secret")
        websocket_mock = MagicMock()
        long_user_agent = "A" * 600  # Longer than 500 char limit
        websocket_mock.headers = {"user-agent": long_user_agent}
        
        # Act
        extracted_ua = gateway._extract_user_agent(websocket_mock)
        
        # Assert
        assert len(extracted_ua) == 503  # 500 + "..."
        assert extracted_ua.endswith("...")
    
    def test_extract_user_agent_missing(self):
        """Test User-Agent extraction when header missing"""
        # Arrange
        gateway = WebSocketGateway(None, None, "test_secret")
        websocket_mock = MagicMock()
        websocket_mock.headers = {}
        
        # Act
        extracted_ua = gateway._extract_user_agent(websocket_mock)
        
        # Assert
        assert extracted_ua == "unknown"
    
    def test_user_agents_significantly_different_same_browser(self):
        """Test User-Agent comparison for same browser"""
        # Arrange
        gateway = WebSocketGateway(None, None, "test_secret")
        ua1 = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/91.0.4472.124"
        ua2 = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/92.0.4515.107"
        
        # Act
        different = gateway._user_agents_significantly_different(ua1, ua2)
        
        # Assert
        assert different is False  # Same browser (Chrome), just different versions
    
    def test_user_agents_significantly_different_browsers(self):
        """Test User-Agent comparison for different browsers"""
        # Arrange
        gateway = WebSocketGateway(None, None, "test_secret")
        ua1 = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/91.0.4472.124"
        ua2 = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:89.0) Firefox/89.0"
        
        # Act
        different = gateway._user_agents_significantly_different(ua1, ua2)
        
        # Assert
        assert different is True  # Different browsers (Chrome vs Firefox)
    
    def test_user_agents_significantly_different_unknown(self):
        """Test User-Agent comparison with unknown values"""
        # Arrange
        gateway = WebSocketGateway(None, None, "test_secret")
        ua1 = "unknown"
        ua2 = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/91.0.4472.124"
        
        # Act
        different = gateway._user_agents_significantly_different(ua1, ua2)
        
        # Assert
        assert different is False  # Should not flag as suspicious when one is unknown
    
    async def test_detect_session_hijacking_ip_change(self):
        """Test session hijacking detection via IP address change"""
        # Arrange
        session_manager_mock = AsyncMock()
        gateway = WebSocketGateway(session_manager_mock, None, "test_secret")
        
        session_id = "test_session_123"
        
        # Create existing connection
        existing_connection = ConnectionInfo(
            websocket=MagicMock(),
            user_id="user123",
            session_id=session_id,
            connected_at=time.time(),
            last_ping=time.time(),
            client_ip="192.168.1.100",
            user_agent="Chrome/91.0",
            is_authenticated=True
        )
        gateway.connections["existing_conn"] = existing_connection
        
        # Create new connection with different IP
        new_connection = ConnectionInfo(
            websocket=MagicMock(),
            user_id="user123", 
            session_id="",  # Will be set by join
            connected_at=time.time(),
            last_ping=time.time(),
            client_ip="203.0.113.42",  # Different IP
            user_agent="Chrome/91.0",
            is_authenticated=True
        )
        new_conn_id = "new_conn"
        gateway.connections[new_conn_id] = new_connection
        
        # Act
        hijacking_detected = await gateway._detect_session_hijacking(session_id, new_conn_id)
        
        # Assert
        assert hijacking_detected is True
    
    async def test_detect_session_hijacking_user_agent_change(self):
        """Test session hijacking detection via User-Agent change"""
        # Arrange
        session_manager_mock = AsyncMock()
        gateway = WebSocketGateway(session_manager_mock, None, "test_secret")
        
        session_id = "test_session_456"
        
        # Create existing connection
        existing_connection = ConnectionInfo(
            websocket=MagicMock(),
            user_id="user456",
            session_id=session_id,
            connected_at=time.time(),
            last_ping=time.time(),
            client_ip="192.168.1.100",
            user_agent="Mozilla/5.0 Chrome/91.0.4472.124",
            is_authenticated=True
        )
        gateway.connections["existing_conn"] = existing_connection
        
        # Create new connection with different browser
        new_connection = ConnectionInfo(
            websocket=MagicMock(),
            user_id="user456",
            session_id="",
            connected_at=time.time(),
            last_ping=time.time(),
            client_ip="192.168.1.100",  # Same IP
            user_agent="Mozilla/5.0 Firefox/89.0",  # Different browser
            is_authenticated=True
        )
        new_conn_id = "new_conn"
        gateway.connections[new_conn_id] = new_connection
        
        # Act
        hijacking_detected = await gateway._detect_session_hijacking(session_id, new_conn_id)
        
        # Assert
        assert hijacking_detected is True
    
    async def test_detect_session_hijacking_legitimate_connection(self):
        """Test session hijacking detection allows legitimate connections"""
        # Arrange
        session_manager_mock = AsyncMock()
        gateway = WebSocketGateway(session_manager_mock, None, "test_secret")
        
        session_id = "test_session_789"
        
        # Create existing connection
        existing_connection = ConnectionInfo(
            websocket=MagicMock(),
            user_id="user789",
            session_id=session_id,
            connected_at=time.time(),
            last_ping=time.time(),
            client_ip="192.168.1.100",
            user_agent="Mozilla/5.0 Chrome/91.0.4472.124",
            is_authenticated=True
        )
        gateway.connections["existing_conn"] = existing_connection
        
        # Create new connection with same client characteristics
        new_connection = ConnectionInfo(
            websocket=MagicMock(),
            user_id="user789",
            session_id="",
            connected_at=time.time(),
            last_ping=time.time(),
            client_ip="192.168.1.100",  # Same IP
            user_agent="Mozilla/5.0 Chrome/92.0.4515.107",  # Same browser, different version
            is_authenticated=True
        )
        new_conn_id = "new_conn"
        gateway.connections[new_conn_id] = new_connection
        
        # Act
        hijacking_detected = await gateway._detect_session_hijacking(session_id, new_conn_id)
        
        # Assert
        assert hijacking_detected is False
    
    async def test_detect_session_hijacking_no_existing_connections(self):
        """Test session hijacking detection with no existing connections"""
        # Arrange
        session_manager_mock = AsyncMock()
        gateway = WebSocketGateway(session_manager_mock, None, "test_secret")
        
        session_id = "new_session_999"
        
        # Create new connection (no existing connections)
        new_connection = ConnectionInfo(
            websocket=MagicMock(),
            user_id="user999",
            session_id="",
            connected_at=time.time(),
            last_ping=time.time(),
            client_ip="192.168.1.100",
            user_agent="Mozilla/5.0 Chrome/91.0.4472.124",
            is_authenticated=True
        )
        new_conn_id = "new_conn"
        gateway.connections[new_conn_id] = new_connection
        
        # Act
        hijacking_detected = await gateway._detect_session_hijacking(session_id, new_conn_id)
        
        # Assert
        assert hijacking_detected is False  # No existing connections to compare
    
    async def test_detect_session_hijacking_error_handling(self):
        """Test session hijacking detection handles errors gracefully"""
        # Arrange
        session_manager_mock = AsyncMock()
        gateway = WebSocketGateway(session_manager_mock, None, "test_secret")
        
        session_id = "error_session"
        new_conn_id = "nonexistent_connection"  # Connection doesn't exist
        
        # Act
        hijacking_detected = await gateway._detect_session_hijacking(session_id, new_conn_id)
        
        # Assert
        assert hijacking_detected is False  # Should fail safely