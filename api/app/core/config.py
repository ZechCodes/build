"""Application configuration with secrets management."""

import asyncio
from functools import lru_cache
from typing import List, Optional

from pydantic import Field, validator
from pydantic_settings import BaseSettings
import structlog

logger = structlog.get_logger(__name__)


class Settings(BaseSettings):
    """Application settings."""

    # Environment
    environment: str = "development"
    debug: bool = False
    log_level: str = "INFO"

    # API Configuration
    api_host: str = "0.0.0.0"
    api_port: int = 8000
    api_workers: int = 1

    # Database
    database_url: str = Field(..., env="DATABASE_URL")
    database_echo: bool = False

    # Redis
    redis_url: str = Field(..., env="REDIS_URL")
    redis_password: str = Field(..., env="REDIS_PASSWORD")
    redis_max_connections: int = 20
    redis_socket_timeout: float = 5.0
    redis_socket_connect_timeout: float = 5.0
    redis_health_check_interval: int = 30

    # JWT
    jwt_secret: str = Field(..., env="JWT_SECRET")
    jwt_algorithm: str = "HS256"
    jwt_access_token_expire_minutes: int = 15
    jwt_refresh_token_expire_days: int = 7

    # MinIO
    minio_endpoint: str = Field(..., env="MINIO_ENDPOINT")
    minio_access_key: str = Field(..., env="MINIO_ACCESS_KEY")
    minio_secret_key: str = Field(..., env="MINIO_SECRET_KEY")
    minio_bucket: str = "build-dev"
    minio_secure: bool = False
    
    # Secrets Management
    secrets_backend: str = Field("environment", env="SECRETS_BACKEND")
    vault_url: Optional[str] = Field(None, env="VAULT_URL")
    vault_token: Optional[str] = Field(None, env="VAULT_TOKEN")
    secrets_file_path: str = Field("/etc/secrets/secrets.json", env="SECRETS_FILE_PATH")
    k8s_namespace: str = Field("default", env="K8S_NAMESPACE")

    # CORS
    allowed_origins: List[str] = ["http://localhost:3000"]
    enable_cors: bool = True

    # Features
    enable_swagger_ui: bool = True
    enable_redoc: bool = True
    enable_debug_toolbar: bool = False

    # Security
    bcrypt_rounds: int = 12
    rate_limit_per_minute: int = 60
    rate_limit_per_hour: int = 1000
    
    # WebSocket Security
    websocket_allowed_origins: List[str] = ["http://localhost:3000", "http://127.0.0.1:3000"]
    websocket_require_origin: bool = True
    websocket_max_message_size: int = 1024 * 1024  # 1MB
    websocket_replay_window: int = 300  # 5 minutes
    websocket_connection_token_lifetime: int = 3600  # 1 hour
    websocket_require_wss_production: bool = True

    # VM Configuration
    vm_max_count_per_user: int = 5
    vm_default_cpu: int = 1
    vm_default_memory: int = 512
    vm_default_disk: int = 2048

    # Session Configuration
    session_timeout_minutes: int = 60
    session_max_idle_minutes: int = 30

    @validator("allowed_origins", pre=True)
    def parse_cors_origins(cls, v):
        """Parse CORS origins from string or list."""
        if isinstance(v, str):
            return [i.strip() for i in v.split(",")]
        return v
    
    @validator("websocket_allowed_origins", pre=True)
    def parse_websocket_origins(cls, v):
        """Parse WebSocket origins from string or list."""
        if isinstance(v, str):
            return [i.strip() for i in v.split(",")]
        return v

    @validator("environment")
    def validate_environment(cls, v):
        """Validate environment value."""
        if v not in ["development", "staging", "production"]:
            raise ValueError("Environment must be one of: development, staging, production")
        return v
    
    @validator("secrets_backend")
    def validate_secrets_backend(cls, v):
        """Validate secrets backend."""
        valid_backends = ["environment", "file", "kubernetes", "vault"]
        if v not in valid_backends:
            raise ValueError(f"Secrets backend must be one of: {valid_backends}")
        return v

    class Config:
        """Pydantic configuration."""
        env_file = ".env.development"
        env_file_encoding = "utf-8"
        case_sensitive = False
        extra = "ignore"  # Ignore extra fields from environment
    
    async def get_secret(self, key: str, default: Optional[str] = None) -> Optional[str]:
        """Get secret using configured backend."""
        try:
            from .secrets import get_secret_manager
            manager = get_secret_manager()
            return await manager.get_secret(key, default)
        except Exception as e:
            logger.error("Failed to get secret from manager", key=key, error=str(e))
            # Fallback to environment variable
            import os
            return os.getenv(key, default)
    
    def get_secret_sync(self, key: str, default: Optional[str] = None) -> Optional[str]:
        """Get secret synchronously (for initialization)."""
        try:
            return asyncio.run(self.get_secret(key, default))
        except Exception as e:
            logger.warning("Failed to get secret async, using env fallback", key=key, error=str(e))
            import os
            return os.getenv(key, default)


@lru_cache()
def get_settings() -> Settings:
    """Get cached settings instance."""
    settings = Settings()
    
    # Validate critical secrets on startup
    critical_secrets = [
        "DATABASE_URL", "REDIS_PASSWORD", "JWT_SECRET", 
        "MINIO_ACCESS_KEY", "MINIO_SECRET_KEY"
    ]
    
    missing_secrets = []
    for secret_key in critical_secrets:
        value = getattr(settings, secret_key.lower().replace('_', '_'), None)
        if not value:
            missing_secrets.append(secret_key)
    
    if missing_secrets:
        logger.error("Critical secrets missing", missing_secrets=missing_secrets)
        if settings.environment == "production":
            raise ValueError(f"Critical secrets missing in production: {missing_secrets}")
        else:
            logger.warning("Running with missing secrets in non-production environment")
    
    return settings