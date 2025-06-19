"""Authentication endpoints with enhanced security."""

from fastapi import APIRouter, Depends, HTTPException, status, Request
from sqlalchemy.ext.asyncio import AsyncSession
import structlog

from app.core.deps import get_db
from app.schemas.auth import (
    Token, UserLogin, UserCreate, RefreshToken, LoginResponse, 
    PasswordResetRequest, PasswordReset, UserResponse
)
from app.services.auth import AuthService

logger = structlog.get_logger(__name__)

router = APIRouter()


@router.post("/login", response_model=LoginResponse)
async def login(
    user_login: UserLogin,
    request: Request,
    db: AsyncSession = Depends(get_db)
):
    """User login endpoint with enhanced security."""
    client_ip = request.client.host if request.client else "unknown"
    user_agent = request.headers.get("user-agent")
    
    try:
        result = await AuthService.login(db, user_login, ip_address=client_ip)
        
        return LoginResponse(
            access_token=result["access_token"],
            refresh_token=result["refresh_token"],
            token_type=result["token_type"],
            user=UserResponse(**result["user"])
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
    user_create: UserCreate,
    request: Request,
    db: AsyncSession = Depends(get_db)
):
    """User registration endpoint with validation."""
    client_ip = request.client.host if request.client else "unknown"
    
    try:
        user = await AuthService.create_user(db, user_create, ip_address=client_ip)
        
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