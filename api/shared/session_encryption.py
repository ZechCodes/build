"""
Session Data Encryption for secure session persistence.

Provides AES-256-GCM encryption for session data including:
- Session state
- Terminal history
- Environment variables
- User preferences
"""

import os
import json
import base64
import hashlib
from datetime import datetime, timedelta
from typing import Dict, Any, Optional, Tuple
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.backends import default_backend
from pydantic import BaseModel, Field


class EncryptedSessionData(BaseModel):
    """Encrypted session data container."""
    encrypted_data: str = Field(..., description="Base64 encoded encrypted data")
    nonce: str = Field(..., description="Base64 encoded nonce")
    salt: str = Field(..., description="Base64 encoded salt")
    key_id: str = Field(..., description="Key identifier for rotation")
    created_at: datetime = Field(default_factory=datetime.utcnow)
    expires_at: Optional[datetime] = None


class SessionData(BaseModel):
    """Decrypted session data structure."""
    session_id: str
    user_id: str
    vm_id: str
    terminal_state: Dict[str, Any] = Field(default_factory=dict)
    environment_vars: Dict[str, str] = Field(default_factory=dict)
    working_directory: str = "/"
    shell: str = "/bin/bash"
    terminal_history: List[str] = Field(default_factory=list)
    user_preferences: Dict[str, Any] = Field(default_factory=dict)
    last_activity: datetime = Field(default_factory=datetime.utcnow)
    created_at: datetime = Field(default_factory=datetime.utcnow)


