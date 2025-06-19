"""Pydantic Logfire setup and configuration."""

import os
import structlog
from typing import Any, Dict, Optional
from fastapi import FastAPI

try:
    import logfire
    LOGFIRE_AVAILABLE = True
except ImportError:
    LOGFIRE_AVAILABLE = False

from ..core.config import get_settings

logger = structlog.get_logger(__name__)


def setup_logfire_monitoring(app: FastAPI) -> None:
    """Configure Pydantic Logfire for comprehensive observability."""
    if not LOGFIRE_AVAILABLE:
        logger.warning("Logfire not available - monitoring disabled")
        return

    settings = get_settings()
    
    try:
        # Configure Logfire with environment-specific settings
        logfire_config = {
            "service_name": "build-api",
            "service_version": "1.0.0",
            "environment": settings.environment,
        }
        
        # Add Logfire token if available
        logfire_token = os.getenv("LOGFIRE_TOKEN")
        if logfire_token:
            logfire_config["token"] = logfire_token
        else:
            logger.warning("LOGFIRE_TOKEN not set - using local mode")
        
        # Configure Logfire
        logfire.configure(**logfire_config)
        
        # Instrument FastAPI application
        logfire.instrument_fastapi(
            app,
            capture_headers=True,
            capture_request_json_schema=True,
            capture_response_json_schema=True,
        )
        
        # Instrument async libraries if available
        try:
            logfire.instrument_asyncpg()
            logger.info("AsyncPG instrumentation enabled")
        except Exception as e:
            logger.debug("AsyncPG instrumentation not available", error=str(e))
        
        try:
            logfire.instrument_redis()
            logger.info("Redis instrumentation enabled")
        except Exception as e:
            logger.debug("Redis instrumentation not available", error=str(e))
        
        try:
            logfire.instrument_httpx()
            logger.info("HTTPX instrumentation enabled")
        except Exception as e:
            logger.debug("HTTPX instrumentation not available", error=str(e))
        
        logger.info("Logfire monitoring configured successfully", 
                   service_name=logfire_config["service_name"],
                   environment=logfire_config["environment"])
        
    except Exception as e:
        logger.error("Failed to configure Logfire", error=str(e))


def setup_structured_logging() -> None:
    """Configure structured logging with Logfire integration."""
    processors = [
        structlog.stdlib.filter_by_level,
        structlog.stdlib.add_logger_name,
        structlog.stdlib.add_log_level,
        structlog.stdlib.PositionalArgumentsFormatter(),
        structlog.processors.TimeStamper(fmt="iso"),
        structlog.processors.StackInfoRenderer(),
        structlog.processors.format_exc_info,
        structlog.processors.UnicodeDecoder(),
    ]
    
    # Add Logfire processor if available
    if LOGFIRE_AVAILABLE:
        try:
            processors.append(logfire.LogfireProcessor())
            logger.info("Logfire processor added to structured logging")
        except Exception as e:
            logger.debug("Failed to add Logfire processor", error=str(e))
    
    # Use JSON renderer for production, console for development
    settings = get_settings()
    if settings.environment == "development":
        processors.append(structlog.dev.ConsoleRenderer(colors=True))
    else:
        processors.append(structlog.processors.JSONRenderer())
    
    structlog.configure(
        processors=processors,
        context_class=dict,
        logger_factory=structlog.stdlib.LoggerFactory(),
        wrapper_class=structlog.stdlib.BoundLogger,
        cache_logger_on_first_use=True,
    )
    
    logger.info("Structured logging configured", 
               environment=settings.environment,
               logfire_enabled=LOGFIRE_AVAILABLE)


def get_logfire_context() -> Dict[str, Any]:
    """Get current Logfire context for manual logging."""
    if not LOGFIRE_AVAILABLE:
        return {}
    
    try:
        return logfire.get_context()
    except Exception as e:
        logger.debug("Failed to get Logfire context", error=str(e))
        return {}


