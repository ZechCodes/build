"""Tests for VM endpoint permission decorators."""

import pytest
from httpx import AsyncClient
from sqlalchemy.ext.asyncio import AsyncSession
from unittest.mock import patch
from datetime import datetime, timezone

from app.services.auth import AuthService
from app.schemas.auth import UserCreate
from app.models.user import User, UserRole


class TestVMPermissions:
    """Test VM endpoint permission decorators."""

    @pytest.mark.asyncio
    async def test_create_vm_requires_vm_create_permission(self, client: AsyncClient, db_session: AsyncSession):
        """Test that creating VM requires VM_CREATE permission."""
        # Create user with USER role (should have VM_CREATE permission)
        user_create = UserCreate(
            email="vmcreate@example.com",
            username="vmcreateuser",
            password="VMCreatePassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "vmcreate@example.com",
            "password": "VMCreatePassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Mock VMService.create_vm to avoid actual VM creation
        with patch('app.services.vm.VMService.create_vm') as mock_create:
            # Mock return value
            mock_vm = type('MockVM', (), {
                'id': 'test-vm-id',
                'name': 'test-vm',
                'state': 'stopped',
                'cpu_count': 2,
                'memory_mb': 2048,
                'disk_gb': 20,
                'firecracker_id': None,
                'created_at': datetime.now(timezone.utc),
                'updated_at': datetime.now(timezone.utc),
                'started_at': None,
                'stopped_at': None,
                'config': {}
            })()
            mock_create.return_value = mock_vm
            
            # Should succeed with valid VM_CREATE permission
            response = await client.post(
                "/api/v1/vms/",
                headers={"Authorization": f"Bearer {access_token}"},
                json={
                    "name": "test-vm",
                    "cpu_count": 2,
                    "memory_mb": 2048,
                    "disk_gb": 20
                }
            )
            
            # Should succeed since USER role has VM_CREATE permission
            assert response.status_code == 201

    @pytest.mark.asyncio
    async def test_list_vms_requires_vm_view_permission(self, client: AsyncClient, db_session: AsyncSession):
        """Test that listing VMs requires VM_VIEW permission."""
        # Create user with USER role (should have VM_VIEW permission)
        user_create = UserCreate(
            email="vmview@example.com",
            username="vmviewuser",
            password="VMViewPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "vmview@example.com",
            "password": "VMViewPassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Mock VMService.list_user_vms
        with patch('app.services.vm.VMService.list_user_vms') as mock_list:
            mock_list.return_value = []
            
            # Should succeed with valid VM_VIEW permission
            response = await client.get(
                "/api/v1/vms/",
                headers={"Authorization": f"Bearer {access_token}"}
            )
            
            # Should succeed since USER role has VM_VIEW permission
            assert response.status_code == 200
            assert response.json() == []

    @pytest.mark.asyncio
    async def test_get_vm_requires_vm_view_permission(self, client: AsyncClient, db_session: AsyncSession):
        """Test that getting VM details requires VM_VIEW permission."""
        # Create user with USER role (should have VM_VIEW permission)
        user_create = UserCreate(
            email="vmgetview@example.com",
            username="vmgetviewuser",
            password="VMGetViewPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "vmgetview@example.com",
            "password": "VMGetViewPassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Mock VMService.get_vm
        with patch('app.services.vm.VMService.get_vm') as mock_get:
            mock_vm = type('MockVM', (), {
                'id': 'test-vm-id',
                'name': 'test-vm',
                'state': 'stopped',
                'cpu_count': 2,
                'memory_mb': 2048,
                'disk_gb': 20,
                'firecracker_id': None,
                'created_at': datetime.now(timezone.utc),
                'updated_at': datetime.now(timezone.utc),
                'started_at': None,
                'stopped_at': None,
                'config': {}
            })()
            mock_get.return_value = mock_vm
            
            # Should succeed with valid VM_VIEW permission
            response = await client.get(
                "/api/v1/vms/test-vm-id",
                headers={"Authorization": f"Bearer {access_token}"}
            )
            
            # Should succeed since USER role has VM_VIEW permission
            assert response.status_code == 200

    @pytest.mark.asyncio
    async def test_update_vm_requires_vm_modify_permission(self, client: AsyncClient, db_session: AsyncSession):
        """Test that updating VM requires VM_MODIFY permission."""
        # Create user with USER role (should have VM_MODIFY permission)
        user_create = UserCreate(
            email="vmmodify@example.com",
            username="vmmodifyuser",
            password="VMModifyPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "vmmodify@example.com",
            "password": "VMModifyPassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Mock VMService.update_vm
        with patch('app.services.vm.VMService.update_vm') as mock_update:
            mock_vm = type('MockVM', (), {
                'id': 'test-vm-id',
                'name': 'updated-vm',
                'state': 'stopped',
                'cpu_count': 4,
                'memory_mb': 4096,
                'disk_gb': 20,
                'firecracker_id': None,
                'created_at': datetime.now(timezone.utc),
                'updated_at': datetime.now(timezone.utc),
                'started_at': None,
                'stopped_at': None,
                'config': {}
            })()
            mock_update.return_value = mock_vm
            
            # Should succeed with valid VM_MODIFY permission
            response = await client.put(
                "/api/v1/vms/test-vm-id",
                headers={"Authorization": f"Bearer {access_token}"},
                json={
                    "name": "updated-vm",
                    "cpu_count": 4,
                    "memory_mb": 4096
                }
            )
            
            # Should succeed since USER role has VM_MODIFY permission
            assert response.status_code == 200

    @pytest.mark.asyncio
    async def test_start_vm_requires_vm_modify_permission(self, client: AsyncClient, db_session: AsyncSession):
        """Test that starting VM requires VM_MODIFY permission."""
        # Create user with USER role (should have VM_MODIFY permission)
        user_create = UserCreate(
            email="vmstart@example.com",
            username="vmstartuser",
            password="VMStartPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "vmstart@example.com",
            "password": "VMStartPassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Mock VMService.start_vm
        with patch('app.services.vm.VMService.start_vm') as mock_start:
            mock_vm = type('MockVM', (), {
                'id': 'test-vm-id',
                'name': 'test-vm',
                'state': 'running',
                'cpu_count': 2,
                'memory_mb': 2048,
                'disk_gb': 20,
                'firecracker_id': None,
                'created_at': datetime.now(timezone.utc),
                'updated_at': datetime.now(timezone.utc),
                'started_at': None,
                'stopped_at': None,
                'config': {}
            })()
            mock_start.return_value = mock_vm
            
            # Should succeed with valid VM_MODIFY permission
            response = await client.post(
                "/api/v1/vms/test-vm-id/start",
                headers={"Authorization": f"Bearer {access_token}"}
            )
            
            # Should succeed since USER role has VM_MODIFY permission
            assert response.status_code == 200

    @pytest.mark.asyncio
    async def test_stop_vm_requires_vm_modify_permission(self, client: AsyncClient, db_session: AsyncSession):
        """Test that stopping VM requires VM_MODIFY permission."""
        # Create user with USER role (should have VM_MODIFY permission)
        user_create = UserCreate(
            email="vmstop@example.com",
            username="vmstopuser",
            password="VMStopPassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "vmstop@example.com",
            "password": "VMStopPassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Mock VMService.stop_vm
        with patch('app.services.vm.VMService.stop_vm') as mock_stop:
            mock_vm = type('MockVM', (), {
                'id': 'test-vm-id',
                'name': 'test-vm',
                'state': 'stopped',
                'cpu_count': 2,
                'memory_mb': 2048,
                'disk_gb': 20,
                'firecracker_id': None,
                'created_at': datetime.now(timezone.utc),
                'updated_at': datetime.now(timezone.utc),
                'started_at': None,
                'stopped_at': None,
                'config': {}
            })()
            mock_stop.return_value = mock_vm
            
            # Should succeed with valid VM_MODIFY permission
            response = await client.post(
                "/api/v1/vms/test-vm-id/stop",
                headers={"Authorization": f"Bearer {access_token}"}
            )
            
            # Should succeed since USER role has VM_MODIFY permission
            assert response.status_code == 200

    @pytest.mark.asyncio
    async def test_delete_vm_requires_vm_delete_permission(self, client: AsyncClient, db_session: AsyncSession):
        """Test that deleting VM requires VM_DELETE permission."""
        # Create user with USER role (should have VM_DELETE permission)
        user_create = UserCreate(
            email="vmdelete@example.com",
            username="vmdeleteuser",
            password="VMDeletePassword123!"
        )
        user = await AuthService.create_user(db_session, user_create)
        
        # Login to get token
        login_response = await client.post("/api/v1/auth/login", json={
            "email": "vmdelete@example.com",
            "password": "VMDeletePassword123!"
        })
        
        assert login_response.status_code == 200
        access_token = login_response.json()["access_token"]
        
        # Mock VMService.delete_vm
        with patch('app.services.vm.VMService.delete_vm') as mock_delete:
            mock_delete.return_value = True
            
            # Should succeed with valid VM_DELETE permission
            response = await client.delete(
                "/api/v1/vms/test-vm-id",
                headers={"Authorization": f"Bearer {access_token}"}
            )
            
            # Should succeed since USER role has VM_DELETE permission
            assert response.status_code == 204

    @pytest.mark.asyncio
    async def test_vm_endpoints_require_authentication(self, client: AsyncClient):
        """Test that all VM endpoints require authentication."""
        # Test endpoints without authentication
        endpoints = [
            ("POST", "/api/v1/vms/", {"name": "test", "cpu_count": 2, "memory_mb": 2048, "disk_gb": 20}),
            ("GET", "/api/v1/vms/", None),
            ("GET", "/api/v1/vms/test-vm-id", None),
            ("PUT", "/api/v1/vms/test-vm-id", {"name": "updated"}),
            ("POST", "/api/v1/vms/test-vm-id/start", None),
            ("POST", "/api/v1/vms/test-vm-id/stop", None),
            ("DELETE", "/api/v1/vms/test-vm-id", None),
        ]
        
        for method, url, json_data in endpoints:
            if method == "POST":
                if json_data:
                    response = await client.post(url, json=json_data)
                else:
                    response = await client.post(url)
            elif method == "GET":
                response = await client.get(url)
            elif method == "PUT":
                response = await client.put(url, json=json_data)
            elif method == "DELETE":
                response = await client.delete(url)
            
            # Should require authentication (401) or fail permission check (403)
            assert response.status_code in [401, 403], f"Endpoint {method} {url} should require authentication or permissions"