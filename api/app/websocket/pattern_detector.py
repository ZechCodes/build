"""Advanced malicious message pattern detection for WebSocket communications."""

import re
import time
import hashlib
import base64
import json
from typing import Dict, Any, List, Set, Optional, Tuple, Pattern
from dataclasses import dataclass
from enum import Enum
import structlog

logger = structlog.get_logger(__name__)


class ThreatLevel(Enum):
    """Threat severity levels."""
    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"
    CRITICAL = "critical"


class PatternCategory(Enum):
    """Categories of malicious patterns."""
    COMMAND_INJECTION = "command_injection"
    XSS_ATTACK = "xss_attack"
    PATH_TRAVERSAL = "path_traversal"
    SQL_INJECTION = "sql_injection"
    BUFFER_OVERFLOW = "buffer_overflow"
    PROTOCOL_ABUSE = "protocol_abuse"
    DATA_EXFILTRATION = "data_exfiltration"
    RECONNAISSANCE = "reconnaissance"
    PRIVILEGE_ESCALATION = "privilege_escalation"
    MALWARE_SIGNATURE = "malware_signature"


@dataclass
class ThreatPattern:
    """A detected threat pattern."""
    name: str
    category: PatternCategory
    level: ThreatLevel
    pattern: str
    description: str
    regex: Pattern[str]
    false_positive_score: float = 0.0  # 0.0 = never false positive, 1.0 = often false positive


@dataclass
class DetectionResult:
    """Result of pattern detection analysis."""
    detected: bool
    threat_level: ThreatLevel
    patterns_found: List[str]
    pattern_details: List[Dict[str, Any]]
    confidence_score: float
    risk_score: int  # 0-100
    recommendations: List[str]


