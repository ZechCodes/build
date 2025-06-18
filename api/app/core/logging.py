"""Logging configuration."""

import sys
from typing import Any, Dict

import structlog
from structlog.types import EventDict


def add_correlation_id(logger: Any, method_name: str, event_dict: EventDict) -> EventDict:
    """Add correlation ID to log events."""
    # This will be enhanced when we add request correlation IDs
    return event_dict


def configure_logging() -> None:
    """Configure structured logging with structlog."""
    structlog.configure(
        processors=[
            # Add the log level and timestamp to the event_dict
            structlog.contextvars.merge_contextvars,
            structlog.processors.add_log_level,
            structlog.processors.StackInfoRenderer(),
            structlog.dev.set_exc_info,
            # Add correlation ID
            add_correlation_id,
            # Prepare for JSON serialization
            structlog.processors.JSONRenderer() if sys.stdout.isatty() else structlog.dev.ConsoleRenderer(),
        ],
        wrapper_class=structlog.make_filtering_bound_logger(30),  # INFO level
        logger_factory=structlog.PrintLoggerFactory(),
        cache_logger_on_first_use=True,
    )