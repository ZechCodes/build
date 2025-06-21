# Data Protection Enhancements

**Priority**: Medium  
**Effort**: High  
**Timeline**: 2-3 Sprints  
**Security Impact**: High for sensitive data handling

## Overview

Data protection enhancements focus on securing sensitive information within terminal sessions through encryption, filtering, and sanitization. These features prevent accidental exposure of passwords, API keys, and other sensitive data that may appear in terminal output.

## Moved from Session 6

The following data protection requirements were identified during Session 6 development but moved to future implementation due to their complexity and specialized nature:

### 1. Buffer Data Encryption in Storage
- **Original requirement**: "Buffer data encrypted in Redis storage"
- **Current status**: Session buffers stored as compressed but unencrypted data
- **Security gap**: Terminal data readable if Redis is compromised

### 2. Sensitive Data Filtering 
- **Original requirement**: "Sensitive data filtering in terminal output"
- **Current status**: No content filtering or redaction applied
- **Security gap**: Passwords and secrets may be stored in session buffers

### 3. Recovery Data Sanitization
- **Original requirement**: "Recovery data sanitization for sensitive information" 
- **Current status**: Recovery transmits raw session data
- **Security gap**: Sensitive data exposed during session recovery

## Technical Implementation

### Buffer Data Encryption

**Objective**: Encrypt all session buffer data before storing in Redis to protect against data breaches.

**Implementation Approach**:

```python
# Enhanced Buffer Manager with Encryption
from cryptography.fernet import Fernet
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC
import base64
import os

class SessionBufferEncryption:
    def __init__(self, encryption_key: str = None, salt: bytes = None):
        self.salt = salt or os.urandom(16)
        self.encryption_key = encryption_key or os.getenv('BUFFER_ENCRYPTION_KEY')
        self.cipher_suite = self._create_cipher_suite()
    
    def _create_cipher_suite(self) -> Fernet:
        """Create Fernet cipher suite from password and salt"""
        if not self.encryption_key:
            raise ValueError("Buffer encryption key not provided")
        
        # Derive key from password using PBKDF2
        kdf = PBKDF2HMAC(
            algorithm=hashes.SHA256(),
            length=32,
            salt=self.salt,
            iterations=100000,
        )
        key = base64.urlsafe_b64encode(kdf.derive(self.encryption_key.encode()))
        return Fernet(key)
    
    def encrypt_buffer_data(self, data: bytes) -> bytes:
        """Encrypt buffer data for storage"""
        try:
            encrypted_data = self.cipher_suite.encrypt(data)
            logger.debug("Buffer data encrypted", 
                        original_size=len(data),
                        encrypted_size=len(encrypted_data))
            return encrypted_data
        except Exception as e:
            logger.error("Buffer encryption failed", error=str(e))
            raise
    
    def decrypt_buffer_data(self, encrypted_data: bytes) -> bytes:
        """Decrypt buffer data for retrieval"""
        try:
            decrypted_data = self.cipher_suite.decrypt(encrypted_data)
            logger.debug("Buffer data decrypted",
                        encrypted_size=len(encrypted_data), 
                        decrypted_size=len(decrypted_data))
            return decrypted_data
        except Exception as e:
            logger.error("Buffer decryption failed", error=str(e))
            raise

# Enhanced SessionBufferManager with encryption
class SessionBufferManager:
    def __init__(self, redis_client: redis.Redis, max_buffer_size: int = 1024 * 1024,
                 encryption_key: str = None):
        # ... existing initialization
        self.encryption = SessionBufferEncryption(encryption_key) if encryption_key else None
        self.encrypt_sensitive_data = bool(encryption_key)
    
    async def store_buffer(self, session_id: str, user_id: str, 
                          buffer_data: bytes, cursor_pos: Tuple[int, int],
                          scroll_pos: int = 0) -> bool:
        """Enhanced store_buffer with encryption support"""
        try:
            # ... existing rate limiting and validation
            
            # Apply sensitive data filtering BEFORE encryption
            filtered_data = self._filter_sensitive_data(buffer_data)
            
            # Encrypt buffer data if encryption is enabled
            store_data = filtered_data
            encrypted = False
            
            if self.encrypt_sensitive_data:
                store_data = self.encryption.encrypt_buffer_data(filtered_data)
                encrypted = True
                logger.debug("Buffer encrypted before storage", session_id=session_id)
            
            # Apply compression after encryption (encrypted data compresses poorly)
            compressed = False
            if len(store_data) > self.compression_threshold and not encrypted:
                store_data = gzip.compress(store_data)
                compressed = True
            
            # ... existing Redis storage logic with encrypted flag
            redis_data = {
                "user_id": user_id,
                "buffer_data": store_data,
                "cursor_x": cursor_pos[0],
                "cursor_y": cursor_pos[1],
                "scroll_position": scroll_pos,
                "last_updated": time.time(),
                "size_bytes": len(filtered_data),  # Original size
                "line_count": filtered_data.count(b'\\n'),
                "compressed": str(compressed).lower(),
                "encrypted": str(encrypted).lower()
            }
            
            # ... existing storage implementation
            
        except Exception as e:
            logger.error("Failed to store encrypted buffer", 
                        session_id=session_id, error=str(e))
            return False
    
    async def retrieve_buffer(self, session_id: str, user_id: str) -> Optional[SessionBuffer]:
        """Enhanced retrieve_buffer with decryption support"""
        try:
            # ... existing retrieval logic
            
            buffer_data = redis_data[b"buffer_data"]
            compressed = redis_data.get(b"compressed", b"false") == b"true"
            encrypted = redis_data.get(b"encrypted", b"false") == b"true"
            
            # Decrypt data if encrypted
            if encrypted and self.encrypt_sensitive_data:
                try:
                    buffer_data = self.encryption.decrypt_buffer_data(buffer_data)
                    logger.debug("Buffer decrypted after retrieval", session_id=session_id)
                except Exception as e:
                    logger.error("Buffer decryption failed", 
                               session_id=session_id, error=str(e))
                    return None
            
            # Decompress if needed (after decryption)
            if compressed and not encrypted:
                try:
                    buffer_data = gzip.decompress(buffer_data)
                except Exception as e:
                    logger.error("Buffer decompression failed", 
                               session_id=session_id, error=str(e))
                    return None
            
            # ... existing SessionBuffer creation
            
        except Exception as e:
            logger.error("Failed to retrieve encrypted buffer", 
                        session_id=session_id, error=str(e))
            return None
```

**Security Benefits**:
- Protects session data at rest in Redis
- Uses industry-standard AES encryption via Fernet
- Key derivation with PBKDF2 for password-based keys
- Encryption metadata tracked for proper decryption

### Sensitive Data Filtering

**Objective**: Automatically detect and redact common sensitive patterns in terminal output.

**Implementation Approach**:

```python
# Sensitive Data Filter
import re
from typing import List, Dict, Pattern
from dataclasses import dataclass

@dataclass
class SensitivePattern:
    name: str
    pattern: Pattern[str]
    replacement: str
    severity: str  # 'high', 'medium', 'low'

class SensitiveDataFilter:
    def __init__(self, custom_patterns: List[SensitivePattern] = None):
        self.patterns = self._load_default_patterns()
        if custom_patterns:
            self.patterns.extend(custom_patterns)
        
        # Statistics tracking
        self.filter_stats = {}
    
    def _load_default_patterns(self) -> List[SensitivePattern]:
        """Load default sensitive data patterns"""
        return [
            # Password patterns
            SensitivePattern(
                name="password_assignment",
                pattern=re.compile(r'(password\s*[:=]\s*)["\']?([^"\'\s]+)["\']?', re.IGNORECASE),
                replacement=r'\\1[REDACTED]',
                severity="high"
            ),
            SensitivePattern(
                name="password_flag",
                pattern=re.compile(r'(-p\s+|--password\s+)["\']?([^"\'\s]+)["\']?', re.IGNORECASE),
                replacement=r'\\1[REDACTED]',
                severity="high"
            ),
            
            # API Keys and tokens
            SensitivePattern(
                name="api_key",
                pattern=re.compile(r'(api[_-]?key\s*[:=]\s*)["\']?([a-zA-Z0-9_-]{20,})["\']?', re.IGNORECASE),
                replacement=r'\\1[REDACTED]',
                severity="high"
            ),
            SensitivePattern(
                name="bearer_token",
                pattern=re.compile(r'(bearer\s+)([a-zA-Z0-9_.-]{20,})', re.IGNORECASE),
                replacement=r'\\1[REDACTED]',
                severity="high"
            ),
            SensitivePattern(
                name="jwt_token",
                pattern=re.compile(r'(eyJ[a-zA-Z0-9_.-]+)', re.IGNORECASE),
                replacement=r'[JWT_REDACTED]',
                severity="high"
            ),
            
            # SSH and certificates
            SensitivePattern(
                name="ssh_private_key",
                pattern=re.compile(r'(-----BEGIN [A-Z ]+PRIVATE KEY-----.*?-----END [A-Z ]+PRIVATE KEY-----)', re.DOTALL),
                replacement=r'[SSH_PRIVATE_KEY_REDACTED]',
                severity="high"
            ),
            
            # Database URLs
            SensitivePattern(
                name="database_url",
                pattern=re.compile(r'(postgresql://|mysql://|mongodb://)([^:]+):([^@]+)@', re.IGNORECASE),
                replacement=r'\\1\\2:[REDACTED]@',
                severity="medium"
            ),
            
            # Credit card numbers
            SensitivePattern(
                name="credit_card",
                pattern=re.compile(r'\\b(?:\\d{4}[-\\s]?){3}\\d{4}\\b'),
                replacement=r'[CARD_REDACTED]',
                severity="high"
            ),
            
            # Social Security Numbers
            SensitivePattern(
                name="ssn",
                pattern=re.compile(r'\\b\\d{3}-\\d{2}-\\d{4}\\b'),
                replacement=r'[SSN_REDACTED]',
                severity="high"
            ),
            
            # Email addresses (configurable)
            SensitivePattern(
                name="email_address",
                pattern=re.compile(r'\\b[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\\.[a-zA-Z]{2,}\\b'),
                replacement=r'[EMAIL_REDACTED]',
                severity="low"
            ),
            
            # AWS Access Keys
            SensitivePattern(
                name="aws_access_key",
                pattern=re.compile(r'\\bAKIA[0-9A-Z]{16}\\b'),
                replacement=r'[AWS_ACCESS_KEY_REDACTED]',
                severity="high"
            ),
            
            # GitHub tokens
            SensitivePattern(
                name="github_token",
                pattern=re.compile(r'\\bgh[pousr]_[A-Za-z0-9_]{36,255}\\b'),
                replacement=r'[GITHUB_TOKEN_REDACTED]',
                severity="high"
            )
        ]
    
    def filter_buffer_data(self, data: bytes) -> bytes:
        """Filter sensitive data from buffer content"""
        try:
            # Convert to text for pattern matching
            text = data.decode('utf-8', errors='replace')
            original_text = text
            
            # Apply each pattern
            matches_found = {}
            for pattern in self.patterns:
                matches = pattern.pattern.findall(text)
                if matches:
                    text = pattern.pattern.sub(pattern.replacement, text)
                    matches_found[pattern.name] = len(matches)
                    
                    # Update statistics
                    if pattern.name not in self.filter_stats:
                        self.filter_stats[pattern.name] = 0
                    self.filter_stats[pattern.name] += len(matches)
            
            # Log filtering activity
            if matches_found:
                logger.info("Sensitive data filtered from buffer",
                           patterns_matched=matches_found,
                           severity_levels=[p.severity for p in self.patterns 
                                          if p.name in matches_found])
                
                # Log high-severity detections for security monitoring
                high_severity_matches = [name for name, count in matches_found.items()
                                       if any(p.severity == 'high' and p.name == name 
                                            for p in self.patterns)]
                if high_severity_matches:
                    logfire.warning("High-severity sensitive data detected and filtered",
                                  patterns=high_severity_matches,
                                  match_count=sum(matches_found[name] for name in high_severity_matches))
            
            return text.encode('utf-8')
            
        except Exception as e:
            logger.error("Sensitive data filtering failed", error=str(e))
            # Fail securely - return empty buffer rather than potentially exposing data
            return b"[CONTENT_FILTERING_ERROR]"
    
    def add_custom_pattern(self, pattern: SensitivePattern):
        """Add custom sensitive data pattern"""
        self.patterns.append(pattern)
        logger.info("Custom sensitive data pattern added", 
                   pattern_name=pattern.name, severity=pattern.severity)
    
    def get_filter_statistics(self) -> Dict[str, int]:
        """Get filtering statistics for monitoring"""
        return self.filter_stats.copy()

# Integration with SessionBufferManager
class SessionBufferManager:
    def __init__(self, redis_client: redis.Redis, max_buffer_size: int = 1024 * 1024,
                 encryption_key: str = None, enable_data_filtering: bool = True):
        # ... existing initialization
        self.data_filter = SensitiveDataFilter() if enable_data_filtering else None
        self.enable_data_filtering = enable_data_filtering
    
    def _filter_sensitive_data(self, buffer_data: bytes) -> bytes:
        """Apply sensitive data filtering if enabled"""
        if not self.enable_data_filtering or not self.data_filter:
            return buffer_data
        
        try:
            return self.data_filter.filter_buffer_data(buffer_data)
        except Exception as e:
            logger.error("Data filtering failed", error=str(e))
            # Fail securely - return filtered data or empty buffer
            return b"[FILTERING_ERROR]"
```