class MaliciousPatternDetector:
    """Advanced pattern detector for malicious content in WebSocket messages."""
    
    def __init__(self):
        # Pattern database
        self.threat_patterns: Dict[str, ThreatPattern] = {}
        self._initialize_patterns()
        
        # Detection settings
        self.enable_adaptive_detection = True
        self.false_positive_threshold = 0.7
        self.confidence_threshold = 0.5
        
        # Behavioral analysis
        self.connection_patterns: Dict[str, Dict[str, Any]] = {}
        self.global_patterns: Dict[str, int] = {}
        
        # Performance tracking
        self.detection_stats = {
            "messages_analyzed": 0,
            "threats_detected": 0,
            "false_positives": 0,
            "processing_time_total": 0.0
        }
    
    def _initialize_patterns(self):
        """Initialize the threat pattern database."""
        patterns = [
            # Command Injection Patterns
            ThreatPattern(
                "shell_injection_basic",
                PatternCategory.COMMAND_INJECTION,
                ThreatLevel.HIGH,
                r"[;&|`$(){}[\]\\]",
                "Basic shell metacharacters that could indicate command injection",
                re.compile(r"[;&|`$(){}[\]\\]"),
                0.3
            ),
            ThreatPattern(
                "shell_injection_advanced",
                PatternCategory.COMMAND_INJECTION,
                ThreatLevel.CRITICAL,
                r"(\$\(.*\)|\`.*\`|;\s*(rm|wget|curl|nc|netcat|bash|sh|python|perl|php))",
                "Advanced command injection patterns",
                re.compile(r"(\$\(.*\)|\`.*\`|;\s*(rm|wget|curl|nc|netcat|bash|sh|python|perl|php))", re.IGNORECASE),
                0.1
            ),
            ThreatPattern(
                "reverse_shell",
                PatternCategory.COMMAND_INJECTION,
                ThreatLevel.CRITICAL,
                r"(nc\s+-.*-e|/bin/(ba)?sh.*>&|python.*socket.*exec|perl.*socket.*exec)",
                "Reverse shell command patterns",
                re.compile(r"(nc\s+-.*-e|/bin/(ba)?sh.*>&|python.*socket.*exec|perl.*socket.*exec)", re.IGNORECASE),
                0.05
            ),
            
            # XSS Attack Patterns
            ThreatPattern(
                "xss_script_tags",
                PatternCategory.XSS_ATTACK,
                ThreatLevel.HIGH,
                r"<script[^>]*>.*?</script>",
                "Script tag injection for XSS attacks",
                re.compile(r"<script[^>]*>.*?</script>", re.IGNORECASE | re.DOTALL),
                0.2
            ),
            ThreatPattern(
                "xss_event_handlers",
                PatternCategory.XSS_ATTACK,
                ThreatLevel.HIGH,
                r"on(load|error|click|mouseover|focus|blur|keypress|keydown|keyup)\s*=",
                "HTML event handler injection",
                re.compile(r"on(load|error|click|mouseover|focus|blur|keypress|keydown|keyup)\s*=", re.IGNORECASE),
                0.3
            ),
            ThreatPattern(
                "xss_javascript_protocol",
                PatternCategory.XSS_ATTACK,
                ThreatLevel.MEDIUM,
                r"javascript:\s*",
                "JavaScript protocol in URLs",
                re.compile(r"javascript:\s*", re.IGNORECASE),
                0.4
            ),
            
            # Path Traversal Patterns
            ThreatPattern(
                "path_traversal_basic",
                PatternCategory.PATH_TRAVERSAL,
                ThreatLevel.MEDIUM,
                r"\.\./",
                "Basic directory traversal patterns",
                re.compile(r"\.\./"),
                0.5
            ),
            ThreatPattern(
                "path_traversal_encoded",
                PatternCategory.PATH_TRAVERSAL,
                ThreatLevel.HIGH,
                r"(%2e%2e%2f|%2e%2e/|..%2f|%252e%252e%252f)",
                "URL-encoded directory traversal",
                re.compile(r"(%2e%2e%2f|%2e%2e/|..%2f|%252e%252e%252f)", re.IGNORECASE),
                0.2
            ),
            ThreatPattern(
                "sensitive_file_access",
                PatternCategory.PATH_TRAVERSAL,
                ThreatLevel.HIGH,
                r"(etc/passwd|etc/shadow|boot\.ini|windows/system32|\.ssh/|\.aws/|\.env)",
                "Attempts to access sensitive system files",
                re.compile(r"(etc/passwd|etc/shadow|boot\.ini|windows/system32|\.ssh/|\.aws/|\.env)", re.IGNORECASE),
                0.1
            ),
            
            # SQL Injection Patterns
            ThreatPattern(
                "sql_injection_basic",
                PatternCategory.SQL_INJECTION,
                ThreatLevel.MEDIUM,
                r"('|\"|;|--|\|\|)",
                "Basic SQL injection characters",
                re.compile(r"('|\"|;|--|\|\|)"),
                0.6
            ),
            ThreatPattern(
                "sql_injection_keywords",
                PatternCategory.SQL_INJECTION,
                ThreatLevel.HIGH,
                r"\b(union|select|insert|update|delete|drop|exec|execute|sp_|xp_)\b",
                "SQL injection keywords",
                re.compile(r"\b(union|select|insert|update|delete|drop|exec|execute|sp_|xp_)\b", re.IGNORECASE),
                0.3
            ),
            
            # Buffer Overflow Patterns
            ThreatPattern(
                "buffer_overflow_nop_sled",
                PatternCategory.BUFFER_OVERFLOW,
                ThreatLevel.CRITICAL,
                r"(\x90{10,}|AAAA{50,}|%u9090{5,})",
                "NOP sled or buffer overflow patterns",
                re.compile(r"(\x90{10,}|AAAA{50,}|%u9090{5,})"),
                0.05
            ),
            ThreatPattern(
                "format_string_attack",
                PatternCategory.BUFFER_OVERFLOW,
                ThreatLevel.HIGH,
                r"(%n|%s|%x|%d){3,}",
                "Format string attack patterns",
                re.compile(r"(%n|%s|%x|%d){3,}"),
                0.2
            ),
            
            # Protocol Abuse Patterns
            ThreatPattern(
                "protocol_abuse_large_message",
                PatternCategory.PROTOCOL_ABUSE,
                ThreatLevel.MEDIUM,
                r".{10000,}",
                "Abnormally large message that could indicate DoS attempt",
                re.compile(r".{10000,}", re.DOTALL),
                0.4
            ),
            ThreatPattern(
                "protocol_abuse_binary_data",
                PatternCategory.PROTOCOL_ABUSE,
                ThreatLevel.MEDIUM,
                r"[\x00-\x08\x0B\x0C\x0E-\x1F\x7F-\xFF]{20,}",
                "Large amounts of binary data in text field",
                re.compile(r"[\x00-\x08\x0B\x0C\x0E-\x1F\x7F-\xFF]{20,}"),
                0.3
            ),
            
            # Data Exfiltration Patterns
            ThreatPattern(
                "data_exfiltration_base64",
                PatternCategory.DATA_EXFILTRATION,
                ThreatLevel.MEDIUM,
                r"[A-Za-z0-9+/]{100,}={0,2}",
                "Large base64 encoded data that might indicate exfiltration",
                re.compile(r"[A-Za-z0-9+/]{100,}={0,2}"),
                0.4
            ),
            ThreatPattern(
                "credential_harvesting",
                PatternCategory.DATA_EXFILTRATION,
                ThreatLevel.HIGH,
                r"(password|passwd|secret|key|token|auth).*[:=]\s*\S+",
                "Potential credential harvesting",
                re.compile(r"(password|passwd|secret|key|token|auth).*[:=]\s*\S+", re.IGNORECASE),
                0.3
            ),
            
            # Reconnaissance Patterns
            ThreatPattern(
                "port_scanning",
                PatternCategory.RECONNAISSANCE,
                ThreatLevel.MEDIUM,
                r"(nmap|masscan|zmap|port.*scan)",
                "Port scanning tool indicators",
                re.compile(r"(nmap|masscan|zmap|port.*scan)", re.IGNORECASE),
                0.2
            ),
            ThreatPattern(
                "system_enumeration",
                PatternCategory.RECONNAISSANCE,
                ThreatLevel.MEDIUM,
                r"(whoami|uname|id|ps\s+aux|netstat|ifconfig|ipconfig)",
                "System enumeration commands",
                re.compile(r"(whoami|uname|id|ps\s+aux|netstat|ifconfig|ipconfig)", re.IGNORECASE),
                0.3
            ),
            
            # Privilege Escalation Patterns
            ThreatPattern(
                "sudo_abuse",
                PatternCategory.PRIVILEGE_ESCALATION,
                ThreatLevel.HIGH,
                r"sudo\s+.*(/bin|/usr/bin|chmod|chown)",
                "Potential sudo privilege escalation",
                re.compile(r"sudo\s+.*(/bin|/usr/bin|chmod|chown)", re.IGNORECASE),
                0.2
            ),
            ThreatPattern(
                "suid_exploitation",
                PatternCategory.PRIVILEGE_ESCALATION,
                ThreatLevel.HIGH,
                r"find.*-perm.*4000",
                "SUID binary enumeration",
                re.compile(r"find.*-perm.*4000", re.IGNORECASE),
                0.1
            ),
            
            # Malware Signatures
            ThreatPattern(
                "known_malware_strings",
                PatternCategory.MALWARE_SIGNATURE,
                ThreatLevel.CRITICAL,
                r"(metasploit|meterpreter|empire|cobalt.*strike|mimikatz)",
                "Known malware and hacking tool signatures",
                re.compile(r"(metasploit|meterpreter|empire|cobalt.*strike|mimikatz)", re.IGNORECASE),
                0.05
            )
        ]
        
        for pattern in patterns:
            self.threat_patterns[pattern.name] = pattern
    
    def analyze_message(self, message: str, connection_id: str, 
                       message_type: str = "unknown", 
                       context: Optional[Dict[str, Any]] = None) -> DetectionResult:
        """Analyze a message for malicious patterns."""
        start_time = time.time()
        
        try:
            # Initialize connection tracking if needed
            if connection_id not in self.connection_patterns:
                self.connection_patterns[connection_id] = {
                    "message_count": 0,
                    "threat_count": 0,
                    "last_threat_time": 0,
                    "pattern_history": []
                }
            
            conn_data = self.connection_patterns[connection_id]
            conn_data["message_count"] += 1
            
            # Detect patterns
            detected_patterns = []
            pattern_details = []
            max_threat_level = ThreatLevel.LOW
            total_confidence = 0.0
            risk_score = 0
            
            # Analyze against all patterns
            for pattern_name, pattern in self.threat_patterns.items():
                matches = pattern.regex.findall(message)
                if matches:
                    # Calculate confidence based on false positive score
                    confidence = 1.0 - pattern.false_positive_score
                    
                    # Adjust confidence based on context
                    if context:
                        confidence = self._adjust_confidence_by_context(confidence, pattern, context)
                    
                    # Adjust confidence based on behavioral analysis
                    confidence = self._adjust_confidence_by_behavior(confidence, pattern, conn_data)
                    
                    if confidence >= self.confidence_threshold:
                        detected_patterns.append(pattern_name)
                        pattern_details.append({
                            "name": pattern_name,
                            "category": pattern.category.value,
                            "level": pattern.level.value,
                            "matches": matches[:5],  # Limit to first 5 matches
                            "confidence": confidence,
                            "description": pattern.description
                        })
                        
                        # Update threat level
                        if pattern.level.value == "critical":
                            max_threat_level = ThreatLevel.CRITICAL
                            risk_score += 40
                        elif pattern.level.value == "high":
                            if max_threat_level != ThreatLevel.CRITICAL:
                                max_threat_level = ThreatLevel.HIGH
                            risk_score += 25
                        elif pattern.level.value == "medium":
                            if max_threat_level not in [ThreatLevel.CRITICAL, ThreatLevel.HIGH]:
                                max_threat_level = ThreatLevel.MEDIUM
                            risk_score += 15
                        else:
                            if max_threat_level == ThreatLevel.LOW:
                                max_threat_level = ThreatLevel.LOW
                            risk_score += 5
                        
                        total_confidence += confidence
            
            # Calculate overall confidence and risk score
            if detected_patterns:
                avg_confidence = total_confidence / len(detected_patterns)
                risk_score = min(100, risk_score)  # Cap at 100
                
                # Update connection tracking
                conn_data["threat_count"] += 1
                conn_data["last_threat_time"] = time.time()
                conn_data["pattern_history"].append({
                    "timestamp": time.time(),
                    "patterns": detected_patterns,
                    "risk_score": risk_score
                })
                
                # Keep only last 50 entries
                if len(conn_data["pattern_history"]) > 50:
                    conn_data["pattern_history"] = conn_data["pattern_history"][-50:]
                
                # Update global statistics
                self.detection_stats["threats_detected"] += 1
                for pattern_name in detected_patterns:
                    self.global_patterns[pattern_name] = self.global_patterns.get(pattern_name, 0) + 1
            else:
                avg_confidence = 0.0
                risk_score = 0
            
            # Generate recommendations
            recommendations = self._generate_recommendations(detected_patterns, pattern_details, risk_score)
            
            # Update statistics
            self.detection_stats["messages_analyzed"] += 1
            self.detection_stats["processing_time_total"] += time.time() - start_time
            
            result = DetectionResult(
                detected=len(detected_patterns) > 0,
                threat_level=max_threat_level,
                patterns_found=detected_patterns,
                pattern_details=pattern_details,
                confidence_score=avg_confidence,
                risk_score=risk_score,
                recommendations=recommendations
            )
            
            if result.detected:
                logger.warning("Malicious patterns detected",
                             connection_id=connection_id,
                             message_type=message_type,
                             patterns=detected_patterns,
                             risk_score=risk_score,
                             threat_level=max_threat_level.value)
            
            return result
            
        except Exception as e:
            logger.error("Pattern detection error", error=str(e))
            return DetectionResult(
                detected=False,
                threat_level=ThreatLevel.LOW,
                patterns_found=[],
                pattern_details=[],
                confidence_score=0.0,
                risk_score=0,
                recommendations=["Pattern detection failed - manual review recommended"]
            )
    
    def analyze_binary_data(self, data: bytes, connection_id: str) -> DetectionResult:
        """Analyze binary data for malicious patterns."""
        try:
            # Convert binary to hex for pattern matching
            hex_data = data.hex()
            
            # Also try to decode as various encodings
            text_representations = []
            
            # Try UTF-8
            try:
                text_representations.append(data.decode('utf-8', errors='ignore'))
            except:
                pass
            
            # Try Latin-1
            try:
                text_representations.append(data.decode('latin-1', errors='ignore'))
            except:
                pass
            
            # Try base64 decode
            try:
                if len(data) % 4 == 0:
                    decoded = base64.b64decode(data, validate=True)
                    text_representations.append(decoded.decode('utf-8', errors='ignore'))
            except:
                pass
            
            # Analyze hex representation
            hex_result = self.analyze_message(hex_data, connection_id, "binary_hex")
            
            # Analyze text representations
            best_result = hex_result
            for text in text_representations:
                if text.strip():
                    text_result = self.analyze_message(text, connection_id, "binary_decoded")
                    if text_result.risk_score > best_result.risk_score:
                        best_result = text_result
            
            return best_result
            
        except Exception as e:
            logger.error("Binary pattern detection error", error=str(e))
            return DetectionResult(
                detected=False,
                threat_level=ThreatLevel.LOW,
                patterns_found=[],
                pattern_details=[],
                confidence_score=0.0,
                risk_score=0,
                recommendations=["Binary analysis failed"]
            )
    
    def _adjust_confidence_by_context(self, confidence: float, pattern: ThreatPattern, 
                                    context: Dict[str, Any]) -> float:
        """Adjust confidence based on message context."""
        # Terminal data context
        if context.get("message_type") == "terminal_data":
            # Shell injection patterns are more concerning in terminal context
            if pattern.category == PatternCategory.COMMAND_INJECTION:
                confidence = min(1.0, confidence * 1.5)
            # XSS patterns are less concerning in terminal context
            elif pattern.category == PatternCategory.XSS_ATTACK:
                confidence = max(0.0, confidence * 0.7)
        
        # Authentication context
        if context.get("is_authenticated", False):
            # Authenticated users are slightly less likely to be attacks
            confidence = max(0.0, confidence * 0.9)
        
        # User role context
        if context.get("user_role") == "admin":
            # Admin users might legitimately use commands that look suspicious
            if pattern.category in [PatternCategory.COMMAND_INJECTION, PatternCategory.PRIVILEGE_ESCALATION]:
                confidence = max(0.0, confidence * 0.8)
        
        return confidence
    
    def _adjust_confidence_by_behavior(self, confidence: float, pattern: ThreatPattern,
                                     conn_data: Dict[str, Any]) -> float:
        """Adjust confidence based on connection behavioral patterns."""
        # If this connection has a history of threats, increase confidence
        threat_ratio = conn_data["threat_count"] / max(1, conn_data["message_count"])
        if threat_ratio > 0.1:  # More than 10% of messages are threats
            confidence = min(1.0, confidence * 1.3)
        
        # Recent threat activity
        if conn_data["last_threat_time"] > 0:
            time_since_last_threat = time.time() - conn_data["last_threat_time"]
            if time_since_last_threat < 60:  # Less than 1 minute
                confidence = min(1.0, confidence * 1.2)
        
        # Pattern repetition
        recent_patterns = [p for entry in conn_data["pattern_history"][-10:] 
                          for p in entry["patterns"]]
        if pattern.name in recent_patterns:
            repetition_count = recent_patterns.count(pattern.name)
            if repetition_count > 2:
                confidence = min(1.0, confidence * 1.4)
        
        return confidence
    
    def _generate_recommendations(self, patterns: List[str], pattern_details: List[Dict[str, Any]], 
                                risk_score: int) -> List[str]:
        """Generate security recommendations based on detected patterns."""
        recommendations = []
        
        if risk_score >= 80:
            recommendations.append("IMMEDIATE ACTION: Block connection and investigate")
            recommendations.append("Alert security team of potential critical threat")
        elif risk_score >= 60:
            recommendations.append("HIGH PRIORITY: Monitor connection closely")
            recommendations.append("Consider implementing additional rate limiting")
        elif risk_score >= 40:
            recommendations.append("MEDIUM PRIORITY: Log and monitor for escalation")
            recommendations.append("Review user permissions and access controls")
        elif risk_score >= 20:
            recommendations.append("LOW PRIORITY: Log for trend analysis")
        
        # Pattern-specific recommendations
        categories_found = set(detail["category"] for detail in pattern_details)
        
        if "command_injection" in categories_found:
            recommendations.append("Implement stricter input validation for terminal commands")
            recommendations.append("Consider sandboxing terminal sessions")
        
        if "xss_attack" in categories_found:
            recommendations.append("Ensure all output is properly sanitized")
            recommendations.append("Review Content Security Policy settings")
        
        if "path_traversal" in categories_found:
            recommendations.append("Validate and restrict file access paths")
            recommendations.append("Implement proper access controls for file operations")
        
        if "data_exfiltration" in categories_found:
            recommendations.append("Monitor for unusual data transfer patterns")
            recommendations.append("Review data loss prevention policies")
        
        if "malware_signature" in categories_found:
            recommendations.append("Quarantine connection immediately")
            recommendations.append("Scan for malware on client system")
        
        return recommendations
    
    def get_connection_risk_profile(self, connection_id: str) -> Dict[str, Any]:
        """Get risk profile for a specific connection."""
        if connection_id not in self.connection_patterns:
            return {"risk_level": "unknown", "message_count": 0}
        
        conn_data = self.connection_patterns[connection_id]
        
        # Calculate risk metrics
        threat_ratio = conn_data["threat_count"] / max(1, conn_data["message_count"])
        recent_activity = len([entry for entry in conn_data["pattern_history"]
                             if time.time() - entry["timestamp"] < 300])  # Last 5 minutes
        
        max_risk_score = max([entry["risk_score"] for entry in conn_data["pattern_history"]], 
                           default=0)
        
        # Determine overall risk level
        if max_risk_score >= 80 or threat_ratio >= 0.2:
            risk_level = "critical"
        elif max_risk_score >= 60 or threat_ratio >= 0.1:
            risk_level = "high"
        elif max_risk_score >= 40 or threat_ratio >= 0.05:
            risk_level = "medium"
        elif max_risk_score >= 20:
            risk_level = "low"
        else:
            risk_level = "minimal"
        
        return {
            "risk_level": risk_level,
            "threat_ratio": threat_ratio,
            "message_count": conn_data["message_count"],
            "threat_count": conn_data["threat_count"],
            "max_risk_score": max_risk_score,
            "recent_activity": recent_activity,
            "last_threat_time": conn_data["last_threat_time"],
            "pattern_history": conn_data["pattern_history"][-10:]  # Last 10 events
        }
    
    def get_detection_stats(self) -> Dict[str, Any]:
        """Get detection statistics."""
        avg_processing_time = (self.detection_stats["processing_time_total"] / 
                             max(1, self.detection_stats["messages_analyzed"]))
        
        return {
            **self.detection_stats,
            "average_processing_time_ms": avg_processing_time * 1000,
            "threat_detection_rate": (self.detection_stats["threats_detected"] / 
                                    max(1, self.detection_stats["messages_analyzed"])),
            "top_patterns": sorted(self.global_patterns.items(), 
                                 key=lambda x: x[1], reverse=True)[:10],
            "active_connections": len(self.connection_patterns)
        }
    
    def cleanup_old_data(self, max_age_seconds: int = 3600):
        """Clean up old connection data."""
        current_time = time.time()
        cutoff_time = current_time - max_age_seconds
        
        # Clean up old connection data
        to_remove = []
        for connection_id, conn_data in self.connection_patterns.items():
            if conn_data["last_threat_time"] > 0 and conn_data["last_threat_time"] < cutoff_time:
                # No recent activity, remove if no threats in history
                if conn_data["threat_count"] == 0:
                    to_remove.append(connection_id)
                else:
                    # Keep but clean old pattern history
                    conn_data["pattern_history"] = [
                        entry for entry in conn_data["pattern_history"]
                        if entry["timestamp"] >= cutoff_time
                    ]
        
        for connection_id in to_remove:
            del self.connection_patterns[connection_id]
        
        logger.debug("Cleaned up pattern detector data", 
                    connections_removed=len(to_remove))


# Global instance
_pattern_detector = None


def get_pattern_detector() -> MaliciousPatternDetector:
    """Get global pattern detector instance."""
    global _pattern_detector
    if _pattern_detector is None:
        _pattern_detector = MaliciousPatternDetector()
        logger.info("Malicious pattern detector initialized",
                   patterns_loaded=len(_pattern_detector.threat_patterns))
    return _pattern_detector