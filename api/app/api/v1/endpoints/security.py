"""Security endpoints for monitoring and configuration."""

from typing import Dict, Any
from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.ext.asyncio import AsyncSession

from ....core.database import get_db_session
from ....services.security_config import SecurityConfigService, get_security_dashboard, validate_security_compliance
from ....services.audit import AuditService
from ....services.security_checklist import run_security_checklist, get_security_readiness_status


router = APIRouter()


@router.get("/dashboard", summary="Get security dashboard")
async def get_security_overview() -> Dict[str, Any]:
    """Get comprehensive security dashboard with current status."""
    try:
        dashboard = await get_security_dashboard()
        return dashboard
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Failed to retrieve security dashboard: {str(e)}"
        )


@router.get("/scan", summary="Run security scan")
async def run_security_scan(
    db: AsyncSession = Depends(get_db_session)
) -> Dict[str, Any]:
    """Run comprehensive security scan of all components."""
    try:
        security_service = SecurityConfigService(db)
        scan_results = await security_service.run_comprehensive_security_scan()
        return scan_results
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Security scan failed: {str(e)}"
        )


@router.get("/compliance", summary="Check security compliance")
async def check_compliance() -> Dict[str, Any]:
    """Check compliance against security standards (OWASP, CIS)."""
    try:
        compliance_results = await validate_security_compliance()
        return compliance_results
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Compliance check failed: {str(e)}"
        )


@router.get("/database", summary="Database security status")
async def get_database_security(
    db: AsyncSession = Depends(get_db_session)
) -> Dict[str, Any]:
    """Get database security configuration and validation."""
    try:
        security_service = SecurityConfigService(db)
        db_security = await security_service.validate_database_security()
        return db_security
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Database security check failed: {str(e)}"
        )


@router.get("/redis", summary="Redis security status")
async def get_redis_security(
    db: AsyncSession = Depends(get_db_session)
) -> Dict[str, Any]:
    """Get Redis security configuration and validation."""
    try:
        security_service = SecurityConfigService(db)
        redis_security = await security_service.validate_redis_security()
        return redis_security
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Redis security check failed: {str(e)}"
        )


@router.get("/application", summary="Application security status")
async def get_application_security(
    db: AsyncSession = Depends(get_db_session)
) -> Dict[str, Any]:
    """Get application security configuration and validation."""
    try:
        security_service = SecurityConfigService(db)
        app_security = await security_service.validate_application_security()
        return app_security
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Application security check failed: {str(e)}"
        )


@router.post("/harden", summary="Apply security hardening")
async def apply_security_hardening(
    db: AsyncSession = Depends(get_db_session)
) -> Dict[str, Any]:
    """Apply automated security hardening where possible."""
    try:
        security_service = SecurityConfigService(db)
        hardening_results = await security_service.apply_security_hardening()
        return hardening_results
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Security hardening failed: {str(e)}"
        )


@router.get("/events", summary="Get security events")
async def get_security_events(
    severity: str = None,
    hours_back: int = 24,
    limit: int = 100,
    db: AsyncSession = Depends(get_db_session)
) -> Dict[str, Any]:
    """Get recent security events and incidents."""
    try:
        audit_service = AuditService(db)
        events = await audit_service.get_security_events(
            severity=severity,
            hours_back=hours_back,
            limit=limit
        )
        
        return {
            "events": [
                {
                    "id": str(event.id),
                    "action": event.action,
                    "timestamp": event.created_at.isoformat(),
                    "user_id": str(event.user_id) if event.user_id else None,
                    "ip_address": event.ip_address,
                    "user_agent": event.user_agent,
                    "details": event.details
                }
                for event in events
            ],
            "total_count": len(events),
            "severity_filter": severity,
            "time_range_hours": hours_back
        }
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Failed to retrieve security events: {str(e)}"
        )


@router.get("/audit/{user_id}", summary="Get user audit logs")
async def get_user_audit_logs(
    user_id: str,
    limit: int = 100,
    offset: int = 0,
    db: AsyncSession = Depends(get_db_session)
) -> Dict[str, Any]:
    """Get audit logs for a specific user."""
    try:
        audit_service = AuditService(db)
        logs = await audit_service.get_user_audit_logs(
            user_id=user_id,
            limit=limit,
            offset=offset
        )
        
        return {
            "audit_logs": [
                {
                    "id": str(log.id),
                    "action": log.action,
                    "timestamp": log.created_at.isoformat(),
                    "resource_type": log.resource_type,
                    "resource_id": str(log.resource_id) if log.resource_id else None,
                    "ip_address": log.ip_address,
                    "user_agent": log.user_agent,
                    "details": log.details
                }
                for log in logs
            ],
            "user_id": user_id,
            "limit": limit,
            "offset": offset
        }
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Failed to retrieve audit logs: {str(e)}"
        )


@router.get("/failed-logins", summary="Get failed login attempts")
async def get_failed_login_attempts(
    ip_address: str = None,
    user_id: str = None,
    hours_back: int = 1,
    db: AsyncSession = Depends(get_db_session)
) -> Dict[str, Any]:
    """Get failed login attempts for security monitoring."""
    try:
        audit_service = AuditService(db)
        failed_attempts = await audit_service.get_failed_login_attempts(
            ip_address=ip_address,
            user_id=user_id,
            hours_back=hours_back
        )
        
        return {
            "failed_attempts": [
                {
                    "id": str(attempt.id),
                    "timestamp": attempt.created_at.isoformat(),
                    "user_id": str(attempt.user_id) if attempt.user_id else None,
                    "ip_address": attempt.ip_address,
                    "user_agent": attempt.user_agent,
                    "details": attempt.details
                }
                for attempt in failed_attempts
            ],
            "total_count": len(failed_attempts),
            "ip_filter": ip_address,
            "user_filter": user_id,
            "time_range_hours": hours_back
        }
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Failed to retrieve login attempts: {str(e)}"
        )


@router.get("/checklist", summary="Run security checklist validation")
async def run_security_checklist_validation() -> Dict[str, Any]:
    """Run comprehensive security checklist validation for Session 1."""
    try:
        checklist_results = await run_security_checklist()
        return checklist_results
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Security checklist validation failed: {str(e)}"
        )


@router.get("/readiness", summary="Get security readiness status")
async def get_readiness_status() -> Dict[str, Any]:
    """Get security readiness status for Session 1 completion."""
    try:
        readiness_status = await get_security_readiness_status()
        return readiness_status
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Failed to get readiness status: {str(e)}"
        )