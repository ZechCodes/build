"""Authentication service with enhanced security features."""

import secrets
import string
from datetime import datetime, timedelta, timezone
from typing import Optional, Dict, Any
import uuid

from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select, and_
from fastapi import HTTPException, status
import structlog

from app.core.security import verify_password, get_password_hash, create_access_token, create_refresh_token, verify_refresh_token
from app.models.user import User
from app.schemas.auth import UserCreate, UserLogin
from app.core.redis import get_redis
from app.services.audit import AuditService

logger = structlog.get_logger(__name__)


class AuthService:
    """Authentication service."""

    @staticmethod
    async def authenticate_user(db: AsyncSession, email: str, password: str, ip_address: str = None) -> Optional[User]:
        """Authenticate user with email and password with brute force protection."""
        # Check for account lockout
        redis = await get_redis()
        lockout_key = f"auth_lockout:{email}"
        attempt_key = f"auth_attempts:{email}"
        
        # Check if account is locked
        is_locked = await redis.get(lockout_key)
        if is_locked:
            logger.warning("Authentication attempt on locked account", email=email, ip_address=ip_address)
            raise HTTPException(
                status_code=status.HTTP_429_TOO_MANY_REQUESTS,
                detail="Account temporarily locked due to multiple failed login attempts"
            )
        
        result = await db.execute(select(User).where(User.email == email))
        user = result.scalar_one_or_none()
        
        if not user or not verify_password(password, user.password_hash):
            # Increment failed attempts
            attempts = await redis.incr(attempt_key)
            await redis.expire(attempt_key, 900)  # 15 minutes
            
            if attempts >= 5:  # Lock after 5 failed attempts
                await redis.setex(lockout_key, 1800, "locked")  # 30 minutes lockout
                await AuditService.log_security_event(
                    db, "account_locked", "authentication", 
                    user_id=user.id if user else None, 
                    ip_address=ip_address,
                    details={"email": email, "failed_attempts": attempts}
                )
                logger.warning("Account locked due to failed attempts", email=email, attempts=attempts)
            
            await AuditService.log_security_event(
                db, "login_failed", "authentication",
                user_id=user.id if user else None,
                ip_address=ip_address,
                details={"email": email, "reason": "invalid_credentials"}
            )
            return None
        
        if not user.is_active:
            await AuditService.log_security_event(
                db, "login_failed", "authentication",
                user_id=user.id,
                ip_address=ip_address,
                details={"email": email, "reason": "account_inactive"}
            )
            return None
        
        # Clear failed attempts on successful login
        await redis.delete(attempt_key)
        
        # Update last login
        user.last_login = datetime.now(timezone.utc)
        await db.commit()
        await db.refresh(user)
        
        await AuditService.log_security_event(
            db, "login_success", "authentication",
            user_id=user.id,
            ip_address=ip_address,
            details={"email": email}
        )
        
        return user

    @staticmethod
    async def create_user(db: AsyncSession, user_create: UserCreate, ip_address: str = None) -> User:
        """Create a new user with validation and audit logging."""
        # Validate password strength
        if not AuthService._validate_password_strength(user_create.password):
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Password does not meet security requirements"
            )
        
        # Check if user already exists (email or username)
        result = await db.execute(
            select(User).where(
                (User.email == user_create.email) | 
                (User.username == getattr(user_create, 'username', ''))
            )
        )
        existing_user = result.scalar_one_or_none()
        if existing_user:
            detail = "Email already registered" if existing_user.email == user_create.email else "Username already taken"
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=detail
            )
        
        # Generate username if not provided
        username = getattr(user_create, 'username', None)
        if not username:
            username = user_create.email.split('@')[0]
            # Ensure username uniqueness
            base_username = username
            counter = 1
            while True:
                result = await db.execute(select(User).where(User.username == username))
                if not result.scalar_one_or_none():
                    break
                username = f"{base_username}{counter}"
                counter += 1
        
        # Create new user
        hashed_password = get_password_hash(user_create.password)
        user = User(
            email=user_create.email,
            username=username,
            password_hash=hashed_password,
            is_active=True,
            is_verified=False,  # Require email verification
        )
        
        db.add(user)
        await db.commit()
        await db.refresh(user)
        
        await AuditService.log_security_event(
            db, "user_created", "authentication",
            user_id=user.id,
            ip_address=ip_address,
            details={"email": user.email, "username": user.username}
        )
        
        logger.info("User created successfully", user_id=str(user.id), email=user.email)
        
        return user

    @staticmethod
    async def login(db: AsyncSession, user_login: UserLogin, ip_address: str = None) -> dict:
        """Login user and return tokens with session management."""
        user = await AuthService.authenticate_user(db, user_login.email, user_login.password, ip_address)
        
        if not user:
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Incorrect email or password",
                headers={"WWW-Authenticate": "Bearer"},
            )
        
        if not user.is_active:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Account is inactive"
            )
        
        # Create tokens
        access_token = create_access_token(subject=str(user.id))
        refresh_token = create_refresh_token(subject=str(user.id))
        
        # Store refresh token in Redis with expiration
        redis = await get_redis()
        refresh_key = f"refresh_token:{str(user.id)}:{refresh_token[-8:]}"
        await redis.setex(refresh_key, 7 * 24 * 3600, refresh_token)  # 7 days
        
        logger.info("User logged in successfully", user_id=str(user.id), email=user.email)
        
        return {
            "access_token": access_token,
            "refresh_token": refresh_token,
            "token_type": "bearer",
            "user": {
                "id": str(user.id),
                "email": user.email,
                "username": user.username,
                "is_verified": user.is_verified
            }
        }

    @staticmethod
    async def refresh_access_token(db: AsyncSession, refresh_token: str, ip_address: str = None) -> dict:
        """Refresh access token using refresh token with validation."""
        user_id = verify_refresh_token(refresh_token)
        
        if not user_id:
            await AuditService.log_security_event(
                db, "token_refresh_failed", "authentication",
                ip_address=ip_address,
                details={"reason": "invalid_token"}
            )
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Invalid refresh token"
            )
        
        # Verify refresh token is still valid in Redis
        redis = await get_redis()
        refresh_key = f"refresh_token:{user_id}:{refresh_token[-8:]}"
        stored_token = await redis.get(refresh_key)
        
        if not stored_token or stored_token != refresh_token:
            await AuditService.log_security_event(
                db, "token_refresh_failed", "authentication",
                user_id=uuid.UUID(user_id) if user_id else None,
                ip_address=ip_address,
                details={"reason": "token_revoked"}
            )
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Refresh token has been revoked"
            )
        
        # Get user
        result = await db.execute(select(User).where(User.id == uuid.UUID(user_id)))
        user = result.scalar_one_or_none()
        
        if not user or not user.is_active:
            await AuditService.log_security_event(
                db, "token_refresh_failed", "authentication",
                user_id=uuid.UUID(user_id) if user_id else None,
                ip_address=ip_address,
                details={"reason": "user_inactive_or_not_found"}
            )
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="User not found or inactive"
            )
        
        # Create new access token
        access_token = create_access_token(subject=str(user.id))
        
        await AuditService.log_security_event(
            db, "token_refreshed", "authentication",
            user_id=user.id,
            ip_address=ip_address
        )
        
        return {
            "access_token": access_token,
            "token_type": "bearer"
        }
    
    @staticmethod
    async def logout(db: AsyncSession, user_id: str, refresh_token: str = None, ip_address: str = None) -> dict:
        """Logout user and revoke tokens."""
        redis = await get_redis()
        
        if refresh_token:
            # Revoke specific refresh token
            refresh_key = f"refresh_token:{user_id}:{refresh_token[-8:]}"
            await redis.delete(refresh_key)
        else:
            # Revoke all refresh tokens for user
            pattern = f"refresh_token:{user_id}:*"
            keys = await redis.keys(pattern)
            if keys:
                await redis.delete(*keys)
        
        await AuditService.log_security_event(
            db, "logout", "authentication",
            user_id=uuid.UUID(user_id),
            ip_address=ip_address
        )
        
        logger.info("User logged out successfully", user_id=user_id)
        
        return {"message": "Successfully logged out"}
    
    @staticmethod
    async def request_password_reset(db: AsyncSession, email: str, ip_address: str = None) -> dict:
        """Request password reset for user."""
        result = await db.execute(select(User).where(User.email == email))
        user = result.scalar_one_or_none()
        
        # Always return success to prevent email enumeration
        if user and user.is_active:
            # Generate reset token
            reset_token = AuthService._generate_reset_token()
            
            # Store reset token in Redis (expires in 1 hour)
            redis = await get_redis()
            reset_key = f"password_reset:{str(user.id)}"
            await redis.setex(reset_key, 3600, reset_token)
            
            await AuditService.log_security_event(
                db, "password_reset_requested", "authentication",
                user_id=user.id,
                ip_address=ip_address,
                details={"email": email}
            )
            
            # TODO: Send email with reset token
            logger.info("Password reset requested", user_id=str(user.id), email=email)
        
        return {"message": "If the email exists, a password reset link has been sent"}
    
    @staticmethod
    async def reset_password(db: AsyncSession, user_id: str, reset_token: str, new_password: str, ip_address: str = None) -> dict:
        """Reset user password with token validation."""
        # Validate password strength
        if not AuthService._validate_password_strength(new_password):
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Password does not meet security requirements"
            )
        
        # Verify reset token
        redis = await get_redis()
        reset_key = f"password_reset:{user_id}"
        stored_token = await redis.get(reset_key)
        
        if not stored_token or stored_token != reset_token:
            await AuditService.log_security_event(
                db, "password_reset_failed", "authentication",
                user_id=uuid.UUID(user_id) if user_id else None,
                ip_address=ip_address,
                details={"reason": "invalid_token"}
            )
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Invalid or expired reset token"
            )
        
        # Get user
        result = await db.execute(select(User).where(User.id == uuid.UUID(user_id)))
        user = result.scalar_one_or_none()
        
        if not user or not user.is_active:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="User not found"
            )
        
        # Update password
        user.password_hash = get_password_hash(new_password)
        await db.commit()
        
        # Remove reset token
        await redis.delete(reset_key)
        
        # Revoke all refresh tokens
        pattern = f"refresh_token:{user_id}:*"
        keys = await redis.keys(pattern)
        if keys:
            await redis.delete(*keys)
        
        await AuditService.log_security_event(
            db, "password_reset_completed", "authentication",
            user_id=user.id,
            ip_address=ip_address
        )
        
        logger.info("Password reset completed", user_id=str(user.id))
        
        return {"message": "Password has been reset successfully"}
    
    @staticmethod
    def _validate_password_strength(password: str) -> bool:
        """Validate password meets security requirements."""
        if len(password) < 8:
            return False
        
        has_upper = any(c.isupper() for c in password)
        has_lower = any(c.islower() for c in password)
        has_digit = any(c.isdigit() for c in password)
        has_special = any(c in "!@#$%^&*()_+-=[]{}|;:,.<>?" for c in password)
        
        return has_upper and has_lower and has_digit and has_special
    
    @staticmethod
    def _generate_reset_token() -> str:
        """Generate secure reset token."""
        alphabet = string.ascii_letters + string.digits
        return ''.join(secrets.choice(alphabet) for _ in range(32))
    
    @staticmethod
    async def cleanup_expired_sessions() -> None:
        """Background task to cleanup expired sessions and tokens."""
        redis = await get_redis()
        
        # This is handled automatically by Redis TTL, but we can add
        # additional cleanup logic here if needed
        logger.info("Session cleanup task completed")