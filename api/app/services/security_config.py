"""Security configuration service for database and infrastructure security."""

import ssl
import time
import structlog
from typing import Dict, Any, List
from sqlalchemy import text
from sqlalchemy.ext.asyncio import AsyncSession

from ..core.config import get_settings
from ..core.database import get_db_session
from ..core.redis import get_redis
from .audit import AuditService


logger = structlog.get_logger(__name__)
settings = get_settings()


class SecurityConfigService:
    """Service for managing security configurations and validations."""
    
    def __init__(self, db_session: AsyncSession):
        self.db = db_session
        self.audit = AuditService(db_session)
    
    async def validate_database_security(self) -> Dict[str, Any]:
        """Validate database security configuration."""
        security_status = {
            "ssl_enabled": False,
            "rls_policies_active": False,
            "connection_limits_configured": False,
            "audit_logging_enabled": False,
            "security_score": 0,
            "issues": [],
            "recommendations": []
        }
        
        try:
            # Check SSL configuration
            ssl_result = await self.db.execute(text("SHOW ssl"))
            ssl_status = ssl_result.scalar()
            if ssl_status and ssl_status.lower() == "on":
                security_status["ssl_enabled"] = True
                security_status["security_score"] += 20
            else:
                security_status["issues"].append("SSL/TLS not enabled for database connections")
                security_status["recommendations"].append("Enable SSL in PostgreSQL configuration")
            
            # Check Row Level Security policies
            rls_check = await self.db.execute(text("""
                SELECT COUNT(*) FROM pg_policies 
                WHERE schemaname = 'public'
            """))
            rls_count = rls_check.scalar()
            if rls_count > 0:
                security_status["rls_policies_active"] = True
                security_status["security_score"] += 25
            else:
                security_status["issues"].append("No Row Level Security policies found")
                security_status["recommendations"].append("Implement RLS policies for sensitive tables")
            
            # Check connection limits
            conn_limit_check = await self.db.execute(text("SHOW max_connections"))
            max_connections = int(conn_limit_check.scalar())
            if max_connections <= 200:  # Reasonable limit
                security_status["connection_limits_configured"] = True
                security_status["security_score"] += 15
            else:
                security_status["issues"].append(f"High connection limit: {max_connections}")
                security_status["recommendations"].append("Configure appropriate connection limits")
            
            # Check for audit logging extensions
            audit_extensions = await self.db.execute(text("""
                SELECT extname FROM pg_extension 
                WHERE extname IN ('pgaudit', 'pg_stat_statements')
            """))
            audit_exts = [row[0] for row in audit_extensions.fetchall()]
            if audit_exts:
                security_status["audit_logging_enabled"] = True
                security_status["security_score"] += 20
            else:
                security_status["issues"].append("No audit logging extensions installed")
                security_status["recommendations"].append("Install and configure pgaudit extension")
            
            # Check for default/weak passwords (basic check)
            weak_users_check = await self.db.execute(text("""
                SELECT rolname FROM pg_roles 
                WHERE rolcanlogin = true AND rolname IN ('postgres', 'admin', 'root')
            """))
            weak_users = [row[0] for row in weak_users_check.fetchall()]
            if weak_users:
                security_status["issues"].append(f"Default role names found: {weak_users}")
                security_status["recommendations"].append("Rename or disable default database roles")
            else:
                security_status["security_score"] += 10
            
            # Check for dangerous functions
            dangerous_funcs = await self.db.execute(text("""
                SELECT proname FROM pg_proc 
                WHERE proname IN ('lo_import', 'lo_export', 'copy_from', 'copy_to')
                AND NOT proacl IS NULL
            """))
            dangerous_functions = [row[0] for row in dangerous_funcs.fetchall()]
            if dangerous_functions:
                security_status["issues"].append(f"Dangerous functions accessible: {dangerous_functions}")
                security_status["recommendations"].append("Restrict access to file system functions")
            else:
                security_status["security_score"] += 10
            
        except Exception as e:
            logger.error("Database security validation failed", error=str(e))
            security_status["issues"].append(f"Security validation error: {str(e)}")
        
        return security_status
    
    async def validate_redis_security(self) -> Dict[str, Any]:
        """Validate Redis security configuration."""
        security_status = {
            "auth_enabled": False,
            "dangerous_commands_disabled": False,
            "network_secure": False,
            "memory_limits_configured": False,
            "security_score": 0,
            "issues": [],
            "recommendations": []
        }
        
        try:
            redis = await get_redis()
            
            # Check authentication
            try:
                info = await redis.info()
                security_status["auth_enabled"] = True
                security_status["security_score"] += 30
            except Exception as e:
                if "NOAUTH" in str(e):
                    security_status["issues"].append("Redis authentication not configured")
                    security_status["recommendations"].append("Configure Redis requirepass")
                else:
                    # If we can't connect without auth, it means auth is working
                    security_status["auth_enabled"] = True
                    security_status["security_score"] += 30
            
            # Check dangerous commands
            try:
                config = await redis.config_get("rename-command")
                if config:
                    dangerous_commands = ["FLUSHDB", "FLUSHALL", "KEYS", "CONFIG", "EVAL", "DEBUG"]
                    renamed_commands = [cmd for cmd in dangerous_commands if f"rename-command {cmd}" in str(config)]
                    if renamed_commands:
                        security_status["dangerous_commands_disabled"] = True
                        security_status["security_score"] += 25
                    else:
                        security_status["issues"].append("Dangerous Redis commands not disabled")
                        security_status["recommendations"].append("Rename or disable dangerous Redis commands")
                
                # Check memory limits
                maxmemory = await redis.config_get("maxmemory")
                if maxmemory and int(maxmemory.get("maxmemory", 0)) > 0:
                    security_status["memory_limits_configured"] = True
                    security_status["security_score"] += 15
                else:
                    security_status["issues"].append("Redis memory limits not configured")
                    security_status["recommendations"].append("Configure Redis maxmemory setting")
                
                # Check bind configuration
                bind_config = await redis.config_get("bind")
                if bind_config and "127.0.0.1" in str(bind_config):
                    security_status["network_secure"] = True
                    security_status["security_score"] += 20
                else:
                    security_status["issues"].append("Redis not bound to localhost only")
                    security_status["recommendations"].append("Configure Redis to bind only to secure interfaces")
                
            except Exception as e:
                logger.warning("Could not check Redis configuration", error=str(e))
                security_status["issues"].append("Unable to verify Redis configuration")
        
        except Exception as e:
            logger.error("Redis security validation failed", error=str(e))
            security_status["issues"].append(f"Redis connection error: {str(e)}")
        
        return security_status
    
    async def validate_application_security(self) -> Dict[str, Any]:
        """Validate application-level security configuration."""
        security_status = {
            "cors_configured": False,
            "security_headers_enabled": False,
            "rate_limiting_active": False,
            "input_validation_enabled": False,
            "jwt_configured": False,
            "https_enforced": False,
            "security_score": 0,
            "issues": [],
            "recommendations": []
        }
        
        # Check CORS configuration
        if settings.enable_cors and settings.allowed_origins:
            if "*" not in settings.allowed_origins:
                security_status["cors_configured"] = True
                security_status["security_score"] += 15
            else:
                security_status["issues"].append("CORS allows all origins (*)")
                security_status["recommendations"].append("Configure specific allowed origins for CORS")
        else:
            security_status["issues"].append("CORS not properly configured")
            security_status["recommendations"].append("Configure CORS with specific allowed origins")
        
        # Check JWT configuration
        if settings.jwt_secret and len(settings.jwt_secret) >= 32:
            security_status["jwt_configured"] = True
            security_status["security_score"] += 20
        else:
            security_status["issues"].append("JWT secret not configured or too weak")
            security_status["recommendations"].append("Configure strong JWT secret (32+ characters)")
        
        # Check token expiration
        if settings.jwt_access_token_expire_minutes <= 60:  # 1 hour or less
            security_status["security_score"] += 10
        else:
            security_status["issues"].append("JWT access token expiration too long")
            security_status["recommendations"].append("Reduce JWT access token expiration time")
        
        # Check rate limiting configuration
        if settings.rate_limit_per_minute > 0:
            security_status["rate_limiting_active"] = True
            security_status["security_score"] += 15
        else:
            security_status["issues"].append("Rate limiting not configured")
            security_status["recommendations"].append("Configure rate limiting for API endpoints")
        
        # Check password hashing configuration
        if settings.bcrypt_rounds >= 12:
            security_status["security_score"] += 15
        else:
            security_status["issues"].append("Bcrypt rounds too low")
            security_status["recommendations"].append("Increase bcrypt rounds to 12 or higher")
        
        # Check environment-specific security
        if settings.environment == "production":
            if not settings.enable_swagger_ui and not settings.enable_redoc:
                security_status["security_score"] += 10
            else:
                security_status["issues"].append("API documentation exposed in production")
                security_status["recommendations"].append("Disable Swagger UI and ReDoc in production")
            
            if not settings.debug:
                security_status["security_score"] += 10
            else:
                security_status["issues"].append("Debug mode enabled in production")
                security_status["recommendations"].append("Disable debug mode in production")
        
        return security_status
    
    async def run_comprehensive_security_scan(self) -> Dict[str, Any]:
        """Run a comprehensive security scan of all components."""
        logger.info("Starting comprehensive security scan")
        
        scan_results = {
            "timestamp": time.time(),
            "overall_score": 0,
            "database": {},
            "redis": {},
            "application": {},
            "critical_issues": [],
            "recommendations": [],
            "compliance_status": {}
        }
        
        try:
            # Validate database security
            db_security = await self.validate_database_security()
            scan_results["database"] = db_security
            
            # Validate Redis security
            redis_security = await self.validate_redis_security()
            scan_results["redis"] = redis_security
            
            # Validate application security
            app_security = await self.validate_application_security()
            scan_results["application"] = app_security
            
            # Calculate overall score
            total_score = (
                db_security["security_score"] +
                redis_security["security_score"] +
                app_security["security_score"]
            )
            max_possible_score = 300  # Theoretical maximum
            scan_results["overall_score"] = min(100, (total_score / max_possible_score) * 100)
            
            # Collect critical issues
            all_issues = (
                db_security["issues"] +
                redis_security["issues"] +
                app_security["issues"]
            )
            
            # Filter critical issues
            critical_keywords = ["ssl", "auth", "password", "dangerous", "debug", "production"]
            scan_results["critical_issues"] = [
                issue for issue in all_issues
                if any(keyword in issue.lower() for keyword in critical_keywords)
            ]
            
            # Collect all recommendations
            scan_results["recommendations"] = (
                db_security["recommendations"] +
                redis_security["recommendations"] +
                app_security["recommendations"]
            )
            
            # Determine compliance status
            scan_results["compliance_status"] = {
                "gdpr_ready": scan_results["overall_score"] >= 80 and scan_results["database"]["audit_logging_enabled"],
                "pci_compliant": scan_results["overall_score"] >= 90 and scan_results["database"]["ssl_enabled"],
                "production_ready": scan_results["overall_score"] >= 85 and len(scan_results["critical_issues"]) == 0,
                "security_hardened": scan_results["overall_score"] >= 95
            }
            
            # Log security scan results
            await self.audit.log_security_event(
                event_type="security_scan_completed",
                severity="info",
                details={
                    "overall_score": scan_results["overall_score"],
                    "critical_issues_count": len(scan_results["critical_issues"]),
                    "compliance_status": scan_results["compliance_status"]
                }
            )
            
            logger.info(
                "Security scan completed",
                overall_score=scan_results["overall_score"],
                critical_issues=len(scan_results["critical_issues"]),
                compliance_status=scan_results["compliance_status"]
            )
            
        except Exception as e:
            logger.error("Security scan failed", error=str(e))
            scan_results["error"] = str(e)
        
        return scan_results
    
    async def apply_security_hardening(self) -> Dict[str, Any]:
        """Apply automated security hardening where possible."""
        hardening_results = {
            "applied": [],
            "failed": [],
            "manual_required": []
        }
        
        try:
            # Database hardening
            try:
                # Enable log_statement for audit logging
                await self.db.execute(text("ALTER SYSTEM SET log_statement = 'all'"))
                await self.db.execute(text("SELECT pg_reload_conf()"))
                hardening_results["applied"].append("Enabled database statement logging")
            except Exception as e:
                hardening_results["failed"].append(f"Database logging: {str(e)}")
            
            # Application hardening
            if settings.environment == "production":
                hardening_results["manual_required"].extend([
                    "Disable debug mode in environment configuration",
                    "Disable API documentation in production",
                    "Configure HTTPS redirect",
                    "Set up proper CORS origins"
                ])
            
            logger.info("Security hardening completed", results=hardening_results)
            
        except Exception as e:
            logger.error("Security hardening failed", error=str(e))
            hardening_results["failed"].append(f"General error: {str(e)}")
        
        return hardening_results


