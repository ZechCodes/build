"""Production secrets management with multiple backend support."""

import os
import json
import base64
from typing import Optional, Dict, Any, Union
from abc import ABC, abstractmethod
import structlog

logger = structlog.get_logger(__name__)


class SecretBackend(ABC):
    """Abstract base class for secret backends."""
    
    @abstractmethod
    async def get_secret(self, key: str) -> Optional[str]:
        """Get a secret value by key."""
        pass
    
    @abstractmethod
    async def set_secret(self, key: str, value: str) -> bool:
        """Set a secret value."""
        pass
    
    @abstractmethod
    async def delete_secret(self, key: str) -> bool:
        """Delete a secret."""
        pass
    
    @abstractmethod
    async def list_secrets(self) -> list[str]:
        """List all secret keys."""
        pass


class EnvironmentSecretBackend(SecretBackend):
    """Environment variable based secret backend for development."""
    
    async def get_secret(self, key: str) -> Optional[str]:
        """Get secret from environment variable."""
        return os.getenv(key)
    
    async def set_secret(self, key: str, value: str) -> bool:
        """Set environment variable (for development only)."""
        os.environ[key] = value
        return True
    
    async def delete_secret(self, key: str) -> bool:
        """Delete environment variable."""
        if key in os.environ:
            del os.environ[key]
            return True
        return False
    
    async def list_secrets(self) -> list[str]:
        """List environment variables (filtered)."""
        # Only return secret-like environment variables
        secret_keys = []
        secret_patterns = ['_SECRET', '_PASSWORD', '_KEY', '_TOKEN', '_CERT']
        
        for key in os.environ.keys():
            if any(pattern in key.upper() for pattern in secret_patterns):
                secret_keys.append(key)
        
        return secret_keys


class FileSecretBackend(SecretBackend):
    """File-based secret backend for local development."""
    
    def __init__(self, secrets_file: str = "/etc/secrets/secrets.json"):
        self.secrets_file = secrets_file
        self._ensure_secrets_file()
    
    def _ensure_secrets_file(self):
        """Ensure secrets file exists."""
        if not os.path.exists(self.secrets_file):
            os.makedirs(os.path.dirname(self.secrets_file), exist_ok=True)
            with open(self.secrets_file, 'w') as f:
                json.dump({}, f)
    
    def _load_secrets(self) -> Dict[str, str]:
        """Load secrets from file."""
        try:
            with open(self.secrets_file, 'r') as f:
                return json.load(f)
        except (FileNotFoundError, json.JSONDecodeError):
            return {}
    
    def _save_secrets(self, secrets: Dict[str, str]) -> bool:
        """Save secrets to file."""
        try:
            with open(self.secrets_file, 'w') as f:
                json.dump(secrets, f, indent=2)
            os.chmod(self.secrets_file, 0o600)  # Secure file permissions
            return True
        except Exception as e:
            logger.error("Failed to save secrets file", error=str(e))
            return False
    
    async def get_secret(self, key: str) -> Optional[str]:
        """Get secret from file."""
        secrets = self._load_secrets()
        return secrets.get(key)
    
    async def set_secret(self, key: str, value: str) -> bool:
        """Set secret in file."""
        secrets = self._load_secrets()
        secrets[key] = value
        return self._save_secrets(secrets)
    
    async def delete_secret(self, key: str) -> bool:
        """Delete secret from file."""
        secrets = self._load_secrets()
        if key in secrets:
            del secrets[key]
            return self._save_secrets(secrets)
        return False
    
    async def list_secrets(self) -> list[str]:
        """List all secret keys."""
        secrets = self._load_secrets()
        return list(secrets.keys())


