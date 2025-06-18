"""Application configuration."""

from functools import lru_cache
from typing import List

from pydantic import Field, validator
from pydantic_settings import BaseSettings


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

    @validator("environment")
    def validate_environment(cls, v):
        """Validate environment value."""
        if v not in ["development", "staging", "production"]:
            raise ValueError("Environment must be one of: development, staging, production")
        return v

    class Config:
        """Pydantic configuration."""
        env_file = ".env.development"
        env_file_encoding = "utf-8"
        case_sensitive = False


@lru_cache()
def get_settings() -> Settings:
    """Get cached settings instance."""
    return Settings()