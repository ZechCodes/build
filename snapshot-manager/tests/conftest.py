"""
Test configuration and shared fixtures for snapshot manager tests.
"""

import sys
import os
from pathlib import Path

# Add the parent directory to Python path so we can import snapshot_manager
snapshot_manager_dir = Path(__file__).parent.parent
sys.path.insert(0, str(snapshot_manager_dir))

import pytest
import asyncio
import structlog
import logfire
from unittest.mock import AsyncMock, MagicMock


# Configure test logging
structlog.configure(
    processors=[
        structlog.stdlib.filter_by_level,
        structlog.stdlib.add_logger_name,
        structlog.stdlib.add_log_level,
        structlog.stdlib.PositionalArgumentsFormatter(),
        structlog.processors.TimeStamper(fmt="iso"),
        structlog.dev.ConsoleRenderer()
    ],
    context_class=dict,
    logger_factory=structlog.stdlib.LoggerFactory(),
    wrapper_class=structlog.stdlib.BoundLogger,
    cache_logger_on_first_use=True,
)


# Mock logfire for testing
class MockLogfire:
    def info(self, *args, **kwargs):
        pass
    
    def error(self, *args, **kwargs):
        pass
    
    def span(self, name):
        return MockSpan()


class MockSpan:
    def __enter__(self):
        return self
    
    def __exit__(self, *args):
        pass
    
    def set_attribute(self, key, value):
        pass


# Replace logfire with mock for testing
logfire.info = MockLogfire().info
logfire.error = MockLogfire().error
logfire.span = MockLogfire().span


@pytest.fixture(scope="session")
def event_loop():
    """Create an instance of the default event loop for the test session."""
    loop = asyncio.get_event_loop_policy().new_event_loop()
    yield loop
    loop.close()


@pytest.fixture
def mock_vm():
    """Mock VM object."""
    vm = MagicMock()
    vm.id = "vm123"
    vm.user_id = "user123"
    vm.get_config.return_value = {"cpu": 1, "memory": 512, "disk": "1GB"}
    return vm