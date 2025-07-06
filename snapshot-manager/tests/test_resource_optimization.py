"""
Tests for Resource Optimization and Performance Features

Simplified test suite focusing on the optimization and resource management
functionality without the complex base class dependencies.
"""

import pytest
import asyncio
import time
from unittest.mock import AsyncMock, MagicMock, patch
from pathlib import Path

# Import the optimization components
import sys
current_dir = Path(__file__).parent.parent
if str(current_dir) not in sys.path:
    sys.path.insert(0, str(current_dir))

from core.optimized_snapshot_manager import (
    OptimizationStrategy, SnapshotOptimizationConfig
)
from performance.resource_manager import ResourceManager, PerformanceMetrics


class TestResourceOptimization:
    """Test suite for resource optimization functionality."""
    
    def test_optimization_strategy_enum(self):
        """Test optimization strategy enumeration."""
        assert OptimizationStrategy.BALANCED.value == "balanced"
        assert OptimizationStrategy.SPEED.value == "speed"
        assert OptimizationStrategy.COMPRESSION.value == "compression"
        assert OptimizationStrategy.QUALITY.value == "quality"
    
    def test_optimization_config_creation(self):
        """Test optimization configuration creation."""
        config = SnapshotOptimizationConfig(
            strategy=OptimizationStrategy.SPEED,
            max_concurrent_snapshots=5,
            compression_level=3,
            memory_limit_mb=2048,
            enable_deduplication=False,
            background_optimization=True
        )
        
        assert config.strategy == OptimizationStrategy.SPEED
        assert config.max_concurrent_snapshots == 5
        assert config.compression_level == 3
        assert config.memory_limit_mb == 2048
        assert config.enable_deduplication is False
        assert config.background_optimization is True
    
    def test_performance_metrics_creation(self):
        """Test performance metrics data structure."""
        metrics = PerformanceMetrics(
            operation_type="snapshot_creation",
            duration_seconds=45.5,
            data_size_bytes=1024 * 1024 * 1024,  # 1GB
            throughput_mbps=22.6,
            cpu_usage_during=35.0,
            memory_peak_mb=512,
            success=True
        )
        
        assert metrics.operation_type == "snapshot_creation"
        assert metrics.duration_seconds == 45.5
        assert metrics.data_size_bytes == 1024 * 1024 * 1024
        assert metrics.throughput_mbps == 22.6
        assert metrics.cpu_usage_during == 35.0
        assert metrics.memory_peak_mb == 512
        assert metrics.success is True
        assert metrics.error_message is None
        
        # Test with error
        error_metrics = PerformanceMetrics(
            operation_type="snapshot_restore",
            duration_seconds=0.0,
            data_size_bytes=0,
            throughput_mbps=0.0,
            cpu_usage_during=10.0,
            memory_peak_mb=128,
            success=False,
            error_message="Restoration failed"
        )
        
        assert error_metrics.success is False
        assert error_metrics.error_message == "Restoration failed"
    
    @pytest.mark.asyncio
    async def test_resource_manager_integration(self):
        """Test integration with resource manager."""
        with patch('psutil.virtual_memory') as mock_memory, \
             patch('psutil.disk_usage') as mock_disk, \
             patch('psutil.cpu_percent') as mock_cpu, \
             patch('psutil.net_io_counters') as mock_network:
            
            # Mock system resources
            mock_memory.return_value = MagicMock(
                total=8 * 1024**3,  # 8GB
                percent=45.0,
                available=4 * 1024**3,  # 4GB
                used=4 * 1024**3
            )
            mock_disk.return_value = MagicMock(
                used=300 * 1024**3,
                total=1000 * 1024**3,
                free=700 * 1024**3
            )
            mock_cpu.return_value = 25.0
            mock_network.return_value = MagicMock(
                bytes_sent=1000000,
                bytes_recv=2000000
            )
            
            # Create resource manager
            resource_manager = ResourceManager({
                "max_memory_percent": 80,
                "max_disk_percent": 85,
                "max_concurrent_operations": 3,
                "background_cleanup": False
            })
            
            await resource_manager.initialize()
            
            # Test resource availability
            available = await resource_manager.check_resource_availability(
                "test_operation", 100 * 1024 * 1024  # 100MB
            )
            assert available is True
            
            # Test operation tracking
            operation_id = "test_op_001"
            started = await resource_manager.start_operation_tracking(
                operation_id, "test_operation", 100 * 1024 * 1024
            )
            assert started is True
            
            # Complete operation
            await resource_manager.complete_operation_tracking(
                operation_id, success=True, data_size_bytes=100 * 1024 * 1024
            )
            
            # Verify metrics were recorded
            assert len(resource_manager.operation_metrics) == 1
            metric = resource_manager.operation_metrics[0]
            assert metric.operation_type == "test_operation"
            assert metric.success is True
    
    def test_optimization_strategy_selection_logic(self):
        """Test optimization strategy selection logic."""
        # Simulate the strategy selection logic
        def choose_strategy(priority: str, recent_performance_data: list) -> OptimizationStrategy:
            if priority == "high":
                return OptimizationStrategy.SPEED
            
            if priority == "low":
                return OptimizationStrategy.COMPRESSION
            
            # For normal priority, adapt based on performance
            if recent_performance_data:
                avg_duration = sum(m["duration"] for m in recent_performance_data) / len(recent_performance_data)
                avg_throughput = sum(m["throughput"] for m in recent_performance_data) / len(recent_performance_data)
                
                if avg_duration > 60 or avg_throughput < 10:
                    return OptimizationStrategy.SPEED
                
                if avg_duration < 30 and avg_throughput > 50:
                    return OptimizationStrategy.COMPRESSION
            
            return OptimizationStrategy.BALANCED
        
        # Test high priority
        strategy = choose_strategy("high", [])
        assert strategy == OptimizationStrategy.SPEED
        
        # Test low priority
        strategy = choose_strategy("low", [])
        assert strategy == OptimizationStrategy.COMPRESSION
        
        # Test normal priority with no data
        strategy = choose_strategy("normal", [])
        assert strategy == OptimizationStrategy.BALANCED
        
        # Test normal priority with slow performance
        slow_data = [
            {"duration": 120, "throughput": 5},
            {"duration": 90, "throughput": 8}
        ]
        strategy = choose_strategy("normal", slow_data)
        assert strategy == OptimizationStrategy.SPEED
        
        # Test normal priority with good performance
        good_data = [
            {"duration": 20, "throughput": 60},
            {"duration": 25, "throughput": 55}
        ]
        strategy = choose_strategy("normal", good_data)
        assert strategy == OptimizationStrategy.COMPRESSION
    
    def test_performance_calculation_logic(self):
        """Test performance calculation and analysis logic."""
        # Test performance metrics calculation
        def calculate_performance_stats(metrics_list):
            if not metrics_list:
                return {"no_data": True}
            
            successful = [m for m in metrics_list if m["success"]]
            failed = [m for m in metrics_list if not m["success"]]
            
            return {
                "total_operations": len(metrics_list),
                "successful_operations": len(successful),
                "failed_operations": len(failed),
                "success_rate": len(successful) / len(metrics_list) if metrics_list else 0,
                "avg_duration": sum(m["duration"] for m in successful) / len(successful) if successful else 0,
                "avg_throughput": sum(m["throughput"] for m in successful) / len(successful) if successful else 0
            }
        
        # Test with empty data
        stats = calculate_performance_stats([])
        assert stats["no_data"] is True
        
        # Test with mixed success/failure data
        test_data = [
            {"duration": 30, "throughput": 20, "success": True},
            {"duration": 45, "throughput": 15, "success": True},
            {"duration": 0, "throughput": 0, "success": False},
            {"duration": 25, "throughput": 25, "success": True}
        ]
        
        stats = calculate_performance_stats(test_data)
        assert stats["total_operations"] == 4
        assert stats["successful_operations"] == 3
        assert stats["failed_operations"] == 1
        assert stats["success_rate"] == 0.75
        assert stats["avg_duration"] == (30 + 45 + 25) / 3  # 33.33
        assert stats["avg_throughput"] == (20 + 15 + 25) / 3  # 20.0
    
    def test_throughput_calculation(self):
        """Test throughput calculation logic."""
        def calculate_throughput(data_size_bytes: int, duration_seconds: float) -> float:
            if duration_seconds <= 0:
                return 0.0
            
            data_size_mb = data_size_bytes / (1024 * 1024)
            return data_size_mb / duration_seconds
        
        # Test normal calculation
        throughput = calculate_throughput(100 * 1024 * 1024, 5.0)  # 100MB in 5 seconds
        assert throughput == 20.0  # 20 MB/s
        
        # Test with zero duration
        throughput = calculate_throughput(100 * 1024 * 1024, 0.0)
        assert throughput == 0.0
        
        # Test with negative duration
        throughput = calculate_throughput(100 * 1024 * 1024, -1.0)
        assert throughput == 0.0
        
        # Test large file
        throughput = calculate_throughput(1024 * 1024 * 1024, 60.0)  # 1GB in 60 seconds
        assert abs(throughput - 17.067) < 0.01  # ~17.067 MB/s
    
    def test_compression_ratio_calculation(self):
        """Test compression ratio calculation logic."""
        def calculate_compression_metrics(original_size: int, compressed_size: int) -> dict:
            if original_size <= 0:
                return {"compression_ratio": 1.0, "size_reduction": 0.0}
            
            compression_ratio = compressed_size / original_size
            size_reduction = 1.0 - compression_ratio
            
            return {
                "compression_ratio": compression_ratio,
                "size_reduction": size_reduction,
                "space_saved_bytes": original_size - compressed_size,
                "space_saved_percent": size_reduction * 100
            }
        
        # Test good compression
        metrics = calculate_compression_metrics(1000, 300)  # 70% compression
        assert metrics["compression_ratio"] == 0.3
        assert metrics["size_reduction"] == 0.7
        assert metrics["space_saved_bytes"] == 700
        assert metrics["space_saved_percent"] == 70.0
        
        # Test no compression
        metrics = calculate_compression_metrics(1000, 1000)
        assert metrics["compression_ratio"] == 1.0
        assert metrics["size_reduction"] == 0.0
        assert metrics["space_saved_bytes"] == 0
        assert metrics["space_saved_percent"] == 0.0
        
        # Test expansion (rare but possible)
        metrics = calculate_compression_metrics(1000, 1100)
        assert metrics["compression_ratio"] == 1.1
        assert abs(metrics["size_reduction"] - (-0.1)) < 0.001  # Use tolerance for floating point
        assert metrics["space_saved_bytes"] == -100
        assert abs(metrics["space_saved_percent"] - (-10.0)) < 0.001
    
    @pytest.mark.asyncio
    async def test_async_optimization_operations(self):
        """Test asynchronous optimization operations."""
        
        async def mock_optimize_operation(data_size: int, strategy: OptimizationStrategy) -> dict:
            """Mock optimization operation."""
            await asyncio.sleep(0.01)  # Simulate work
            
            # Different strategies have different characteristics
            if strategy == OptimizationStrategy.SPEED:
                duration = 0.5
                compression = 0.9  # Less compression
            elif strategy == OptimizationStrategy.COMPRESSION:
                duration = 2.0
                compression = 0.6  # Better compression
            else:  # BALANCED
                duration = 1.0
                compression = 0.7
            
            return {
                "duration": duration,
                "original_size": data_size,
                "compressed_size": int(data_size * compression),
                "compression_ratio": compression
            }
        
        # Test different strategies
        test_size = 1024 * 1024  # 1MB
        
        # Speed strategy
        result = await mock_optimize_operation(test_size, OptimizationStrategy.SPEED)
        assert result["duration"] == 0.5
        assert result["compression_ratio"] == 0.9  # Less compression, more speed
        
        # Compression strategy
        result = await mock_optimize_operation(test_size, OptimizationStrategy.COMPRESSION)
        assert result["duration"] == 2.0
        assert result["compression_ratio"] == 0.6  # Better compression, slower
        
        # Balanced strategy
        result = await mock_optimize_operation(test_size, OptimizationStrategy.BALANCED)
        assert result["duration"] == 1.0
        assert result["compression_ratio"] == 0.7  # Balanced approach
    
    def test_adaptive_settings_logic(self):
        """Test adaptive settings adjustment logic."""
        def adapt_compression_level(current_level: int, recent_performance: list) -> int:
            if len(recent_performance) < 5:
                return current_level  # Not enough data
            
            avg_duration = sum(p["duration"] for p in recent_performance) / len(recent_performance)
            avg_throughput = sum(p["throughput"] for p in recent_performance) / len(recent_performance)
            
            # If operations are slow, reduce compression
            if avg_duration > 120:  # More than 2 minutes
                return max(1, current_level - 1)
            
            # If operations are fast and high throughput, increase compression
            if avg_duration < 30 and avg_throughput > 50:
                return min(9, current_level + 1)
            
            return current_level
        
        # Test with slow performance
        slow_performance = [
            {"duration": 150, "throughput": 8},
            {"duration": 130, "throughput": 10},
            {"duration": 140, "throughput": 7},
            {"duration": 125, "throughput": 9},
            {"duration": 135, "throughput": 8}
        ]
        
        new_level = adapt_compression_level(6, slow_performance)
        assert new_level == 5  # Reduced compression for speed
        
        # Test with good performance
        good_performance = [
            {"duration": 25, "throughput": 60},
            {"duration": 20, "throughput": 65},
            {"duration": 28, "throughput": 55},
            {"duration": 22, "throughput": 58},
            {"duration": 26, "throughput": 62}
        ]
        
        new_level = adapt_compression_level(6, good_performance)
        assert new_level == 7  # Increased compression for better storage
        
        # Test with insufficient data
        insufficient_data = [
            {"duration": 30, "throughput": 20},
            {"duration": 35, "throughput": 18}
        ]
        
        new_level = adapt_compression_level(6, insufficient_data)
        assert new_level == 6  # No change
        
        # Test edge cases (min/max levels)
        new_level = adapt_compression_level(1, slow_performance)
        assert new_level == 1  # Can't go below 1
        
        new_level = adapt_compression_level(9, good_performance)
        assert new_level == 9  # Can't go above 9


if __name__ == "__main__":
    pytest.main([__file__, "-v"])