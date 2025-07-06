"""
Cryptographic Signature Service for VM Snapshot Authenticity

Provides HMAC-based digital signatures for snapshot integrity and authenticity
verification, completing the final security requirement for 100% compliance.
"""

import hmac
import hashlib
import time
import json
from typing import Dict, Any, Optional, Tuple
from dataclasses import dataclass
from pathlib import Path
import structlog
import logfire

logger = structlog.get_logger()


@dataclass 
class SnapshotSignature:
    """Cryptographic signature for snapshot authenticity."""
    signature: str
    algorithm: str
    timestamp: float
    key_id: str
    metadata_hash: str


class CryptographicSignatureService:
    """
    Cryptographic signature service for snapshot authenticity verification.
    
    Uses HMAC-SHA256 for creating and verifying digital signatures that ensure
    snapshot authenticity and prevent tampering.
    """

    def __init__(self, signing_key: Optional[str] = None):
        """Initialize signature service with signing key."""
        # In production, this would be loaded from secure key management
        self.signing_key = signing_key or self._generate_default_key()
        self.algorithm = "HMAC-SHA256"
        self.key_id = "snapshot-signing-key-v1"
        
    def _generate_default_key(self) -> str:
        """Generate a default signing key for development."""
        # In production, use proper key derivation and management
        return "snapshot-signing-key-development-only-change-in-production"
    
    def sign_snapshot(self, snapshot_metadata: Dict[str, Any], 
                     snapshot_data_hash: str) -> SnapshotSignature:
        """
        Create cryptographic signature for snapshot authenticity.
        
        Args:
            snapshot_metadata: Snapshot metadata dictionary
            snapshot_data_hash: SHA-256 hash of snapshot data
            
        Returns:
            SnapshotSignature: Cryptographic signature object
        """
        try:
            # Create payload for signing
            signing_payload = {
                "snapshot_id": snapshot_metadata.get("snapshot_id"),
                "user_id": snapshot_metadata.get("user_id"),
                "vm_id": snapshot_metadata.get("vm_id"),
                "created_at": snapshot_metadata.get("created_at"),
                "data_hash": snapshot_data_hash,
                "size_bytes": snapshot_metadata.get("size_bytes"),
                "vm_config_hash": self._hash_vm_config(snapshot_metadata.get("vm_config", {}))
            }
            
            # Create canonical representation
            canonical_payload = json.dumps(signing_payload, sort_keys=True, separators=(',', ':'))
            metadata_hash = hashlib.sha256(canonical_payload.encode()).hexdigest()
            
            # Generate HMAC signature
            signature = hmac.new(
                self.signing_key.encode(),
                canonical_payload.encode(),
                hashlib.sha256
            ).hexdigest()
            
            # Create signature object
            snapshot_signature = SnapshotSignature(
                signature=signature,
                algorithm=self.algorithm,
                timestamp=time.time(),
                key_id=self.key_id,
                metadata_hash=metadata_hash
            )
            
            logger.info("Snapshot signature created",
                       snapshot_id=snapshot_metadata.get("snapshot_id"),
                       algorithm=self.algorithm,
                       key_id=self.key_id)
            
            logfire.info("Cryptographic signature generated",
                        snapshot_id=snapshot_metadata.get("snapshot_id"),
                        signature_algorithm=self.algorithm,
                        metadata_hash=metadata_hash)
            
            return snapshot_signature
            
        except Exception as e:
            logger.error("Failed to create snapshot signature",
                        snapshot_id=snapshot_metadata.get("snapshot_id"),
                        error=str(e))
            raise ValueError(f"Signature creation failed: {str(e)}")
    
    def verify_snapshot_signature(self, snapshot_metadata: Dict[str, Any],
                                 snapshot_data_hash: str,
                                 signature: SnapshotSignature) -> bool:
        """
        Verify cryptographic signature for snapshot authenticity.
        
        Args:
            snapshot_metadata: Snapshot metadata dictionary
            snapshot_data_hash: SHA-256 hash of snapshot data
            signature: Signature to verify
            
        Returns:
            bool: True if signature is valid, False otherwise
        """
        try:
            # Recreate signing payload
            signing_payload = {
                "snapshot_id": snapshot_metadata.get("snapshot_id"),
                "user_id": snapshot_metadata.get("user_id"),
                "vm_id": snapshot_metadata.get("vm_id"),
                "created_at": snapshot_metadata.get("created_at"),
                "data_hash": snapshot_data_hash,
                "size_bytes": snapshot_metadata.get("size_bytes"),
                "vm_config_hash": self._hash_vm_config(snapshot_metadata.get("vm_config", {}))
            }
            
            # Create canonical representation
            canonical_payload = json.dumps(signing_payload, sort_keys=True, separators=(',', ':'))
            
            # Verify metadata hash
            expected_metadata_hash = hashlib.sha256(canonical_payload.encode()).hexdigest()
            if expected_metadata_hash != signature.metadata_hash:
                logger.warning("Metadata hash mismatch during signature verification",
                             snapshot_id=snapshot_metadata.get("snapshot_id"),
                             expected=expected_metadata_hash,
                             provided=signature.metadata_hash)
                return False
            
            # Verify HMAC signature
            expected_signature = hmac.new(
                self.signing_key.encode(),
                canonical_payload.encode(),
                hashlib.sha256
            ).hexdigest()
            
            # Use secure comparison to prevent timing attacks
            signature_valid = hmac.compare_digest(expected_signature, signature.signature)
            
            if signature_valid:
                logger.info("Snapshot signature verification successful",
                           snapshot_id=snapshot_metadata.get("snapshot_id"),
                           algorithm=signature.algorithm)
                
                logfire.info("Signature verification passed",
                            snapshot_id=snapshot_metadata.get("snapshot_id"),
                            signature_algorithm=signature.algorithm)
            else:
                logger.warning("Snapshot signature verification failed",
                              snapshot_id=snapshot_metadata.get("snapshot_id"),
                              algorithm=signature.algorithm)
                
                logfire.warning("Signature verification failed",
                               snapshot_id=snapshot_metadata.get("snapshot_id"),
                               reason="HMAC signature mismatch")
            
            return signature_valid
            
        except Exception as e:
            logger.error("Signature verification error",
                        snapshot_id=snapshot_metadata.get("snapshot_id"),
                        error=str(e))
            return False
    
    def _hash_vm_config(self, vm_config: Dict[str, Any]) -> str:
        """Create stable hash of VM configuration."""
        # Create canonical representation of VM config
        canonical_config = json.dumps(vm_config, sort_keys=True, separators=(',', ':'))
        return hashlib.sha256(canonical_config.encode()).hexdigest()
    
    def sign_snapshot_metadata_file(self, metadata_file_path: str) -> str:
        """
        Sign a snapshot metadata file and return signature.
        
        Args:
            metadata_file_path: Path to metadata file
            
        Returns:
            str: Signature string
        """
        try:
            metadata_path = Path(metadata_file_path)
            if not metadata_path.exists():
                raise FileNotFoundError(f"Metadata file not found: {metadata_file_path}")
            
            # Read and hash file contents
            with open(metadata_path, 'rb') as f:
                file_data = f.read()
            
            file_hash = hashlib.sha256(file_data).hexdigest()
            
            # Create signature
            signature = hmac.new(
                self.signing_key.encode(),
                file_data,
                hashlib.sha256
            ).hexdigest()
            
            logger.info("Metadata file signed",
                       file_path=metadata_file_path,
                       file_hash=file_hash,
                       signature_length=len(signature))
            
            return signature
            
        except Exception as e:
            logger.error("Failed to sign metadata file",
                        file_path=metadata_file_path,
                        error=str(e))
            raise
    
    def verify_metadata_file_signature(self, metadata_file_path: str, 
                                     provided_signature: str) -> bool:
        """
        Verify signature of a snapshot metadata file.
        
        Args:
            metadata_file_path: Path to metadata file
            provided_signature: Signature to verify
            
        Returns:
            bool: True if signature is valid, False otherwise
        """
        try:
            metadata_path = Path(metadata_file_path)
            if not metadata_path.exists():
                logger.warning("Metadata file not found for signature verification",
                              file_path=metadata_file_path)
                return False
            
            # Read file and compute expected signature
            with open(metadata_path, 'rb') as f:
                file_data = f.read()
            
            expected_signature = hmac.new(
                self.signing_key.encode(),
                file_data,
                hashlib.sha256
            ).hexdigest()
            
            # Secure comparison
            signature_valid = hmac.compare_digest(expected_signature, provided_signature)
            
            if signature_valid:
                logger.info("Metadata file signature verification successful",
                           file_path=metadata_file_path)
            else:
                logger.warning("Metadata file signature verification failed",
                              file_path=metadata_file_path)
            
            return signature_valid
            
        except Exception as e:
            logger.error("Metadata file signature verification error",
                        file_path=metadata_file_path,
                        error=str(e))
            return False
    
    def create_signature_manifest(self, signatures: Dict[str, SnapshotSignature]) -> Dict[str, Any]:
        """
        Create signature manifest for multiple snapshots.
        
        Args:
            signatures: Dictionary of snapshot_id -> SnapshotSignature
            
        Returns:
            Dict: Signature manifest
        """
        manifest = {
            "version": "1.0",
            "created_at": time.time(),
            "algorithm": self.algorithm,
            "key_id": self.key_id,
            "signatures": {}
        }
        
        for snapshot_id, signature in signatures.items():
            manifest["signatures"][snapshot_id] = {
                "signature": signature.signature,
                "timestamp": signature.timestamp,
                "metadata_hash": signature.metadata_hash
            }
        
        # Sign the manifest itself
        manifest_data = json.dumps(manifest, sort_keys=True, separators=(',', ':'))
        manifest_signature = hmac.new(
            self.signing_key.encode(),
            manifest_data.encode(),
            hashlib.sha256
        ).hexdigest()
        
        manifest["manifest_signature"] = manifest_signature
        
        logger.info("Signature manifest created",
                   signature_count=len(signatures),
                   algorithm=self.algorithm)
        
        return manifest
    
    def verify_signature_manifest(self, manifest: Dict[str, Any]) -> bool:
        """
        Verify signature manifest integrity.
        
        Args:
            manifest: Signature manifest to verify
            
        Returns:
            bool: True if manifest is valid, False otherwise
        """
        try:
            # Extract manifest signature
            provided_signature = manifest.pop("manifest_signature", None)
            if not provided_signature:
                logger.warning("Manifest signature missing")
                return False
            
            # Recreate manifest data for verification
            manifest_data = json.dumps(manifest, sort_keys=True, separators=(',', ':'))
            expected_signature = hmac.new(
                self.signing_key.encode(),
                manifest_data.encode(),
                hashlib.sha256
            ).hexdigest()
            
            # Restore manifest signature
            manifest["manifest_signature"] = provided_signature
            
            # Verify signature
            signature_valid = hmac.compare_digest(expected_signature, provided_signature)
            
            if signature_valid:
                logger.info("Signature manifest verification successful",
                           signature_count=len(manifest.get("signatures", {})))
            else:
                logger.warning("Signature manifest verification failed")
            
            return signature_valid
            
        except Exception as e:
            logger.error("Signature manifest verification error", error=str(e))
            return False
    
    def get_signature_info(self) -> Dict[str, Any]:
        """Get information about the signature service configuration."""
        return {
            "algorithm": self.algorithm,
            "key_id": self.key_id,
            "supported_operations": [
                "sign_snapshot",
                "verify_snapshot_signature", 
                "sign_metadata_file",
                "verify_metadata_file_signature"
            ],
            "security_level": "HMAC-SHA256",
            "timestamp": time.time()
        }


# Global signature service instance
_signature_service: Optional[CryptographicSignatureService] = None


def get_signature_service() -> CryptographicSignatureService:
    """Get global signature service instance."""
    global _signature_service
    if _signature_service is None:
        _signature_service = CryptographicSignatureService()
    return _signature_service


def initialize_signature_service(signing_key: Optional[str] = None) -> CryptographicSignatureService:
    """Initialize global signature service with custom key."""
    global _signature_service
    _signature_service = CryptographicSignatureService(signing_key)
    return _signature_service