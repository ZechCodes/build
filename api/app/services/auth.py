"""Authentication service with enhanced security features."""

import secrets
import string
from datetime import datetime, timedelta, timezone
from typing import Optional, Dict, Any
import uuid

from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select, and_
from fastapi import HTTPException, status
from pydantic import ValidationError
import structlog

from app.core.security import verify_password, get_password_hash
from app.security.jwt import JWTManager
from app.core.config import get_settings
from app.models.user import User
from app.schemas.auth import UserCreate, UserLogin
from app.core.redis import get_redis
from app.services.audit import AuditService

logger = structlog.get_logger(__name__)


class AuthService:
    """Authentication service."""

    @staticmethod
    async def authenticate_user(db: AsyncSession, email: str, password: str, ip_address: str = None) -> Optional[User]:
        """Authenticate user with email and password with database-based account lockout protection."""
        # Get user from database
        result = await db.execute(select(User).where(User.email == email))
        user = result.scalar_one_or_none()
        
        # Check if user exists
        if not user:
            await AuditService.log_security_event(
                db, "login_failed", "authentication",
                user_id=None,
                ip_address=ip_address,
                details={"email": email, "reason": "user_not_found"}
            )
            logger.warning("Login attempt failed", email=email, error="User not found", ip_address=ip_address)
            return None
        
        # Check if account is locked (but unlock if lockout period expired)
        if user.locked_until:
            if user.locked_until > datetime.now(timezone.utc):
                # Account is still locked
                await AuditService.log_security_event(
                    db, "login_failed", "authentication",
                    user_id=user.id,
                    ip_address=ip_address,
                    details={"email": email, "reason": "account_locked"}
                )
                logger.warning("Login attempt on locked account", email=email, ip_address=ip_address, 
                             locked_until=user.locked_until.isoformat())
                raise HTTPException(
                    status_code=status.HTTP_423_LOCKED,
                    detail="Account is temporarily locked due to multiple failed login attempts"
                )
            else:
                # Lockout period expired - unlock the account
                user.failed_login_attempts = 0
                user.locked_until = None
                await db.commit()
                logger.info("Account automatically unlocked after lockout period", 
                           email=email, user_id=str(user.id))
        
        # Check if account is inactive
        if not user.is_active:
            await AuditService.log_security_event(
                db, "login_failed", "authentication",
                user_id=user.id,
                ip_address=ip_address,
                details={"email": email, "reason": "account_inactive"}
            )
            return None
        
        # Verify password
        if not verify_password(password, user.password_hash):
            # Increment failed attempts and potentially lock account
            await AuthService._handle_failed_login(db, user, ip_address)
            
            await AuditService.log_security_event(
                db, "login_failed", "authentication",
                user_id=user.id,
                ip_address=ip_address,
                details={"email": email, "reason": "invalid_password"}
            )
            logger.warning("Login attempt failed", email=email, error="Incorrect email or password", 
                         ip_address=ip_address, user_agent=None)
            return None
        
        # Successful login - reset failed attempts and update last login
        await AuthService._handle_successful_login(db, user, ip_address)
        
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
        
        # Create tokens using JWT manager
        settings = get_settings()
        jwt_manager = JWTManager(secret_key=settings.jwt_secret)
        
        token_data = {
            "sub": str(user.id),
            "email": user.email
        }
        
        access_token = jwt_manager.create_access_token(token_data)
        refresh_token = jwt_manager.create_refresh_token(token_data)
        
        # Store refresh token in Redis with expiration (gracefully handle Redis issues)
        try:
            redis = await get_redis()
            refresh_key = f"refresh_token:{str(user.id)}:{refresh_token[-8:]}"
            await redis.set(refresh_key, refresh_token, ex=7 * 24 * 3600)  # 7 days
        except Exception as e:
            logger.warning("Failed to store refresh token in Redis", error=str(e))
        
        logger.info("User logged in successfully", user_id=str(user.id), email=user.email)
        
        return {
            "access_token": access_token,
            "refresh_token": refresh_token,
            "token_type": "bearer",
            "user": {
                "id": str(user.id),
                "email": user.email,
                "username": user.username,
                "role": user.role.value,
                "is_active": user.is_active,
                "is_verified": user.is_verified
            }
        }

    @staticmethod
    async def refresh_access_token(db: AsyncSession, refresh_token: str, ip_address: str = None) -> dict:
        """Refresh access token using refresh token with validation."""
        from app.security.jwt import JWTManager
        from app.core.config import get_settings
        
        settings = get_settings()
        jwt_manager = JWTManager(secret_key=settings.jwt_secret)
        
        # Verify refresh token
        try:
            payload = jwt_manager.verify_token(refresh_token, token_type="refresh")
            user_id = payload.get("sub")
        except Exception:
            await AuditService.log_security_event(
                db, "token_refresh_failed", "authentication",
                ip_address=ip_address,
                details={"reason": "invalid_token"}
            )
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Invalid refresh token"
            )
        
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
        
        # Verify refresh token is still valid in Redis (gracefully handle Redis issues)
        try:
            redis = await get_redis()
            refresh_key = f"refresh_token:{user_id}:{refresh_token[-8:]}"
            stored_token = await redis.get(refresh_key)
            
            if stored_token and stored_token != refresh_token:
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
        except Exception as e:
            logger.warning("Redis error during token refresh", error=str(e))
            # Continue without Redis validation if Redis is unavailable
        
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
        token_data = {
            "sub": str(user.id),
            "email": user.email
        }
        access_token = jwt_manager.create_access_token(token_data)
        
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
        try:
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
        except Exception as e:
            logger.warning("Redis error during logout", error=str(e))
            # Continue with logout even if Redis fails
        
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
            try:
                redis = await get_redis()
                reset_key = f"password_reset:{str(user.id)}"
                await redis.set(reset_key, reset_token, ex=3600)
            except Exception as e:
                logger.warning("Failed to store password reset token in Redis", error=str(e))
            
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
        try:
            redis = await get_redis()
            reset_key = f"password_reset:{user_id}"
            stored_token = await redis.get(reset_key)
        except Exception as e:
            logger.warning("Redis not available for password reset token verification", error=str(e))
            # If Redis is unavailable, we can't verify the token, so we fail safely
            await AuditService.log_security_event(
                db, "password_reset_failed", "authentication",
                user_id=uuid.UUID(user_id) if user_id else None,
                ip_address=ip_address,
                details={"reason": "token_verification_unavailable"}
            )
            raise HTTPException(
                status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
                detail="Password reset service temporarily unavailable"
            )
        
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
        
        # Remove reset token and revoke refresh tokens
        try:
            await redis.delete(reset_key)
            
            # Revoke all refresh tokens
            pattern = f"refresh_token:{user_id}:*"
            keys = await redis.keys(pattern)
            if keys:
                await redis.delete(*keys)
        except Exception as e:
            logger.warning("Failed to cleanup Redis tokens after password reset", error=str(e))
        
        await AuditService.log_security_event(
            db, "password_reset_completed", "authentication",
            user_id=user.id,
            ip_address=ip_address
        )
        
        logger.info("Password reset completed", user_id=str(user.id))
        
        return {"message": "Password has been reset successfully"}
    
    @staticmethod
    async def update_user_profile(db: AsyncSession, user: User, update_data: dict, ip_address: str = None) -> User:
        """Update user profile with validation."""
        from ..schemas.auth import UserProfileUpdate
        
        # Validate input data
        try:
            validated_data = UserProfileUpdate(**update_data)
        except ValidationError as e:
            raise HTTPException(
                status_code=status.HTTP_422_UNPROCESSABLE_ENTITY,
                detail=e.errors()
            )
        
        # Check for conflicts with existing users
        if validated_data.email and validated_data.email != user.email:
            result = await db.execute(select(User).where(User.email == validated_data.email))
            existing_user = result.scalar_one_or_none()
            if existing_user:
                raise HTTPException(
                    status_code=status.HTTP_409_CONFLICT,
                    detail="Email already exists"
                )
        
        if validated_data.username and validated_data.username != user.username:
            result = await db.execute(select(User).where(User.username == validated_data.username))
            existing_user = result.scalar_one_or_none()
            if existing_user:
                raise HTTPException(
                    status_code=status.HTTP_409_CONFLICT,
                    detail="Username already exists"
                )
        
        # Update user fields
        update_fields = {}
        if validated_data.email is not None:
            user.email = validated_data.email
            update_fields["email"] = validated_data.email
        
        if validated_data.username is not None:
            user.username = validated_data.username
            update_fields["username"] = validated_data.username
        
        # Commit changes
        await db.commit()
        await db.refresh(user)
        
        # Log audit event
        await AuditService.log_security_event(
            db, "profile_updated", "user_management",
            user_id=user.id,
            ip_address=ip_address,
            details={"updated_fields": list(update_fields.keys())}
        )
        
        logger.info("User profile updated", user_id=str(user.id), updated_fields=update_fields)
        
        return user
    
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
    
    @staticmethod
    async def _handle_failed_login(db: AsyncSession, user: User, ip_address: str = None) -> None:
        """Handle failed login attempt - increment counter and potentially lock account."""
        # Increment failed attempts
        user.failed_login_attempts += 1
        
        # Check if we should lock the account (5 failed attempts = lockout)
        if user.failed_login_attempts >= 5:
            # Lock for 30 minutes
            user.locked_until = datetime.now(timezone.utc) + timedelta(minutes=30)
            
            await AuditService.log_security_event(
                db, "account_locked", "authentication",
                user_id=user.id,
                ip_address=ip_address,
                details={
                    "email": user.email,
                    "failed_attempts": user.failed_login_attempts,
                    "locked_until": user.locked_until.isoformat()
                }
            )
            
            logger.warning("Account locked due to failed attempts", 
                         user_id=str(user.id), email=user.email, 
                         failed_attempts=user.failed_login_attempts,
                         locked_until=user.locked_until.isoformat())
        
        await db.commit()
    
    @staticmethod
    async def _handle_successful_login(db: AsyncSession, user: User, ip_address: str = None) -> None:
        """Handle successful login - reset failed attempts and update last login."""
        # Clear failed attempts and unlock account
        user.failed_login_attempts = 0
        user.locked_until = None
        user.last_login = datetime.now(timezone.utc)
        
        # Also clear Redis-based lockout if it exists (graceful fallback)
        try:
            redis = await get_redis()
            lockout_key = f"auth_lockout:{user.email}"
            attempt_key = f"auth_attempts:{user.email}"
            await redis.delete(lockout_key)
            await redis.delete(attempt_key)
        except Exception as e:
            logger.warning("Failed to clear Redis lockout data", error=str(e))
        
        await db.commit()