**Security Benefits**:
- Prevents accidental storage of passwords and API keys
- Configurable patterns for different sensitivity levels
- Comprehensive coverage of common sensitive data types
- Statistics tracking for security monitoring and compliance

### Recovery Data Sanitization

**Objective**: Apply additional sanitization to data transmitted during session recovery.

**Implementation Approach**:

```python
# Enhanced Recovery Manager with Data Sanitization
class RecoveryDataSanitizer:
    def __init__(self, sanitization_level: str = "standard"):
        self.sanitization_level = sanitization_level  # 'strict', 'standard', 'minimal'
        self.recovery_filter = SensitiveDataFilter()
        
        # Add recovery-specific patterns
        self._add_recovery_patterns()
    
    def _add_recovery_patterns(self):
        """Add patterns specific to recovery data sanitization"""
        recovery_patterns = [
            # Command history with sensitive flags
            SensitivePattern(
                name="command_with_password",
                pattern=re.compile(r'^(.*(?:mysql|psql|ssh).*-p\s*)[^\\s]+(.*)$', re.MULTILINE),
                replacement=r'\\1[REDACTED]\\2',
                severity="high"
            ),
            
            # Environment variable exports
            SensitivePattern(
                name="env_var_secret",
                pattern=re.compile(r'(export\\s+\\w*(?:SECRET|KEY|TOKEN|PASSWORD)\\w*=)[^\\s]+', re.IGNORECASE),
                replacement=r'\\1[REDACTED]',
                severity="high"
            ),
            
            # File paths with potential sensitive info
            SensitivePattern(
                name="sensitive_file_path",
                pattern=re.compile(r'(/[^\\s]*(?:ssh|key|cert|password|secret)[^\\s]*)', re.IGNORECASE),
                replacement=r'[PATH_REDACTED]',
                severity="medium"
            )
        ]
        
        for pattern in recovery_patterns:
            self.recovery_filter.add_custom_pattern(pattern)
    
    def sanitize_recovery_data(self, buffer_data: SessionBuffer) -> SessionBuffer:
        """Sanitize buffer data for recovery transmission"""
        try:
            # Apply different sanitization levels
            if self.sanitization_level == "minimal":
                # Only filter high-severity patterns
                high_severity_filter = SensitiveDataFilter([
                    p for p in self.recovery_filter.patterns 
                    if p.severity == "high"
                ])
                sanitized_data = high_severity_filter.filter_buffer_data(buffer_data.buffer_data)
            
            elif self.sanitization_level == "strict":
                # Filter all patterns including low-severity ones
                sanitized_data = self.recovery_filter.filter_buffer_data(buffer_data.buffer_data)
                
                # Additional strict measures
                sanitized_data = self._apply_strict_sanitization(sanitized_data)
            
            else:  # standard
                # Filter high and medium severity patterns
                standard_filter = SensitiveDataFilter([
                    p for p in self.recovery_filter.patterns 
                    if p.severity in ["high", "medium"]
                ])
                sanitized_data = standard_filter.filter_buffer_data(buffer_data.buffer_data)
            
            # Create sanitized buffer copy
            sanitized_buffer = SessionBuffer(
                session_id=buffer_data.session_id,
                user_id=buffer_data.user_id,
                buffer_data=sanitized_data,
                cursor_position=buffer_data.cursor_position,
                scroll_position=buffer_data.scroll_position,
                last_updated=buffer_data.last_updated,
                size_bytes=len(sanitized_data),
                line_count=sanitized_data.count(b'\\n')
            )
            
            logger.info("Recovery data sanitized",
                       session_id=buffer_data.session_id,
                       original_size=len(buffer_data.buffer_data),
                       sanitized_size=len(sanitized_data),
                       sanitization_level=self.sanitization_level)
            
            return sanitized_buffer
            
        except Exception as e:
            logger.error("Recovery data sanitization failed", 
                        session_id=buffer_data.session_id, error=str(e))
            # Return empty buffer for security
            return SessionBuffer(
                session_id=buffer_data.session_id,
                user_id=buffer_data.user_id,
                buffer_data=b"[SANITIZATION_ERROR]",
                cursor_position=(0, 0),
                scroll_position=0,
                last_updated=time.time(),
                size_bytes=20,
                line_count=1
            )
    
    def _apply_strict_sanitization(self, data: bytes) -> bytes:
        """Apply additional strict sanitization measures"""
        try:
            text = data.decode('utf-8', errors='replace')
            
            # Remove command history entirely in strict mode
            lines = text.split('\\n')
            sanitized_lines = []
            
            for line in lines:
                # Skip lines that look like command prompts with sensitive commands
                if any(cmd in line.lower() for cmd in ['password', 'secret', 'key', 'token']):
                    sanitized_lines.append('[COMMAND_REDACTED]')
                else:
                    sanitized_lines.append(line)
            
            return '\\n'.join(sanitized_lines).encode('utf-8')
            
        except Exception as e:
            logger.error("Strict sanitization failed", error=str(e))
            return b"[STRICT_SANITIZATION_ERROR]"

# Enhanced Recovery Manager
class RecoveryManager:
    def __init__(self, session_manager, buffer_manager, websocket_bridge,
                 sanitization_level: str = "standard"):
        # ... existing initialization
        self.data_sanitizer = RecoveryDataSanitizer(sanitization_level)
    
    async def _perform_recovery(self, recovery_info: RecoveryInfo):
        """Enhanced recovery with data sanitization"""
        try:
            # ... existing recovery logic until buffer retrieval
            
            buffer_data = await self.buffer_manager.retrieve_buffer(session_id, user_id)
            if not buffer_data:
                # ... existing error handling
                return False
            
            # Apply sanitization before transmission
            sanitized_buffer = self.data_sanitizer.sanitize_recovery_data(buffer_data)
            
            # Validate sanitized data integrity
            if not await self._validate_recovery_data(sanitized_buffer):
                logger.error("Sanitized recovery data integrity check failed", 
                           session_id=session_id)
                await self._send_recovery_error(connection_id, "Data sanitization validation failed")
                return False
            
            # Prepare recovery message with sanitized data
            recovery_message = {
                "type": "session_recovery",
                "session_id": session_id,
                "recovery_data": {
                    "buffer": sanitized_buffer.buffer_data.decode('utf-8', errors='replace'),
                    "cursor_position": sanitized_buffer.cursor_position,
                    "scroll_position": sanitized_buffer.scroll_position,
                    "last_activity": recovery_info.last_seen_timestamp,
                    "recovery_timestamp": recovery_info.recovery_start_time,
                    "buffer_metadata": {
                        "size_bytes": sanitized_buffer.size_bytes,
                        "line_count": sanitized_buffer.line_count,
                        "last_updated": sanitized_buffer.last_updated,
                        "sanitized": True  # Indicate data has been sanitized
                    }
                }
            }
            
            # ... existing recovery transmission logic
            
        except Exception as e:
            logger.error("Enhanced recovery with sanitization failed", 
                        session_id=recovery_info.session_id, error=str(e))
            return False
```