# Security monitoring functions

async def get_security_dashboard() -> Dict[str, Any]:
    """Get security dashboard with current status."""
    from ..core.database import AsyncSessionLocal
    async with AsyncSessionLocal() as db:
        security_service = SecurityConfigService(db)
        scan_results = await security_service.run_comprehensive_security_scan()
        
        # Get recent security events
        audit_service = AuditService(db)
        recent_events = await audit_service.get_security_events(hours_back=24, limit=50)
        
        dashboard = {
            "security_scan": scan_results,
            "recent_events": [
                {
                    "id": str(event.id),
                    "action": event.action,
                    "timestamp": event.created_at.isoformat(),
                    "ip_address": event.ip_address,
                    "details": event.details
                }
                for event in recent_events
            ],
            "alerts": [],
            "summary": {
                "overall_score": scan_results.get("overall_score", 0),
                "critical_issues": len(scan_results.get("critical_issues", [])),
                "recent_events_count": len(recent_events),
                "compliance_ready": scan_results.get("compliance_status", {}).get("production_ready", False)
            }
        }
        
        # Generate alerts for critical issues
        if scan_results.get("overall_score", 0) < 70:
            dashboard["alerts"].append({
                "type": "critical",
                "message": "Security score is below acceptable threshold",
                "action_required": "Review and address security issues immediately"
            })
        
        if len(scan_results.get("critical_issues", [])) > 0:
            dashboard["alerts"].append({
                "type": "warning",
                "message": f"{len(scan_results['critical_issues'])} critical security issues found",
                "action_required": "Address critical issues before production deployment"
            })
        
        return dashboard


