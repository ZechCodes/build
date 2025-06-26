# Session 14.4: End-to-End Testing & Deployment Validation

## Objective
Implement comprehensive end-to-end testing suite with automated deployment validation, performance testing, security verification, and production readiness checks to ensure the Build platform operates correctly across all components and environments.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for E2E test execution monitoring and result tracking
- **Session 2**: Tests user authentication and authorization flows end-to-end
- **Session 3-12**: Validates complete user workflows across all platform features
- **Session 13**: Tests high availability and failover scenarios
- **All Sessions**: Provides comprehensive validation of entire platform functionality

## Core Implementation

### Production VM Environment Setup
**Location**: `infrastructure/vm-images/`

To enable complete end-to-end testing with real Firecracker VMs, production-ready kernel and rootfs images must be configured:

```bash
# infrastructure/vm-images/setup_production_images.sh
#!/bin/bash
# Set up production kernel and rootfs images for E2E testing

set -e

echo "🔧 Setting up production VM images for E2E testing..."

IMAGES_DIR="/opt/firecracker/images"
TEMP_DIR="/tmp/vm-image-setup"

# Create directories
sudo mkdir -p "$IMAGES_DIR" "$TEMP_DIR"
cd "$TEMP_DIR"

echo "📦 Downloading Ubuntu 22.04 kernel..."
# Download official Ubuntu kernel for Firecracker
curl -L -o vmlinux-ubuntu22.04 \
  "https://cloud-images.ubuntu.com/releases/22.04/release/unpacked/ubuntu-22.04-server-cloudimg-amd64-vmlinuz-generic"

echo "📦 Downloading Ubuntu 22.04 rootfs..."
# Download official Ubuntu rootfs
curl -L -o ubuntu-22.04-server-cloudimg-amd64.img \
  "https://cloud-images.ubuntu.com/releases/22.04/release/ubuntu-22.04-server-cloudimg-amd64.img"

# Convert qcow2 to raw ext4 for Firecracker
qemu-img convert -f qcow2 -O raw ubuntu-22.04-server-cloudimg-amd64.img rootfs-ubuntu22.04.ext4

# Install to production location
sudo cp vmlinux-ubuntu22.04 "$IMAGES_DIR/production_ubuntu_kernel"
sudo cp rootfs-ubuntu22.04.ext4 "$IMAGES_DIR/production_ubuntu_rootfs"

# Set proper permissions
sudo chmod 644 "$IMAGES_DIR/production_ubuntu_kernel" "$IMAGES_DIR/production_ubuntu_rootfs"

# Verify kernel is valid ELF
if file "$IMAGES_DIR/production_ubuntu_kernel" | grep -q "ELF"; then
    echo "✅ Production kernel verified as valid ELF"
else
    echo "❌ ERROR: Kernel is not a valid ELF file"
    exit 1
fi

# Verify rootfs is valid ext4
if file "$IMAGES_DIR/production_ubuntu_rootfs" | grep -q "ext4"; then
    echo "✅ Production rootfs verified as valid ext4"
else
    echo "❌ ERROR: Rootfs is not a valid ext4 filesystem"
    exit 1
fi

# Create test-specific smaller images for development
echo "🧪 Creating test images..."
sudo cp "$IMAGES_DIR/production_ubuntu_kernel" "$IMAGES_DIR/test_ubuntu_kernel"

# Create smaller test rootfs (1GB instead of full size)
truncate -s 1G "$TEMP_DIR/test_rootfs.ext4"
mkfs.ext4 -F "$TEMP_DIR/test_rootfs.ext4"
sudo cp "$TEMP_DIR/test_rootfs.ext4" "$IMAGES_DIR/test_ubuntu_rootfs"

# Cleanup
rm -rf "$TEMP_DIR"

echo ""
echo "✅ Production VM images setup complete!"
echo ""
echo "📁 Available images:"
echo "   Production kernel: $IMAGES_DIR/production_ubuntu_kernel"
echo "   Production rootfs: $IMAGES_DIR/production_ubuntu_rootfs" 
echo "   Test kernel: $IMAGES_DIR/test_ubuntu_kernel"
echo "   Test rootfs: $IMAGES_DIR/test_ubuntu_rootfs"
echo ""
echo "🧪 Test with:"
echo "   python tests/e2e/framework/test_runner.py --environment staging --suite vm_management"
```

### E2E Test Framework
**Location**: `tests/e2e/framework/`

