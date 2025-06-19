#!/usr/bin/env python3
"""Initialize enhanced authentication and monitoring systems."""

import asyncio
import sys
import os
from pathlib import Path

# Add the app directory to Python path
sys.path.insert(0, str(Path(__file__).parent.parent / "api"))

from app.core.config import get_settings
from app.core.database import get_db_session
from app.core.redis import redis_manager
from app.monitoring.alerts import get_alert_manager, setup_default_alert_rules
from app.monitoring.dashboard import get_dashboard_manager, setup_default_dashboards
from app.monitoring.metrics import metrics_collector
from app.services.auth import AuthService
from app.schemas.auth import UserCreate
import structlog

logger = structlog.get_logger(__name__)


async def initialize_database():
    """Initialize database connection and verify schema."""
    logger.info("Initializing database connection...")
    
    try:
        async with get_db_session() as db:
            # Test database connection
            result = await db.execute("SELECT 1 as test")
            test_value = result.scalar()
            
            if test_value == 1:
                logger.info("Database connection successful")
                
                # Check if tables exist
                result = await db.execute("""
                    SELECT table_name 
                    FROM information_schema.tables 
                    WHERE table_schema = 'public'
                """)
                tables = [row[0] for row in result.fetchall()]
                
                required_tables = ['users', 'vm_instances', 'sessions', 'snapshots', 'audit_logs']
                missing_tables = [table for table in required_tables if table not in tables]
                
                if missing_tables:
                    logger.error("Missing required tables", missing_tables=missing_tables)
                    logger.info("Run 'alembic upgrade head' to create missing tables")
                    return False
                else:
                    logger.info("All required tables present", table_count=len(tables))
                    return True
            else:
                logger.error("Database connection test failed")
                return False
                
    except Exception as e:
        logger.error("Database initialization failed", error=str(e))
        return False


async def initialize_redis():
    """Initialize Redis connection and verify functionality."""
    logger.info("Initializing Redis connection...")
    
    try:
        await redis_manager.connect()
        redis = await redis_manager.get_redis()
        
        # Test Redis connection
        pong = await redis.ping()
        if pong:
            logger.info("Redis connection successful")
            
            # Test basic operations
            await redis.set("test_key", "test_value", ex=10)
            value = await redis.get("test_key")
            
            if value == "test_value":
                logger.info("Redis operations working correctly")
                await redis.delete("test_key")
                return True
            else:
                logger.error("Redis operations failed")
                return False
        else:
            logger.error("Redis ping failed")
            return False
            
    except Exception as e:
        logger.error("Redis initialization failed", error=str(e))
        return False


async def create_admin_user():
    """Create initial admin user if it doesn't exist."""
    logger.info("Checking for admin user...")
    
    try:
        async with get_db_session() as db:
            from sqlalchemy import select
            from app.models.user import User
            
            # Check if admin user exists
            result = await db.execute(
                select(User).where(User.email == "admin@buildplatform.dev")
            )
            admin_user = result.scalar_one_or_none()
            
            if admin_user:
                logger.info("Admin user already exists", user_id=str(admin_user.id))
                return True
            
            # Create admin user
            admin_create = UserCreate(
                email="admin@buildplatform.dev",
                username="admin",
                password="AdminPassword123!"  # This should be changed immediately
            )
            
            admin_user = await AuthService.create_user(db, admin_create)
            logger.info("Admin user created", user_id=str(admin_user.id))
            logger.warning("IMPORTANT: Change admin password immediately!")
            
            return True
            
    except Exception as e:
        logger.error("Failed to create admin user", error=str(e))
        return False


