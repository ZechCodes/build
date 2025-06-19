"""Test authorization and permissions system according to Session 2 requirements."""

import pytest
from unittest.mock import MagicMock
from fastapi import HTTPException

from app.models.user import User, UserRole
from app.authorization.permissions import Permission, PermissionChecker


class TestPermissions:
    """Test permission system functionality."""

    def test_permission_enum_values(self):
        """Test that all required permissions are defined."""
        # VM Management permissions
        assert Permission.VM_CREATE.value == "vm:create"
        assert Permission.VM_DELETE.value == "vm:delete"
        assert Permission.VM_MODIFY.value == "vm:modify"
        assert Permission.VM_VIEW.value == "vm:view"
        
        # Session Management permissions
        assert Permission.SESSION_CREATE.value == "session:create"
        assert Permission.SESSION_VIEW.value == "session:view"
        assert Permission.SESSION_TERMINATE.value == "session:terminate"
        
        # Snapshot Management permissions
        assert Permission.SNAPSHOT_CREATE.value == "snapshot:create"
        assert Permission.SNAPSHOT_DELETE.value == "snapshot:delete"
        assert Permission.SNAPSHOT_RESTORE.value == "snapshot:restore"
        assert Permission.SNAPSHOT_VIEW.value == "snapshot:view"
        
        # Admin permissions
        assert Permission.USER_MANAGE.value == "user:manage"
        assert Permission.SYSTEM_ADMIN.value == "system:admin"

    def test_user_role_permissions(self):
        """Test that role permissions are correctly configured."""
        # Test USER role permissions
        user_permissions = PermissionChecker.role_permissions[UserRole.USER]
        
        # Users should have all basic VM, session, and snapshot permissions
        assert Permission.VM_CREATE in user_permissions
        assert Permission.VM_DELETE in user_permissions
        assert Permission.VM_MODIFY in user_permissions
        assert Permission.VM_VIEW in user_permissions
        
        assert Permission.SESSION_CREATE in user_permissions
        assert Permission.SESSION_VIEW in user_permissions
        assert Permission.SESSION_TERMINATE in user_permissions
        
        assert Permission.SNAPSHOT_CREATE in user_permissions
        assert Permission.SNAPSHOT_DELETE in user_permissions
        assert Permission.SNAPSHOT_RESTORE in user_permissions
        assert Permission.SNAPSHOT_VIEW in user_permissions
        
        # Users should NOT have admin permissions
        assert Permission.USER_MANAGE not in user_permissions
        assert Permission.SYSTEM_ADMIN not in user_permissions

    def test_moderator_role_permissions(self):
        """Test that moderator role has correct permissions."""
        moderator_permissions = PermissionChecker.role_permissions[UserRole.MODERATOR]
        
        # Moderators should have all user permissions
        user_permissions = PermissionChecker.role_permissions[UserRole.USER]
        for perm in user_permissions:
            assert perm in moderator_permissions
        
        # Moderators should have user management permission
        assert Permission.USER_MANAGE in moderator_permissions
        
        # But not system admin
        assert Permission.SYSTEM_ADMIN not in moderator_permissions

    def test_admin_role_permissions(self):
        """Test that admin role has all permissions."""
        admin_permissions = PermissionChecker.role_permissions[UserRole.ADMIN]
        
        # Admins should have ALL permissions
        all_permissions = set(Permission)
        assert admin_permissions == all_permissions

    def test_user_has_permission_success(self):
        """Test successful permission check."""
        user = User(
            email="test@example.com",
            username="testuser",
            password_hash="hashed",
            role=UserRole.USER
        )
        
        # User should have VM creation permission
        assert PermissionChecker.user_has_permission(user, Permission.VM_CREATE)
        assert PermissionChecker.user_has_permission(user, Permission.SESSION_CREATE)
        assert PermissionChecker.user_has_permission(user, Permission.SNAPSHOT_VIEW)

    def test_user_has_permission_failure(self):
        """Test failed permission check."""
        user = User(
            email="test@example.com",
            username="testuser",
            password_hash="hashed",
            role=UserRole.USER
        )
        
        # User should NOT have admin permissions
        assert not PermissionChecker.user_has_permission(user, Permission.USER_MANAGE)
        assert not PermissionChecker.user_has_permission(user, Permission.SYSTEM_ADMIN)

    def test_moderator_permissions(self):
        """Test moderator-specific permissions."""
        moderator = User(
            email="mod@example.com",
            username="moderator",
            password_hash="hashed",
            role=UserRole.MODERATOR
        )
        
        # Moderator should have user management permission
        assert PermissionChecker.user_has_permission(moderator, Permission.USER_MANAGE)
        assert PermissionChecker.user_has_permission(moderator, Permission.VM_CREATE)
        
        # But not system admin
        assert not PermissionChecker.user_has_permission(moderator, Permission.SYSTEM_ADMIN)

    def test_admin_permissions(self):
        """Test admin permissions."""
        admin = User(
            email="admin@example.com",
            username="admin",
            password_hash="hashed",
            role=UserRole.ADMIN
        )
        
        # Admin should have ALL permissions
        for permission in Permission:
            assert PermissionChecker.user_has_permission(admin, permission)

    @pytest.mark.asyncio
    async def test_require_permission_decorator_success(self):
        """Test successful permission decorator."""
        user = User(
            email="test@example.com",
            username="testuser", 
            password_hash="hashed",
            role=UserRole.USER
        )
        
        @PermissionChecker.require_permission(Permission.VM_CREATE)
        async def create_vm(current_user=None):
            return {"message": "VM created"}
        
        # Should succeed for user with VM_CREATE permission
        result = await create_vm(current_user=user)
        assert result["message"] == "VM created"

    @pytest.mark.asyncio
    async def test_require_permission_decorator_failure(self):
        """Test failed permission decorator."""
        user = User(
            email="test@example.com",
            username="testuser",
            password_hash="hashed", 
            role=UserRole.USER
        )
        
        @PermissionChecker.require_permission(Permission.SYSTEM_ADMIN)
        async def admin_function(current_user=None):
            return {"message": "Admin action"}
        
        # Should fail for user without SYSTEM_ADMIN permission
        with pytest.raises(HTTPException) as exc_info:
            await admin_function(current_user=user)
        
        assert exc_info.value.status_code == 403
        assert "Insufficient permissions" in str(exc_info.value.detail)

    @pytest.mark.asyncio
    async def test_require_permission_decorator_no_user(self):
        """Test permission decorator with no user."""
        @PermissionChecker.require_permission(Permission.VM_CREATE)
        async def create_vm(current_user=None):
            return {"message": "VM created"}
        
        # Should fail when no user provided
        with pytest.raises(HTTPException) as exc_info:
            await create_vm(current_user=None)
        
        assert exc_info.value.status_code == 401
        assert "Authentication required" in str(exc_info.value.detail)

    def test_permission_inheritance(self):
        """Test that permission inheritance works correctly."""
        # Test that higher roles include lower role permissions
        user_perms = PermissionChecker.role_permissions[UserRole.USER]
        mod_perms = PermissionChecker.role_permissions[UserRole.MODERATOR]
        admin_perms = PermissionChecker.role_permissions[UserRole.ADMIN]
        
        # All user permissions should be in moderator permissions
        assert user_perms.issubset(mod_perms)
        
        # All moderator permissions should be in admin permissions
        assert mod_perms.issubset(admin_perms)
        
        # Admin should have the most permissions
        assert len(admin_perms) >= len(mod_perms) >= len(user_perms)

    def test_resource_ownership_permissions(self):
        """Test resource ownership validation concept."""
        # This would be implemented in actual endpoint handlers
        user1 = User(id="user1", role=UserRole.USER, email="user1@example.com", username="user1", password_hash="hash")
        user2 = User(id="user2", role=UserRole.USER, email="user2@example.com", username="user2", password_hash="hash")
        
        # Both users have VM_VIEW permission
        assert PermissionChecker.user_has_permission(user1, Permission.VM_VIEW)
        assert PermissionChecker.user_has_permission(user2, Permission.VM_VIEW)
        
        # But in actual implementation, they should only view their own VMs
        # This would be enforced in the service layer, not just permissions

    def test_get_user_permissions(self):
        """Test getting all permissions for a user."""
        user = User(
            email="test@example.com",
            username="testuser",
            password_hash="hashed",
            role=UserRole.USER
        )
        
        permissions = PermissionChecker.get_user_permissions(user)
        
        # Should return set of permissions for user role
        expected_permissions = PermissionChecker.role_permissions[UserRole.USER]
        assert permissions == expected_permissions

    def test_check_multiple_permissions(self):
        """Test checking multiple permissions at once."""
        user = User(
            email="test@example.com",
            username="testuser",
            password_hash="hashed",
            role=UserRole.USER
        )
        
        # Test user has some permissions but not others
        vm_permissions = [Permission.VM_CREATE, Permission.VM_VIEW, Permission.VM_DELETE]
        admin_permissions = [Permission.USER_MANAGE, Permission.SYSTEM_ADMIN]
        
        assert PermissionChecker.has_any_permission(user, vm_permissions)
        assert PermissionChecker.has_all_permissions(user, vm_permissions)
        
        assert not PermissionChecker.has_any_permission(user, admin_permissions)
        assert not PermissionChecker.has_all_permissions(user, admin_permissions)