**Security Benefits**:
- Prevents sensitive data exposure during recovery
- Configurable sanitization levels for different environments
- Recovery-specific pattern detection
- Maintains session functionality while protecting sensitive data

## Configuration and Integration

### Environment Configuration

```python
# Data protection configuration
@dataclass
class DataProtectionConfig:
    # Encryption settings
    buffer_encryption_enabled: bool = True
    encryption_key: str = None
    key_rotation_interval_days: int = 90
    
    # Filtering settings
    sensitive_data_filtering_enabled: bool = True
    filtering_patterns_file: str = None
    custom_patterns: List[SensitivePattern] = None
    
    # Recovery sanitization
    recovery_sanitization_level: str = "standard"  # minimal, standard, strict
    sanitize_command_history: bool = True
    sanitize_file_paths: bool = True
    
    # Compliance settings
    log_filtering_activity: bool = True
    export_filter_statistics: bool = True
    data_retention_policy_days: int = 30

# Integration with existing Session 6 components
def create_enhanced_session_manager(config: DataProtectionConfig):
    # Create enhanced buffer manager with data protection
    buffer_manager = SessionBufferManager(
        redis_client=redis_client,
        encryption_key=config.encryption_key,
        enable_data_filtering=config.sensitive_data_filtering_enabled
    )
    
    # Create enhanced recovery manager with sanitization
    recovery_manager = RecoveryManager(
        session_manager=session_manager,
        buffer_manager=buffer_manager,
        websocket_bridge=websocket_bridge,
        sanitization_level=config.recovery_sanitization_level
    )
    
    return session_manager, buffer_manager, recovery_manager
```

