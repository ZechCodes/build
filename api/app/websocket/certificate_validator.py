"""Certificate validation and pinning for WebSocket transport security."""

import ssl
import socket
import hashlib
import base64
from typing import Dict, Any, List, Optional, Tuple
from dataclasses import dataclass
from enum import Enum
import structlog
from urllib.parse import urlparse

logger = structlog.get_logger(__name__)


class CertificateValidationError(Exception):
    """Exception raised when certificate validation fails."""
    pass


class PinningMethod(Enum):
    """Certificate pinning methods."""
    CERTIFICATE = "certificate"      # Pin entire certificate
    PUBLIC_KEY = "public_key"       # Pin public key only
    SUBJECT_HASH = "subject_hash"   # Pin subject name hash
    SPKI_HASH = "spki_hash"        # Pin Subject Public Key Info hash


@dataclass
class CertificatePin:
    """Certificate pin configuration."""
    method: PinningMethod
    value: str
    algorithm: str = "sha256"
    description: str = ""


@dataclass 
class CertificateValidationResult:
    """Result of certificate validation."""
    valid: bool
    certificate_chain: List[Dict[str, Any]]
    pinning_validated: bool
    errors: List[str]
    warnings: List[str]
    certificate_info: Dict[str, Any]
    
    def __post_init__(self):
        if self.errors is None:
            self.errors = []
        if self.warnings is None:
            self.warnings = []


