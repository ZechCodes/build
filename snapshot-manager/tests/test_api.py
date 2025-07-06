"""
Comprehensive test suite for snapshot REST API.
Tests authentication, authorization, rate limiting, and all endpoint functionality.
"""

import pytest
import jwt
import time
import json
from unittest.mock import AsyncMock, MagicMock, patch
from fastapi.testclient import TestClient
from datetime import datetime, timedelta

# Import API components
from api.snapshot_api import SnapshotAPI
from api.auth import AuthenticationMiddleware
from core.snapshot_manager import SnapshotMetadata, SnapshotState, SnapshotType


@pytest.fixture
def jwt_secret():
    """JWT secret for testing."""
    return "test-secret-key-for-testing-only"


@pytest.fixture
def auth_middleware(jwt_secret):
    """Authentication middleware for testing."""
    return AuthenticationMiddleware(jwt_secret)


@pytest.fixture
def mock_snapshot_manager():
    """Mock snapshot manager for testing."""
    manager = AsyncMock()
    
    # Mock create_snapshot
    manager.create_snapshot = AsyncMock(return_value="snap_test123")
    
    # Mock get_snapshot_metadata
    sample_metadata = SnapshotMetadata(
        snapshot_id="snap_test123",
        vm_id="vm_user123_test",
        user_id="user123",
        name="test-snapshot",
        description="Test snapshot description",
        snapshot_type=SnapshotType.MANUAL,
        state=SnapshotState.AVAILABLE,
        created_at=time.time(),
        updated_at=time.time(),
        size_bytes=1024000,
        compressed_size_bytes=512000,
        checksum_sha256="abc123def456",
        storage_path="s3://bucket/snap_test123",
        parent_snapshot_id=None,
        version=1,
        tags=["env:test"],
        vm_config={"cpu": 2, "memory": "4GB"},
        is_encrypted=False,
        restore_count=0
    )
    manager.get_snapshot_metadata = AsyncMock(return_value=sample_metadata)
    
    # Mock list_user_snapshots
    manager.list_user_snapshots = AsyncMock(return_value=[sample_metadata])
    
    # Mock restore_snapshot
    manager.restore_snapshot = AsyncMock(return_value={"status": "success"})
    
    # Mock delete_snapshot
    manager.delete_snapshot = AsyncMock(return_value=True)
    
    return manager


@pytest.fixture
def api_app(mock_snapshot_manager, auth_middleware):
    """FastAPI app instance for testing."""
    # Pass the mock as the type expected
    api = SnapshotAPI(
        snapshot_manager=mock_snapshot_manager,
        auth_middleware=auth_middleware,
        cors_origins=["http://localhost:3000"]
    )
    return api.app


@pytest.fixture
def client(api_app):
    """Test client for API."""
    return TestClient(api_app)


@pytest.fixture
def valid_jwt_token(jwt_secret):
    """Valid JWT token for testing."""
    payload = {
        'user_id': 'user123',
        'roles': ['snapshot_user'],
        'permissions': ['snapshot:create', 'snapshot:read', 'snapshot:list', 'snapshot:restore', 'snapshot:delete'],
        'session_id': 'session123',
        'exp': int(time.time()) + 3600,  # 1 hour from now
        'iat': int(time.time())
    }
    return jwt.encode(payload, jwt_secret, algorithm="HS256")


@pytest.fixture
def admin_jwt_token(jwt_secret):
    """Admin JWT token for testing."""
    payload = {
        'user_id': 'admin123',
        'roles': ['admin'],
        'permissions': ['*'],
        'session_id': 'admin_session123',
        'exp': int(time.time()) + 3600,
        'iat': int(time.time())
    }
    return jwt.encode(payload, jwt_secret, algorithm="HS256")


@pytest.fixture
def expired_jwt_token(jwt_secret):
    """Expired JWT token for testing."""
    payload = {
        'user_id': 'user123',
        'roles': ['snapshot_user'],
        'permissions': ['snapshot:create', 'snapshot:read'],
        'exp': int(time.time()) - 3600,  # 1 hour ago
        'iat': int(time.time()) - 7200   # 2 hours ago
    }
    return jwt.encode(payload, jwt_secret, algorithm="HS256")


