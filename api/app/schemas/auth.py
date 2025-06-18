"""Authentication schemas."""

from typing import Optional
from pydantic import BaseModel, EmailStr


class Token(BaseModel):
    """Token response schema."""
    access_token: str
    refresh_token: str
    token_type: str = "bearer"


class TokenData(BaseModel):
    """Token data schema."""
    user_id: Optional[int] = None


class UserLogin(BaseModel):
    """User login schema."""
    email: EmailStr
    password: str


class UserCreate(BaseModel):
    """User creation schema."""
    email: EmailStr
    password: str
    full_name: Optional[str] = None
    is_superuser: bool = False


class RefreshToken(BaseModel):
    """Refresh token schema."""
    refresh_token: str