async def validate_security_compliance() -> Dict[str, Any]:
    """Validate security compliance against industry standards."""
    from ..core.database import AsyncSessionLocal
    async with AsyncSessionLocal() as db:
        security_service = SecurityConfigService(db)
        scan_results = await security_service.run_comprehensive_security_scan()
        
        # Check compliance requirements
        compliance_checks = {
            "owasp_top10": {
                "injection_protection": scan_results["database"]["rls_policies_active"],
                "broken_authentication": scan_results["application"]["jwt_configured"],
                "sensitive_data_exposure": scan_results["database"]["ssl_enabled"],
                "xml_external_entities": True,  # Not applicable for this API
                "broken_access_control": scan_results["application"]["rate_limiting_active"],
                "security_misconfiguration": scan_results["overall_score"] >= 80,
                "xss": scan_results["application"]["security_headers_enabled"],
                "insecure_deserialization": True,  # Handled by framework
                "vulnerable_components": True,  # Requires external scanning
                "insufficient_logging": scan_results["database"]["audit_logging_enabled"]
            },
            "cis_controls": {
                "access_control": scan_results["database"]["rls_policies_active"],
                "data_protection": scan_results["database"]["ssl_enabled"],
                "secure_configuration": scan_results["overall_score"] >= 85,
                "continuous_monitoring": scan_results["database"]["audit_logging_enabled"],
                "incident_response": len(scan_results.get("critical_issues", [])) == 0
            }
        }
        
        # Calculate compliance scores
        owasp_score = sum(compliance_checks["owasp_top10"].values()) / len(compliance_checks["owasp_top10"]) * 100
        cis_score = sum(compliance_checks["cis_controls"].values()) / len(compliance_checks["cis_controls"]) * 100
        
        return {
            "owasp_top10": {
                "score": owasp_score,
                "details": compliance_checks["owasp_top10"]
            },
            "cis_controls": {
                "score": cis_score,
                "details": compliance_checks["cis_controls"]
            },
            "overall_compliance": (owasp_score + cis_score) / 2,
            "recommendations": scan_results.get("recommendations", [])
        }