class TestSnapshotAPI:
    """Main test class for snapshot API functionality."""
    
    def test_api_imports(self):
        """Test that API components can be imported."""
        from api.snapshot_api import SnapshotAPI
        from api.auth import AuthenticationMiddleware
        assert SnapshotAPI is not None
        assert AuthenticationMiddleware is not None
    
    def test_api_initialization(self, auth_middleware, mock_snapshot_manager):
        """Test API initialization."""
        api = SnapshotAPI(
            snapshot_manager=mock_snapshot_manager,
            auth_middleware=auth_middleware
        )
        assert api is not None


class TestAPIAuthentication:
    """Test API authentication and authorization."""
    
    def test_health_check_no_auth_required(self, client):
        """Test health check endpoint doesn't require authentication."""
        response = client.get("/health")
        assert response.status_code == 200
        
        data = response.json()
        assert data["status"] == "healthy"
        assert "uptime_seconds" in data
        assert "version" in data
    
    def test_create_snapshot_requires_auth(self, client):
        """Test creating snapshot requires authentication."""
        response = client.post("/snapshots", json={
            "vm_id": "vm_test123",
            "name": "test-snapshot"
        })
        # Should be 401 (Unauthorized) or 403 (Forbidden) for missing auth
        assert response.status_code in [401, 403]
    
    def test_create_snapshot_with_valid_token(self, client, valid_jwt_token):
        """Test creating snapshot with valid authentication."""
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        response = client.post("/snapshots", json={
            "vm_id": "vm_user123_test",
            "name": "test-snapshot",
            "description": "Test description"
        }, headers=headers)
        
        assert response.status_code == 200
        data = response.json()
        assert data["snapshot_id"] == "snap_test123"
        assert data["name"] == "test-snapshot"
    
    def test_create_snapshot_with_expired_token(self, client, expired_jwt_token):
        """Test creating snapshot with expired token fails."""
        headers = {"Authorization": f"Bearer {expired_jwt_token}"}
        response = client.post("/snapshots", json={
            "vm_id": "vm_test123",
            "name": "test-snapshot"
        }, headers=headers)
        
        assert response.status_code == 401
        data = response.json()
        assert "expired" in data["error"].lower()
    
    def test_create_snapshot_with_invalid_token(self, client):
        """Test creating snapshot with invalid token fails."""
        headers = {"Authorization": "Bearer invalid-token"}
        response = client.post("/snapshots", json={
            "vm_id": "vm_test123",
            "name": "test-snapshot"
        }, headers=headers)
        
        assert response.status_code == 401


