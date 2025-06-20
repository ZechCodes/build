"""
Performance benchmark tests for Session 6 implementation

Tests validate that Session Manager meets all performance targets:
- Session creation response time < 200ms
- Session retrieval response time < 50ms
- Session state updates < 25ms
- Buffer write operations < 100ms
- Buffer read operations < 25ms
- Recovery initiation < 200ms
- Recovery completion < 5 seconds
- Memory usage < 100MB per 1000 sessions
"""
import pytest
import asyncio
import time
import psutil
import statistics
from typing import List, Dict, Any
from unittest.mock import AsyncMock

import sys
import os

# Add the session-manager directory to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '../..'))

from core.state_manager import SessionStateManager, SessionState
from websocket.gateway import WebSocketGateway
from persistence.buffer_manager import SessionBufferManager
from persistence.recovery_manager import RecoveryManager
from monitoring.performance_monitor import SessionPerformanceMonitor


class PerformanceBenchmarks:
    """Performance benchmark test suite"""
    
    def __init__(self):
        self.results: Dict[str, List[float]] = {}
        self.memory_usage: List[float] = []
        
    def record_operation(self, operation: str, duration_ms: float):
        """Record operation timing"""
        if operation not in self.results:
            self.results[operation] = []
        self.results[operation].append(duration_ms)
    
    def record_memory_usage(self):
        """Record current memory usage"""
        process = psutil.Process()
        memory_mb = process.memory_info().rss / 1024 / 1024
        self.memory_usage.append(memory_mb)
    
    def get_stats(self, operation: str) -> Dict[str, float]:
        """Get statistics for an operation"""
        if operation not in self.results or not self.results[operation]:
            return {}
        
        timings = self.results[operation]
        return {
            "count": len(timings),
            "mean": statistics.mean(timings),
            "median": statistics.median(timings),
            "p95": self._percentile(timings, 95),
            "p99": self._percentile(timings, 99),
            "min": min(timings),
            "max": max(timings)
        }
    
    def _percentile(self, data: List[float], percentile: float) -> float:
        """Calculate percentile"""
        if not data:
            return 0.0
        sorted_data = sorted(data)
        index = (percentile / 100) * (len(sorted_data) - 1)
        lower = int(index)
        upper = min(lower + 1, len(sorted_data) - 1)
        weight = index - lower
        return sorted_data[lower] * (1 - weight) + sorted_data[upper] * weight


