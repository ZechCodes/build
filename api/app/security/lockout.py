"""Account lockout protection according to Session 2 requirements."""

from datetime import datetime, timezone, timedelta
from typing import Optional
from sqlalchemy.ext.asyncio import AsyncSession
import structlog

from app.models.user import User

logger = structlog.get_logger(__name__)


class AccountLockoutManager:
    """Account lockout manager for brute force protection."""

    def __init__(self, max_attempts: int = 5, lockout_duration_minutes: int = 30):
        """Initialize account lockout manager.
        
        Session 2 requirements:
        - 5 failed attempts = 30 minute lockout
        """
        self.max_attempts = max_attempts
        self.lockout_duration = timedelta(minutes=lockout_duration_minutes)

    async def record_failed_attempt(self, user: User, db: AsyncSession):
        """Record a failed login attempt and apply lockout if needed."""
        user.failed_login_attempts += 1

        # Apply lockout if max attempts reached
        if user.failed_login_attempts >= self.max_attempts:
            user.locked_until = datetime.now(timezone.utc) + self.lockout_duration
            
            logger.warning(
                "Account locked due to failed login attempts",
                user_id=str(user.id),
                email=user.email,
                failed_attempts=user.failed_login_attempts,
                locked_until=user.locked_until.isoformat()
            )

        await db.commit()

    async def reset_failed_attempts(self, user: User, db: AsyncSession):
        """Reset failed login attempts and clear lockout."""
        previous_attempts = user.failed_login_attempts
        previous_locked_until = user.locked_until

        user.failed_login_attempts = 0
        user.locked_until = None
        
        if previous_attempts > 0 or previous_locked_until:
            logger.info(
                "Failed login attempts reset",
                user_id=str(user.id),
                email=user.email,
                previous_attempts=previous_attempts,
                was_locked=previous_locked_until is not None
            )

        await db.commit()

    def is_locked(self, user: User) -> bool:
        """Check if user account is currently locked."""
        if user.locked_until is None:
            return False
        
        now = datetime.now(timezone.utc)
        # Handle timezone-naive datetime from database
        locked_until = user.locked_until
        if locked_until.tzinfo is None:
            locked_until = locked_until.replace(tzinfo=timezone.utc)
        
        return now < locked_until

    def can_attempt_login(self, user: User) -> bool:
        """Check if user can attempt to login (not locked)."""
        return not self.is_locked(user)

    def get_lockout_remaining_time(self, user: User) -> Optional[timedelta]:
        """Get remaining lockout time, if any."""
        if not self.is_locked(user):
            return None
        
        now = datetime.now(timezone.utc)
        locked_until = user.locked_until
        if locked_until.tzinfo is None:
            locked_until = locked_until.replace(tzinfo=timezone.utc)
        
        remaining = locked_until - now
        return remaining if remaining.total_seconds() > 0 else None

    async def auto_unlock_expired(self, user: User, db: AsyncSession) -> bool:
        """Automatically unlock account if lockout has expired."""
        if user.locked_until is None:
            return False
        
        if not self.is_locked(user):
            # Lockout has expired, reset the account
            logger.info(
                "Auto-unlocking expired account lockout",
                user_id=str(user.id),
                email=user.email,
                was_locked_until=user.locked_until.isoformat()
            )
            
            await self.reset_failed_attempts(user, db)
            return True
        
        return False

    def get_attempts_remaining(self, user: User) -> int:
        """Get number of login attempts remaining before lockout."""
        if self.is_locked(user):
            return 0
        
        return max(0, self.max_attempts - user.failed_login_attempts)

    def get_lockout_info(self, user: User) -> dict:
        """Get comprehensive lockout information for user."""
        return {
            "is_locked": self.is_locked(user),
            "failed_attempts": user.failed_login_attempts,
            "max_attempts": self.max_attempts,
            "attempts_remaining": self.get_attempts_remaining(user),
            "locked_until": user.locked_until,
            "remaining_time": self.get_lockout_remaining_time(user),
            "can_login": self.can_attempt_login(user)
        }

    async def force_unlock(self, user: User, db: AsyncSession, admin_user_id: str = None):
        """Force unlock user account (admin function)."""
        if user.locked_until is not None or user.failed_login_attempts > 0:
            logger.warning(
                "Account force unlocked by admin",
                user_id=str(user.id),
                email=user.email,
                admin_user_id=admin_user_id,
                previous_attempts=user.failed_login_attempts,
                was_locked_until=user.locked_until.isoformat() if user.locked_until else None
            )
            
            await self.reset_failed_attempts(user, db)
            return True
        
        return False