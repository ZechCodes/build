"""MessagePack compression with integrity checks for WebSocket messages."""

import zlib
import gzip
import brotli
import msgpack
import hashlib
import hmac
import base64
import time
from typing import Dict, Any, Optional, Tuple, Union, List
from dataclasses import dataclass
from enum import Enum
import structlog

logger = structlog.get_logger(__name__)


class CompressionAlgorithm(Enum):
    """Supported compression algorithms."""
    NONE = "none"
    GZIP = "gzip"
    DEFLATE = "deflate"
    BROTLI = "brotli"
    ZLIB = "zlib"


class CompressionLevel(Enum):
    """Compression level presets."""
    FAST = 1      # Fastest compression, larger size
    BALANCED = 6  # Balanced compression and speed
    BEST = 9      # Best compression, slower


@dataclass
class CompressionResult:
    """Result of compression operation."""
    compressed_data: bytes
    original_size: int
    compressed_size: int
    compression_ratio: float
    algorithm: CompressionAlgorithm
    integrity_hash: str
    compression_time_ms: float
    
    @property
    def size_reduction_percentage(self) -> float:
        """Calculate size reduction percentage."""
        if self.original_size == 0:
            return 0.0
        return ((self.original_size - self.compressed_size) / self.original_size) * 100


@dataclass
class DecompressionResult:
    """Result of decompression operation."""
    decompressed_data: bytes
    original_size: int
    compressed_size: int
    algorithm: CompressionAlgorithm
    integrity_verified: bool
    decompression_time_ms: float
    warnings: List[str] = None
    
    def __post_init__(self):
        if self.warnings is None:
            self.warnings = []