class KubernetesSecretBackend(SecretBackend):
    """Kubernetes secret backend for production."""
    
    def __init__(self, namespace: str = "default"):
        self.namespace = namespace
        self._k8s_client = None
    
    async def _get_k8s_client(self):
        """Get Kubernetes client."""
        if self._k8s_client is None:
            try:
                from kubernetes import client, config
                config.load_incluster_config()  # For in-cluster usage
                self._k8s_client = client.CoreV1Api()
            except Exception:
                try:
                    config.load_kube_config()  # For local development
                    self._k8s_client = client.CoreV1Api()
                except Exception as e:
                    logger.error("Failed to load Kubernetes config", error=str(e))
                    raise
        return self._k8s_client
    
    async def get_secret(self, key: str) -> Optional[str]:
        """Get secret from Kubernetes secret."""
        try:
            k8s_client = await self._get_k8s_client()
            secret_name, secret_key = self._parse_key(key)
            
            secret = k8s_client.read_namespaced_secret(
                name=secret_name, 
                namespace=self.namespace
            )
            
            if secret.data and secret_key in secret.data:
                return base64.b64decode(secret.data[secret_key]).decode('utf-8')
            
            return None
        except Exception as e:
            logger.error("Failed to get Kubernetes secret", key=key, error=str(e))
            return None
    
    async def set_secret(self, key: str, value: str) -> bool:
        """Set secret in Kubernetes."""
        try:
            k8s_client = await self._get_k8s_client()
            secret_name, secret_key = self._parse_key(key)
            
            # Encode value to base64
            encoded_value = base64.b64encode(value.encode('utf-8')).decode('utf-8')
            
            # Try to get existing secret
            try:
                secret = k8s_client.read_namespaced_secret(
                    name=secret_name, 
                    namespace=self.namespace
                )
                # Update existing secret
                if not secret.data:
                    secret.data = {}
                secret.data[secret_key] = encoded_value
                
                k8s_client.patch_namespaced_secret(
                    name=secret_name,
                    namespace=self.namespace,
                    body=secret
                )
            except client.ApiException as e:
                if e.status == 404:
                    # Create new secret
                    from kubernetes import client as k8s_client_module
                    secret = k8s_client_module.V1Secret(
                        metadata=k8s_client_module.V1ObjectMeta(name=secret_name),
                        data={secret_key: encoded_value}
                    )
                    k8s_client.create_namespaced_secret(
                        namespace=self.namespace,
                        body=secret
                    )
                else:
                    raise
            
            return True
        except Exception as e:
            logger.error("Failed to set Kubernetes secret", key=key, error=str(e))
            return False
    
    async def delete_secret(self, key: str) -> bool:
        """Delete secret from Kubernetes."""
        try:
            k8s_client = await self._get_k8s_client()
            secret_name, secret_key = self._parse_key(key)
            
            secret = k8s_client.read_namespaced_secret(
                name=secret_name, 
                namespace=self.namespace
            )
            
            if secret.data and secret_key in secret.data:
                del secret.data[secret_key]
                
                if not secret.data:
                    # Delete entire secret if no keys left
                    k8s_client.delete_namespaced_secret(
                        name=secret_name,
                        namespace=self.namespace
                    )
                else:
                    # Update secret with key removed
                    k8s_client.patch_namespaced_secret(
                        name=secret_name,
                        namespace=self.namespace,
                        body=secret
                    )
                
                return True
            
            return False
        except Exception as e:
            logger.error("Failed to delete Kubernetes secret", key=key, error=str(e))
            return False
    
    async def list_secrets(self) -> list[str]:
        """List all secret keys."""
        try:
            k8s_client = await self._get_k8s_client()
            secrets_list = k8s_client.list_namespaced_secret(namespace=self.namespace)
            
            all_keys = []
            for secret in secrets_list.items:
                if secret.data:
                    for key in secret.data.keys():
                        all_keys.append(f"{secret.metadata.name}/{key}")
            
            return all_keys
        except Exception as e:
            logger.error("Failed to list Kubernetes secrets", error=str(e))
            return []
    
    def _parse_key(self, key: str) -> tuple[str, str]:
        """Parse key into secret name and key name."""
        if '/' in key:
            secret_name, secret_key = key.split('/', 1)
        else:
            secret_name = 'build-platform-secrets'
            secret_key = key
        return secret_name, secret_key


