"""Binary data validation and integrity checks for WebSocket messages."""

import hashlib
import hmac
import zlib
import base64
import secrets
from typing import Dict, Any, Optional, Tuple, List, Union
from dataclasses import dataclass
from enum import Enum
import structlog

logger = structlog.get_logger(__name__)


class BinaryFormat(Enum):
    """Supported binary data formats."""
    RAW_BYTES = "raw_bytes"
    BASE64 = "base64"
    UTF8_TEXT = "utf8_text"
    TERMINAL_DATA = "terminal_data"
    COMPRESSED = "compressed"
    UNKNOWN = "unknown"


@dataclass
class BinaryValidationResult:
    """Result of binary data validation."""
    valid: bool
    format_detected: BinaryFormat
    size_bytes: int
    integrity_hash: str
    compression_ratio: Optional[float] = None
    mime_type: Optional[str] = None
    errors: List[str] = None
    warnings: List[str] = None
    
    def __post_init__(self):
        if self.errors is None:
            self.errors = []
        if self.warnings is None:
            self.warnings = []


class BinaryDataValidator:
    """Advanced binary data validator for WebSocket messages."""
    
    def __init__(self):
        # Size limits
        self.max_binary_size = 1024 * 1024  # 1MB
        self.max_terminal_chunk = 64 * 1024  # 64KB for terminal data
        self.max_compression_ratio = 10.0  # Maximum compression ratio
        
        # Integrity validation
        self.enable_integrity_checks = True
        self.integrity_algorithm = "sha256"
        self.hmac_key = secrets.token_bytes(32)
        
        # Simple format detection patterns
        self.format_patterns = {
            b'\x1f\x8b': 'application/gzip',
            b'PK\x03\x04': 'application/zip',
            b'\x89PNG': 'image/png',
            b'\xff\xd8\xff': 'image/jpeg',
            b'GIF87a': 'image/gif',
            b'GIF89a': 'image/gif',
            b'%PDF': 'application/pdf'
        }
        
        # Suspicious patterns
        self.suspicious_patterns = [
            b'\x00' * 100,  # Long null sequences
            b'\xFF' * 100,  # Long 0xFF sequences
            b'javascript:',  # JavaScript URLs
            b'<script',      # Script tags
            b'eval(',        # Eval functions
            b'exec(',        # Exec functions
        ]
        
        # Terminal control sequences that should be validated
        self.dangerous_terminal_sequences = [
            b'\x1b]0;',      # Set window title (potential for malicious titles)
            b'\x1b]1;',      # Set icon name
            b'\x1b]2;',      # Set window title
            b'\x1b[6n',      # Device status report (can leak info)
            b'\x1b[c',       # Device attributes
            b'\x1b[>c',      # Secondary device attributes
        ]
    
    async def validate_binary_data(self, data: Union[bytes, str], 
                                 expected_format: BinaryFormat = BinaryFormat.RAW_BYTES,
                                 context: str = "unknown") -> BinaryValidationResult:
        """
        Comprehensive validation of binary data.
        
        Args:
            data: Binary data to validate
            expected_format: Expected format of the data
            context: Context where data is used (e.g., "terminal_output", "file_upload")
        
        Returns:
            BinaryValidationResult with validation details
        """
        result = BinaryValidationResult(
            valid=True,
            format_detected=BinaryFormat.UNKNOWN,
            size_bytes=0,
            integrity_hash=""
        )
        
        try:
            # Convert to bytes if string
            if isinstance(data, str):
                if expected_format == BinaryFormat.BASE64:
                    try:
                        data = base64.b64decode(data)
                        result.format_detected = BinaryFormat.BASE64
                    except Exception as e:
                        result.errors.append(f"Invalid base64 encoding: {e}")
                        result.valid = False
                        return result
                else:
                    data = data.encode('utf-8')
                    result.format_detected = BinaryFormat.UTF8_TEXT
            else:
                result.format_detected = BinaryFormat.RAW_BYTES
            
            # Size validation
            result.size_bytes = len(data)
            if result.size_bytes > self.max_binary_size:
                result.errors.append(f"Data too large: {result.size_bytes} > {self.max_binary_size}")
                result.valid = False
                return result
            
            # Context-specific size limits
            if context == "terminal_data" and result.size_bytes > self.max_terminal_chunk:
                result.errors.append(f"Terminal data too large: {result.size_bytes} > {self.max_terminal_chunk}")
                result.valid = False
                return result
            
            # Calculate integrity hash
            result.integrity_hash = self._calculate_integrity_hash(data)
            
            # Simple format detection using magic bytes
            try:
                result.mime_type = self._detect_mime_type(data)
                if result.mime_type:
                    logger.debug("MIME type detected", mime_type=result.mime_type, size=result.size_bytes)
            except Exception as e:
                result.warnings.append(f"MIME detection failed: {e}")
            
            # Validate specific formats
            if expected_format == BinaryFormat.TERMINAL_DATA:
                await self._validate_terminal_data(data, result)
            elif expected_format == BinaryFormat.COMPRESSED:
                await self._validate_compressed_data(data, result)
            
            # Check for suspicious patterns
            await self._check_suspicious_patterns(data, result, context)
            
            # Compression analysis
            await self._analyze_compression(data, result)
            
            # Zero-byte validation
            await self._validate_zero_bytes(data, result, context)
            
            logger.debug("Binary validation completed", 
                        valid=result.valid,
                        format=result.format_detected.value,
                        size=result.size_bytes,
                        errors=len(result.errors),
                        warnings=len(result.warnings))
            
            return result
            
        except Exception as e:
            logger.error("Binary validation error", error=str(e))
            result.errors.append(f"Validation error: {e}")
            result.valid = False
            return result
    
    def _calculate_integrity_hash(self, data: bytes) -> str:
        """Calculate integrity hash for binary data."""
        if self.integrity_algorithm == "sha256":
            return hashlib.sha256(data).hexdigest()
        elif self.integrity_algorithm == "sha512":
            return hashlib.sha512(data).hexdigest()
        else:
            return hashlib.md5(data).hexdigest()
    
    def _detect_mime_type(self, data: bytes) -> Optional[str]:
        """Simple MIME type detection using magic bytes."""
        if len(data) < 4:
            return None
        
        # Check for known patterns
        for pattern, mime_type in self.format_patterns.items():
            if data.startswith(pattern):
                return mime_type
        
        # Check for text content
        try:
            data.decode('utf-8')
            return 'text/plain'
        except UnicodeDecodeError:
            pass
        
        # Default to binary
        return 'application/octet-stream'
    
    def calculate_hmac_signature(self, data: bytes, additional_data: str = "") -> str:
        """Calculate HMAC signature for data integrity."""
        combined_data = data + additional_data.encode('utf-8')
        return hmac.new(self.hmac_key, combined_data, hashlib.sha256).hexdigest()
    
    def verify_hmac_signature(self, data: bytes, signature: str, additional_data: str = "") -> bool:
        """Verify HMAC signature for data integrity."""
        expected_signature = self.calculate_hmac_signature(data, additional_data)
        return hmac.compare_digest(signature, expected_signature)
    
    async def _validate_terminal_data(self, data: bytes, result: BinaryValidationResult):
        """Validate terminal-specific binary data."""
        # Check for dangerous terminal control sequences
        for dangerous_seq in self.dangerous_terminal_sequences:
            if dangerous_seq in data:
                result.warnings.append(f"Potentially dangerous terminal sequence detected: {dangerous_seq.hex()}")
        
        # Validate UTF-8 encoding for terminal data
        try:
            text = data.decode('utf-8', errors='strict')
            
            # Check for null characters (can cause terminal issues)
            if '\x00' in text:
                result.warnings.append("Null characters detected in terminal data")
            
            # Check for excessive control characters
            control_chars = sum(1 for c in text if ord(c) < 32 and c not in '\t\n\r\x1b')
            if control_chars > len(text) * 0.1:  # More than 10% control chars
                result.warnings.append(f"High percentage of control characters: {control_chars}/{len(text)}")
                
        except UnicodeDecodeError as e:
            result.errors.append(f"Invalid UTF-8 in terminal data: {e}")
            result.valid = False
    
    async def _validate_compressed_data(self, data: bytes, result: BinaryValidationResult):
        """Validate compressed binary data."""
        try:
            # Try to decompress to check validity
            decompressed = zlib.decompress(data)
            result.compression_ratio = len(data) / len(decompressed) if len(decompressed) > 0 else 0
            
            # Check for compression bombs
            if result.compression_ratio > self.max_compression_ratio:
                result.errors.append(f"Suspicious compression ratio: {result.compression_ratio}")
                result.valid = False
                return
            
            # Check decompressed size
            if len(decompressed) > self.max_binary_size * 10:  # 10MB decompressed limit
                result.errors.append(f"Decompressed data too large: {len(decompressed)}")
                result.valid = False
                return
            
            result.format_detected = BinaryFormat.COMPRESSED
            logger.debug("Compressed data validated", 
                        original_size=len(data),
                        decompressed_size=len(decompressed),
                        compression_ratio=result.compression_ratio)
            
        except zlib.error as e:
            result.errors.append(f"Invalid compressed data: {e}")
            result.valid = False
    
    async def _check_suspicious_patterns(self, data: bytes, result: BinaryValidationResult, context: str):
        """Check for suspicious binary patterns."""
        for pattern in self.suspicious_patterns:
            if pattern in data:
                result.warnings.append(f"Suspicious pattern detected: {pattern.hex()[:20]}...")
        
        # Check for potential buffer overflow patterns
        if len(data) > 1000:
            # Look for repeated patterns that might indicate buffer overflow attempts
            for i in range(0, min(len(data) - 100, 1000), 100):
                chunk = data[i:i+100]
                if data.count(chunk) > 5:  # Same 100-byte chunk repeated more than 5 times
                    result.warnings.append("Potential buffer overflow pattern detected")
                    break
        
        # Context-specific checks
        if context == "terminal_data":
            # Check for shell injection patterns
            shell_patterns = [b'$(', b'`', b'&&', b'||', b';']
            for pattern in shell_patterns:
                if pattern in data:
                    result.warnings.append(f"Potential shell injection pattern: {pattern}")
    
    async def _analyze_compression(self, data: bytes, result: BinaryValidationResult):
        """Analyze compression characteristics of the data."""
        try:
            # Test compression to understand data entropy
            compressed = zlib.compress(data, level=6)
            compression_ratio = len(compressed) / len(data) if len(data) > 0 else 1.0
            
            # High compression ratio might indicate repeated data (potential attack)
            if compression_ratio < 0.1 and len(data) > 1000:
                result.warnings.append(f"Unusually high compression ratio: {compression_ratio:.3f}")
            
            # Very low compression might indicate random/encrypted data
            if compression_ratio > 0.95 and len(data) > 1000:
                result.warnings.append("Data appears to be random or encrypted")
            
        except Exception as e:
            logger.debug("Compression analysis failed", error=str(e))
    
    async def _validate_zero_bytes(self, data: bytes, result: BinaryValidationResult, context: str):
        """Validate handling of zero bytes in data."""
        zero_count = data.count(b'\x00')
        
        if zero_count > 0:
            zero_percentage = zero_count / len(data) * 100
            
            if context == "terminal_data" and zero_count > 10:
                result.warnings.append(f"High number of null bytes in terminal data: {zero_count}")
            
            if zero_percentage > 50:
                result.warnings.append(f"Data is {zero_percentage:.1f}% null bytes")
            
            # Check for null byte injection patterns
            if b'\x00' in data and context in ["terminal_data", "file_path"]:
                result.warnings.append("Null byte injection pattern detected")
    
    def create_integrity_metadata(self, data: bytes, additional_info: Dict[str, Any] = None) -> Dict[str, Any]:
        """Create integrity metadata for binary data."""
        metadata = {
            "size": len(data),
            "hash": self._calculate_integrity_hash(data),
            "hmac": self.calculate_hmac_signature(data),
            "timestamp": __import__('time').time(),
            "algorithm": self.integrity_algorithm
        }
        
        if additional_info:
            metadata.update(additional_info)
        
        return metadata
    
    def verify_integrity_metadata(self, data: bytes, metadata: Dict[str, Any]) -> Tuple[bool, List[str]]:
        """Verify integrity using stored metadata."""
        errors = []
        
        # Verify size
        if len(data) != metadata.get("size", 0):
            errors.append(f"Size mismatch: expected {metadata.get('size')}, got {len(data)}")
        
        # Verify hash
        expected_hash = metadata.get("hash", "")
        actual_hash = self._calculate_integrity_hash(data)
        if not hmac.compare_digest(expected_hash, actual_hash):
            errors.append("Hash verification failed")
        
        # Verify HMAC if present
        if "hmac" in metadata:
            expected_hmac = metadata["hmac"]
            if not self.verify_hmac_signature(data, expected_hmac):
                errors.append("HMAC verification failed")
        
        return len(errors) == 0, errors
    
    def get_validation_stats(self) -> Dict[str, Any]:
        """Get validation statistics for monitoring."""
        return {
            "max_binary_size": self.max_binary_size,
            "max_terminal_chunk": self.max_terminal_chunk,
            "max_compression_ratio": self.max_compression_ratio,
            "integrity_algorithm": self.integrity_algorithm,
            "integrity_checks_enabled": self.enable_integrity_checks,
            "suspicious_patterns_count": len(self.suspicious_patterns),
            "dangerous_terminal_sequences_count": len(self.dangerous_terminal_sequences)
        }


# Global instance
_binary_validator = None


async def get_binary_validator() -> BinaryDataValidator:
    """Get global binary data validator instance."""
    global _binary_validator
    if _binary_validator is None:
        _binary_validator = BinaryDataValidator()
        logger.info("Binary data validator initialized")
    return _binary_validator