class CertificateValidator:
    """Advanced certificate validator for WebSocket connections."""
    
    def __init__(self):
        # Certificate pinning configuration
        self.pins: Dict[str, List[CertificatePin]] = {}
        self.pinning_enabled = True
        self.backup_pins_required = True  # Require at least 2 pins per host
        
        # Validation settings
        self.verify_hostname = True
        self.verify_chain = True
        self.check_ocsp = False  # OCSP checking disabled by default
        self.allow_self_signed = False  # For development only
        
        # Certificate transparency
        self.require_ct_logs = False
        self.ct_log_count_minimum = 2
        
        # Security settings
        self.minimum_key_size = 2048
        self.allowed_signature_algorithms = [
            'sha256WithRSAEncryption',
            'sha384WithRSAEncryption', 
            'sha512WithRSAEncryption',
            'ecdsa-with-SHA256',
            'ecdsa-with-SHA384',
            'ecdsa-with-SHA512'
        ]
        
        # Weak algorithms to reject
        self.weak_signature_algorithms = [
            'md5WithRSAEncryption',
            'sha1WithRSAEncryption'
        ]
        
        # Certificate age limits
        self.max_certificate_age_days = 365 * 3  # 3 years
        self.warn_expiry_days = 30  # Warn if expiring in 30 days
    
    def add_certificate_pin(self, hostname: str, pin: CertificatePin):
        """Add a certificate pin for a hostname."""
        if hostname not in self.pins:
            self.pins[hostname] = []
        
        self.pins[hostname].append(pin)
        logger.info("Certificate pin added", 
                   hostname=hostname, 
                   method=pin.method.value,
                   algorithm=pin.algorithm)
    
    def add_spki_pin(self, hostname: str, spki_hash: str, description: str = ""):
        """Add a Subject Public Key Info pin (recommended method)."""
        pin = CertificatePin(
            method=PinningMethod.SPKI_HASH,
            value=spki_hash,
            algorithm="sha256",
            description=description
        )
        self.add_certificate_pin(hostname, pin)
    
    def validate_websocket_certificate(self, websocket_url: str, 
                                     certificate_chain: Optional[List[bytes]] = None) -> CertificateValidationResult:
        """
        Validate certificate for WebSocket connection.
        
        Args:
            websocket_url: WebSocket URL (wss://example.com/ws)
            certificate_chain: Optional certificate chain to validate
        
        Returns:
            CertificateValidationResult with validation details
        """
        result = CertificateValidationResult(
            valid=True,
            certificate_chain=[],
            pinning_validated=False,
            errors=[],
            warnings=[],
            certificate_info={}
        )
        
        try:
            # Parse URL to get hostname and port
            parsed_url = urlparse(websocket_url)
            if parsed_url.scheme not in ['wss', 'https']:
                result.errors.append(f"Unsecure protocol: {parsed_url.scheme}")
                result.valid = False
                return result
            
            hostname = parsed_url.hostname
            port = parsed_url.port or (443 if parsed_url.scheme == 'wss' else 80)
            
            if not hostname:
                result.errors.append("No hostname found in URL")
                result.valid = False
                return result
            
            # Get certificate chain if not provided
            if certificate_chain is None:
                certificate_chain = self._get_certificate_chain(hostname, port)
            
            if not certificate_chain:
                result.errors.append("No certificate chain available")
                result.valid = False
                return result
            
            # Parse certificates
            certificates = []
            for cert_der in certificate_chain:
                try:
                    cert_info = self._parse_certificate(cert_der)
                    certificates.append(cert_info)
                except Exception as e:
                    result.errors.append(f"Failed to parse certificate: {e}")
                    result.valid = False
                    continue
            
            result.certificate_chain = certificates
            
            if not certificates:
                result.errors.append("No valid certificates in chain")
                result.valid = False
                return result
            
            # Validate leaf certificate
            leaf_cert = certificates[0]
            result.certificate_info = leaf_cert
            
            # Hostname verification
            if self.verify_hostname:
                if not self._verify_hostname(hostname, leaf_cert):
                    result.errors.append(f"Hostname verification failed for {hostname}")
                    result.valid = False
            
            # Certificate chain validation
            if self.verify_chain:
                chain_valid, chain_errors = self._validate_certificate_chain(certificates)
                if not chain_valid:
                    result.errors.extend(chain_errors)
                    result.valid = False
            
            # Security validation
            security_valid, security_errors, security_warnings = self._validate_certificate_security(leaf_cert)
            if not security_valid:
                result.errors.extend(security_errors)
                result.valid = False
            result.warnings.extend(security_warnings)
            
            # Certificate pinning validation
            if self.pinning_enabled and hostname in self.pins:
                pinning_valid, pinning_errors = self._validate_certificate_pinning(
                    hostname, certificates
                )
                result.pinning_validated = pinning_valid
                if not pinning_valid:
                    result.errors.extend(pinning_errors)
                    result.valid = False
            else:
                result.pinning_validated = True  # No pins configured
            
            # Certificate age and expiry warnings
            self._check_certificate_expiry(leaf_cert, result)
            
            logger.info("Certificate validation completed",
                       hostname=hostname,
                       valid=result.valid,
                       pinning_validated=result.pinning_validated,
                       errors=len(result.errors),
                       warnings=len(result.warnings))
            
            return result
            
        except Exception as e:
            logger.error("Certificate validation error", error=str(e))
            result.errors.append(f"Validation error: {e}")
            result.valid = False
            return result
    
    def _get_certificate_chain(self, hostname: str, port: int) -> List[bytes]:
        """Retrieve certificate chain from server."""
        try:
            # Create SSL context
            context = ssl.create_default_context()
            context.check_hostname = False
            context.verify_mode = ssl.CERT_NONE
            
            # Connect and get certificate chain
            with socket.create_connection((hostname, port), timeout=10) as sock:
                with context.wrap_socket(sock, server_hostname=hostname) as ssock:
                    cert_chain = ssock.getpeercert_chain()
                    if cert_chain:
                        return [cert.public_bytes(encoding=ssl.Encoding.DER) for cert in cert_chain]
            
        except Exception as e:
            logger.error("Failed to retrieve certificate chain", 
                        hostname=hostname, port=port, error=str(e))
        
        return []
    
    def _parse_certificate(self, cert_der: bytes) -> Dict[str, Any]:
        """Parse DER certificate into structured information."""
        try:
            import cryptography.x509 as x509
            from cryptography.hazmat.primitives import hashes, serialization
            
            cert = x509.load_der_x509_certificate(cert_der)
            
            # Extract certificate information
            cert_info = {
                "subject": cert.subject.rfc4514_string(),
                "issuer": cert.issuer.rfc4514_string(),
                "serial_number": str(cert.serial_number),
                "not_valid_before": cert.not_valid_before.isoformat(),
                "not_valid_after": cert.not_valid_after.isoformat(),
                "signature_algorithm": cert.signature_algorithm_oid._name,
                "version": cert.version.name,
                "der_bytes": cert_der,
                "pem_bytes": cert.public_bytes(encoding=serialization.Encoding.PEM)
            }
            
            # Extract public key information
            public_key = cert.public_key()
            cert_info["public_key_algorithm"] = public_key.__class__.__name__
            
            # Get key size
            if hasattr(public_key, 'key_size'):
                cert_info["key_size"] = public_key.key_size
            
            # Extract Subject Alternative Names
            try:
                san_ext = cert.extensions.get_extension_for_oid(x509.ExtensionOID.SUBJECT_ALTERNATIVE_NAME)
                san_names = []
                for name in san_ext.value:
                    if isinstance(name, x509.DNSName):
                        san_names.append(f"DNS:{name.value}")
                    elif isinstance(name, x509.IPAddress):
                        san_names.append(f"IP:{name.value}")
                cert_info["subject_alt_names"] = san_names
            except x509.ExtensionNotFound:
                cert_info["subject_alt_names"] = []
            
            # Calculate SPKI hash for pinning
            spki_der = public_key.public_bytes(
                encoding=serialization.Encoding.DER,
                format=serialization.PublicFormat.SubjectPublicKeyInfo
            )
            cert_info["spki_hash"] = hashlib.sha256(spki_der).digest()
            cert_info["spki_hash_b64"] = base64.b64encode(cert_info["spki_hash"]).decode('ascii')
            
            # Calculate certificate hash
            cert_info["cert_hash"] = hashlib.sha256(cert_der).digest()
            cert_info["cert_hash_b64"] = base64.b64encode(cert_info["cert_hash"]).decode('ascii')
            
            return cert_info
            
        except ImportError:
            # Fallback to basic SSL certificate parsing
            logger.warning("cryptography library not available, using basic parsing")
            return {
                "der_bytes": cert_der,
                "cert_hash": hashlib.sha256(cert_der).digest(),
                "cert_hash_b64": base64.b64encode(hashlib.sha256(cert_der).digest()).decode('ascii'),
                "parse_method": "basic"
            }
        except Exception as e:
            raise CertificateValidationError(f"Failed to parse certificate: {e}")
    
    def _verify_hostname(self, hostname: str, cert_info: Dict[str, Any]) -> bool:
        """Verify that certificate is valid for hostname."""
        try:
            # Check subject CN
            subject = cert_info.get("subject", "")
            if f"CN={hostname}" in subject:
                return True
            
            # Check Subject Alternative Names
            san_names = cert_info.get("subject_alt_names", [])
            for san in san_names:
                if san == f"DNS:{hostname}":
                    return True
                # Check wildcard matching
                if san.startswith("DNS:*.") and hostname.endswith(san[6:]):
                    return True
            
            return False
            
        except Exception as e:
            logger.error("Hostname verification error", error=str(e))
            return False
    
    def _validate_certificate_chain(self, certificates: List[Dict[str, Any]]) -> Tuple[bool, List[str]]:
        """Validate certificate chain integrity."""
        errors = []
        
        if len(certificates) < 1:
            errors.append("Empty certificate chain")
            return False, errors
        
        # Basic chain validation (simplified)
        for i, cert in enumerate(certificates):
            # Check certificate validity period
            try:
                from datetime import datetime
                not_before = datetime.fromisoformat(cert["not_valid_before"].replace('Z', '+00:00'))
                not_after = datetime.fromisoformat(cert["not_valid_after"].replace('Z', '+00:00'))
                now = datetime.now(not_before.tzinfo)
                
                if now < not_before:
                    errors.append(f"Certificate {i} not yet valid")
                if now > not_after:
                    errors.append(f"Certificate {i} has expired")
                    
            except Exception as e:
                errors.append(f"Certificate {i} date validation error: {e}")
        
        return len(errors) == 0, errors
    
    def _validate_certificate_security(self, cert_info: Dict[str, Any]) -> Tuple[bool, List[str], List[str]]:
        """Validate certificate security properties."""
        errors = []
        warnings = []
        
        # Check signature algorithm
        sig_alg = cert_info.get("signature_algorithm", "")
        if sig_alg in self.weak_signature_algorithms:
            errors.append(f"Weak signature algorithm: {sig_alg}")
        elif sig_alg not in self.allowed_signature_algorithms:
            warnings.append(f"Unknown signature algorithm: {sig_alg}")
        
        # Check key size
        key_size = cert_info.get("key_size")
        if key_size and key_size < self.minimum_key_size:
            errors.append(f"Key size too small: {key_size} < {self.minimum_key_size}")
        
        # Check for self-signed certificates
        subject = cert_info.get("subject", "")
        issuer = cert_info.get("issuer", "")
        if subject == issuer and not self.allow_self_signed:
            errors.append("Self-signed certificate not allowed")
        
        return len(errors) == 0, errors, warnings
    
    def _validate_certificate_pinning(self, hostname: str, 
                                    certificates: List[Dict[str, Any]]) -> Tuple[bool, List[str]]:
        """Validate certificate against configured pins."""
        errors = []
        pins = self.pins.get(hostname, [])
        
        if not pins:
            return True, []  # No pins configured
        
        # Check backup pin requirement
        if self.backup_pins_required and len(pins) < 2:
            errors.append(f"Insufficient backup pins for {hostname} (minimum 2 required)")
        
        # Validate each pin
        pin_matches = 0
        for pin in pins:
            if self._check_pin_match(pin, certificates):
                pin_matches += 1
                break
        
        if pin_matches == 0:
            errors.append(f"No certificate pins match for {hostname}")
            return False, errors
        
        return True, []
    
    def _check_pin_match(self, pin: CertificatePin, certificates: List[Dict[str, Any]]) -> bool:
        """Check if a pin matches any certificate in the chain."""
        for cert in certificates:
            try:
                if pin.method == PinningMethod.SPKI_HASH:
                    # Compare SPKI hash
                    cert_spki_hash = cert.get("spki_hash_b64", "")
                    if cert_spki_hash == pin.value:
                        return True
                
                elif pin.method == PinningMethod.CERTIFICATE:
                    # Compare full certificate hash
                    cert_hash = cert.get("cert_hash_b64", "")
                    if cert_hash == pin.value:
                        return True
                
                elif pin.method == PinningMethod.SUBJECT_HASH:
                    # Compare subject hash
                    subject = cert.get("subject", "")
                    subject_hash = base64.b64encode(
                        hashlib.sha256(subject.encode('utf-8')).digest()
                    ).decode('ascii')
                    if subject_hash == pin.value:
                        return True
                        
            except Exception as e:
                logger.error("Pin matching error", pin_method=pin.method.value, error=str(e))
        
        return False
    
    def _check_certificate_expiry(self, cert_info: Dict[str, Any], result: CertificateValidationResult):
        """Check certificate expiry and add warnings."""
        try:
            from datetime import datetime, timedelta
            
            not_after = datetime.fromisoformat(cert_info["not_valid_after"].replace('Z', '+00:00'))
            now = datetime.now(not_after.tzinfo)
            
            days_until_expiry = (not_after - now).days
            
            if days_until_expiry <= 0:
                result.errors.append("Certificate has expired")
            elif days_until_expiry <= self.warn_expiry_days:
                result.warnings.append(f"Certificate expires in {days_until_expiry} days")
            
            # Check certificate age
            not_before = datetime.fromisoformat(cert_info["not_valid_before"].replace('Z', '+00:00'))
            certificate_age_days = (now - not_before).days
            
            if certificate_age_days > self.max_certificate_age_days:
                result.warnings.append(f"Certificate is {certificate_age_days} days old (very old)")
                
        except Exception as e:
            result.warnings.append(f"Expiry check error: {e}")
    
    def get_validation_config(self) -> Dict[str, Any]:
        """Get current validation configuration."""
        return {
            "pinning_enabled": self.pinning_enabled,
            "verify_hostname": self.verify_hostname,
            "verify_chain": self.verify_chain,
            "allow_self_signed": self.allow_self_signed,
            "minimum_key_size": self.minimum_key_size,
            "pinned_hosts": list(self.pins.keys()),
            "pin_count_per_host": {host: len(pins) for host, pins in self.pins.items()},
            "weak_algorithms_blocked": self.weak_signature_algorithms,
            "allowed_algorithms": self.allowed_signature_algorithms
        }


# Global instance
_certificate_validator = None


def get_certificate_validator() -> CertificateValidator:
    """Get global certificate validator instance."""
    global _certificate_validator
    if _certificate_validator is None:
        _certificate_validator = CertificateValidator()
        logger.info("Certificate validator initialized")
    return _certificate_validator