class HashiCorpVaultBackend(SecretBackend):
    """HashiCorp Vault secret backend for production."""
    
    def __init__(self, vault_url: str, vault_token: str, mount_point: str = "secret"):
        self.vault_url = vault_url
        self.vault_token = vault_token
        self.mount_point = mount_point
        self._vault_client = None
    
    async def _get_vault_client(self):
        """Get Vault client."""
        if self._vault_client is None:
            try:
                import hvac
                self._vault_client = hvac.Client(
                    url=self.vault_url,
                    token=self.vault_token
                )
                if not self._vault_client.is_authenticated():
                    raise ValueError("Vault authentication failed")
            except Exception as e:
                logger.error("Failed to connect to Vault", error=str(e))
                raise
        return self._vault_client
    
    async def get_secret(self, key: str) -> Optional[str]:
        """Get secret from Vault."""
        try:
            vault_client = await self._get_vault_client()
            response = vault_client.secrets.kv.v2.read_secret_version(
                path=key,
                mount_point=self.mount_point
            )
            
            if response and 'data' in response and 'data' in response['data']:
                return response['data']['data'].get('value')
            
            return None
        except Exception as e:
            logger.error("Failed to get Vault secret", key=key, error=str(e))
            return None
    
    async def set_secret(self, key: str, value: str) -> bool:
        """Set secret in Vault."""
        try:
            vault_client = await self._get_vault_client()
            vault_client.secrets.kv.v2.create_or_update_secret(
                path=key,
                secret={'value': value},
                mount_point=self.mount_point
            )
            return True
        except Exception as e:
            logger.error("Failed to set Vault secret", key=key, error=str(e))
            return False
    
    async def delete_secret(self, key: str) -> bool:
        """Delete secret from Vault."""
        try:
            vault_client = await self._get_vault_client()
            vault_client.secrets.kv.v2.delete_metadata_and_all_versions(
                path=key,
                mount_point=self.mount_point
            )
            return True
        except Exception as e:
            logger.error("Failed to delete Vault secret", key=key, error=str(e))
            return False
    
    async def list_secrets(self) -> list[str]:
        """List all secret keys."""
        try:
            vault_client = await self._get_vault_client()
            response = vault_client.secrets.kv.v2.list_secrets(
                path="",
                mount_point=self.mount_point
            )
            
            if response and 'data' in response and 'keys' in response['data']:
                return response['data']['keys']
            
            return []
        except Exception as e:
            logger.error("Failed to list Vault secrets", error=str(e))
            return []