### Key Management

```python
# Encryption key management
class EncryptionKeyManager:
    def __init__(self, key_store_path: str = "/secure/keys"):
        self.key_store_path = key_store_path
        self.current_key_id = None
        self.keys = {}
    
    def generate_new_key(self) -> str:
        """Generate new encryption key"""
        key = Fernet.generate_key()
        key_id = f"key_{int(time.time())}"
        
        # Store key securely
        key_file = os.path.join(self.key_store_path, f"{key_id}.key")
        with open(key_file, 'wb') as f:
            f.write(key)
        
        # Set appropriate permissions
        os.chmod(key_file, 0o600)
        
        self.keys[key_id] = key
        self.current_key_id = key_id
        
        logger.info("New encryption key generated", key_id=key_id)
        return key_id
    
    def rotate_keys(self) -> str:
        """Rotate encryption keys"""
        old_key_id = self.current_key_id
        new_key_id = self.generate_new_key()
        
        logger.info("Encryption key rotated", 
                   old_key_id=old_key_id, new_key_id=new_key_id)
        
        # Schedule re-encryption of existing data
        asyncio.create_task(self._re_encrypt_existing_data(old_key_id, new_key_id))
        
        return new_key_id
    
    async def _re_encrypt_existing_data(self, old_key_id: str, new_key_id: str):
        """Re-encrypt existing data with new key (background task)"""
        try:
            # Implementation for re-encrypting existing session buffers
            # This would iterate through Redis keys and re-encrypt with new key
            pass
        except Exception as e:
            logger.error("Key rotation re-encryption failed", error=str(e))
```

