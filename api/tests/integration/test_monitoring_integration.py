"""Integration tests for monitoring system."""

import pytest
from httpx import AsyncClient
import asyncio

from app.monitoring.alerts import get_alert_manager, AlertSeverity, AlertRule
from app.monitoring.dashboard import get_dashboard_manager, setup_default_dashboards
from app.monitoring.metrics import metrics_collector


class TestMonitoringIntegration:
    """Test monitoring system integration."""
    
    @pytest.mark.asyncio
    async def test_health_endpoints(self, async_client: AsyncClient):
        """Test health check endpoints."""
        # Basic health check
        response = await async_client.get("/health")
        assert response.status_code == 200
        
        health_data = response.json()
        assert health_data["status"] == "healthy"
        assert health_data["service"] == "build-api"
        
        # Detailed health check
        response = await async_client.get("/health/detailed")
        assert response.status_code == 200
        
        detailed_health = response.json()
        assert "dependencies" in detailed_health
        assert "database" in detailed_health["dependencies"]
        assert "redis" in detailed_health["dependencies"]
        assert "system" in detailed_health["dependencies"]
    
    @pytest.mark.asyncio
    async def test_metrics_endpoint(self, async_client: AsyncClient):
        """Test metrics endpoint."""
        response = await async_client.get("/metrics")
        assert response.status_code == 200
        
        # Should return Prometheus metrics format
        metrics_text = response.text
        assert "# HELP" in metrics_text or "# TYPE" in metrics_text
    
    @pytest.mark.asyncio
    async def test_system_metrics_endpoint(self, async_client: AsyncClient):
        """Test system metrics endpoint."""
        response = await async_client.get("/api/v1/health/system")
        assert response.status_code == 200
        
        system_metrics = response.json()
        assert "status" in system_metrics
        # Check for common system metrics
        expected_metrics = ["cpu_usage", "memory_usage", "disk_usage"]
        for metric in expected_metrics:
            if metric in system_metrics:
                assert isinstance(system_metrics[metric], (int, float))
    
    @pytest.mark.asyncio
    async def test_cache_metrics_endpoint(self, async_client: AsyncClient):
        """Test cache metrics endpoint."""
        response = await async_client.get("/api/v1/health/cache")
        assert response.status_code == 200
        
        cache_metrics = response.json()
        assert "status" in cache_metrics
    
    @pytest.mark.asyncio
    async def test_logfire_metrics_endpoint(self, async_client: AsyncClient):
        """Test Logfire metrics endpoint."""
        response = await async_client.get("/api/v1/metrics/logfire")
        
        # This might not be available in test environment
        if response.status_code == 200:
            logfire_metrics = response.json()
            assert "status" in logfire_metrics
            assert "logfire_enabled" in logfire_metrics
        else:
            # If Logfire is not available, should return error
            assert response.status_code in [200, 500]
    
    @pytest.mark.asyncio
    async def test_alert_manager_functionality(self):
        """Test alert manager functionality."""
        alert_manager = get_alert_manager()
        
        # Test adding an alert rule
        test_condition = lambda: True  # Always true for testing
        
        rule = AlertRule(
            name="test_alert",
            condition=test_condition,
            severity=AlertSeverity.MEDIUM,
            message="Test alert message",
            check_interval=1,  # 1 second for testing
            cooldown_minutes=0  # No cooldown for testing
        )
        
        alert_manager.add_rule(rule)
        assert "test_alert" in alert_manager.rules
        
        # Test firing an alert
        alert = await alert_manager.fire_alert(
            name="manual_test_alert",
            severity=AlertSeverity.HIGH,
            message="Manual test alert",
            labels={"test": "true"}
        )
        
        assert alert.name == "manual_test_alert"
        assert alert.severity == AlertSeverity.HIGH
        assert alert.labels["test"] == "true"
        
        # Test getting alerts
        alerts = await alert_manager.get_alerts()
        assert len(alerts) > 0
        assert any(a.name == "manual_test_alert" for a in alerts)
        
        # Test resolving alert
        resolved = await alert_manager.resolve_alert(alert.id)
        assert resolved is True
        
        # Test acknowledging alert (create another one first)
        alert2 = await alert_manager.fire_alert(
            name="ack_test_alert",
            severity=AlertSeverity.LOW,
            message="Acknowledgment test alert"
        )
        
        acknowledged = await alert_manager.acknowledge_alert(alert2.id, "test_user")
        assert acknowledged is True
        
        # Cleanup
        alert_manager.remove_rule("test_alert")
        await alert_manager.resolve_alert(alert2.id)
    
    @pytest.mark.asyncio
    async def test_dashboard_manager_functionality(self):
        """Test dashboard manager functionality."""
        dashboard_manager = get_dashboard_manager()
        
        # Setup default dashboards
        setup_default_dashboards()
        
        # Test listing dashboards
        dashboards = dashboard_manager.list_dashboards()
        assert len(dashboards) > 0
        
        # Test getting a specific dashboard
        main_dashboard = dashboard_manager.get_dashboard("main")
        assert main_dashboard is not None
        assert main_dashboard.title == "Build Platform - System Overview"
        assert len(main_dashboard.widgets) > 0
        
        # Test refreshing a dashboard
        refreshed = await dashboard_manager.refresh_dashboard("main")
        assert refreshed is True
        
        # Verify widgets have data after refresh
        for widget in main_dashboard.widgets.values():
            if widget.data is not None:
                assert isinstance(widget.data, dict)
    
    @pytest.mark.asyncio
    async def test_metrics_collection(self):
        """Test metrics collection functionality."""
        # Test basic metrics collection
        initial_count = metrics_collector.get_request_count()
        
        # Simulate some requests (this would normally be done by middleware)
        metrics_collector.record_request("GET", "/test", 200, 0.1)
        metrics_collector.record_request("POST", "/test", 201, 0.2)
        metrics_collector.record_request("GET", "/test", 500, 0.15)
        
        # Check that metrics were recorded
        new_count = metrics_collector.get_request_count()
        assert new_count >= initial_count + 3
        
        # Test error rate calculation
        error_rate = metrics_collector.get_error_rate(window_minutes=1)
        assert isinstance(error_rate, float)
        assert 0 <= error_rate <= 1
        
        # Test response time metrics
        avg_response_time = metrics_collector.get_avg_response_time(window_minutes=1)
        if avg_response_time is not None:
            assert isinstance(avg_response_time, float)
            assert avg_response_time > 0
    
    @pytest.mark.asyncio
    async def test_monitoring_middleware_integration(self, async_client: AsyncClient):
        """Test that monitoring middleware is working."""
        # Make a request to trigger middleware
        response = await async_client.get("/health")
        assert response.status_code == 200
        
        # Check that request was recorded in metrics
        request_count = metrics_collector.get_request_count()
        assert request_count > 0
        
        # Check for monitoring headers (if middleware adds them)
        if "X-Request-ID" in response.headers:
            assert len(response.headers["X-Request-ID"]) > 0
        
        if "X-Process-Time" in response.headers:
            process_time = float(response.headers["X-Process-Time"])
            assert process_time > 0
    
    @pytest.mark.asyncio
    async def test_alert_notification_flow(self):
        """Test alert notification flow."""
        alert_manager = get_alert_manager()
        notifications_received = []
        
        # Add a test notification handler
        def test_notification_handler(alert):
            notifications_received.append(alert)
        
        alert_manager.add_notification_handler(test_notification_handler)
        
        # Fire an alert
        alert = await alert_manager.fire_alert(
            name="notification_test",
            severity=AlertSeverity.CRITICAL,
            message="Test notification alert"
        )
        
        # Give some time for notification to be processed
        await asyncio.sleep(0.1)
        
        # Check that notification was received
        assert len(notifications_received) > 0
        assert notifications_received[-1].name == "notification_test"
        
        # Cleanup
        await alert_manager.resolve_alert(alert.id)
    
    @pytest.mark.asyncio
    async def test_dashboard_widget_refresh(self):
        """Test dashboard widget refresh functionality."""
        from app.monitoring.dashboard import DashboardWidget
        
        # Create a test widget with a simple query function
        call_count = 0
        
        def test_query():
            nonlocal call_count
            call_count += 1
            return {"test_value": call_count, "timestamp": "test"}
        
        widget = DashboardWidget(
            widget_id="test_widget",
            title="Test Widget",
            widget_type="gauge",
            query_func=test_query,
            refresh_interval=1
        )
        
        # Refresh the widget
        data = await widget.refresh_data()
        
        assert data["test_value"] == 1
        assert widget.last_updated is not None
        
        # Refresh again
        data2 = await widget.refresh_data()
        assert data2["test_value"] == 2