class SecretManager:
    """Central secret manager with multiple backend support."""
    
    def __init__(self, backend: Optional[SecretBackend] = None):
        self.backend = backend or self._get_default_backend()
        logger.info("Secret manager initialized", backend_type=type(self.backend).__name__)
    
    def _get_default_backend(self) -> SecretBackend:
        """Get default secret backend based on environment."""
        environment = os.getenv("ENVIRONMENT", "development")
        
        if environment == "production":
            # Try Kubernetes first, then Vault, then environment
            vault_url = os.getenv("VAULT_URL")
            vault_token = os.getenv("VAULT_TOKEN")
            
            if vault_url and vault_token:
                logger.info("Using HashiCorp Vault backend")
                return HashiCorpVaultBackend(vault_url, vault_token)
            
            # Try Kubernetes secrets
            try:
                logger.info("Using Kubernetes secrets backend")
                return KubernetesSecretBackend()
            except Exception:
                logger.warning("Kubernetes not available, falling back to environment")
                return EnvironmentSecretBackend()
        
        elif environment == "staging":
            # Use file-based secrets for staging
            logger.info("Using file-based secrets backend")
            return FileSecretBackend()
        
        else:
            # Development - use environment variables
            logger.info("Using environment variable backend")
            return EnvironmentSecretBackend()
    
    async def get_secret(self, key: str, default: Optional[str] = None) -> Optional[str]:
        """Get a secret value."""
        try:
            value = await self.backend.get_secret(key)
            if value is None:
                logger.debug("Secret not found", key=key)
                return default
            return value
        except Exception as e:
            logger.error("Failed to get secret", key=key, error=str(e))
            return default
    
    async def set_secret(self, key: str, value: str) -> bool:
        """Set a secret value."""
        try:
            result = await self.backend.set_secret(key, value)
            if result:
                logger.info("Secret set successfully", key=key)
            else:
                logger.error("Failed to set secret", key=key)
            return result
        except Exception as e:
            logger.error("Failed to set secret", key=key, error=str(e))
            return False
    
    async def delete_secret(self, key: str) -> bool:
        """Delete a secret."""
        try:
            result = await self.backend.delete_secret(key)
            if result:
                logger.info("Secret deleted successfully", key=key)
            else:
                logger.warning("Secret not found for deletion", key=key)
            return result
        except Exception as e:
            logger.error("Failed to delete secret", key=key, error=str(e))
            return False
    
    async def list_secrets(self) -> list[str]:
        """List all secret keys."""
        try:
            return await self.backend.list_secrets()
        except Exception as e:
            logger.error("Failed to list secrets", error=str(e))
            return []
    
    async def rotate_secret(self, key: str, generator_func=None) -> Optional[str]:
        """Rotate a secret value."""
        try:
            if generator_func:
                new_value = generator_func()
            else:
                # Default rotation - generate random string
                import secrets
                import string
                alphabet = string.ascii_letters + string.digits + "!@#$%^&*"
                new_value = ''.join(secrets.choice(alphabet) for _ in range(32))
            
            if await self.set_secret(key, new_value):
                logger.info("Secret rotated successfully", key=key)
                return new_value
            
            return None
        except Exception as e:
            logger.error("Failed to rotate secret", key=key, error=str(e))
            return None


# Global secret manager instance
_secret_manager: Optional[SecretManager] = None


def get_secret_manager() -> SecretManager:
    """Get global secret manager instance."""
    global _secret_manager
    if _secret_manager is None:
        _secret_manager = SecretManager()
    return _secret_manager


async def get_secret(key: str, default: Optional[str] = None) -> Optional[str]:
    """Convenience function to get a secret."""
    manager = get_secret_manager()
    return await manager.get_secret(key, default)


async def set_secret(key: str, value: str) -> bool:
    """Convenience function to set a secret."""
    manager = get_secret_manager()
    return await manager.set_secret(key, value)


# Secret validation and security utilities

def validate_secret_strength(secret: str, min_length: int = 16) -> tuple[bool, str]:
    """Validate secret strength."""
    if len(secret) < min_length:
        return False, f"Secret must be at least {min_length} characters long"
    
    # Check for common weak patterns
    weak_patterns = [
        "password", "123456", "admin", "test", "default", 
        "secret", "key", "token", "changeme"
    ]
    
    secret_lower = secret.lower()
    for pattern in weak_patterns:
        if pattern in secret_lower:
            return False, f"Secret contains weak pattern: {pattern}"
    
    # Check for character diversity
    has_upper = any(c.isupper() for c in secret)
    has_lower = any(c.islower() for c in secret)
    has_digit = any(c.isdigit() for c in secret)
    has_special = any(c in "!@#$%^&*()_+-=[]{}|;:,.<>?" for c in secret)
    
    char_types = sum([has_upper, has_lower, has_digit, has_special])
    if char_types < 3:
        return False, "Secret must contain at least 3 different character types"
    
    return True, "Secret strength is adequate"


def mask_secret(secret: str, visible_chars: int = 4) -> str:
    """Mask a secret for logging purposes."""
    if len(secret) <= visible_chars * 2:
        return "*" * len(secret)
    
    return secret[:visible_chars] + "*" * (len(secret) - visible_chars * 2) + secret[-visible_chars:]