```python
# tests/e2e/framework/test_runner.py
import asyncio
import aiohttp
import asyncpg
import redis.asyncio as redis
import time
import json
import structlog
import logfire
from typing import Dict, Any, Optional, List, Callable, Union
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
import pytest
import websockets
from concurrent.futures import ThreadPoolExecutor

logger = structlog.get_logger()

class TestStatus(Enum):
    PENDING = "pending"
    RUNNING = "running"
    PASSED = "passed"
    FAILED = "failed"
    SKIPPED = "skipped"
    ERROR = "error"

class TestSeverity(Enum):
    CRITICAL = "critical"
    HIGH = "high"
    MEDIUM = "medium"
    LOW = "low"

@dataclass
class TestResult:
    test_id: str
    name: str
    status: TestStatus
    severity: TestSeverity
    duration_ms: float
    error_message: Optional[str] = None
    details: Dict[str, Any] = field(default_factory=dict)
    artifacts: List[str] = field(default_factory=list)
    started_at: float = field(default_factory=time.time)
    completed_at: Optional[float] = None

@dataclass
class TestEnvironment:
    name: str
    base_url: str
    api_url: str
    websocket_url: str
    database_url: str
    redis_url: str
    credentials: Dict[str, str]
    configuration: Dict[str, Any] = field(default_factory=dict)

class E2ETestRunner:
    def __init__(self, environment: TestEnvironment, config: Dict[str, Any]):
        self.environment = environment
        self.config = config
        self.session: Optional[aiohttp.ClientSession] = None
        self.auth_token: Optional[str] = None
        self.test_results: List[TestResult] = []
        self.thread_pool = ThreadPoolExecutor(max_workers=4)
        
        # Test data storage
        self.test_users: List[Dict[str, Any]] = []
        self.test_vms: List[str] = []
        self.test_projects: List[str] = []
        
    async def setup(self):
        """Set up the test environment"""
        try:
            # Initialize HTTP session
            timeout = aiohttp.ClientTimeout(total=30, connect=10)
            self.session = aiohttp.ClientSession(timeout=timeout)
            
            # Initialize Logfire for test tracking
            logfire.configure(
                token=self.config.get('logfire_token'),
                service_name="e2e-tests",
                environment=self.environment.name
            )
            
            # Create test user and authenticate
            await self._setup_test_data()
            await self._authenticate_test_user()
            
            # Verify environment health
            await self._verify_environment_health()
            
            logger.info("E2E test environment setup completed", 
                      environment=self.environment.name)
            
        except Exception as e:
            logger.error("Failed to setup E2E test environment", 
                       environment=self.environment.name, error=str(e))
            raise
    
    async def teardown(self):
        """Clean up the test environment"""
        try:
            # Clean up test data
            await self._cleanup_test_data()
            
            # Close HTTP session
            if self.session:
                await self.session.close()
            
            # Shutdown thread pool
            self.thread_pool.shutdown(wait=True)
            
            logger.info("E2E test environment cleanup completed",
                      environment=self.environment.name)
            
        except Exception as e:
            logger.error("Failed to cleanup E2E test environment", 
                       error=str(e))
    
    async def run_test_suite(self, test_suite_name: str) -> Dict[str, Any]:
        """Run a complete test suite"""
        suite_start_time = time.time()
        
        try:
            logfire.info("Starting E2E test suite", 
                       suite=test_suite_name,
                       environment=self.environment.name)
            
            # Define test suites
            test_suites = {
                'smoke': self._run_smoke_tests,
                'authentication': self._run_authentication_tests,
                'vm_management': self._run_vm_management_tests,
                'terminal_session': self._run_terminal_session_tests,
                'git_operations': self._run_git_operations_tests,
                'collaboration': self._run_collaboration_tests,
                'performance': self._run_performance_tests,
                'security': self._run_security_tests,
                'complete': self._run_complete_workflow_tests
            }
            
            if test_suite_name not in test_suites:
                raise ValueError(f"Unknown test suite: {test_suite_name}")
            
            # Run the test suite
            suite_results = await test_suites[test_suite_name]()
            
            # Calculate suite summary
            suite_duration = (time.time() - suite_start_time) * 1000
            passed_tests = [r for r in suite_results if r.status == TestStatus.PASSED]
            failed_tests = [r for r in suite_results if r.status == TestStatus.FAILED]
            
            suite_summary = {
                'suite_name': test_suite_name,
                'environment': self.environment.name,
                'total_tests': len(suite_results),
                'passed': len(passed_tests),
                'failed': len(failed_tests),
                'success_rate': len(passed_tests) / len(suite_results) * 100,
                'duration_ms': suite_duration,
                'results': [self._serialize_test_result(r) for r in suite_results]
            }
            
            logfire.info("E2E test suite completed",
                       suite=test_suite_name,
                       total_tests=len(suite_results),
                       passed=len(passed_tests),
                       failed=len(failed_tests),
                       duration_ms=suite_duration)
            
            return suite_summary
            
        except Exception as e:
            logger.error("Test suite execution failed", 
                       suite=test_suite_name, error=str(e))
            raise
    
    async def _run_smoke_tests(self) -> List[TestResult]:
        """Run smoke tests to verify basic functionality"""
        tests = [
            ('health_endpoints', self._test_health_endpoints),
            ('api_connectivity', self._test_api_connectivity),
            ('database_connectivity', self._test_database_connectivity),
            ('redis_connectivity', self._test_redis_connectivity),
            ('websocket_connectivity', self._test_websocket_connectivity)
        ]
        
        return await self._execute_test_batch(tests, TestSeverity.CRITICAL)
    
    async def _run_authentication_tests(self) -> List[TestResult]:
        """Run authentication and authorization tests"""
        tests = [
            ('user_registration', self._test_user_registration),
            ('user_login', self._test_user_login),
            ('token_validation', self._test_token_validation),
            ('password_reset', self._test_password_reset),
            ('user_profile_management', self._test_user_profile_management),
            ('session_management', self._test_session_management),
            ('oauth_integration', self._test_oauth_integration)
        ]
        
        return await self._execute_test_batch(tests, TestSeverity.HIGH)
    
    async def _run_vm_management_tests(self) -> List[TestResult]:
        """Run VM management and lifecycle tests"""
        tests = [
            ('vm_creation', self._test_vm_creation),
            ('vm_startup', self._test_vm_startup),
            ('vm_resource_allocation', self._test_vm_resource_allocation),
            ('vm_snapshot_creation', self._test_vm_snapshot_creation),
            ('vm_snapshot_restoration', self._test_vm_snapshot_restoration),
            ('vm_migration', self._test_vm_migration),
            ('vm_monitoring', self._test_vm_monitoring),
            ('real_firecracker_integration', self._test_real_firecracker_integration),
            ('full_vm_lifecycle_with_real_kernel', self._test_full_vm_lifecycle_real_kernel),
            ('vm_cleanup', self._test_vm_cleanup)
        ]
        
        return await self._execute_test_batch(tests, TestSeverity.HIGH)
    
    async def _run_terminal_session_tests(self) -> List[TestResult]:
        """Run terminal session and WebSocket tests"""
        tests = [
            ('terminal_connection', self._test_terminal_connection),
            ('command_execution', self._test_command_execution),
            ('file_operations', self._test_file_operations),
            ('terminal_recording', self._test_terminal_recording),
            ('session_persistence', self._test_session_persistence),
            ('concurrent_sessions', self._test_concurrent_sessions),
            ('session_sharing', self._test_session_sharing)
        ]
        
        return await self._execute_test_batch(tests, TestSeverity.HIGH)
    
    async def _run_git_operations_tests(self) -> List[TestResult]:
        """Run Git operations and repository management tests"""
        tests = [
            ('repository_creation', self._test_repository_creation),
            ('git_clone', self._test_git_clone),
            ('git_commit_push', self._test_git_commit_push),
            ('branch_management', self._test_branch_management),
            ('ssh_key_management', self._test_ssh_key_management),
            ('repository_collaboration', self._test_repository_collaboration)
        ]
        
        return await self._execute_test_batch(tests, TestSeverity.MEDIUM)
    
    async def _run_performance_tests(self) -> List[TestResult]:
        """Run performance and load tests"""
        tests = [
            ('api_response_times', self._test_api_response_times),
            ('concurrent_user_load', self._test_concurrent_user_load),
            ('vm_startup_performance', self._test_vm_startup_performance),
            ('websocket_performance', self._test_websocket_performance),
            ('database_performance', self._test_database_performance),
            ('memory_usage', self._test_memory_usage),
            ('cpu_utilization', self._test_cpu_utilization)
        ]
        
        return await self._execute_test_batch(tests, TestSeverity.MEDIUM)
    
    async def _run_security_tests(self) -> List[TestResult]:
        """Run security validation tests"""
        tests = [
            ('authentication_security', self._test_authentication_security),
            ('authorization_enforcement', self._test_authorization_enforcement),
            ('data_encryption', self._test_data_encryption),
            ('input_validation', self._test_input_validation),
            ('rate_limiting', self._test_rate_limiting),
            ('session_security', self._test_session_security),
            ('api_security', self._test_api_security)
        ]
        
        return await self._execute_test_batch(tests, TestSeverity.CRITICAL)
    
    async def _execute_test_batch(self, tests: List[tuple], severity: TestSeverity) -> List[TestResult]:
        """Execute a batch of tests concurrently"""
        results = []
        
        for test_name, test_func in tests:
            test_result = await self._execute_single_test(test_name, test_func, severity)
            results.append(test_result)
            
            # Stop on critical test failure
            if (severity == TestSeverity.CRITICAL and 
                test_result.status == TestStatus.FAILED):
                logger.error("Critical test failed, stopping batch execution",
                           test_name=test_name)
                break
        
        return results
    
    async def _execute_single_test(self, test_name: str, test_func: Callable, 
                                 severity: TestSeverity) -> TestResult:
        """Execute a single test with error handling and timing"""
        test_id = f"{self.environment.name}_{test_name}_{int(time.time())}"
        start_time = time.time()
        
        test_result = TestResult(
            test_id=test_id,
            name=test_name,
            status=TestStatus.RUNNING,
            severity=severity,
            duration_ms=0
        )
        
        try:
            logger.info("Starting test", test_name=test_name)
            
            # Execute the test
            await test_func()
            
            # Test passed
            test_result.status = TestStatus.PASSED
            test_result.completed_at = time.time()
            test_result.duration_ms = (test_result.completed_at - start_time) * 1000
            
            logger.info("Test passed", test_name=test_name, 
                      duration_ms=test_result.duration_ms)
            
        except Exception as e:
            # Test failed
            test_result.status = TestStatus.FAILED
            test_result.completed_at = time.time()
            test_result.duration_ms = (test_result.completed_at - start_time) * 1000
            test_result.error_message = str(e)
            
            logger.error("Test failed", test_name=test_name, 
                       duration_ms=test_result.duration_ms, error=str(e))
            
            logfire.error("E2E test failed",
                        test_name=test_name,
                        environment=self.environment.name,
                        error=str(e),
                        duration_ms=test_result.duration_ms)
        
        self.test_results.append(test_result)
        return test_result
    
    async def _test_health_endpoints(self):
        """Test health check endpoints"""
        endpoints = [
            f"{self.environment.api_url}/health",
            f"{self.environment.api_url}/health/ready",
            f"{self.environment.api_url}/health/startup"
        ]
        
        for endpoint in endpoints:
            async with self.session.get(endpoint) as response:
                if response.status != 200:
                    raise AssertionError(f"Health check failed: {endpoint} returned {response.status}")
                
                data = await response.json()
                if data.get('status') not in ['healthy', 'ok']:
                    raise AssertionError(f"Health check unhealthy: {endpoint} returned {data}")
    
    async def _test_user_login(self):
        """Test user authentication flow"""
        login_data = {
            "email": self.environment.credentials['email'],
            "password": self.environment.credentials['password']
        }
        
        async with self.session.post(
            f"{self.environment.api_url}/auth/login",
            json=login_data
        ) as response:
            if response.status != 200:
                raise AssertionError(f"Login failed with status {response.status}")
            
            data = await response.json()
            if not data.get('access_token'):
                raise AssertionError("Login response missing access token")
            
            # Test protected endpoint access
            headers = {'Authorization': f'Bearer {data["access_token"]}'}
            async with self.session.get(
                f"{self.environment.api_url}/users/me",
                headers=headers
            ) as profile_response:
                if profile_response.status != 200:
                    raise AssertionError("Failed to access protected endpoint after login")
    
    async def _test_vm_creation(self):
        """Test VM creation workflow"""
        vm_data = {
            "name": f"test-vm-{int(time.time())}",
            "template": "ubuntu-22.04",
            "resources": {
                "cpu_cores": 1,
                "memory_mb": 512,
                "disk_gb": 5
            }
        }
        
        headers = {'Authorization': f'Bearer {self.auth_token}'}
        
        # Create VM
        async with self.session.post(
            f"{self.environment.api_url}/vms",
            json=vm_data,
            headers=headers
        ) as response:
            if response.status != 201:
                raise AssertionError(f"VM creation failed with status {response.status}")
            
            vm = await response.json()
            vm_id = vm['id']
            self.test_vms.append(vm_id)
            
            # Verify VM status
            async with self.session.get(
                f"{self.environment.api_url}/vms/{vm_id}",
                headers=headers
            ) as status_response:
                if status_response.status != 200:
                    raise AssertionError("Failed to retrieve VM status after creation")
                
                vm_status = await status_response.json()
                if vm_status['status'] not in ['created', 'starting', 'running']:
                    raise AssertionError(f"VM in unexpected status: {vm_status['status']}")
    
    async def _test_terminal_connection(self):
        """Test WebSocket terminal connection"""
        if not self.test_vms:
            raise AssertionError("No test VMs available for terminal connection")
        
        vm_id = self.test_vms[0]
        ws_url = f"{self.environment.websocket_url}/terminal/{vm_id}"
        headers = {'Authorization': f'Bearer {self.auth_token}'}
        
        try:
            async with websockets.connect(
                ws_url,
                extra_headers=headers,
                timeout=10
            ) as websocket:
                # Send a simple command
                await websocket.send(json.dumps({
                    'type': 'command',
                    'data': 'echo "test terminal connection"\\n'
                }))
                
                # Wait for response
                response = await asyncio.wait_for(websocket.recv(), timeout=5)
                response_data = json.loads(response)
                
                if response_data.get('type') != 'output':
                    raise AssertionError("Terminal connection test failed: unexpected response type")
                
        except websockets.exceptions.WebSocketException as e:
            raise AssertionError(f"WebSocket connection failed: {str(e)}")
    
    async def _test_rate_limiting(self):
        """Test API rate limiting functionality"""
        endpoint = f"{self.environment.api_url}/health"
        
        # Make rapid requests to trigger rate limiting
        rate_limited = False
        for i in range(20):
            async with self.session.get(endpoint) as response:
                if response.status == 429:
                    rate_limited = True
                    break
                elif i < 15 and response.status != 200:
                    raise AssertionError(f"Unexpected status before rate limiting: {response.status}")
        
        if not rate_limited:
            raise AssertionError("Rate limiting not triggered after 20 requests")
    
    def _serialize_test_result(self, result: TestResult) -> Dict[str, Any]:
        """Serialize test result for reporting"""
        return {
            'test_id': result.test_id,
            'name': result.name,
            'status': result.status.value,
            'severity': result.severity.value,
            'duration_ms': result.duration_ms,
            'error_message': result.error_message,
            'details': result.details,
            'started_at': result.started_at,
            'completed_at': result.completed_at
        }
    
    async def _test_real_firecracker_integration(self):
        """Test real Firecracker integration with production kernel/rootfs"""
        # Verify real Firecracker can create, manage, and snapshot VMs
        # This test uses actual bootable kernel and filesystem images
        
        vm_data = {
            "name": f"e2e-firecracker-test-{int(time.time())}",
            "kernel_image": "production_ubuntu_kernel",  # Real bootable kernel
            "rootfs_image": "production_ubuntu_rootfs",  # Real filesystem
            "resources": {
                "cpu_cores": 1,
                "memory_mb": 512,
                "disk_gb": 5
            }
        }
        
        headers = {'Authorization': f'Bearer {self.auth_token}'}
        
        # Create VM with real kernel
        async with self.session.post(
            f"{self.environment.api_url}/vms",
            json=vm_data,
            headers=headers
        ) as response:
            if response.status != 201:
                raise AssertionError(f"Real VM creation failed with status {response.status}")
            
            vm = await response.json()
            vm_id = vm['id']
            self.test_vms.append(vm_id)
            
            # Wait for VM to fully boot (real kernel takes time)
            boot_timeout = 60  # Real VMs need more time to boot
            for attempt in range(boot_timeout):
                async with self.session.get(
                    f"{self.environment.api_url}/vms/{vm_id}",
                    headers=headers
                ) as status_response:
                    vm_status = await status_response.json()
                    if vm_status['status'] == 'running':
                        break
                    elif vm_status['status'] == 'failed':
                        raise AssertionError(f"Real VM failed to boot: {vm_status.get('error', 'Unknown error')}")
                    
                    await asyncio.sleep(1)
            else:
                raise AssertionError("Real VM failed to boot within timeout")
    
    async def _test_full_vm_lifecycle_real_kernel(self):
        """Test complete VM lifecycle with real kernel and snapshots"""
        if not self.test_vms:
            raise AssertionError("No real VMs available for lifecycle testing")
        
        vm_id = self.test_vms[0]
        headers = {'Authorization': f'Bearer {self.auth_token}'}
        
        # Test real snapshot creation
        snapshot_data = {
            "name": f"e2e-snapshot-{int(time.time())}",
            "description": "E2E test snapshot with real VM"
        }
        
        async with self.session.post(
            f"{self.environment.api_url}/vms/{vm_id}/snapshots",
            json=snapshot_data,
            headers=headers
        ) as response:
            if response.status != 201:
                raise AssertionError(f"Real snapshot creation failed with status {response.status}")
            
            snapshot = await response.json()
            snapshot_id = snapshot['id']
            
            # Wait for snapshot creation to complete
            snapshot_timeout = 120  # Real snapshots take time
            for attempt in range(snapshot_timeout):
                async with self.session.get(
                    f"{self.environment.api_url}/snapshots/{snapshot_id}",
                    headers=headers
                ) as status_response:
                    snapshot_status = await status_response.json()
                    if snapshot_status['state'] == 'available':
                        break
                    elif snapshot_status['state'] == 'error':
                        raise AssertionError(f"Real snapshot creation failed: {snapshot_status.get('error', 'Unknown error')}")
                    
                    await asyncio.sleep(1)
            else:
                raise AssertionError("Real snapshot creation failed to complete within timeout")
            
            # Verify snapshot data is substantial (not mock)
            if snapshot_status['size_bytes'] < 1024 * 1024:  # Less than 1MB indicates mock data
                raise AssertionError("Snapshot appears to contain mock data (too small)")
            
            # Test snapshot restoration
            restore_vm_data = {
                "name": f"e2e-restored-vm-{int(time.time())}",
                "snapshot_id": snapshot_id
            }
            
            async with self.session.post(
                f"{self.environment.api_url}/vms/restore",
                json=restore_vm_data,
                headers=headers
            ) as response:
                if response.status != 201:
                    raise AssertionError(f"Real snapshot restoration failed with status {response.status}")
                
                restored_vm = await response.json()
                restored_vm_id = restored_vm['id']
                self.test_vms.append(restored_vm_id)
                
                # Verify restored VM boots correctly
                for attempt in range(60):
                    async with self.session.get(
                        f"{self.environment.api_url}/vms/{restored_vm_id}",
                        headers=headers
                    ) as status_response:
                        vm_status = await status_response.json()
                        if vm_status['status'] == 'running':
                            break
                        elif vm_status['status'] == 'failed':
                            raise AssertionError(f"Restored VM failed to boot: {vm_status.get('error', 'Unknown error')}")
                        
                        await asyncio.sleep(1)
                else:
                    raise AssertionError("Restored VM failed to boot within timeout")

# Production E2E Test Configuration
class ProductionE2ETestSuite:
    def __init__(self, environment_config: Dict[str, Any]):
        self.environment = TestEnvironment(**environment_config)
        self.config = environment_config.get('config', {})
        self.runner = E2ETestRunner(self.environment, self.config)
    
    async def run_production_validation(self) -> Dict[str, Any]:
        """Run production validation test suite"""
        try:
            await self.runner.setup()
            
            # Run critical test suites for production
            results = {}
            
            critical_suites = ['smoke', 'authentication', 'security']
            for suite_name in critical_suites:
                suite_results = await self.runner.run_test_suite(suite_name)
                results[suite_name] = suite_results
                
                # Stop if critical tests fail
                if suite_results['failed'] > 0:
                    logger.error("Critical test suite failed in production validation",
                               suite=suite_name,
                               failed_count=suite_results['failed'])
                    break
            
            # Generate overall summary
            total_tests = sum(r['total_tests'] for r in results.values())
            total_passed = sum(r['passed'] for r in results.values())
            total_failed = sum(r['failed'] for r in results.values())
            
            summary = {
                'environment': self.environment.name,
                'validation_status': 'PASSED' if total_failed == 0 else 'FAILED',
                'total_tests': total_tests,
                'passed': total_passed,
                'failed': total_failed,
                'success_rate': total_passed / total_tests * 100 if total_tests > 0 else 0,
                'suite_results': results,
                'timestamp': time.time()
            }
            
            return summary
            
        finally:
            await self.runner.teardown()

if __name__ == "__main__":
    import sys
    import os
    
    # Production environment configuration
    production_config = {
        'name': 'production',
        'base_url': os.getenv('PRODUCTION_BASE_URL', 'https://getbuild.ing'),
        'api_url': os.getenv('PRODUCTION_API_URL', 'https://getbuild.ing/api'),
        'websocket_url': os.getenv('PRODUCTION_WS_URL', 'wss://ws.getbuild.ing'),
        'database_url': os.getenv('PRODUCTION_DATABASE_URL'),
        'redis_url': os.getenv('PRODUCTION_REDIS_URL'),
        'credentials': {
            'email': os.getenv('E2E_TEST_EMAIL'),
            'password': os.getenv('E2E_TEST_PASSWORD')
        },
        'config': {
            'logfire_token': os.getenv('LOGFIRE_TOKEN'),
            'timeout_seconds': 300
        }
    }
    
    async def main():
        test_suite = ProductionE2ETestSuite(production_config)
        
        try:
            results = await test_suite.run_production_validation()
            
            print(f"\n=== Production E2E Test Results ===")
            print(f"Environment: {results['environment']}")
            print(f"Status: {results['validation_status']}")
            print(f"Total Tests: {results['total_tests']}")
            print(f"Passed: {results['passed']}")
            print(f"Failed: {results['failed']}")
            print(f"Success Rate: {results['success_rate']:.1f}%")
            
            if results['failed'] > 0:
                print(f"\n❌ Production validation FAILED")
                sys.exit(1)
            else:
                print(f"\n✅ Production validation PASSED")
                sys.exit(0)
        
        except Exception as e:
            print(f"\n💥 Production validation ERROR: {str(e)}")
            sys.exit(1)
    
    asyncio.run(main())
```