class SessionEncryption:
    """Session data encryption manager."""
    
    def __init__(self, master_key: Optional[str] = None):
        """Initialize with master key."""
        self.master_key = master_key or os.environ.get("SESSION_MASTER_KEY")
        if not self.master_key:
            raise ValueError("Master key not provided")
        
        self.key_rotation_interval = timedelta(hours=24)
        self.max_history_size = 10000  # lines
        
    def _derive_session_key(self, session_id: str, salt: bytes) -> bytes:
        """Derive session-specific encryption key."""
        key_material = f"{self.master_key}:{session_id}".encode()
        
        kdf = PBKDF2HMAC(
            algorithm=hashes.SHA256(),
            length=32,  # 256 bits for AES-256
            salt=salt,
            iterations=100000,
            backend=default_backend()
        )
        
        return kdf.derive(key_material)
    
    def _generate_key_id(self, session_id: str, timestamp: datetime) -> str:
        """Generate unique key identifier for rotation tracking."""
        data = f"{session_id}:{timestamp.isoformat()}"
        return hashlib.sha256(data.encode()).hexdigest()[:16]
    
    def encrypt_session_data(self, session_data: SessionData) -> EncryptedSessionData:
        """Encrypt session data with AES-256-GCM."""
        # Generate random salt and nonce
        salt = os.urandom(16)
        nonce = os.urandom(12)  # 96 bits for GCM
        
        # Derive session-specific key
        session_key = self._derive_session_key(session_data.session_id, salt)
        
        # Prepare data for encryption
        data_dict = session_data.model_dump()
        
        # Limit terminal history size
        if len(data_dict.get("terminal_history", [])) > self.max_history_size:
            data_dict["terminal_history"] = data_dict["terminal_history"][-self.max_history_size:]
        
        # Serialize to JSON
        plaintext = json.dumps(data_dict, default=str).encode()
        
        # Encrypt with AES-GCM
        aesgcm = AESGCM(session_key)
        ciphertext = aesgcm.encrypt(nonce, plaintext, None)
        
        # Generate key ID
        key_id = self._generate_key_id(session_data.session_id, datetime.utcnow())
        
        # Set expiration
        expires_at = datetime.utcnow() + self.key_rotation_interval
        
        return EncryptedSessionData(
            encrypted_data=base64.b64encode(ciphertext).decode(),
            nonce=base64.b64encode(nonce).decode(),
            salt=base64.b64encode(salt).decode(),
            key_id=key_id,
            expires_at=expires_at
        )
    
    def decrypt_session_data(self, encrypted_data: EncryptedSessionData) -> SessionData:
        """Decrypt session data."""
        try:
            # Decode base64 components
            ciphertext = base64.b64decode(encrypted_data.encrypted_data)
            nonce = base64.b64decode(encrypted_data.nonce)
            salt = base64.b64decode(encrypted_data.salt)
            
            # Check if data has expired
            if encrypted_data.expires_at and datetime.utcnow() > encrypted_data.expires_at:
                raise ValueError("Encrypted session data has expired")
            
            # We need the session_id to derive the key, but it's in the encrypted data
            # This is a chicken-and-egg problem. In practice, we'd store session_id separately
            # or derive it from the storage key. For now, we'll try different approaches:
            
            # Try to extract session_id from context or use a brute force approach
            # In production, session_id would be passed separately or stored unencrypted
            raise NotImplementedError("Session ID derivation strategy needed")
            
        except Exception as e:
            raise ValueError(f"Failed to decrypt session data: {str(e)}")
    
    def decrypt_session_data_with_id(self, encrypted_data: EncryptedSessionData, session_id: str) -> SessionData:
        """Decrypt session data with known session ID."""
        try:
            # Decode base64 components
            ciphertext = base64.b64decode(encrypted_data.encrypted_data)
            nonce = base64.b64decode(encrypted_data.nonce)
            salt = base64.b64decode(encrypted_data.salt)
            
            # Check if data has expired
            if encrypted_data.expires_at and datetime.utcnow() > encrypted_data.expires_at:
                raise ValueError("Encrypted session data has expired")
            
            # Derive session-specific key
            session_key = self._derive_session_key(session_id, salt)
            
            # Decrypt with AES-GCM
            aesgcm = AESGCM(session_key)
            plaintext = aesgcm.decrypt(nonce, ciphertext, None)
            
            # Parse JSON
            data_dict = json.loads(plaintext.decode())
            
            return SessionData(**data_dict)
            
        except Exception as e:
            raise ValueError(f"Failed to decrypt session data: {str(e)}")
    
    def rotate_session_key(self, session_data: SessionData) -> EncryptedSessionData:
        """Rotate encryption key for session."""
        # Update last activity
        session_data.last_activity = datetime.utcnow()
        
        # Re-encrypt with new salt/nonce (effectively rotating the key)
        return self.encrypt_session_data(session_data)
    
    def is_key_rotation_needed(self, encrypted_data: EncryptedSessionData) -> bool:
        """Check if key rotation is needed."""
        if not encrypted_data.expires_at:
            return True
        
        # Rotate if within 1 hour of expiration
        time_until_expiry = encrypted_data.expires_at - datetime.utcnow()
        return time_until_expiry < timedelta(hours=1)


