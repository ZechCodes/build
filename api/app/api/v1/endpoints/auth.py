"""Authentication endpoints with enhanced security according to Session 2."""

from fastapi import APIRouter, Depends, HTTPException, status, Request
from sqlalchemy.ext.asyncio import AsyncSession
import structlog

from app.core.deps import get_db
from app.schemas.auth import (
    Token, UserLogin, UserCreate, RefreshToken, LoginResponse, 
    PasswordResetRequest, PasswordReset, UserResponse, UserRegistration,
    TokenResponse, UserProfile
)
from app.services.auth import AuthService
from app.security.dependencies import get_current_active_user, get_current_user
from app.security.lockout import AccountLockoutManager
from app.models.user import User

logger = structlog.get_logger(__name__)

router = APIRouter()
lockout_manager = AccountLockoutManager()


@router.post("/login", response_model=TokenResponse)
async def login(
    user_login: UserLogin,
    request: Request,
    db: AsyncSession = Depends(get_db)
):
    """User login endpoint with enhanced security and account lockout protection."""
    client_ip = request.client.host if request.client else "unknown"
    user_agent = request.headers.get("user-agent")
    
    try:
        result = await AuthService.login(db, user_login, ip_address=client_ip)
        
        return TokenResponse(
            access_token=result["access_token"],
            refresh_token=result["refresh_token"],
            token_type=result["token_type"],
            expires_in=900  # 15 minutes in seconds
        )
    except HTTPException as e:
        logger.warning(
            "Login attempt failed", 
            email=user_login.email, 
            ip_address=client_ip,
            user_agent=user_agent,
            error=str(e.detail)
        )
        raise


@router.post("/register", response_model=UserResponse, status_code=status.HTTP_201_CREATED)
async def register(
    user_create: UserRegistration,
    request: Request,
    db: AsyncSession = Depends(get_db)
):
    """User registration endpoint with enhanced validation."""
    client_ip = request.client.host if request.client else "unknown"
    
    try:
        # Convert UserRegistration to UserCreate for backward compatibility
        user_create_data = UserCreate(
            email=user_create.email,
            username=user_create.username,
            password=user_create.password
        )
        
        user = await AuthService.create_user(db, user_create_data, ip_address=client_ip)
        
        return UserResponse(
            id=str(user.id),
            email=user.email,
            username=user.username,
            is_active=user.is_active,
            is_verified=user.is_verified
        )
    except HTTPException as e:
        logger.warning(
            "Registration attempt failed",
            email=user_create.email,
            ip_address=client_ip,
            error=str(e.detail)
        )
        raise


@router.post("/refresh", response_model=dict)
async def refresh_token(
    refresh_data: RefreshToken,
    request: Request,
    db: AsyncSession = Depends(get_db)
):
    """Refresh JWT token endpoint with validation."""
    client_ip = request.client.host if request.client else "unknown"
    
    try:
        result = await AuthService.refresh_access_token(
            db, refresh_data.refresh_token, ip_address=client_ip
        )
        return result
    except HTTPException as e:
        logger.warning(
            "Token refresh failed",
            ip_address=client_ip,
            error=str(e.detail)
        )
        raise


@router.post("/logout")
async def logout(
    refresh_data: RefreshToken,
    request: Request,
    db: AsyncSession = Depends(get_db)
):
    """User logout endpoint with token revocation."""
    client_ip = request.client.host if request.client else "unknown"
    
    # For now, we'll extract user_id from the refresh token
    # In a real implementation, you'd get this from the access token
    from app.core.security import verify_refresh_token
    user_id = verify_refresh_token(refresh_data.refresh_token)
    
    if not user_id:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Invalid refresh token"
        )
    
    result = await AuthService.logout(
        db, user_id, refresh_data.refresh_token, ip_address=client_ip
    )
    
    return result


@router.post("/request-password-reset")
async def request_password_reset(
    reset_request: PasswordResetRequest,
    request: Request,
    db: AsyncSession = Depends(get_db)
):
    """Request password reset endpoint."""
    client_ip = request.client.host if request.client else "unknown"
    
    result = await AuthService.request_password_reset(
        db, reset_request.email, ip_address=client_ip
    )
    
    return result


@router.post("/reset-password")
async def reset_password(
    reset_data: PasswordReset,
    request: Request,
    db: AsyncSession = Depends(get_db)
):
    """Reset password endpoint."""
    client_ip = request.client.host if request.client else "unknown"
    
    result = await AuthService.reset_password(
        db, reset_data.user_id, reset_data.reset_token, 
        reset_data.new_password, ip_address=client_ip
    )
    
    return result


@router.get("/me", response_model=UserProfile)
async def get_current_user_profile(
    current_user: User = Depends(get_current_active_user)
):
    """Get current user profile with enhanced information."""
    return UserProfile(
        id=str(current_user.id),
        email=current_user.email,
        username=current_user.username,
        role=current_user.role.value,
        is_verified=current_user.is_verified,
        created_at=current_user.created_at
    )


@router.put("/me", response_model=UserProfile)
async def update_user_profile(
    user_updates: dict,  # Define proper schema for updates
    current_user: User = Depends(get_current_active_user),
    db: AsyncSession = Depends(get_db)
):
    """Update current user profile."""
    # Implementation would go here for profile updates
    # For now, just return current profile
    return UserProfile(
        id=str(current_user.id),
        email=current_user.email,
        username=current_user.username,
        role=current_user.role.value,
        is_verified=current_user.is_verified,
        created_at=current_user.created_at
    )


@router.get("/permissions")
async def get_user_permissions(
    current_user: User = Depends(get_current_active_user)
):
    """Get user permissions."""
    from app.authorization.permissions import PermissionChecker
    
    permissions = PermissionChecker.get_user_permissions(current_user)
    return {
        "permissions": [perm.value for perm in permissions],
        "role": current_user.role.value
    }


@router.post("/check-permission")
async def check_permission(
    permission_data: dict,  # {"permission": "vm:create"}
    current_user: User = Depends(get_current_active_user)
):
    """Check if user has specific permission."""
    from app.authorization.permissions import PermissionChecker, Permission
    
    permission_name = permission_data.get("permission")
    if not permission_name:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Permission name required"
        )
    
    try:
        permission = Permission(permission_name)
        has_permission = PermissionChecker.user_has_permission(current_user, permission)
        
        return {
            "permission": permission_name,
            "granted": has_permission,
            "role": current_user.role.value
        }
    except ValueError:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Invalid permission name"
        )