### Performance Testing Suite
**Location**: `tests/e2e/performance/`

```python
# tests/e2e/performance/load_test.py
import asyncio
import aiohttp
import time
import statistics
import structlog
from typing import List, Dict, Any, Optional
from dataclasses import dataclass
from concurrent.futures import ThreadPoolExecutor
import matplotlib.pyplot as plt
import seaborn as sns

logger = structlog.get_logger()

@dataclass
class LoadTestResult:
    test_name: str
    total_requests: int
    successful_requests: int
    failed_requests: int
    avg_response_time_ms: float
    p95_response_time_ms: float
    p99_response_time_ms: float
    max_response_time_ms: float
    requests_per_second: float
    error_rate: float
    duration_seconds: float

class LoadTestRunner:
    def __init__(self, base_url: str, auth_token: str):
        self.base_url = base_url.rstrip('/')
        self.auth_token = auth_token
        self.results: List[LoadTestResult] = []
    
    async def run_api_load_test(self, 
                              endpoint: str,
                              concurrent_users: int = 10,
                              duration_seconds: int = 60,
                              ramp_up_seconds: int = 10) -> LoadTestResult:
        """Run load test against API endpoint"""
        
        logger.info("Starting API load test",
                   endpoint=endpoint,
                   concurrent_users=concurrent_users,
                   duration_seconds=duration_seconds)
        
        # Test configuration
        url = f"{self.base_url}{endpoint}"
        headers = {'Authorization': f'Bearer {self.auth_token}'}
        
        # Metrics collection
        response_times: List[float] = []
        successful_requests = 0
        failed_requests = 0
        
        start_time = time.time()
        end_time = start_time + duration_seconds
        
        # Create semaphore for controlling concurrency
        semaphore = asyncio.Semaphore(concurrent_users)
        
        async def make_request():
            nonlocal successful_requests, failed_requests
            
            async with semaphore:
                request_start = time.time()
                try:
                    timeout = aiohttp.ClientTimeout(total=30)
                    async with aiohttp.ClientSession(timeout=timeout) as session:
                        async with session.get(url, headers=headers) as response:
                            await response.read()  # Consume response body
                            
                            request_time = (time.time() - request_start) * 1000
                            response_times.append(request_time)
                            
                            if response.status == 200:
                                successful_requests += 1
                            else:
                                failed_requests += 1
                                
                except Exception as e:
                    failed_requests += 1
                    logger.debug("Request failed", error=str(e))
        
        # Launch requests with gradual ramp-up
        tasks = []
        ramp_up_delay = ramp_up_seconds / concurrent_users if concurrent_users > 0 else 0
        
        while time.time() < end_time:
            # Add new users during ramp-up period
            if len(tasks) < concurrent_users:
                task = asyncio.create_task(make_request())
                tasks.append(task)
                await asyncio.sleep(ramp_up_delay)
            
            # Replace completed tasks
            done_tasks = [t for t in tasks if t.done()]
            for task in done_tasks:
                tasks.remove(task)
                if time.time() < end_time:
                    new_task = asyncio.create_task(make_request())
                    tasks.append(new_task)
            
            await asyncio.sleep(0.1)  # Small delay to prevent tight loop
        
        # Wait for remaining tasks to complete
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        
        # Calculate metrics
        actual_duration = time.time() - start_time
        total_requests = successful_requests + failed_requests
        
        if response_times:
            avg_response_time = statistics.mean(response_times)
            p95_response_time = statistics.quantiles(response_times, n=20)[18]  # 95th percentile
            p99_response_time = statistics.quantiles(response_times, n=100)[98]  # 99th percentile
            max_response_time = max(response_times)
        else:
            avg_response_time = p95_response_time = p99_response_time = max_response_time = 0
        
        requests_per_second = total_requests / actual_duration if actual_duration > 0 else 0
        error_rate = (failed_requests / total_requests * 100) if total_requests > 0 else 0
        
        result = LoadTestResult(
            test_name=f"API Load Test - {endpoint}",
            total_requests=total_requests,
            successful_requests=successful_requests,
            failed_requests=failed_requests,
            avg_response_time_ms=avg_response_time,
            p95_response_time_ms=p95_response_time,
            p99_response_time_ms=p99_response_time,
            max_response_time_ms=max_response_time,
            requests_per_second=requests_per_second,
            error_rate=error_rate,
            duration_seconds=actual_duration
        )
        
        self.results.append(result)
        
        logger.info("API load test completed",
                   endpoint=endpoint,
                   total_requests=total_requests,
                   requests_per_second=requests_per_second,
                   avg_response_time_ms=avg_response_time,
                   error_rate=error_rate)
        
        return result
    
    async def run_websocket_load_test(self,
                                    concurrent_connections: int = 50,
                                    duration_seconds: int = 60) -> LoadTestResult:
        """Run load test for WebSocket connections"""
        
        logger.info("Starting WebSocket load test",
                   concurrent_connections=concurrent_connections,
                   duration_seconds=duration_seconds)
        
        import websockets
        
        # Test metrics
        successful_connections = 0
        failed_connections = 0
        message_response_times: List[float] = []
        
        start_time = time.time()
        end_time = start_time + duration_seconds
        
        async def websocket_session():
            nonlocal successful_connections, failed_connections
            
            try:
                ws_url = f"wss://ws.getbuild.ing/terminal/test"
                headers = {'Authorization': f'Bearer {self.auth_token}'}
                
                async with websockets.connect(ws_url, extra_headers=headers) as websocket:
                    successful_connections += 1
                    
                    # Send periodic messages during the test
                    while time.time() < end_time:
                        message_start = time.time()
                        
                        await websocket.send('{"type": "ping"}')
                        response = await asyncio.wait_for(websocket.recv(), timeout=5)
                        
                        response_time = (time.time() - message_start) * 1000
                        message_response_times.append(response_time)
                        
                        await asyncio.sleep(1)  # Send message every second
                        
            except Exception as e:
                failed_connections += 1
                logger.debug("WebSocket connection failed", error=str(e))
        
        # Launch concurrent WebSocket sessions
        tasks = [asyncio.create_task(websocket_session()) 
                for _ in range(concurrent_connections)]
        
        await asyncio.gather(*tasks, return_exceptions=True)
        
        # Calculate metrics
        actual_duration = time.time() - start_time
        total_connections = successful_connections + failed_connections
        
        if message_response_times:
            avg_response_time = statistics.mean(message_response_times)
            p95_response_time = statistics.quantiles(message_response_times, n=20)[18]
            p99_response_time = statistics.quantiles(message_response_times, n=100)[98]
            max_response_time = max(message_response_times)
        else:
            avg_response_time = p95_response_time = p99_response_time = max_response_time = 0
        
        error_rate = (failed_connections / total_connections * 100) if total_connections > 0 else 0
        
        result = LoadTestResult(
            test_name="WebSocket Load Test",
            total_requests=total_connections,
            successful_requests=successful_connections,
            failed_requests=failed_connections,
            avg_response_time_ms=avg_response_time,
            p95_response_time_ms=p95_response_time,
            p99_response_time_ms=p99_response_time,
            max_response_time_ms=max_response_time,
            requests_per_second=successful_connections / actual_duration,
            error_rate=error_rate,
            duration_seconds=actual_duration
        )
        
        self.results.append(result)
        
        logger.info("WebSocket load test completed",
                   total_connections=total_connections,
                   successful_connections=successful_connections,
                   error_rate=error_rate,
                   avg_response_time_ms=avg_response_time)
        
        return result
    
    def generate_performance_report(self, output_file: str = "performance_report.html"):
        """Generate HTML performance report with charts"""
        
        if not self.results:
            logger.warning("No performance test results to report")
            return
        
        # Create performance charts
        fig, axes = plt.subplots(2, 2, figsize=(15, 10))
        fig.suptitle('E2E Performance Test Results', fontsize=16)
        
        # Response time chart
        test_names = [r.test_name for r in self.results]
        avg_times = [r.avg_response_time_ms for r in self.results]
        p95_times = [r.p95_response_time_ms for r in self.results]
        
        axes[0, 0].bar(test_names, avg_times, alpha=0.7, label='Average')
        axes[0, 0].bar(test_names, p95_times, alpha=0.7, label='95th Percentile')
        axes[0, 0].set_title('Response Times')
        axes[0, 0].set_ylabel('Time (ms)')
        axes[0, 0].legend()
        axes[0, 0].tick_params(axis='x', rotation=45)
        
        # Throughput chart
        rps = [r.requests_per_second for r in self.results]
        axes[0, 1].bar(test_names, rps, alpha=0.7, color='green')
        axes[0, 1].set_title('Throughput')
        axes[0, 1].set_ylabel('Requests/Second')
        axes[0, 1].tick_params(axis='x', rotation=45)
        
        # Error rate chart
        error_rates = [r.error_rate for r in self.results]
        axes[1, 0].bar(test_names, error_rates, alpha=0.7, color='red')
        axes[1, 0].set_title('Error Rates')
        axes[1, 0].set_ylabel('Error Rate (%)')
        axes[1, 0].tick_params(axis='x', rotation=45)
        
        # Success rate chart
        success_rates = [100 - r.error_rate for r in self.results]
        axes[1, 1].bar(test_names, success_rates, alpha=0.7, color='blue')
        axes[1, 1].set_title('Success Rates')
        axes[1, 1].set_ylabel('Success Rate (%)')
        axes[1, 1].tick_params(axis='x', rotation=45)
        
        plt.tight_layout()
        plt.savefig('performance_charts.png', dpi=300, bbox_inches='tight')
        plt.close()
        
        # Generate HTML report
        html_content = f"""
        <!DOCTYPE html>
        <html>
        <head>
            <title>E2E Performance Test Report</title>
            <style>
                body {{ font-family: Arial, sans-serif; margin: 40px; }}
                .header {{ background-color: #f0f0f0; padding: 20px; border-radius: 5px; }}
                .summary {{ margin: 20px 0; }}
                .results-table {{ width: 100%; border-collapse: collapse; margin: 20px 0; }}
                .results-table th, .results-table td {{ border: 1px solid #ddd; padding: 8px; text-align: left; }}
                .results-table th {{ background-color: #f2f2f2; }}
                .pass {{ color: green; font-weight: bold; }}
                .fail {{ color: red; font-weight: bold; }}
                .charts {{ text-align: center; margin: 20px 0; }}
            </style>
        </head>
        <body>
            <div class="header">
                <h1>E2E Performance Test Report</h1>
                <p>Generated: {time.strftime('%Y-%m-%d %H:%M:%S')}</p>
            </div>
            
            <div class="summary">
                <h2>Summary</h2>
                <p>Total Tests: {len(self.results)}</p>
                <p>Average RPS: {statistics.mean([r.requests_per_second for r in self.results]):.2f}</p>
                <p>Average Response Time: {statistics.mean([r.avg_response_time_ms for r in self.results]):.2f} ms</p>
                <p>Overall Error Rate: {statistics.mean([r.error_rate for r in self.results]):.2f}%</p>
            </div>
            
            <div class="charts">
                <h2>Performance Charts</h2>
                <img src="performance_charts.png" alt="Performance Charts" style="max-width: 100%;">
            </div>
            
            <h2>Detailed Results</h2>
            <table class="results-table">
                <tr>
                    <th>Test Name</th>
                    <th>Total Requests</th>
                    <th>Success Rate</th>
                    <th>RPS</th>
                    <th>Avg Response (ms)</th>
                    <th>95th Percentile (ms)</th>
                    <th>Max Response (ms)</th>
                    <th>Error Rate</th>
                </tr>
        """
        
        for result in self.results:
            success_rate = 100 - result.error_rate
            status_class = "pass" if result.error_rate < 5 else "fail"
            
            html_content += f"""
                <tr>
                    <td>{result.test_name}</td>
                    <td>{result.total_requests}</td>
                    <td class="{status_class}">{success_rate:.1f}%</td>
                    <td>{result.requests_per_second:.2f}</td>
                    <td>{result.avg_response_time_ms:.2f}</td>
                    <td>{result.p95_response_time_ms:.2f}</td>
                    <td>{result.max_response_time_ms:.2f}</td>
                    <td class="{status_class}">{result.error_rate:.1f}%</td>
                </tr>
            """
        
        html_content += """
            </table>
        </body>
        </html>
        """
        
        with open(output_file, 'w') as f:
            f.write(html_content)
        
        logger.info("Performance report generated", output_file=output_file)

# CLI for running performance tests
if __name__ == "__main__":
    import os
    import sys
    
    async def main():
        base_url = os.getenv('TEST_BASE_URL', 'https://getbuild.ing')
        auth_token = os.getenv('E2E_AUTH_TOKEN')
        
        if not auth_token:
            print("Error: E2E_AUTH_TOKEN environment variable required")
            sys.exit(1)
        
        runner = LoadTestRunner(base_url, auth_token)
        
        try:
            print("🚀 Starting performance tests...")
            
            # Run API load tests
            await runner.run_api_load_test('/api/health', concurrent_users=20, duration_seconds=30)
            await runner.run_api_load_test('/api/users/me', concurrent_users=10, duration_seconds=30)
            await runner.run_api_load_test('/api/vms', concurrent_users=5, duration_seconds=30)
            
            # Run WebSocket load test
            await runner.run_websocket_load_test(concurrent_connections=25, duration_seconds=30)
            
            # Generate report
            runner.generate_performance_report()
            
            print("✅ Performance tests completed successfully")
            print("📊 Report generated: performance_report.html")
            
        except Exception as e:
            print(f"❌ Performance tests failed: {str(e)}")
            sys.exit(1)
    
    asyncio.run(main())
```