async def initialize_monitoring():
    """Initialize monitoring alerts and dashboards."""
    logger.info("Initializing monitoring systems...")
    
    try:
        # Setup alert rules
        setup_default_alert_rules()
        alert_manager = get_alert_manager()
        
        logger.info("Alert rules configured", rule_count=len(alert_manager.rules))
        
        # Start alert monitoring
        await alert_manager.start_monitoring()
        logger.info("Alert monitoring started")
        
        # Setup dashboards
        setup_default_dashboards()
        dashboard_manager = get_dashboard_manager()
        
        dashboards = dashboard_manager.list_dashboards()
        logger.info("Dashboards configured", dashboard_count=len(dashboards))
        
        # Start dashboard auto-refresh
        await dashboard_manager.start_auto_refresh(interval=60)
        logger.info("Dashboard auto-refresh started")
        
        return True
        
    except Exception as e:
        logger.error("Failed to initialize monitoring", error=str(e))
        return False


async def verify_security_configuration():
    """Verify security configuration is properly set up."""
    logger.info("Verifying security configuration...")
    
    settings = get_settings()
    
    # Check critical security settings
    security_checks = []
    
    # JWT Secret
    if len(settings.jwt_secret) >= 32:
        security_checks.append(("JWT Secret Length", True))
    else:
        security_checks.append(("JWT Secret Length", False))
        logger.error("JWT secret is too short (< 32 characters)")
    
    # Redis Password
    if settings.redis_password and len(settings.redis_password) >= 16:
        security_checks.append(("Redis Password", True))
    else:
        security_checks.append(("Redis Password", False))
        logger.error("Redis password is weak or missing")
    
    # Database URL security
    if "password" in settings.database_url and "localhost" not in settings.database_url:
        security_checks.append(("Database Security", True))
    else:
        security_checks.append(("Database Security", False))
        logger.warning("Database may not be properly secured")
    
    # Environment check
    if settings.environment == "production":
        if settings.debug:
            security_checks.append(("Debug Mode", False))
            logger.error("Debug mode enabled in production")
        else:
            security_checks.append(("Debug Mode", True))
    
    passed_checks = sum(1 for _, passed in security_checks if passed)
    total_checks = len(security_checks)
    
    logger.info("Security checks completed", 
                passed=passed_checks, 
                total=total_checks, 
                percentage=f"{(passed_checks/total_checks)*100:.1f}%")
    
    if passed_checks == total_checks:
        logger.info("All security checks passed")
        return True
    else:
        logger.warning("Some security checks failed")
        return False


async def run_system_tests():
    """Run basic system tests to verify functionality."""
    logger.info("Running system tests...")
    
    tests_passed = 0
    total_tests = 0
    
    # Test 1: Database query
    total_tests += 1
    try:
        async with get_db_session() as db:
            result = await db.execute("SELECT COUNT(*) FROM users")
            user_count = result.scalar()
            logger.info("Database test passed", user_count=user_count)
            tests_passed += 1
    except Exception as e:
        logger.error("Database test failed", error=str(e))
    
    # Test 2: Redis operations
    total_tests += 1
    try:
        redis = await redis_manager.get_redis()
        await redis.set("system_test", "success", ex=10)
        value = await redis.get("system_test")
        if value == "success":
            logger.info("Redis test passed")
            tests_passed += 1
            await redis.delete("system_test")
        else:
            logger.error("Redis test failed - value mismatch")
    except Exception as e:
        logger.error("Redis test failed", error=str(e))
    
    # Test 3: Metrics collection
    total_tests += 1
    try:
        initial_count = metrics_collector.get_request_count()
        metrics_collector.record_request("GET", "/test", 200, 0.1)
        new_count = metrics_collector.get_request_count()
        if new_count > initial_count:
            logger.info("Metrics test passed")
            tests_passed += 1
        else:
            logger.error("Metrics test failed - count not increased")
    except Exception as e:
        logger.error("Metrics test failed", error=str(e))
    
    # Test 4: Alert system
    total_tests += 1
    try:
        alert_manager = get_alert_manager()
        alert = await alert_manager.fire_alert(
            name="system_test_alert",
            severity=alert_manager.AlertSeverity.LOW,
            message="System initialization test alert"
        )
        await alert_manager.resolve_alert(alert.id)
        logger.info("Alert system test passed")
        tests_passed += 1
    except Exception as e:
        logger.error("Alert system test failed", error=str(e))
    
    logger.info("System tests completed", 
                passed=tests_passed, 
                total=total_tests,
                percentage=f"{(tests_passed/total_tests)*100:.1f}%")
    
    return tests_passed == total_tests


