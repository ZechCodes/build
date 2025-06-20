"""
Unit tests for SessionPerformanceMonitor
"""
import pytest
import asyncio
import time
from unittest.mock import AsyncMock, MagicMock, patch

import sys
import os

# Add the session-manager directory to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '../..'))

from monitoring.performance_monitor import SessionPerformanceMonitor, PerformanceMetrics


class TestSessionPerformanceMonitor:
    """Test suite for SessionPerformanceMonitor"""
    
    async def test_performance_monitor_initialization(self, session_manager_mock, websocket_gateway_mock, buffer_manager_mock):
        """Test performance monitor initialization"""
        # Arrange
        monitor = SessionPerformanceMonitor(
            session_manager_mock,
            websocket_gateway_mock, 
            buffer_manager_mock
        )
        
        # Act
        await monitor.initialize()
        
        try:
            # Assert
            assert monitor.monitoring_task is not None
            assert not monitor.monitoring_task.done()
            assert len(monitor.metrics_history) == 0
            assert monitor.collection_interval == 10
            
        finally:
            await monitor.stop()
    
    async def test_metrics_collection(self, session_manager_mock, websocket_gateway_mock, buffer_manager_mock):
        """Test performance metrics collection"""
        # Arrange
        monitor = SessionPerformanceMonitor(
            session_manager_mock,
            websocket_gateway_mock,
            buffer_manager_mock
        )
        
        # Mock session manager with sessions
        session_manager_mock.sessions = {"session1": {}, "session2": {}}
        
        # Mock websocket gateway connection count
        websocket_gateway_mock.get_connection_count = MagicMock(return_value=5)
        
        # Mock buffer manager with Redis
        buffer_manager_mock.redis = AsyncMock()
        buffer_manager_mock.redis.set = AsyncMock()
        buffer_manager_mock.redis.get = AsyncMock()
        buffer_manager_mock.redis.delete = AsyncMock()
        
        try:
            # Act
            await monitor._collect_performance_metrics()
            
            # Assert
            assert len(monitor.metrics_history) == 1
            
            metrics = monitor.metrics_history[0]
            assert isinstance(metrics, PerformanceMetrics)
            assert metrics.session_count == 2
            assert metrics.active_connections == 5
            assert metrics.timestamp > 0
            assert metrics.memory_usage_mb >= 0
            assert metrics.cpu_usage_percent >= 0
            
        except Exception as e:
            pytest.fail(f"Metrics collection failed: {e}")
    
    async def test_redis_latency_measurement(self, session_manager_mock, websocket_gateway_mock, buffer_manager_mock):
        """Test Redis latency measurement"""
        # Arrange
        monitor = SessionPerformanceMonitor(
            session_manager_mock,
            websocket_gateway_mock,
            buffer_manager_mock
        )
        
        # Mock Redis operations
        buffer_manager_mock.redis = AsyncMock()
        buffer_manager_mock.redis.set = AsyncMock()
        buffer_manager_mock.redis.get = AsyncMock()
        buffer_manager_mock.redis.delete = AsyncMock()
        
        # Act
        latency = await monitor._measure_redis_latency()
        
        # Assert
        assert isinstance(latency, float)
        assert latency >= 0
        assert latency < 1000  # Should be reasonable for mock
        
        # Verify Redis operations were called
        buffer_manager_mock.redis.set.assert_called_once()
        buffer_manager_mock.redis.get.assert_called_once()
        buffer_manager_mock.redis.delete.assert_called_once()
    
    async def test_redis_latency_error_handling(self, session_manager_mock, websocket_gateway_mock, buffer_manager_mock):
        """Test Redis latency measurement error handling"""
        # Arrange
        monitor = SessionPerformanceMonitor(
            session_manager_mock,
            websocket_gateway_mock,
            buffer_manager_mock
        )
        
        # Mock Redis operations to fail
        buffer_manager_mock.redis = AsyncMock()
        buffer_manager_mock.redis.set = AsyncMock(side_effect=Exception("Redis error"))
        
        # Act
        latency = await monitor._measure_redis_latency()
        
        # Assert
        assert latency == 999.0  # Error indicator
    
    async def test_operation_recording(self, session_manager_mock, websocket_gateway_mock, buffer_manager_mock):
        """Test operation recording for performance tracking"""
        # Arrange
        monitor = SessionPerformanceMonitor(
            session_manager_mock,
            websocket_gateway_mock,
            buffer_manager_mock
        )
        
        # Act
        monitor.record_operation("session_created", duration_ms=100, success=True)
        monitor.record_operation("session_created", duration_ms=150, success=True)
        monitor.record_operation("buffer_stored", success=False)
        
        # Assert
        assert monitor.operation_counts["session_created"] == 2
        assert monitor.operation_counts["errors"] == 1
        assert "session_created" in monitor.operation_timings
        assert len(monitor.operation_timings["session_created"]) == 2
        assert monitor.operation_timings["session_created"] == [100, 150]
    
    async def test_operations_per_second_calculation(self, session_manager_mock, websocket_gateway_mock, buffer_manager_mock):
        """Test operations per second calculation"""
        # Arrange
        monitor = SessionPerformanceMonitor(
            session_manager_mock,
            websocket_gateway_mock,
            buffer_manager_mock
        )
        
        # Add some operation counts
        monitor.operation_counts = {
            "session_created": 30,
            "buffer_stored": 45,
            "websocket_connected": 15
        }
        
        # Add some fake metrics history
        current_time = time.time()
        for i in range(5):
            metrics = PerformanceMetrics(
                timestamp=current_time - (i * 10),
                session_count=10,
                active_connections=5,
                memory_usage_mb=100,
                cpu_usage_percent=25,
                redis_latency_ms=50,
                database_latency_ms=30,
                websocket_latency_ms=25,
                session_operations_per_second=0,
                error_rate_percent=0
            )
            monitor.metrics_history.append(metrics)
        
        # Act
        ops_per_second = await monitor._calculate_operations_per_second()
        
        # Assert
        assert isinstance(ops_per_second, float)
        assert ops_per_second >= 0
        # Should be total operations (90) / 60 seconds = 1.5 ops/sec
        assert ops_per_second == 1.5
    
    async def test_error_rate_calculation(self, session_manager_mock, websocket_gateway_mock, buffer_manager_mock):
        """Test error rate calculation"""
        # Arrange
        monitor = SessionPerformanceMonitor(
            session_manager_mock,
            websocket_gateway_mock,
            buffer_manager_mock
        )
        
        # Set operation counts with errors
        monitor.operation_counts = {
            "session_created": 80,
            "buffer_stored": 15,
            "errors": 5  # 5% error rate
        }
        
        # Act
        error_rate = await monitor._calculate_error_rate()
        
        # Assert
        assert isinstance(error_rate, float)
        assert error_rate == 5.0  # 5 errors out of 100 total operations = 5%
    
    async def test_performance_threshold_alerts(self, session_manager_mock, websocket_gateway_mock, buffer_manager_mock):
        """Test performance threshold alerting"""
        # Arrange
        monitor = SessionPerformanceMonitor(
            session_manager_mock,
            websocket_gateway_mock,
            buffer_manager_mock
        )
        
        # Add metrics that exceed thresholds
        high_latency_metrics = PerformanceMetrics(
            timestamp=time.time(),
            session_count=10,
            active_connections=5,
            memory_usage_mb=1500,  # Exceeds 1000MB threshold
            cpu_usage_percent=90,   # Exceeds 80% threshold
            redis_latency_ms=200,   # Exceeds 100ms threshold
            database_latency_ms=30,
            websocket_latency_ms=25,
            session_operations_per_second=10,
            error_rate_percent=10   # Exceeds 5% threshold
        )
        monitor.metrics_history.append(high_latency_metrics)
        
        # Act
        alerts = monitor.get_alerts()
        
        # Assert
        assert len(alerts) == 4  # Memory, CPU, Redis latency, error rate
        
        alert_metrics = [alert["metric"] for alert in alerts]
        assert "memory_usage" in alert_metrics
        assert "cpu_usage" in alert_metrics
        assert "redis_latency" in alert_metrics
        assert "error_rate" in alert_metrics
        
        # Check alert severity
        memory_alert = next(a for a in alerts if a["metric"] == "memory_usage")
        assert memory_alert["severity"] == "critical"
    
    async def test_performance_summary_generation(self, session_manager_mock, websocket_gateway_mock, buffer_manager_mock):
        """Test performance summary generation"""
        # Arrange
        monitor = SessionPerformanceMonitor(
            session_manager_mock,
            websocket_gateway_mock,
            buffer_manager_mock
        )
        
        # Add sample metrics history
        current_time = time.time()
        for i in range(10):
            metrics = PerformanceMetrics(
                timestamp=current_time - (i * 60),  # Every minute
                session_count=10 + i,
                active_connections=5 + i,
                memory_usage_mb=100 + (i * 10),
                cpu_usage_percent=20 + (i * 2),
                redis_latency_ms=50 + i,
                database_latency_ms=30,
                websocket_latency_ms=25,
                session_operations_per_second=5,
                error_rate_percent=1
            )
            monitor.metrics_history.append(metrics)
        
        # Add operation timings
        monitor.operation_timings = {
            "session_created": [100, 120, 90, 110],
            "buffer_stored": [50, 60, 45]
        }
        
        # Act
        summary = monitor.get_performance_summary(duration_minutes=10)
        
        # Assert
        assert "averages" in summary
        assert "peaks" in summary
        assert "operation_stats" in summary
        assert "latest" in summary
        
        # Check averages
        assert summary["averages"]["session_count"] > 0
        assert summary["averages"]["memory_usage_mb"] > 0
        
        # Check operation stats
        assert "session_created" in summary["operation_stats"]
        assert summary["operation_stats"]["session_created"]["count"] == 4
        assert summary["operation_stats"]["session_created"]["avg_ms"] == 105
    
    async def test_threshold_updates(self, session_manager_mock, websocket_gateway_mock, buffer_manager_mock):
        """Test performance threshold updates"""
        # Arrange
        monitor = SessionPerformanceMonitor(
            session_manager_mock,
            websocket_gateway_mock,
            buffer_manager_mock
        )
        
        original_memory_threshold = monitor.thresholds["memory_usage_mb"]
        
        # Act
        monitor.set_threshold("memory_usage_mb", 2000)
        
        # Assert
        assert monitor.thresholds["memory_usage_mb"] == 2000
        assert monitor.thresholds["memory_usage_mb"] != original_memory_threshold
        
        # Test invalid threshold
        monitor.set_threshold("invalid_metric", 100)
        # Should not crash, just log warning
    
    async def test_operation_counts_reset(self, session_manager_mock, websocket_gateway_mock, buffer_manager_mock):
        """Test operation counts reset functionality"""
        # Arrange
        monitor = SessionPerformanceMonitor(
            session_manager_mock,
            websocket_gateway_mock,
            buffer_manager_mock
        )
        
        # Add some data
        monitor.record_operation("session_created", duration_ms=100)
        monitor.record_operation("buffer_stored", duration_ms=50)
        
        assert len(monitor.operation_counts) > 0
        assert len(monitor.operation_timings) > 0
        
        # Act
        monitor.reset_operation_counts()
        
        # Assert
        assert len(monitor.operation_counts) == 0
        assert len(monitor.operation_timings) == 0
    
    async def test_current_metrics_retrieval(self, session_manager_mock, websocket_gateway_mock, buffer_manager_mock):
        """Test current metrics retrieval"""
        # Arrange
        monitor = SessionPerformanceMonitor(
            session_manager_mock,
            websocket_gateway_mock,
            buffer_manager_mock
        )
        
        # Act - No metrics yet
        current = monitor.get_current_metrics()
        
        # Assert
        assert current is None
        
        # Add a metric
        metrics = PerformanceMetrics(
            timestamp=time.time(),
            session_count=5,
            active_connections=3,
            memory_usage_mb=150,
            cpu_usage_percent=30,
            redis_latency_ms=60,
            database_latency_ms=40,
            websocket_latency_ms=20,
            session_operations_per_second=2,
            error_rate_percent=1
        )
        monitor.metrics_history.append(metrics)
        
        # Act
        current = monitor.get_current_metrics()
        
        # Assert
        assert current is not None
        assert current.session_count == 5
        assert current.active_connections == 3


@pytest.fixture
def websocket_gateway_mock():
    """Mock WebSocket gateway"""
    gateway = AsyncMock()
    gateway.get_connection_count = MagicMock(return_value=0)
    return gateway