class TestSessionManagerPerformance:
    """Performance test suite for Session Manager"""
    
    @pytest.fixture
    def benchmarks(self):
        """Performance benchmark recording fixture"""
        return PerformanceBenchmarks()
    
    async def test_session_creation_performance(self, stateful_redis_mock, benchmarks):
        """Test session creation meets performance target < 200ms"""
        # Arrange
        state_manager = SessionStateManager(stateful_redis_mock)
        await state_manager.initialize()
        
        try:
            # Warm up
            for i in range(10):
                await state_manager.create_session(f"warmup_user_{i}", f"warmup_vm_{i}")
            
            # Benchmark session creation
            for i in range(100):
                start_time = time.time()
                
                session_id = await state_manager.create_session(
                    user_id=f"perf_user_{i}",
                    vm_id=f"perf_vm_{i}",
                    terminal_size=(80, 24),
                    environment_vars={"TERM": "xterm-256color"}
                )
                
                duration_ms = (time.time() - start_time) * 1000
                benchmarks.record_operation("session_creation", duration_ms)
                benchmarks.record_memory_usage()
                
                assert session_id is not None
            
            # Analyze results
            stats = benchmarks.get_stats("session_creation")
            print(f"Session Creation Performance:")
            print(f"  Mean: {stats['mean']:.2f}ms")
            print(f"  Median: {stats['median']:.2f}ms")
            print(f"  P95: {stats['p95']:.2f}ms")
            print(f"  P99: {stats['p99']:.2f}ms")
            print(f"  Max: {stats['max']:.2f}ms")
            
            # Assert performance targets
            assert stats['mean'] < 200.0, f"Mean session creation time {stats['mean']:.2f}ms exceeds 200ms target"
            assert stats['p95'] < 300.0, f"P95 session creation time {stats['p95']:.2f}ms exceeds 300ms tolerance"
            assert stats['p99'] < 500.0, f"P99 session creation time {stats['p99']:.2f}ms exceeds 500ms tolerance"
            
        finally:
            await state_manager.stop()
    
    async def test_session_retrieval_performance(self, stateful_redis_mock, benchmarks):
        """Test session retrieval meets performance target < 50ms"""
        # Arrange
        state_manager = SessionStateManager(stateful_redis_mock)
        await state_manager.initialize()
        
        try:
            # Create test sessions
            session_ids = []
            for i in range(50):
                session_id = await state_manager.create_session(
                    user_id=f"retrieval_user_{i}",
                    vm_id=f"retrieval_vm_{i}"
                )
                session_ids.append(session_id)
            
            # Benchmark session retrieval
            for session_id in session_ids:
                start_time = time.time()
                
                session = await state_manager.get_session(session_id)
                
                duration_ms = (time.time() - start_time) * 1000
                benchmarks.record_operation("session_retrieval", duration_ms)
                
                assert session is not None
            
            # Analyze results
            stats = benchmarks.get_stats("session_retrieval")
            print(f"Session Retrieval Performance:")
            print(f"  Mean: {stats['mean']:.2f}ms")
            print(f"  Median: {stats['median']:.2f}ms")
            print(f"  P95: {stats['p95']:.2f}ms")
            print(f"  P99: {stats['p99']:.2f}ms")
            
            # Assert performance targets
            assert stats['mean'] < 50.0, f"Mean session retrieval time {stats['mean']:.2f}ms exceeds 50ms target"
            assert stats['p95'] < 75.0, f"P95 session retrieval time {stats['p95']:.2f}ms exceeds 75ms tolerance"
            
        finally:
            await state_manager.stop()
    
    async def test_session_state_update_performance(self, stateful_redis_mock, benchmarks):
        """Test session state updates meet performance target < 25ms"""
        # Arrange
        state_manager = SessionStateManager(stateful_redis_mock)
        await state_manager.initialize()
        
        try:
            # Create test sessions
            session_ids = []
            for i in range(50):
                session_id = await state_manager.create_session(
                    user_id=f"update_user_{i}",
                    vm_id=f"update_vm_{i}"
                )
                session_ids.append(session_id)
            
            # Benchmark state updates
            states = [SessionState.ACTIVE, SessionState.IDLE, SessionState.SUSPENDED]
            for i, session_id in enumerate(session_ids):
                state = states[i % len(states)]
                
                start_time = time.time()
                
                success = await state_manager.update_session_state(session_id, state)
                
                duration_ms = (time.time() - start_time) * 1000
                benchmarks.record_operation("state_update", duration_ms)
                
                assert success is True
            
            # Analyze results
            stats = benchmarks.get_stats("state_update")
            print(f"Session State Update Performance:")
            print(f"  Mean: {stats['mean']:.2f}ms")
            print(f"  Median: {stats['median']:.2f}ms")
            print(f"  P95: {stats['p95']:.2f}ms")
            print(f"  P99: {stats['p99']:.2f}ms")
            
            # Assert performance targets
            assert stats['mean'] < 25.0, f"Mean state update time {stats['mean']:.2f}ms exceeds 25ms target"
            assert stats['p95'] < 50.0, f"P95 state update time {stats['p95']:.2f}ms exceeds 50ms tolerance"
            
        finally:
            await state_manager.stop()
    
    async def test_buffer_write_performance(self, stateful_redis_mock, benchmarks):
        """Test buffer write operations meet performance target < 100ms"""
        # Arrange
        buffer_manager = SessionBufferManager(stateful_redis_mock)
        
        # Create test sessions
        session_data = []
        for i in range(50):
            session_data.append({
                "session_id": f"buffer_session_{i}",
                "user_id": f"buffer_user_{i}",
                "buffer_data": f"Performance test buffer data for session {i}\n$ command\noutput line 1\noutput line 2\n".encode(),
                "cursor_pos": (0, 3),
                "scroll_pos": 1
            })
        
        # Benchmark buffer writes
        for data in session_data:
            start_time = time.time()
            
            success = await buffer_manager.store_buffer(
                session_id=data["session_id"],
                user_id=data["user_id"],
                buffer_data=data["buffer_data"],
                cursor_pos=data["cursor_pos"],
                scroll_pos=data["scroll_pos"]
            )
            
            duration_ms = (time.time() - start_time) * 1000
            benchmarks.record_operation("buffer_write", duration_ms)
            benchmarks.record_memory_usage()
            
            assert success is True
        
        # Analyze results
        stats = benchmarks.get_stats("buffer_write")
        print(f"Buffer Write Performance:")
        print(f"  Mean: {stats['mean']:.2f}ms")
        print(f"  Median: {stats['median']:.2f}ms")
        print(f"  P95: {stats['p95']:.2f}ms")
        print(f"  P99: {stats['p99']:.2f}ms")
        
        # Assert performance targets
        assert stats['mean'] < 100.0, f"Mean buffer write time {stats['mean']:.2f}ms exceeds 100ms target"
        assert stats['p95'] < 150.0, f"P95 buffer write time {stats['p95']:.2f}ms exceeds 150ms tolerance"
    
    async def test_buffer_read_performance(self, stateful_redis_mock, benchmarks):
        """Test buffer read operations meet performance target < 25ms"""
        # Arrange
        buffer_manager = SessionBufferManager(stateful_redis_mock)
        
        # Create and store test data
        test_sessions = []
        for i in range(50):
            session_id = f"read_session_{i}"
            user_id = f"read_user_{i}"
            buffer_data = f"Read performance test data {i}\n$ command\noutput\n".encode()
            
            await buffer_manager.store_buffer(
                session_id=session_id,
                user_id=user_id,
                buffer_data=buffer_data,
                cursor_pos=(0, 2)
            )
            test_sessions.append((session_id, user_id))
        
        # Benchmark buffer reads
        for session_id, user_id in test_sessions:
            start_time = time.time()
            
            buffer_data = await buffer_manager.retrieve_buffer(session_id, user_id)
            
            duration_ms = (time.time() - start_time) * 1000
            benchmarks.record_operation("buffer_read", duration_ms)
            
            assert buffer_data is not None
        
        # Analyze results
        stats = benchmarks.get_stats("buffer_read")
        print(f"Buffer Read Performance:")
        print(f"  Mean: {stats['mean']:.2f}ms")
        print(f"  Median: {stats['median']:.2f}ms")
        print(f"  P95: {stats['p95']:.2f}ms")
        print(f"  P99: {stats['p99']:.2f}ms")
        
        # Assert performance targets
        assert stats['mean'] < 25.0, f"Mean buffer read time {stats['mean']:.2f}ms exceeds 25ms target"
        assert stats['p95'] < 50.0, f"P95 buffer read time {stats['p95']:.2f}ms exceeds 50ms tolerance"
    
    async def test_recovery_initiation_performance(self, stateful_redis_mock, benchmarks):
        """Test recovery initiation meets performance target < 200ms"""
        # Arrange
        state_manager = SessionStateManager(stateful_redis_mock)
        await state_manager.initialize()
        
        buffer_manager = SessionBufferManager(stateful_redis_mock)
        
        # Mock WebSocket gateway for recovery
        gateway_mock = AsyncMock()
        gateway_mock.send_to_connection = AsyncMock(return_value=True)
        
        recovery_manager = RecoveryManager(state_manager, buffer_manager, gateway_mock)
        await recovery_manager.initialize()
        
        try:
            # Create test sessions with buffer data
            session_data = []
            for i in range(25):
                user_id = f"recovery_user_{i}"
                session_id = await state_manager.create_session(user_id, f"recovery_vm_{i}")
                
                # Store buffer data
                buffer_data = f"Recovery test data {i}\n$ history\ncommand {i}\n".encode()
                await buffer_manager.store_buffer(
                    session_id=session_id,
                    user_id=user_id,
                    buffer_data=buffer_data,
                    cursor_pos=(0, 2)
                )
                
                session_data.append((session_id, user_id))
            
            # Benchmark recovery initiation
            for session_id, user_id in session_data:
                connection_id = f"conn_{session_id}"
                
                start_time = time.time()
                
                success = await recovery_manager.initiate_recovery(
                    session_id=session_id,
                    user_id=user_id,
                    connection_id=connection_id
                )
                
                duration_ms = (time.time() - start_time) * 1000
                benchmarks.record_operation("recovery_initiation", duration_ms)
                
                assert success is True
            
            # Analyze results
            stats = benchmarks.get_stats("recovery_initiation")
            print(f"Recovery Initiation Performance:")
            print(f"  Mean: {stats['mean']:.2f}ms")
            print(f"  Median: {stats['median']:.2f}ms")
            print(f"  P95: {stats['p95']:.2f}ms")
            print(f"  P99: {stats['p99']:.2f}ms")
            
            # Assert performance targets
            assert stats['mean'] < 200.0, f"Mean recovery initiation time {stats['mean']:.2f}ms exceeds 200ms target"
            assert stats['p95'] < 300.0, f"P95 recovery initiation time {stats['p95']:.2f}ms exceeds 300ms tolerance"
            
        finally:
            await state_manager.stop()
            await recovery_manager.stop()
    
    async def test_memory_usage_performance(self, stateful_redis_mock, benchmarks):
        """Test memory usage meets target < 100MB per 1000 sessions"""
        # Arrange
        state_manager = SessionStateManager(stateful_redis_mock)
        await state_manager.initialize()
        
        buffer_manager = SessionBufferManager(stateful_redis_mock)
        
        try:
            # Record initial memory
            initial_memory = psutil.Process().memory_info().rss / 1024 / 1024
            benchmarks.record_memory_usage()
            
            # Create 100 sessions (scaled down for testing)
            session_count = 100
            session_ids = []
            
            for i in range(session_count):
                # Create session
                session_id = await state_manager.create_session(
                    user_id=f"memory_user_{i}",
                    vm_id=f"memory_vm_{i}",
                    terminal_size=(80, 24),
                    environment_vars={"TERM": "xterm-256color", "USER": f"user_{i}"}
                )
                session_ids.append(session_id)
                
                # Store buffer data
                buffer_data = f"Memory test session {i} with terminal output\n$ ls -la\ntotal 4\ndrwxr-xr-x 2 user user 4096 Jan  1 12:00 .\ndrwxr-xr-x 3 user user 4096 Jan  1 12:00 ..\n-rw-r--r-- 1 user user   25 Jan  1 12:00 file{i}.txt\n".encode()
                await buffer_manager.store_buffer(
                    session_id=session_id,
                    user_id=f"memory_user_{i}",
                    buffer_data=buffer_data,
                    cursor_pos=(0, 5)
                )
                
                # Record memory every 10 sessions
                if i % 10 == 0:
                    benchmarks.record_memory_usage()
            
            # Final memory measurement
            final_memory = psutil.Process().memory_info().rss / 1024 / 1024
            benchmarks.record_memory_usage()
            
            # Calculate memory usage
            memory_increase = final_memory - initial_memory
            memory_per_session = memory_increase / session_count
            
            # Scale to 1000 sessions
            memory_per_1000_sessions = memory_per_session * 1000
            
            print(f"Memory Usage Performance:")
            print(f"  Initial Memory: {initial_memory:.2f}MB")
            print(f"  Final Memory: {final_memory:.2f}MB")
            print(f"  Memory Increase: {memory_increase:.2f}MB")
            print(f"  Memory per Session: {memory_per_session:.4f}MB")
            print(f"  Projected Memory per 1000 Sessions: {memory_per_1000_sessions:.2f}MB")
            print(f"  Session Count: {session_count}")
            
            # Assert memory target
            assert memory_per_1000_sessions < 100.0, f"Memory usage {memory_per_1000_sessions:.2f}MB per 1000 sessions exceeds 100MB target"
            
            # Verify sessions are accessible
            sample_sessions = session_ids[::10]  # Every 10th session
            for session_id in sample_sessions:
                session = await state_manager.get_session(session_id)
                assert session is not None
                
        finally:
            await state_manager.stop()
    
    async def test_concurrent_performance(self, stateful_redis_mock, benchmarks):
        """Test performance under concurrent load"""
        # Arrange
        state_manager = SessionStateManager(stateful_redis_mock)
        await state_manager.initialize()
        
        buffer_manager = SessionBufferManager(stateful_redis_mock)
        
        try:
            async def concurrent_operations(worker_id: int):
                """Perform concurrent session operations"""
                results = []
                
                for i in range(10):
                    operation_id = f"{worker_id}_{i}"
                    
                    # Create session
                    start = time.time()
                    session_id = await state_manager.create_session(
                        user_id=f"concurrent_user_{operation_id}",
                        vm_id=f"concurrent_vm_{operation_id}"
                    )
                    results.append(("session_creation", (time.time() - start) * 1000))
                    
                    # Store buffer
                    start = time.time()
                    await buffer_manager.store_buffer(
                        session_id=session_id,
                        user_id=f"concurrent_user_{operation_id}",
                        buffer_data=f"Concurrent test data {operation_id}".encode(),
                        cursor_pos=(0, 1)
                    )
                    results.append(("buffer_write", (time.time() - start) * 1000))
                    
                    # Retrieve session
                    start = time.time()
                    session = await state_manager.get_session(session_id)
                    results.append(("session_retrieval", (time.time() - start) * 1000))
                    
                    # Update state
                    start = time.time()
                    await state_manager.update_session_state(session_id, SessionState.IDLE)
                    results.append(("state_update", (time.time() - start) * 1000))
                
                return results
            
            # Run 10 concurrent workers
            workers = [concurrent_operations(i) for i in range(10)]
            all_results = await asyncio.gather(*workers)
            
            # Process results
            for worker_results in all_results:
                for operation, duration in worker_results:
                    benchmarks.record_operation(f"concurrent_{operation}", duration)
            
            # Analyze concurrent performance
            operations = ["session_creation", "buffer_write", "session_retrieval", "state_update"]
            targets = {"session_creation": 200, "buffer_write": 100, "session_retrieval": 50, "state_update": 25}
            
            for operation in operations:
                concurrent_op = f"concurrent_{operation}"
                stats = benchmarks.get_stats(concurrent_op)
                target = targets[operation]
                
                print(f"Concurrent {operation.replace('_', ' ').title()} Performance:")
                print(f"  Mean: {stats['mean']:.2f}ms (target: <{target}ms)")
                print(f"  P95: {stats['p95']:.2f}ms")
                
                # Allow 50% degradation under concurrent load
                tolerance = target * 1.5
                assert stats['mean'] < tolerance, f"Concurrent {operation} performance {stats['mean']:.2f}ms exceeds tolerance {tolerance}ms"
        
        finally:
            await state_manager.stop()
    
    async def test_stress_test_performance(self, stateful_redis_mock, benchmarks):
        """Stress test to validate system stability under high load"""
        # Arrange
        state_manager = SessionStateManager(stateful_redis_mock)
        await state_manager.initialize()
        
        buffer_manager = SessionBufferManager(stateful_redis_mock)
        monitor = SessionPerformanceMonitor(state_manager, None, buffer_manager)
        await monitor.initialize()
        
        try:
            # Record initial metrics
            start_memory = psutil.Process().memory_info().rss / 1024 / 1024
            start_time = time.time()
            
            # Stress test: Create many sessions rapidly
            session_count = 200
            session_ids = []
            
            print(f"Starting stress test with {session_count} sessions...")
            
            for i in range(session_count):
                session_id = await state_manager.create_session(
                    user_id=f"stress_user_{i}",
                    vm_id=f"stress_vm_{i}"
                )
                session_ids.append(session_id)
                
                # Store buffer data
                buffer_data = f"Stress test session {i}\n$ stress_command_{i}\noutput_{i}\n".encode()
                await buffer_manager.store_buffer(
                    session_id=session_id,
                    user_id=f"stress_user_{i}",
                    buffer_data=buffer_data,
                    cursor_pos=(0, 2)
                )
                
                # Update state
                await state_manager.update_session_state(session_id, SessionState.ACTIVE)
                
                # Record metrics periodically
                if i % 20 == 0:
                    monitor.record_operation("stress_session_created", success=True)
                    benchmarks.record_memory_usage()
            
            # Test retrieval performance under load
            retrieval_times = []
            for session_id in session_ids[::10]:  # Every 10th session
                start = time.time()
                session = await state_manager.get_session(session_id)
                duration = (time.time() - start) * 1000
                retrieval_times.append(duration)
                assert session is not None
            
            # Final measurements
            end_memory = psutil.Process().memory_info().rss / 1024 / 1024
            total_duration = time.time() - start_time
            
            # Analyze stress test results
            memory_increase = end_memory - start_memory
            sessions_per_second = session_count / total_duration
            avg_retrieval_time = statistics.mean(retrieval_times)
            
            print(f"Stress Test Results:")
            print(f"  Sessions Created: {session_count}")
            print(f"  Total Duration: {total_duration:.2f}s")
            print(f"  Sessions/Second: {sessions_per_second:.2f}")
            print(f"  Memory Increase: {memory_increase:.2f}MB")
            print(f"  Average Retrieval Time: {avg_retrieval_time:.2f}ms")
            
            # Assert stress test passed
            assert sessions_per_second >= 20, f"Session creation rate {sessions_per_second:.2f}/s too low"
            assert avg_retrieval_time < 100, f"Retrieval time {avg_retrieval_time:.2f}ms too high under load"
            assert memory_increase < 50, f"Memory increase {memory_increase:.2f}MB too high for {session_count} sessions"
            
        finally:
            await state_manager.stop()
            await monitor.stop()


if __name__ == "__main__":
    pytest.main([__file__, "-v", "-s"])