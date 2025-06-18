"""Authentication endpoints."""

from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.ext.asyncio import AsyncSession

from app.core.deps import get_db
from app.schemas.auth import Token, UserLogin, UserCreate, RefreshToken
from app.schemas.user import User
from app.services.auth import AuthService

router = APIRouter()


@router.post("/login", response_model=Token)
async def login(
    user_login: UserLogin,
    db: AsyncSession = Depends(get_db)
):
    """User login endpoint."""
    result = await AuthService.login(db, user_login)
    
    return Token(
        access_token=result["access_token"],
        refresh_token=result["refresh_token"],
        token_type=result["token_type"]
    )


@router.post("/register", response_model=User)
async def register(
    user_create: UserCreate,
    db: AsyncSession = Depends(get_db)
):
    """User registration endpoint."""
    user = await AuthService.create_user(db, user_create)
    return user


@router.post("/refresh", response_model=dict)
async def refresh_token(
    refresh_data: RefreshToken,
    db: AsyncSession = Depends(get_db)
):
    """Refresh JWT token endpoint."""
    result = await AuthService.refresh_access_token(db, refresh_data.refresh_token)
    return result


@router.post("/logout")
async def logout():
    """User logout endpoint."""
    # In a stateless JWT system, logout is typically handled client-side
    # by discarding the tokens. For enhanced security, you might implement
    # a token blacklist stored in Redis.
    return {"message": "Successfully logged out"}