"""Permission system according to Session 2 requirements."""

from enum import Enum
from typing import List, Set
from fastapi import HTTPException, status
import structlog

from app.models.user import User, UserRole

logger = structlog.get_logger(__name__)


class Permission(Enum):
    """System permissions for authorization."""
    
    # VM Management
    VM_CREATE = "vm:create"
    VM_DELETE = "vm:delete"
    VM_MODIFY = "vm:modify"
    VM_VIEW = "vm:view"
    
    # Session Management
    SESSION_CREATE = "session:create"
    SESSION_VIEW = "session:view"
    SESSION_TERMINATE = "session:terminate"
    
    # Snapshot Management
    SNAPSHOT_CREATE = "snapshot:create"
    SNAPSHOT_DELETE = "snapshot:delete"
    SNAPSHOT_RESTORE = "snapshot:restore"
    SNAPSHOT_VIEW = "snapshot:view"
    
    # Admin Functions
    USER_MANAGE = "user:manage"
    SYSTEM_ADMIN = "system:admin"


class PermissionChecker:
    """Permission checker for role-based access control."""
    
    # Role-based permission mapping according to Session 2
    role_permissions = {
        UserRole.USER: {
            Permission.VM_CREATE, Permission.VM_DELETE, Permission.VM_MODIFY, Permission.VM_VIEW,
            Permission.SESSION_CREATE, Permission.SESSION_VIEW, Permission.SESSION_TERMINATE,
            Permission.SNAPSHOT_CREATE, Permission.SNAPSHOT_DELETE, Permission.SNAPSHOT_RESTORE, Permission.SNAPSHOT_VIEW
        },
        UserRole.MODERATOR: {
            # Include all user permissions plus moderator-specific ones
            Permission.VM_CREATE, Permission.VM_DELETE, Permission.VM_MODIFY, Permission.VM_VIEW,
            Permission.SESSION_CREATE, Permission.SESSION_VIEW, Permission.SESSION_TERMINATE,
            Permission.SNAPSHOT_CREATE, Permission.SNAPSHOT_DELETE, Permission.SNAPSHOT_RESTORE, Permission.SNAPSHOT_VIEW,
            Permission.USER_MANAGE
        },
        UserRole.ADMIN: {
            # Include all permissions
            *[perm for perm in Permission]
        }
    }
    
    @classmethod
    def user_has_permission(cls, user: User, permission: Permission) -> bool:
        """Check if user has specific permission."""
        user_permissions = cls.role_permissions.get(user.role, set())
        has_permission = permission in user_permissions
        
        logger.debug(
            "Permission check",
            user_id=str(user.id) if user.id else None,
            user_role=user.role.value,
            permission=permission.value,
            granted=has_permission
        )
        
        return has_permission
    
    @classmethod
    def get_user_permissions(cls, user: User) -> Set[Permission]:
        """Get all permissions for a user."""
        return cls.role_permissions.get(user.role, set())
    
    @classmethod
    def has_any_permission(cls, user: User, permissions: List[Permission]) -> bool:
        """Check if user has any of the specified permissions."""
        user_permissions = cls.get_user_permissions(user)
        return any(perm in user_permissions for perm in permissions)
    
    @classmethod
    def has_all_permissions(cls, user: User, permissions: List[Permission]) -> bool:
        """Check if user has all of the specified permissions."""
        user_permissions = cls.get_user_permissions(user)
        return all(perm in user_permissions for perm in permissions)
    
    @classmethod
    def require_permission(cls, permission: Permission):
        """Decorator to require specific permission for endpoint access."""
        def decorator(func):
            async def wrapper(*args, **kwargs):
                # Extract user from dependencies (should be named 'current_user')
                current_user = kwargs.get('current_user')
                if not current_user:
                    logger.warning(
                        "Permission check failed - no user provided",
                        permission=permission.value,
                        function=func.__name__
                    )
                    raise HTTPException(
                        status_code=status.HTTP_401_UNAUTHORIZED,
                        detail="Authentication required"
                    )
                
                if not cls.user_has_permission(current_user, permission):
                    logger.warning(
                        "Permission denied",
                        user_id=str(current_user.id) if current_user.id else None,
                        user_role=current_user.role.value,
                        permission=permission.value,
                        function=func.__name__
                    )
                    raise HTTPException(
                        status_code=status.HTTP_403_FORBIDDEN,
                        detail="Insufficient permissions"
                    )
                
                return await func(*args, **kwargs)
            return wrapper
        return decorator
    
    @classmethod
    def require_any_permission(cls, permissions: List[Permission]):
        """Decorator to require any of the specified permissions."""
        def decorator(func):
            async def wrapper(*args, **kwargs):
                current_user = kwargs.get('current_user')
                if not current_user:
                    raise HTTPException(
                        status_code=status.HTTP_401_UNAUTHORIZED,
                        detail="Authentication required"
                    )
                
                if not cls.has_any_permission(current_user, permissions):
                    logger.warning(
                        "Permission denied - user lacks any required permission",
                        user_id=str(current_user.id) if current_user.id else None,
                        user_role=current_user.role.value,
                        required_permissions=[p.value for p in permissions],
                        function=func.__name__
                    )
                    raise HTTPException(
                        status_code=status.HTTP_403_FORBIDDEN,
                        detail="Insufficient permissions"
                    )
                
                return await func(*args, **kwargs)
            return wrapper
        return decorator
    
    @classmethod
    def require_all_permissions(cls, permissions: List[Permission]):
        """Decorator to require all of the specified permissions."""
        def decorator(func):
            async def wrapper(*args, **kwargs):
                current_user = kwargs.get('current_user')
                if not current_user:
                    raise HTTPException(
                        status_code=status.HTTP_401_UNAUTHORIZED,
                        detail="Authentication required"
                    )
                
                if not cls.has_all_permissions(current_user, permissions):
                    logger.warning(
                        "Permission denied - user lacks all required permissions",
                        user_id=str(current_user.id) if current_user.id else None,
                        user_role=current_user.role.value,
                        required_permissions=[p.value for p in permissions],
                        function=func.__name__
                    )
                    raise HTTPException(
                        status_code=status.HTTP_403_FORBIDDEN,
                        detail="Insufficient permissions"
                    )
                
                return await func(*args, **kwargs)
            return wrapper
        return decorator
    
    @classmethod
    def is_admin(cls, user: User) -> bool:
        """Check if user is admin."""
        return user.role == UserRole.ADMIN
    
    @classmethod
    def is_moderator_or_admin(cls, user: User) -> bool:
        """Check if user is moderator or admin."""
        return user.role in [UserRole.MODERATOR, UserRole.ADMIN]
    
    @classmethod
    def can_manage_user(cls, current_user: User, target_user: User) -> bool:
        """Check if current user can manage target user."""
        # Admins can manage anyone
        if current_user.role == UserRole.ADMIN:
            return True
        
        # Moderators can manage regular users but not other moderators/admins
        if current_user.role == UserRole.MODERATOR:
            return target_user.role == UserRole.USER
        
        # Regular users cannot manage other users
        return False
    
    @classmethod
    def can_access_resource(cls, user: User, resource_owner_id: str, permission: Permission) -> bool:
        """Check if user can access a resource owned by another user."""
        # Check if user has the required permission
        if not cls.user_has_permission(user, permission):
            return False
        
        # Users can always access their own resources
        if str(user.id) == resource_owner_id:
            return True
        
        # Admins can access any resource
        if user.role == UserRole.ADMIN:
            return True
        
        # For other roles, they can only access their own resources
        return False