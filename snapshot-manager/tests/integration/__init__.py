"""
Integration tests for VM Snapshot Manager.

Comprehensive end-to-end testing including performance benchmarks,
stress testing, and service integration validation.
"""

from .test_integration import IntegrationTestSuite, PerformanceMetrics, IntegrationTestResult

__all__ = ["IntegrationTestSuite", "PerformanceMetrics", "IntegrationTestResult"]