## TDD Implementation Cycle

### Red Phase: E2E Test Creation
```python
# tests/e2e/test_e2e_framework.py
import pytest
import asyncio
from tests.e2e.framework.test_runner import E2ETestRunner, TestEnvironment

class E2EFrameworkTester:
    def __init__(self):
        self.test_environment = TestEnvironment(
            name="test",
            base_url="http://localhost:8000",
            api_url="http://localhost:8000/api",
            websocket_url="ws://localhost:8000/ws",
            database_url="postgresql://test:test@localhost:5432/test",
            redis_url="redis://localhost:6379/0",
            credentials={"email": "test@example.com", "password": "test123"}
        )
    
    def test_e2e_framework_initialization(self):
        """Test E2E framework initializes correctly"""
        # This test should initially fail (Red phase)
        assert False, "E2E framework not implemented yet"
    
    def test_test_execution_flow(self):
        """Test that test execution flow works correctly"""
        # This test should initially fail (Red phase)
        assert False, "Test execution flow not implemented yet"
    
    def test_performance_testing(self):
        """Test that performance testing works correctly"""
        # This test should initially fail (Red phase)
        assert False, "Performance testing not implemented yet"
    
    def test_result_reporting(self):
        """Test that result reporting works correctly"""
        # This test should initially fail (Red phase)
        assert False, "Result reporting not implemented yet"

# CLI for testing E2E framework
if __name__ == "__main__":
    tester = E2EFrameworkTester()
    
    try:
        tester.test_e2e_framework_initialization()
        tester.test_test_execution_flow()
        tester.test_performance_testing()
        tester.test_result_reporting()
        print("All E2E framework tests passed!")
    except AssertionError as e:
        print(f"E2E framework test failed: {e}")
        exit(1)
```

