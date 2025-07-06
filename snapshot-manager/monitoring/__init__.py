"""
Monitoring and observability components.

This module provides monitoring, logging, and observability features for
the snapshot manager including Logfire integration and metrics collection.
"""

from .logfire_integration import LogfireMonitoring

__all__ = ["LogfireMonitoring"]