def log_user_action(
    user_id: str, 
    action: str, 
    resource_type: Optional[str] = None,
    resource_id: Optional[str] = None,
    metadata: Optional[Dict[str, Any]] = None
) -> None:
    """Log user action with Logfire tracking."""
    log_data = {
        "user_id": user_id,
        "action": action,
        "resource_type": resource_type,
        "resource_id": resource_id,
        "metadata": metadata or {}
    }
    
    if LOGFIRE_AVAILABLE:
        try:
            with logfire.span("User Action", **log_data):
                logger.info("User action recorded", **log_data)
        except Exception as e:
            logger.warning("Failed to log user action to Logfire", error=str(e))
            logger.info("User action recorded", **log_data)
    else:
        logger.info("User action recorded", **log_data)


def log_system_event(
    event_type: str,
    message: str,
    level: str = "info",
    metadata: Optional[Dict[str, Any]] = None
) -> None:
    """Log system event with appropriate level."""
    log_data = {
        "event_type": event_type,
        "message": message,
        "metadata": metadata or {}
    }
    
    if LOGFIRE_AVAILABLE:
        try:
            with logfire.span("System Event", **log_data):
                getattr(logger, level)(message, **log_data)
        except Exception as e:
            logger.warning("Failed to log system event to Logfire", error=str(e))
            getattr(logger, level)(message, **log_data)
    else:
        getattr(logger, level)(message, **log_data)


def log_performance_metric(
    metric_name: str,
    value: float,
    unit: str = "ms",
    tags: Optional[Dict[str, str]] = None
) -> None:
    """Log performance metric."""
    metric_data = {
        "metric_name": metric_name,
        "value": value,
        "unit": unit,
        "tags": tags or {}
    }
    
    if LOGFIRE_AVAILABLE:
        try:
            logfire.log("Performance Metric", **metric_data)
        except Exception as e:
            logger.debug("Failed to log performance metric to Logfire", error=str(e))
    
    logger.info("Performance metric", **metric_data)


def log_error_with_context(
    error: Exception,
    context: Optional[Dict[str, Any]] = None,
    user_id: Optional[str] = None
) -> None:
    """Log error with full context and Logfire tracking."""
    error_data = {
        "error_type": type(error).__name__,
        "error_message": str(error),
        "context": context or {},
        "user_id": user_id
    }
    
    if LOGFIRE_AVAILABLE:
        try:
            with logfire.span("Error", **error_data) as span:
                span.record_exception(error)
                logger.error("Error occurred", **error_data, exc_info=error)
        except Exception as e:
            logger.warning("Failed to log error to Logfire", error=str(e))
            logger.error("Error occurred", **error_data, exc_info=error)
    else:
        logger.error("Error occurred", **error_data, exc_info=error)


class LogfireContextManager:
    """Context manager for Logfire spans."""
    
    def __init__(self, span_name: str, **attributes):
        self.span_name = span_name
        self.attributes = attributes
        self.span = None
    
    def __enter__(self):
        if LOGFIRE_AVAILABLE:
            try:
                self.span = logfire.span(self.span_name, **self.attributes)
                return self.span.__enter__()
            except Exception as e:
                logger.debug("Failed to create Logfire span", error=str(e))
        return self
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        if self.span:
            try:
                return self.span.__exit__(exc_type, exc_val, exc_tb)
            except Exception as e:
                logger.debug("Failed to close Logfire span", error=str(e))
        return False
    
    def set_attribute(self, key: str, value: Any):
        """Set span attribute."""
        if self.span:
            try:
                self.span.set_attribute(key, value)
            except Exception as e:
                logger.debug("Failed to set span attribute", error=str(e))
    
    def record_exception(self, exception: Exception):
        """Record exception in span."""
        if self.span:
            try:
                self.span.record_exception(exception)
            except Exception as e:
                logger.debug("Failed to record exception", error=str(e))


def create_span(span_name: str, **attributes) -> LogfireContextManager:
    """Create a Logfire span context manager."""
    return LogfireContextManager(span_name, **attributes)