## Testing and Validation

### Comprehensive Testing Strategy

```python
async def test_data_protection_features():
    """Comprehensive data protection testing"""
    
    # Test encryption functionality
    await test_buffer_encryption_decryption()
    await test_encryption_key_rotation()
    await test_encryption_error_handling()
    
    # Test sensitive data filtering
    await test_sensitive_pattern_detection()
    await test_custom_pattern_addition()
    await test_filtering_statistics()
    
    # Test recovery sanitization
    await test_recovery_data_sanitization()
    await test_sanitization_levels()
    await test_sanitization_error_handling()

async def test_sensitive_pattern_detection():
    """Test detection of various sensitive data patterns"""
    filter = SensitiveDataFilter()
    
    # Test password detection
    test_data = b"mysql -u user -p secretpassword123 -h localhost"
    filtered = filter.filter_buffer_data(test_data)
    assert b"secretpassword123" not in filtered
    assert b"[REDACTED]" in filtered
    
    # Test API key detection
    test_data = b"export API_KEY=ak_test_51234567890abcdef"
    filtered = filter.filter_buffer_data(test_data)
    assert b"ak_test_51234567890abcdef" not in filtered
    
    # Test JWT token detection
    test_data = b"Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
    filtered = filter.filter_buffer_data(test_data)
    assert b"eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9" not in filtered

async def test_buffer_encryption_decryption():
    """Test buffer encryption and decryption"""
    encryption = SessionBufferEncryption("test_key_123")
    
    original_data = b"sensitive session data with passwords"
    
    # Test encryption
    encrypted_data = encryption.encrypt_buffer_data(original_data)
    assert encrypted_data != original_data
    assert len(encrypted_data) > len(original_data)  # Encryption adds overhead
    
    # Test decryption
    decrypted_data = encryption.decrypt_buffer_data(encrypted_data)
    assert decrypted_data == original_data

async def test_recovery_data_sanitization():
    """Test recovery data sanitization at different levels"""
    sanitizer = RecoveryDataSanitizer("standard")
    
    # Create test buffer with sensitive data
    test_buffer = SessionBuffer(
        session_id="test_session",
        user_id="test_user",
        buffer_data=b"mysql -u root -p password123\\necho $SECRET_TOKEN\\n",
        cursor_position=(0, 0),
        scroll_position=0,
        last_updated=time.time(),
        size_bytes=50,
        line_count=2
    )
    
    # Test sanitization
    sanitized = sanitizer.sanitize_recovery_data(test_buffer)
    
    assert b"password123" not in sanitized.buffer_data
    assert b"SECRET_TOKEN" not in sanitized.buffer_data
    assert b"[REDACTED]" in sanitized.buffer_data
```

### Performance and Security Benchmarks

```python
async def benchmark_data_protection():
    """Benchmark data protection performance impact"""
    
    # Encryption benchmarks
    encryption = SessionBufferEncryption("test_key")
    test_data = b"x" * 1024 * 100  # 100KB test data
    
    start_time = time.time()
    encrypted = encryption.encrypt_buffer_data(test_data)
    encrypt_time = time.time() - start_time
    
    start_time = time.time()
    decrypted = encryption.decrypt_buffer_data(encrypted)
    decrypt_time = time.time() - start_time
    
    # Filtering benchmarks
    filter = SensitiveDataFilter()
    start_time = time.time()
    filtered = filter.filter_buffer_data(test_data)
    filter_time = time.time() - start_time
    
    print(f"Encryption: {encrypt_time*1000:.2f}ms")
    print(f"Decryption: {decrypt_time*1000:.2f}ms") 
    print(f"Filtering: {filter_time*1000:.2f}ms")
    
    # Verify acceptable performance thresholds
    assert encrypt_time < 0.1  # <100ms for 100KB
    assert decrypt_time < 0.1
    assert filter_time < 0.05  # <50ms for filtering
```