async def cleanup_and_shutdown():
    """Clean up resources and shutdown gracefully."""
    logger.info("Cleaning up resources...")
    
    try:
        # Stop monitoring tasks
        alert_manager = get_alert_manager()
        await alert_manager.stop_monitoring()
        
        dashboard_manager = get_dashboard_manager()
        await dashboard_manager.stop_auto_refresh()
        
        # Close Redis connection
        await redis_manager.disconnect()
        
        logger.info("Cleanup completed successfully")
        
    except Exception as e:
        logger.error("Error during cleanup", error=str(e))


async def main():
    """Main initialization function."""
    logger.info("Starting Build Platform enhanced systems initialization")
    
    initialization_steps = [
        ("Database", initialize_database),
        ("Redis", initialize_redis),
        ("Admin User", create_admin_user),
        ("Monitoring", initialize_monitoring),
        ("Security Configuration", verify_security_configuration),
        ("System Tests", run_system_tests),
    ]
    
    passed_steps = 0
    
    try:
        for step_name, step_function in initialization_steps:
            logger.info(f"Running step: {step_name}")
            
            success = await step_function()
            if success:
                logger.info(f"Step completed successfully: {step_name}")
                passed_steps += 1
            else:
                logger.error(f"Step failed: {step_name}")
        
        # Summary
        logger.info("Initialization completed", 
                   passed_steps=passed_steps,
                   total_steps=len(initialization_steps),
                   success_rate=f"{(passed_steps/len(initialization_steps))*100:.1f}%")
        
        if passed_steps == len(initialization_steps):
            logger.info("🎉 All systems initialized successfully!")
            logger.info("Build Platform is ready for operation")
            
            # Print important notes
            print("\n" + "="*60)
            print("BUILD PLATFORM INITIALIZATION COMPLETE")
            print("="*60)
            print("\nIMPORTANT NEXT STEPS:")
            print("1. Change the admin password immediately")
            print("2. Configure production secrets management")
            print("3. Set up monitoring alerts endpoints")
            print("4. Review security configuration")
            print("5. Test all functionality in staging")
            print("\nAccess Points:")
            print("- API Health: http://localhost:8000/health")
            print("- API Docs: http://localhost:8000/docs")
            print("- Metrics: http://localhost:8000/metrics")
            print("="*60)
            
            return True
        else:
            logger.error("Initialization completed with errors")
            return False
            
    except KeyboardInterrupt:
        logger.info("Initialization interrupted by user")
        return False
    except Exception as e:
        logger.error("Unexpected error during initialization", error=str(e))
        return False
    finally:
        # Always cleanup
        await cleanup_and_shutdown()


if __name__ == "__main__":
    # Configure structured logging for the script
    import structlog
    
    structlog.configure(
        processors=[
            structlog.stdlib.filter_by_level,
            structlog.stdlib.add_logger_name,
            structlog.stdlib.add_log_level,
            structlog.stdlib.PositionalArgumentsFormatter(),
            structlog.processors.TimeStamper(fmt="iso"),
            structlog.processors.StackInfoRenderer(),
            structlog.processors.format_exc_info,
            structlog.processors.UnicodeDecoder(),
            structlog.dev.ConsoleRenderer(colors=True)
        ],
        context_class=dict,
        logger_factory=structlog.stdlib.LoggerFactory(),
        wrapper_class=structlog.stdlib.BoundLogger,
        cache_logger_on_first_use=True,
    )
    
    # Run the initialization
    try:
        success = asyncio.run(main())
        sys.exit(0 if success else 1)
    except Exception as e:
        print(f"Fatal error: {e}")
        sys.exit(1)