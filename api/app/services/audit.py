"""Audit logging service for security compliance."""

import time
import structlog
from typing import Any, Dict, Optional, Union
from uuid import UUID
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select

from ..models.audit import AuditLog
from ..models.user import User
from ..core.database import get_db_session


logger = structlog.get_logger(__name__)


class AuditService:
    """Service for managing audit logs."""
    
    @staticmethod
    async def log_action(
        db: AsyncSession,
        action: str,
        user_id: Optional[Union[str, UUID]] = None,
        resource_type: Optional[str] = None,
        resource_id: Optional[Union[str, UUID]] = None,
        ip_address: Optional[str] = None,
        user_agent: Optional[str] = None,
        details: Optional[Dict[str, Any]] = None
    ) -> AuditLog:
        """Log an audit event."""
        
        # Convert string UUIDs to UUID objects
        if isinstance(user_id, str):
            user_id = UUID(user_id)
        if isinstance(resource_id, str):
            resource_id = UUID(resource_id)
        
        audit_log = AuditLog(
            user_id=user_id,
            action=action,
            resource_type=resource_type,
            resource_id=resource_id,
            ip_address=ip_address,
            user_agent=user_agent,
            details=details or {}
        )
        
        db.add(audit_log)
        await db.commit()
        await db.refresh(audit_log)
        
        # Log to structured logging for real-time monitoring
        logger.info(
            "Audit event logged",
            audit_id=str(audit_log.id),
            action=action,
            user_id=str(user_id) if user_id else None,
            resource_type=resource_type,
            resource_id=str(resource_id) if resource_id else None,
            ip_address=ip_address,
            timestamp=audit_log.created_at.isoformat()
        )
        
        return audit_log
    
    @staticmethod
    async def log_security_event(
        db: AsyncSession,
        event_type: str,
        resource_type: str,
        user_id: Optional[Union[str, UUID]] = None,
        ip_address: Optional[str] = None,
        user_agent: Optional[str] = None,
        details: Optional[Dict[str, Any]] = None
    ) -> AuditLog:
        """Log a security-specific event."""
        
        # Determine severity based on event type
        severity_map = {
            "user_created": "info",
            "user_login": "info", 
            "user_logout": "info",
            "login_failed": "warning",
            "password_changed": "info",
            "password_reset": "warning",
            "account_locked": "warning",
            "permission_denied": "warning",
            "suspicious_activity": "high"
        }
        severity = severity_map.get(event_type, "info")
        
        security_details = {
            "severity": severity,
            "event_type": event_type,
            "timestamp": time.time(),
            **(details or {})
        }
        
        return await AuditService.log_action(
            db=db,
            action=event_type,
            user_id=user_id,
            resource_type=resource_type,
            ip_address=ip_address,
            user_agent=user_agent,
            details=security_details
        )
    
    async def log_database_operation(
        self,
        operation: str,
        table: str,
        record_id: Optional[Union[str, UUID]] = None,
        user_id: Optional[Union[str, UUID]] = None,
        details: Optional[Dict[str, Any]] = None
    ) -> AuditLog:
        """Log database operations for compliance."""
        
        db_details = {
            "table": table,
            "operation": operation.upper(),
            "timestamp": time.time(),
            **(details or {})
        }
        
        return await self.log_action(
            action=f"database.{operation.lower()}",
            user_id=user_id,
            resource_type="database",
            resource_id=record_id,
            details=db_details
        )
    
    async def get_user_audit_logs(
        self,
        user_id: Union[str, UUID],
        limit: int = 100,
        offset: int = 0
    ) -> list[AuditLog]:
        """Get audit logs for a specific user."""
        
        if isinstance(user_id, str):
            user_id = UUID(user_id)
        
        query = (
            select(AuditLog)
            .where(AuditLog.user_id == user_id)
            .order_by(AuditLog.created_at.desc())
            .limit(limit)
            .offset(offset)
        )
        
        result = await self.db.execute(query)
        return list(result.scalars().all())
    
    async def get_security_events(
        self,
        severity: Optional[str] = None,
        hours_back: int = 24,
        limit: int = 100
    ) -> list[AuditLog]:
        """Get recent security events."""
        
        query = (
            select(AuditLog)
            .where(AuditLog.action.like("security.%"))
            .order_by(AuditLog.created_at.desc())
        )
        
        if severity:
            query = query.where(AuditLog.details.op("->>")(f"severity") == severity)
        
        if hours_back:
            from datetime import datetime, timedelta
            cutoff_time = datetime.utcnow() - timedelta(hours=hours_back)
            query = query.where(AuditLog.created_at >= cutoff_time)
        
        query = query.limit(limit)
        
        result = await self.db.execute(query)
        return list(result.scalars().all())
    
    async def get_failed_login_attempts(
        self,
        ip_address: Optional[str] = None,
        user_id: Optional[Union[str, UUID]] = None,
        hours_back: int = 1
    ) -> list[AuditLog]:
        """Get failed login attempts for monitoring."""
        
        from datetime import datetime, timedelta
        cutoff_time = datetime.utcnow() - timedelta(hours=hours_back)
        
        query = (
            select(AuditLog)
            .where(AuditLog.action == "auth.login_failed")
            .where(AuditLog.created_at >= cutoff_time)
            .order_by(AuditLog.created_at.desc())
        )
        
        if ip_address:
            query = query.where(AuditLog.ip_address == ip_address)
        
        if user_id:
            if isinstance(user_id, str):
                user_id = UUID(user_id)
            query = query.where(AuditLog.user_id == user_id)
        
        result = await self.db.execute(query)
        return list(result.scalars().all())


# Utility functions for common audit operations

async def log_user_action(
    action: str,
    user_id: Optional[Union[str, UUID]] = None,
    resource_type: Optional[str] = None,
    resource_id: Optional[Union[str, UUID]] = None,
    ip_address: Optional[str] = None,
    user_agent: Optional[str] = None,
    details: Optional[Dict[str, Any]] = None
) -> None:
    """Helper function to log user actions."""
    
    async with get_db_session() as db:
        audit_service = AuditService(db)
        await audit_service.log_action(
            action=action,
            user_id=user_id,
            resource_type=resource_type,
            resource_id=resource_id,
            ip_address=ip_address,
            user_agent=user_agent,
            details=details
        )


async def log_security_event(
    event_type: str,
    severity: str = "medium",
    user_id: Optional[Union[str, UUID]] = None,
    ip_address: Optional[str] = None,
    user_agent: Optional[str] = None,
    details: Optional[Dict[str, Any]] = None
) -> None:
    """Helper function to log security events."""
    
    try:
        from app.core.database import AsyncSessionLocal
        async with AsyncSessionLocal() as db:
            audit_service = AuditService(db)
            await audit_service.log_security_event(
                event_type=event_type,
                severity=severity,
                user_id=user_id,
                ip_address=ip_address,
                user_agent=user_agent,
                details=details
            )
    except Exception as e:
        # Log the error but don't crash the application
        import structlog
        logger = structlog.get_logger(__name__)
        logger.error("Failed to log security event", error=str(e), event_type=event_type)


async def log_database_operation(
    operation: str,
    table: str,
    record_id: Optional[Union[str, UUID]] = None,
    user_id: Optional[Union[str, UUID]] = None,
    details: Optional[Dict[str, Any]] = None
) -> None:
    """Helper function to log database operations."""
    
    async with get_db_session() as db:
        audit_service = AuditService(db)
        await audit_service.log_database_operation(
            operation=operation,
            table=table,
            record_id=record_id,
            user_id=user_id,
            details=details
        )