## Compliance and Monitoring

### Security Monitoring

```python
# Data protection monitoring
class DataProtectionMonitor:
    def __init__(self):
        self.metrics = {
            'encryption_operations': 0,
            'decryption_operations': 0,
            'filtering_detections': {},
            'sanitization_operations': 0,
            'key_rotations': 0
        }
    
    def record_encryption(self, data_size: int, duration: float):
        """Record encryption operation metrics"""
        self.metrics['encryption_operations'] += 1
        logfire.info("Buffer encryption performed",
                    data_size=data_size,
                    duration_ms=duration * 1000)
    
    def record_filtering_detection(self, pattern_name: str, severity: str):
        """Record sensitive data detection"""
        if pattern_name not in self.metrics['filtering_detections']:
            self.metrics['filtering_detections'][pattern_name] = 0
        self.metrics['filtering_detections'][pattern_name] += 1
        
        # Alert on high-severity detections
        if severity == 'high':
            logfire.warning("High-severity sensitive data detected",
                          pattern=pattern_name,
                          total_detections=self.metrics['filtering_detections'][pattern_name])
    
    def generate_compliance_report(self) -> Dict[str, Any]:
        """Generate compliance report for auditing"""
        return {
            'timestamp': time.time(),
            'encryption_enabled': True,
            'filtering_enabled': True,
            'sanitization_enabled': True,
            'operations_summary': self.metrics,
            'key_rotation_status': 'current',
            'compliance_status': 'compliant'
        }
```

### Audit Logging

```python
# Comprehensive audit logging for data protection
def log_data_protection_event(event_type: str, **kwargs):
    """Log data protection events for audit trail"""
    audit_event = {
        'timestamp': time.time(),
        'event_type': event_type,
        'component': 'data_protection',
        **kwargs
    }
    
    # Log to both application logs and audit system
    logger.info("Data protection audit event", **audit_event)
    logfire.info("Data protection audit", **audit_event)
    
    # Store in audit database for compliance
    # Implementation would depend on audit storage system

# Usage examples
log_data_protection_event('encryption_enabled', session_id='sess_123')
log_data_protection_event('sensitive_data_filtered', 
                         pattern='password_assignment', 
                         session_id='sess_123')
log_data_protection_event('recovery_data_sanitized',
                         sanitization_level='standard',
                         session_id='sess_123')
```

## Success Criteria

### Functional Requirements
- ✅ All session buffer data encrypted at rest using AES-256
- ✅ Automatic detection and redaction of 15+ sensitive data patterns
- ✅ Configurable sanitization levels for recovery data
- ✅ Key rotation and management capabilities
- ✅ Zero breaking changes to existing Session 6 functionality

### Security Requirements
- ✅ Buffer data unreadable without encryption keys
- ✅ Sensitive patterns filtered with >95% accuracy
- ✅ Recovery data sanitized based on configurable policies
- ✅ Comprehensive audit logging for all data protection operations
- ✅ Secure key storage and rotation procedures

### Performance Requirements
- ✅ <100ms encryption/decryption overhead for typical buffer sizes
- ✅ <50ms filtering overhead for sensitive data detection
- ✅ <10% overall performance impact on session operations
- ✅ Graceful degradation if data protection services fail

### Compliance Requirements
- ✅ GDPR Article 32 - Security of processing (encryption)
- ✅ SOC 2 Type II - Data encryption and access controls
- ✅ PCI DSS - Protection of cardholder data (if applicable)
- ✅ HIPAA - Technical safeguards for PHI (if applicable)

This data protection implementation provides comprehensive security for sensitive information in terminal sessions while maintaining full compatibility with the existing Session 6 architecture.