class SessionEncryptionManager:
    """High-level session encryption management."""
    
    def __init__(self, redis_client, master_key: str = None):
        """Initialize with Redis client for key storage."""
        self.redis = redis_client
        self.encryption = SessionEncryption(master_key)
        self.key_prefix = "session:encrypted:"
        
    async def store_session(self, session_data: SessionData, ttl: int = 86400) -> str:
        """Store encrypted session data in Redis."""
        # Encrypt the session data
        encrypted = self.encryption.encrypt_session_data(session_data)
        
        # Store in Redis with TTL
        key = f"{self.key_prefix}{session_data.session_id}"
        await self.redis.setex(
            key,
            ttl,
            encrypted.model_dump_json()
        )
        
        return encrypted.key_id
    
    async def retrieve_session(self, session_id: str) -> Optional[SessionData]:
        """Retrieve and decrypt session data from Redis."""
        key = f"{self.key_prefix}{session_id}"
        encrypted_json = await self.redis.get(key)
        
        if not encrypted_json:
            return None
        
        try:
            encrypted_data = EncryptedSessionData.model_validate_json(encrypted_json)
            return self.encryption.decrypt_session_data_with_id(encrypted_data, session_id)
        except Exception:
            # Data may be corrupted, remove it
            await self.redis.delete(key)
            return None
    
    async def update_session(self, session_data: SessionData) -> str:
        """Update session data with potential key rotation."""
        # Get current encrypted data to check if rotation is needed
        key = f"{self.key_prefix}{session_data.session_id}"
        current_encrypted_json = await self.redis.get(key)
        
        should_rotate = True
        if current_encrypted_json:
            try:
                current_encrypted = EncryptedSessionData.model_validate_json(current_encrypted_json)
                should_rotate = self.encryption.is_key_rotation_needed(current_encrypted)
            except Exception:
                pass  # If we can't parse it, rotate anyway
        
        if should_rotate:
            # Rotate key by re-encrypting
            encrypted = self.encryption.rotate_session_key(session_data)
        else:
            # Just re-encrypt with current parameters
            encrypted = self.encryption.encrypt_session_data(session_data)
        
        # Store updated data
        await self.redis.setex(
            key,
            86400,  # 24 hours TTL
            encrypted.model_dump_json()
        )
        
        return encrypted.key_id
    
    async def delete_session(self, session_id: str) -> bool:
        """Delete encrypted session data."""
        key = f"{self.key_prefix}{session_id}"
        result = await self.redis.delete(key)
        return result > 0
    
    async def cleanup_expired_sessions(self) -> int:
        """Clean up expired session data."""
        pattern = f"{self.key_prefix}*"
        keys = await self.redis.keys(pattern)
        
        deleted_count = 0
        for key in keys:
            try:
                encrypted_json = await self.redis.get(key)
                if encrypted_json:
                    encrypted_data = EncryptedSessionData.model_validate_json(encrypted_json)
                    if (encrypted_data.expires_at and 
                        datetime.utcnow() > encrypted_data.expires_at):
                        await self.redis.delete(key)
                        deleted_count += 1
            except Exception:
                # If we can't parse it, delete it
                await self.redis.delete(key)
                deleted_count += 1
        
        return deleted_count


# Configuration for session encryption
class SessionEncryptionConfig:
    """Configuration for session encryption."""
    
    # Key management
    KEY_ROTATION_INTERVAL_HOURS = 24
    KEY_DERIVATION_ITERATIONS = 100000
    
    # Data limits
    MAX_TERMINAL_HISTORY_LINES = 10000
    MAX_ENVIRONMENT_VARS = 1000
    MAX_SESSION_DATA_SIZE_MB = 10
    
    # Security
    ENCRYPTION_ALGORITHM = "AES-256-GCM"
    KEY_SIZE_BITS = 256
    NONCE_SIZE_BITS = 96
    SALT_SIZE_BITS = 128
    
    # Storage
    DEFAULT_TTL_SECONDS = 86400  # 24 hours
    CLEANUP_INTERVAL_HOURS = 6


def generate_master_key() -> str:
    """Generate a new master key for session encryption."""
    return base64.b64encode(os.urandom(32)).decode()


# Example usage and testing functions
async def example_usage():
    """Example of how to use session encryption."""
    import asyncio
    import redis.asyncio as redis
    
    # Initialize Redis client
    redis_client = redis.Redis(host='localhost', port=6379, decode_responses=False)
    
    # Initialize encryption manager
    master_key = generate_master_key()
    manager = SessionEncryptionManager(redis_client, master_key)
    
    # Create sample session data
    session_data = SessionData(
        session_id="sess_123",
        user_id="user_456",
        vm_id="vm_789",
        terminal_state={"cursor_pos": [10, 5], "scroll_pos": 100},
        environment_vars={"PATH": "/usr/bin", "HOME": "/home/user"},
        working_directory="/home/user/project",
        terminal_history=["ls -la", "cd project", "vim main.py"],
        user_preferences={"theme": "dark", "font_size": 14}
    )
    
    # Store encrypted session
    key_id = await manager.store_session(session_data)
    print(f"Stored session with key ID: {key_id}")
    
    # Retrieve and decrypt session
    retrieved_data = await manager.retrieve_session("sess_123")
    if retrieved_data:
        print(f"Retrieved session for user: {retrieved_data.user_id}")
    
    await redis_client.close()


if __name__ == "__main__":
    asyncio.run(example_usage())