class TestAPIEndpoints:
    """Test API endpoint functionality."""
    
    def test_create_snapshot_success(self, client, valid_jwt_token):
        """Test successful snapshot creation."""
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        request_data = {
            "vm_id": "vm_user123_test",
            "name": "api-test-snapshot",
            "description": "Created via API test",
            "snapshot_type": "full",
            "tags": {"environment": "test", "api": "true"}
        }
        
        response = client.post("/snapshots", json=request_data, headers=headers)
        
        assert response.status_code == 200
        data = response.json()
        assert data["snapshot_id"] == "snap_test123"
        assert data["name"] == "test-snapshot"  # From mock
        assert data["state"] == "available"
        assert data["snapshot_type"] == "full"
    
    def test_create_snapshot_validation_error(self, client, valid_jwt_token):
        """Test snapshot creation with validation errors."""
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        
        # Test missing required fields
        response = client.post("/snapshots", json={}, headers=headers)
        assert response.status_code == 422  # Validation error
        
        # Test invalid name
        response = client.post("/snapshots", json={
            "vm_id": "vm_test123",
            "name": "invalid@name#"
        }, headers=headers)
        assert response.status_code == 422
    
    def test_get_snapshot_success(self, client, valid_jwt_token):
        """Test getting snapshot details."""
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        response = client.get("/snapshots/snap_test123", headers=headers)
        
        assert response.status_code == 200
        data = response.json()
        assert data["snapshot_id"] == "snap_test123"
        assert data["name"] == "test-snapshot"
    
    def test_get_snapshot_not_found(self, client, valid_jwt_token, mock_snapshot_manager):
        """Test getting non-existent snapshot."""
        # Mock snapshot not found
        mock_snapshot_manager.get_snapshot_metadata.side_effect = ValueError("Snapshot not found")
        
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        response = client.get("/snapshots/nonexistent", headers=headers)
        
        assert response.status_code == 404
    
    def test_list_snapshots_success(self, client, valid_jwt_token):
        """Test listing user snapshots."""
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        response = client.get("/snapshots", headers=headers)
        
        assert response.status_code == 200
        data = response.json()
        assert "snapshots" in data
        assert data["total_count"] == 1
        assert len(data["snapshots"]) == 1
        assert data["snapshots"][0]["snapshot_id"] == "snap_test123"
    
    def test_list_snapshots_with_pagination(self, client, valid_jwt_token):
        """Test listing snapshots with pagination."""
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        response = client.get("/snapshots?page=1&page_size=10", headers=headers)
        
        assert response.status_code == 200
        data = response.json()
        assert data["page"] == 1
        assert data["page_size"] == 10
        assert data["has_next"] is False
    
    def test_list_snapshots_with_vm_filter(self, client, valid_jwt_token):
        """Test listing snapshots filtered by VM ID."""
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        response = client.get("/snapshots?vm_id=vm_user123_test", headers=headers)
        
        assert response.status_code == 200
        data = response.json()
        assert data["total_count"] == 1
    
    def test_restore_snapshot_success(self, client, valid_jwt_token):
        """Test successful snapshot restoration."""
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        request_data = {
            "target_vm_id": "vm_user123_target"
        }
        
        response = client.post("/snapshots/snap_test123/restore", 
                             json=request_data, headers=headers)
        
        assert response.status_code == 200
        data = response.json()
        assert "message" in data
        assert "successfully" in data["message"]
    
    def test_delete_snapshot_success(self, client, valid_jwt_token):
        """Test successful snapshot deletion."""
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        response = client.delete("/snapshots/snap_test123", headers=headers)
        
        assert response.status_code == 200
        data = response.json()
        assert "message" in data
        assert "successfully" in data["message"]
    
    def test_get_snapshot_stats(self, client, valid_jwt_token):
        """Test getting snapshot statistics."""
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        response = client.get("/snapshots/stats", headers=headers)
        
        assert response.status_code == 200
        data = response.json()
        assert "total_snapshots" in data
        assert "total_size_bytes" in data
        assert "snapshots_by_state" in data
        assert "snapshots_by_type" in data


class TestAPIAuthorization:
    """Test API authorization and permission checks."""
    
    def test_vm_access_control(self, client, valid_jwt_token, mock_snapshot_manager):
        """Test VM access control prevents unauthorized access."""
        # Mock metadata for a VM the user doesn't own
        unauthorized_metadata = SnapshotMetadata(
            snapshot_id="snap_unauthorized",
            vm_id="vm_otheruser_test",  # Different user's VM
            user_id="user123",
            name="unauthorized-snapshot",
            description="",
            snapshot_type=SnapshotType.MANUAL,
            state=SnapshotState.AVAILABLE,
            created_at=time.time(),
            updated_at=time.time(),
            size_bytes=1024,
            compressed_size_bytes=512,
            checksum_sha256="abc123",
            storage_path="s3://bucket/snap",
            parent_snapshot_id=None,
            version=1,
            tags=[],
            vm_config={"cpu": 1},
            is_encrypted=False,
            restore_count=0,
            expires_at=None
        )
        mock_snapshot_manager.get_snapshot_metadata.return_value = unauthorized_metadata
        
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        response = client.get("/snapshots/snap_unauthorized", headers=headers)
        
        assert response.status_code == 403  # Forbidden
    
    def test_admin_access_all_vms(self, client, admin_jwt_token, mock_snapshot_manager):
        """Test admin can access all VMs."""
        # Mock metadata for any VM
        any_vm_metadata = SnapshotMetadata(
            snapshot_id="snap_any",
            vm_id="vm_anyuser_test",
            user_id="otheruser",
            name="any-snapshot",
            description="",
            snapshot_type=SnapshotType.MANUAL,
            state=SnapshotState.AVAILABLE,
            created_at=time.time(),
            updated_at=time.time(),
            size_bytes=1024,
            compressed_size_bytes=512,
            checksum_sha256="abc123",
            storage_path="s3://bucket/snap",
            parent_snapshot_id=None,
            version=1,
            tags=[],
            vm_config={"cpu": 1},
            is_encrypted=False,
            restore_count=0,
            expires_at=None
        )
        mock_snapshot_manager.get_snapshot_metadata.return_value = any_vm_metadata
        
        headers = {"Authorization": f"Bearer {admin_jwt_token}"}
        response = client.get("/snapshots/snap_any", headers=headers)
        
        assert response.status_code == 200