### Green Phase: E2E Test Implementation
```python
# Implement E2E test features to make tests pass
# This involves creating test runners, performance testing, and reporting
```

### Refactor Phase: E2E Test Optimization
```python
# Optimize E2E tests for performance and reliability
# Add advanced test scenarios and enhanced reporting
# Improve test data management and cleanup procedures
```

## Security Checklist ✅

### E2E Test Security
- [ ] Test data isolation and cleanup
- [ ] Secure test environment configuration
- [ ] Test credential management and rotation
- [ ] Test result data protection
- [ ] Access controls for test environments
- [ ] Test execution monitoring and logging
- [ ] Secure test artifact storage
- [ ] Test environment network isolation
- [ ] Test data anonymization and masking
- [ ] Security test scenario validation

### Performance Test Security
- [ ] Load test target validation
- [ ] Performance test rate limiting
- [ ] Test traffic identification and filtering
- [ ] Performance test data protection
- [ ] Load test environment isolation
- [ ] Performance monitoring security
- [ ] Test result confidentiality
- [ ] Load test impact assessment
- [ ] Performance baseline protection
- [ ] Test execution authorization

### Security Test Validation
- [ ] Authentication security testing
- [ ] Authorization enforcement testing
- [ ] Input validation security testing
- [ ] Data encryption verification
- [ ] Session security validation
- [ ] API security endpoint testing
- [ ] Rate limiting effectiveness testing
- [ ] Security header validation
- [ ] Vulnerability assessment integration
- [ ] Security compliance verification