class MessagePackCompressor:
    """Advanced MessagePack compression with integrity checks."""
    
    def __init__(self):
        # Compression settings
        self.default_algorithm = CompressionAlgorithm.GZIP
        self.default_level = CompressionLevel.BALANCED
        self.min_compression_threshold = 100  # Only compress messages > 100 bytes
        self.max_compression_ratio = 50.0  # Reject if compression ratio too high (bomb detection)
        
        # MessagePack settings
        self.msgpack_max_bin_len = 1024 * 1024  # 1MB max binary length
        self.msgpack_max_array_len = 10000      # Max array length
        self.msgpack_max_map_len = 10000        # Max map length
        self.msgpack_max_str_len = 1024 * 1024  # 1MB max string length
        
        # Integrity settings
        self.integrity_algorithm = "sha256"
        self.hmac_key = b"websocket_compression_key_" + b"0" * 16  # Should be configurable
        
        # Performance tracking
        self.compression_stats = {
            "total_compressed": 0,
            "total_decompressed": 0,
            "total_bytes_saved": 0,
            "average_compression_ratio": 0.0,
            "algorithms_used": {alg.value: 0 for alg in CompressionAlgorithm}
        }
    
    def compress_message(self, message: Dict[str, Any], 
                        algorithm: Optional[CompressionAlgorithm] = None,
                        level: Optional[CompressionLevel] = None) -> CompressionResult:
        """
        Compress a message with integrity checks.
        
        Args:
            message: Message to compress
            algorithm: Compression algorithm to use
            level: Compression level
        
        Returns:
            CompressionResult with compressed data and metadata
        """
        start_time = time.time()
        
        algorithm = algorithm or self.default_algorithm
        level = level or self.default_level
        
        try:
            # Serialize message with MessagePack
            msgpack_data = msgpack.packb(
                message,
                use_bin_type=True,
                strict_types=True
            )
            
            original_size = len(msgpack_data)
            
            # Check if compression is worthwhile
            if original_size < self.min_compression_threshold:
                # Return "compressed" data without actual compression
                result = CompressionResult(
                    compressed_data=msgpack_data,
                    original_size=original_size,
                    compressed_size=original_size,
                    compression_ratio=1.0,
                    algorithm=CompressionAlgorithm.NONE,
                    integrity_hash=self._calculate_integrity_hash(msgpack_data),
                    compression_time_ms=(time.time() - start_time) * 1000
                )
                logger.debug("Message too small for compression", 
                           size=original_size, threshold=self.min_compression_threshold)
                return result
            
            # Compress the data
            compressed_data = self._compress_data(msgpack_data, algorithm, level.value)
            compressed_size = len(compressed_data)
            compression_ratio = compressed_size / original_size if original_size > 0 else 1.0
            
            # Check for compression bombs
            if compression_ratio > self.max_compression_ratio:
                raise ValueError(f"Suspicious compression ratio: {compression_ratio}")
            
            # Calculate integrity hash
            integrity_hash = self._calculate_integrity_hash(compressed_data)
            
            # Create result
            result = CompressionResult(
                compressed_data=compressed_data,
                original_size=original_size,
                compressed_size=compressed_size,
                compression_ratio=compression_ratio,
                algorithm=algorithm,
                integrity_hash=integrity_hash,
                compression_time_ms=(time.time() - start_time) * 1000
            )
            
            # Update statistics
            self._update_compression_stats(result)
            
            logger.debug("Message compressed",
                        algorithm=algorithm.value,
                        original_size=original_size,
                        compressed_size=compressed_size,
                        ratio=compression_ratio,
                        reduction_pct=result.size_reduction_percentage)
            
            return result
            
        except Exception as e:
            logger.error("Compression failed", error=str(e))
            raise ValueError(f"Compression error: {e}")
    
    def decompress_message(self, compressed_data: bytes,
                          algorithm: CompressionAlgorithm,
                          expected_hash: Optional[str] = None) -> DecompressionResult:
        """
        Decompress a message with integrity verification.
        
        Args:
            compressed_data: Compressed data to decompress
            algorithm: Algorithm used for compression
            expected_hash: Expected integrity hash
        
        Returns:
            DecompressionResult with decompressed message and metadata
        """
        start_time = time.time()
        warnings = []
        
        try:
            compressed_size = len(compressed_data)
            
            # Verify integrity hash if provided
            integrity_verified = True
            if expected_hash:
                actual_hash = self._calculate_integrity_hash(compressed_data)
                if not hmac.compare_digest(expected_hash, actual_hash):
                    integrity_verified = False
                    warnings.append("Integrity hash verification failed")
            
            # Decompress the data
            if algorithm == CompressionAlgorithm.NONE:
                decompressed_data = compressed_data
            else:
                decompressed_data = self._decompress_data(compressed_data, algorithm)
            
            # Check decompressed size for bombs
            original_size = len(decompressed_data)
            if original_size > self.msgpack_max_bin_len * 10:  # 10MB limit
                raise ValueError(f"Decompressed data too large: {original_size} bytes")
            
            # Deserialize with MessagePack
            try:
                message = msgpack.unpackb(
                    decompressed_data,
                    raw=False,
                    strict_map_key=False,
                    max_bin_len=self.msgpack_max_bin_len,
                    max_array_len=self.msgpack_max_array_len,
                    max_map_len=self.msgpack_max_map_len,
                    max_str_len=self.msgpack_max_str_len
                )
            except Exception as e:
                raise ValueError(f"MessagePack deserialization failed: {e}")
            
            # Create result
            result = DecompressionResult(
                decompressed_data=decompressed_data,
                original_size=original_size,
                compressed_size=compressed_size,
                algorithm=algorithm,
                integrity_verified=integrity_verified,
                decompression_time_ms=(time.time() - start_time) * 1000,
                warnings=warnings
            )
            
            # Update statistics
            self.compression_stats["total_decompressed"] += 1
            
            logger.debug("Message decompressed",
                        algorithm=algorithm.value,
                        compressed_size=compressed_size,
                        original_size=original_size,
                        integrity_verified=integrity_verified)
            
            # Return the deserialized message as decompressed_data for convenience
            result.decompressed_data = message
            
            return result
            
        except Exception as e:
            logger.error("Decompression failed", algorithm=algorithm.value, error=str(e))
            raise ValueError(f"Decompression error: {e}")
    
    def _compress_data(self, data: bytes, algorithm: CompressionAlgorithm, level: int) -> bytes:
        """Compress data using specified algorithm."""
        if algorithm == CompressionAlgorithm.GZIP:
            return gzip.compress(data, compresslevel=level)
        elif algorithm == CompressionAlgorithm.DEFLATE:
            return zlib.compress(data, level=level)
        elif algorithm == CompressionAlgorithm.ZLIB:
            return zlib.compress(data, level=level)
        elif algorithm == CompressionAlgorithm.BROTLI:
            return brotli.compress(data, quality=level)
        else:
            raise ValueError(f"Unsupported compression algorithm: {algorithm}")
    
    def _decompress_data(self, data: bytes, algorithm: CompressionAlgorithm) -> bytes:
        """Decompress data using specified algorithm."""
        try:
            if algorithm == CompressionAlgorithm.GZIP:
                return gzip.decompress(data)
            elif algorithm == CompressionAlgorithm.DEFLATE:
                return zlib.decompress(data)
            elif algorithm == CompressionAlgorithm.ZLIB:
                return zlib.decompress(data)
            elif algorithm == CompressionAlgorithm.BROTLI:
                return brotli.decompress(data)
            else:
                raise ValueError(f"Unsupported decompression algorithm: {algorithm}")
        except Exception as e:
            raise ValueError(f"Decompression failed with {algorithm.value}: {e}")
    
    def _calculate_integrity_hash(self, data: bytes) -> str:
        """Calculate integrity hash for compressed data."""
        if self.integrity_algorithm == "sha256":
            return hashlib.sha256(data).hexdigest()
        elif self.integrity_algorithm == "sha512":
            return hashlib.sha512(data).hexdigest()
        else:
            return hashlib.md5(data).hexdigest()
    
    def _calculate_hmac(self, data: bytes) -> str:
        """Calculate HMAC for data integrity."""
        return hmac.new(self.hmac_key, data, hashlib.sha256).hexdigest()
    
    def _update_compression_stats(self, result: CompressionResult):
        """Update compression statistics."""
        self.compression_stats["total_compressed"] += 1
        self.compression_stats["total_bytes_saved"] += (result.original_size - result.compressed_size)
        self.compression_stats["algorithms_used"][result.algorithm.value] += 1
        
        # Update average compression ratio
        total_compressed = self.compression_stats["total_compressed"]
        current_avg = self.compression_stats["average_compression_ratio"]
        new_avg = ((current_avg * (total_compressed - 1)) + result.compression_ratio) / total_compressed
        self.compression_stats["average_compression_ratio"] = new_avg
    
    def choose_best_algorithm(self, message: Dict[str, Any]) -> CompressionAlgorithm:
        """Choose best compression algorithm based on message characteristics."""
        try:
            # Quick MessagePack serialization to estimate data characteristics
            msgpack_data = msgpack.packb(message, use_bin_type=True)
            size = len(msgpack_data)
            
            # For small messages, use fast compression
            if size < 1000:
                return CompressionAlgorithm.DEFLATE
            
            # For medium messages, use balanced compression
            if size < 10000:
                return CompressionAlgorithm.GZIP
            
            # For large messages, use best compression
            return CompressionAlgorithm.BROTLI
            
        except Exception:
            # Fallback to default
            return self.default_algorithm
    
    def create_compressed_message_envelope(self, message: Dict[str, Any],
                                         algorithm: Optional[CompressionAlgorithm] = None) -> Dict[str, Any]:
        """Create a message envelope with compression metadata."""
        # Choose algorithm if not specified
        if algorithm is None:
            algorithm = self.choose_best_algorithm(message)
        
        # Compress the message
        compression_result = self.compress_message(message, algorithm)
        
        # Create envelope
        envelope = {
            "type": "compressed_message",
            "compression": {
                "algorithm": compression_result.algorithm.value,
                "original_size": compression_result.original_size,
                "compressed_size": compression_result.compressed_size,
                "compression_ratio": compression_result.compression_ratio,
                "integrity_hash": compression_result.integrity_hash
            },
            "data": base64.b64encode(compression_result.compressed_data).decode('ascii'),
            "timestamp": time.time()
        }
        
        return envelope
    
    def extract_compressed_message(self, envelope: Dict[str, Any]) -> Dict[str, Any]:
        """Extract and decompress message from envelope."""
        if envelope.get("type") != "compressed_message":
            raise ValueError("Not a compressed message envelope")
        
        compression_info = envelope.get("compression", {})
        algorithm_str = compression_info.get("algorithm", "none")
        expected_hash = compression_info.get("integrity_hash")
        
        # Parse algorithm
        try:
            algorithm = CompressionAlgorithm(algorithm_str)
        except ValueError:
            raise ValueError(f"Unknown compression algorithm: {algorithm_str}")
        
        # Decode base64 data
        try:
            compressed_data = base64.b64decode(envelope["data"])
        except Exception as e:
            raise ValueError(f"Invalid base64 data: {e}")
        
        # Decompress
        decompression_result = self.decompress_message(
            compressed_data, algorithm, expected_hash
        )
        
        if not decompression_result.integrity_verified:
            logger.warning("Message integrity verification failed during decompression")
        
        return decompression_result.decompressed_data
    
    def get_compression_stats(self) -> Dict[str, Any]:
        """Get compression statistics for monitoring."""
        stats = self.compression_stats.copy()
        stats.update({
            "config": {
                "default_algorithm": self.default_algorithm.value,
                "min_compression_threshold": self.min_compression_threshold,
                "max_compression_ratio": self.max_compression_ratio,
                "integrity_algorithm": self.integrity_algorithm
            },
            "msgpack_limits": {
                "max_bin_len": self.msgpack_max_bin_len,
                "max_array_len": self.msgpack_max_array_len,
                "max_map_len": self.msgpack_max_map_len,
                "max_str_len": self.msgpack_max_str_len
            }
        })
        return stats
    
    def reset_stats(self):
        """Reset compression statistics."""
        self.compression_stats = {
            "total_compressed": 0,
            "total_decompressed": 0,
            "total_bytes_saved": 0,
            "average_compression_ratio": 0.0,
            "algorithms_used": {alg.value: 0 for alg in CompressionAlgorithm}
        }


# Global instance
_message_compressor = None


def get_message_compressor() -> MessagePackCompressor:
    """Get global MessagePack compressor instance."""
    global _message_compressor
    if _message_compressor is None:
        _message_compressor = MessagePackCompressor()
        logger.info("MessagePack compressor initialized")
    return _message_compressor