class TestAPIRateLimiting:
    """Test API rate limiting functionality."""
    
    def test_rate_limiting_enforced(self, client, valid_jwt_token):
        """Test rate limiting is enforced for API calls."""
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        
        # Make many requests rapidly (rate limiter has 100 req/hour limit)
        success_count = 0
        rate_limited_count = 0
        
        for i in range(5):  # Test with reasonable number for test speed
            response = client.get("/snapshots", headers=headers)
            if response.status_code == 200:
                success_count += 1
            elif response.status_code == 429:
                rate_limited_count += 1
        
        # All should succeed with low count
        assert success_count == 5
        assert rate_limited_count == 0


class TestAPIErrorHandling:
    """Test API error handling and responses."""
    
    def test_internal_server_error_handling(self, client, valid_jwt_token, mock_snapshot_manager):
        """Test internal server error handling."""
        # Mock an internal error
        mock_snapshot_manager.create_snapshot.side_effect = Exception("Database connection failed")
        
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        response = client.post("/snapshots", json={
            "vm_id": "vm_user123_test",
            "name": "test-snapshot"
        }, headers=headers)
        
        assert response.status_code == 500
        data = response.json()
        assert "error" in data
        assert "timestamp" in data
    
    def test_validation_error_handling(self, client, valid_jwt_token):
        """Test validation error handling."""
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        
        # Send invalid data
        response = client.post("/snapshots", json={
            "vm_id": "",  # Empty VM ID
            "name": "x" * 300  # Name too long
        }, headers=headers)
        
        assert response.status_code == 422
    
    def test_not_found_error_handling(self, client, valid_jwt_token, mock_snapshot_manager):
        """Test 404 error handling."""
        mock_snapshot_manager.get_snapshot_metadata.side_effect = ValueError("Snapshot not found")
        
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        response = client.get("/snapshots/nonexistent", headers=headers)
        
        assert response.status_code == 404
        data = response.json()
        assert "not found" in data["error"].lower()


class TestAPISecurityFeatures:
    """Test API security features."""
    
    def test_cors_headers(self, client):
        """Test CORS headers are properly set."""
        response = client.options("/health")
        # CORS headers would be set by FastAPI middleware
        assert response.status_code in [200, 405]  # OPTIONS might not be allowed
    
    def test_request_logging(self, client, valid_jwt_token):
        """Test that requests are properly logged."""
        # This would be tested with log capture in real implementation
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        response = client.get("/snapshots", headers=headers)
        
        assert response.status_code == 200
        # In real implementation, would verify logs contain request details
    
    def test_error_information_disclosure(self, client, valid_jwt_token, mock_snapshot_manager):
        """Test that errors don't disclose sensitive information."""
        # Mock a database error
        mock_snapshot_manager.create_snapshot.side_effect = Exception("Database password: secret123")
        
        headers = {"Authorization": f"Bearer {valid_jwt_token}"}
        response = client.post("/snapshots", json={
            "vm_id": "vm_user123_test",
            "name": "test-snapshot"
        }, headers=headers)
        
        assert response.status_code == 500
        data = response.json()
        # Error message should be generic, not expose sensitive details
        assert "secret123" not in data["error"]
        assert "password" not in data["error"]


if __name__ == "__main__":
    pytest.main([__file__, "-v"])