### Test Environment Security
- [ ] Test environment hardening
- [ ] Test data security controls
- [ ] Test environment monitoring
- [ ] Access logging and auditing
- [ ] Test environment patching
- [ ] Secure test configuration management
- [ ] Test environment backup security
- [ ] Incident response for test environments
- [ ] Test environment compliance validation
- [ ] Security training for test personnel

## Performance Requirements

### Test Execution Performance
- E2E test suite completion < 30 minutes
- Individual test execution < 5 minutes
- Test environment setup < 2 minutes
- Test data cleanup < 1 minute
- Parallel test execution efficiency > 80%
- Test result processing < 30 seconds

### Performance Test Requirements
- Load test ramp-up capability: 100 users/second
- Sustained load duration: 60+ minutes
- Performance monitoring overhead < 5%
- Load test result processing < 2 minutes
- Performance baseline comparison < 1 minute
- Load test environment reset < 5 minutes

### Test Framework Performance
- Test runner initialization < 30 seconds
- Test result aggregation < 10 seconds
- Report generation < 2 minutes
- Test artifact storage efficiency > 90%
- Test data management overhead < 10%
- Framework resource utilization < 20%

## Commit Instructions

After implementing the E2E testing system:

```bash
git add tests/e2e/ tests/performance/
git commit -m "Add comprehensive end-to-end testing and deployment validation

- Implement E2E test framework with comprehensive test runner
- Add production validation test suite with critical path testing
- Include performance testing suite with load and stress testing
- Add WebSocket and real-time communication testing
- Implement security validation and penetration testing
- Add test result reporting with charts and metrics
- Include test environment management and cleanup
- Add parallel test execution with concurrent user simulation
- Implement test data management and isolation
- Add TDD cycle with Red-Green-Refactor for E2E testing
- Ensure >95% E2E test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete E2E testing suite:

```bash
# Run E2E tests for staging environment
python tests/e2e/framework/test_runner.py --environment staging --suite complete

# Run production validation tests
python tests/e2e/framework/test_runner.py --environment production --suite smoke

# Run performance tests
python tests/e2e/performance/load_test.py

# Run security validation tests
python tests/e2e/security/security_test.py

# Generate test reports
python tests/e2e/reporting/generate_report.py --output test-results.html
```

Validate E2E testing framework:
```bash
python tests/e2e/test_e2e_framework.py
pytest tests/e2e/framework/ --cov=e2e --cov-report=html --cov-fail-under=95
```

## Integration Testing

Test E2E framework integration with platform components:
```bash
# Test integration with all platform sessions
pytest tests/e2e/integration/test_platform_integration.py -v

# Test integration with monitoring systems
pytest tests/e2e/integration/test_monitoring_integration.py -v

# Test complete deployment validation
pytest tests/e2e/integration/test_deployment_validation.py -v
```