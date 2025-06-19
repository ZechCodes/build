# Configuration Reference

This document provides comprehensive configuration reference for all components in Session 1 of the Build Platform.

## Configuration Overview

The Build Platform uses environment-based configuration with the following hierarchy:

1. **Default Values** - Hardcoded defaults in application
2. **Configuration Files** - YAML/JSON configuration files
3. **Environment Variables** - Runtime environment configuration
4. **Command Line Arguments** - Override specific settings
5. **Runtime Settings** - Dynamic configuration via API (future)

## Environment Variables

### Core Application Settings

```bash
# Environment
ENVIRONMENT=development|staging|production
DEBUG=true|false
LOG_LEVEL=DEBUG|INFO|WARNING|ERROR|CRITICAL

# Service Information
SERVICE_NAME=build-api
SERVICE_VERSION=1.0.0
API_HOST=0.0.0.0
API_PORT=8000
```

### Database Configuration

```bash
# PostgreSQL Settings
DATABASE_URL=postgresql://user:password@host:port/database
DATABASE_HOST=localhost
DATABASE_PORT=5432
DATABASE_NAME=build_dev
DATABASE_USER=postgres
DATABASE_PASSWORD=your_password

# Connection Pool Settings
DATABASE_POOL_SIZE=20
DATABASE_MAX_OVERFLOW=30
DATABASE_POOL_TIMEOUT=30
DATABASE_POOL_RECYCLE=3600

# Database Features
DATABASE_ECHO=false
DATABASE_SSL_MODE=prefer
DATABASE_CONNECT_TIMEOUT=60
```

### Redis Configuration

```bash
# Redis Connection
REDIS_URL=redis://:password@host:port/db
REDIS_HOST=localhost
REDIS_PORT=6379
REDIS_PASSWORD=your_redis_password
REDIS_DB=0

# Redis Features
REDIS_SSL=false
REDIS_SOCKET_TIMEOUT=5
REDIS_SOCKET_CONNECT_TIMEOUT=5
REDIS_RETRY_ON_TIMEOUT=true
REDIS_HEALTH_CHECK_INTERVAL=30

# Redis Memory Management
REDIS_MAX_MEMORY=2gb
REDIS_MAX_MEMORY_POLICY=allkeys-lru
```

### Security Configuration

```bash
# JWT Settings
JWT_SECRET=your-super-secret-jwt-key
JWT_ALGORITHM=HS256
JWT_ACCESS_TOKEN_EXPIRE_MINUTES=15
JWT_REFRESH_TOKEN_EXPIRE_DAYS=7

# Password Security
BCRYPT_ROUNDS=12
PASSWORD_MIN_LENGTH=12

# Session Management
SESSION_EXPIRE_HOURS=24
SESSION_REFRESH_THRESHOLD_HOURS=6

# Rate Limiting
RATE_LIMIT_PER_MINUTE=60
RATE_LIMIT_BURST=20
RATE_LIMIT_AUTH_PER_MINUTE=10
```

### CORS and Security Headers

```bash
# CORS Configuration
CORS_ENABLED=true
ALLOWED_ORIGINS=["http://localhost:3000", "https://getbuild.ing"]
ALLOWED_METHODS=["GET", "POST", "PUT", "DELETE", "OPTIONS"]
ALLOWED_HEADERS=["*"]
ALLOW_CREDENTIALS=true

# Security Headers
ENABLE_SECURITY_HEADERS=true
HSTS_MAX_AGE=31536000
CSP_POLICY=default-src 'self'
```

### Monitoring and Logging

```bash
# Logfire Configuration
LOGFIRE_ENABLED=true
LOGFIRE_TOKEN=your-logfire-token
LOGFIRE_PROJECT_NAME=build-platform
LOGFIRE_SEND_TO_LOGFIRE=true
LOGFIRE_CONSOLE=true

# Structured Logging
LOG_FORMAT=json|text
LOG_FILE_PATH=/app/logs/api.log
LOG_MAX_SIZE=10MB
LOG_BACKUP_COUNT=5

# Metrics
PROMETHEUS_ENABLED=true
METRICS_ENDPOINT=/metrics
```

### External Services

```bash
# MinIO S3 Storage (Future)
MINIO_ENDPOINT=localhost:9000
MINIO_ACCESS_KEY=minioadmin
MINIO_SECRET_KEY=minioadmin123
MINIO_BUCKET=build-dev
MINIO_SECURE=false

# Git Server (Future)
GIT_SERVER_HOST=localhost
GIT_SERVER_PORT=23231
GIT_SERVER_HTTP_PORT=23232

# Email Service (Future)
SMTP_HOST=smtp.example.com
SMTP_PORT=587
SMTP_USERNAME=noreply@getbuild.ing
SMTP_PASSWORD=your_smtp_password
SMTP_USE_TLS=true
```

## Configuration Files

### Application Configuration

**File**: `api/config/settings.py`

```python
from pydantic import BaseSettings, validator
from typing import List, Optional
import os

class Settings(BaseSettings):
    """Application settings with validation and defaults."""
    
    # Environment
    environment: str = "development"
    debug: bool = False
    log_level: str = "INFO"
    
    # API Configuration
    api_host: str = "0.0.0.0"
    api_port: int = 8000
    api_prefix: str = "/api/v1"
    
    # Database
    database_url: str
    database_echo: bool = False
    database_pool_size: int = 20
    database_max_overflow: int = 30
    database_pool_timeout: int = 30
    database_pool_recycle: int = 3600
    
    # Redis
    redis_url: str
    redis_socket_timeout: int = 5
    redis_socket_connect_timeout: int = 5
    redis_retry_on_timeout: bool = True
    redis_health_check_interval: int = 30
    
    # Security
    jwt_secret: str
    jwt_algorithm: str = "HS256"
    jwt_access_token_expire_minutes: int = 15
    jwt_refresh_token_expire_days: int = 7
    bcrypt_rounds: int = 12
    
    # CORS
    cors_enabled: bool = True
    allowed_origins: List[str] = ["http://localhost:3000"]
    allowed_methods: List[str] = ["GET", "POST", "PUT", "DELETE", "OPTIONS"]
    allowed_headers: List[str] = ["*"]
    allow_credentials: bool = True
    
    # Rate Limiting
    rate_limit_per_minute: int = 60
    rate_limit_burst: int = 20
    rate_limit_auth_per_minute: int = 10
    
    # Monitoring
    logfire_enabled: bool = True
    logfire_token: Optional[str] = None
    logfire_project_name: str = "build-platform"
    prometheus_enabled: bool = True
    
    # Features
    enable_swagger_ui: bool = True
    enable_redoc: bool = True
    enable_security_headers: bool = True
    
    @validator('allowed_origins', pre=True)
    def parse_cors_origins(cls, v):
        if isinstance(v, str):
            return [origin.strip() for origin in v.split(',')]
        return v
    
    @validator('environment')
    def validate_environment(cls, v):
        if v not in ['development', 'staging', 'production']:
            raise ValueError('Environment must be development, staging, or production')
        return v
    
    @validator('log_level')
    def validate_log_level(cls, v):
        valid_levels = ['DEBUG', 'INFO', 'WARNING', 'ERROR', 'CRITICAL']
        if v.upper() not in valid_levels:
            raise ValueError(f'Log level must be one of: {valid_levels}')
        return v.upper()
    
    @property
    def is_development(self) -> bool:
        return self.environment == "development"
    
    @property
    def is_production(self) -> bool:
        return self.environment == "production"
    
    class Config:
        env_file = ".env"
        env_file_encoding = "utf-8"
        case_sensitive = False

# Global settings instance
settings = Settings()
```

### Database Configuration

**File**: `api/config/database.py`

```python
from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker
from sqlalchemy.pool import NullPool, QueuePool
from .settings import settings
import ssl

class DatabaseConfig:
    """Database configuration and connection management."""
    
    def __init__(self):
        self.database_url = self._build_database_url()
        self.engine_kwargs = self._build_engine_kwargs()
        self.session_kwargs = self._build_session_kwargs()
    
    def _build_database_url(self) -> str:
        """Build database URL with proper SSL configuration."""
        url = settings.database_url
        
        # Add SSL mode if not present
        if "sslmode" not in url and settings.environment == "production":
            separator = "&" if "?" in url else "?"
            url += f"{separator}sslmode=require"
        
        return url
    
    def _build_engine_kwargs(self) -> dict:
        """Build engine configuration based on environment."""
        kwargs = {
            "echo": settings.database_echo and settings.is_development,
            "pool_size": settings.database_pool_size,
            "max_overflow": settings.database_max_overflow,
            "pool_timeout": settings.database_pool_timeout,
            "pool_recycle": settings.database_pool_recycle,
        }
        
        # Production-specific settings
        if settings.is_production:
            kwargs.update({
                "poolclass": QueuePool,
                "pool_pre_ping": True,
                "connect_args": {
                    "ssl": self._create_ssl_context(),
                    "command_timeout": 60
                }
            })
        else:
            # Development settings
            kwargs.update({
                "connect_args": {
                    "command_timeout": 30
                }
            })
        
        return kwargs
    
    def _build_session_kwargs(self) -> dict:
        """Build session configuration."""
        return {
            "class_": AsyncSession,
            "expire_on_commit": False,
            "autoflush": True,
            "autocommit": False
        }
    
    def _create_ssl_context(self):
        """Create SSL context for database connections."""
        context = ssl.create_default_context()
        if settings.is_development:
            context.check_hostname = False
            context.verify_mode = ssl.CERT_NONE
        return context
    
    def create_engine(self):
        """Create database engine."""
        return create_async_engine(self.database_url, **self.engine_kwargs)
    
    def create_session_factory(self, engine):
        """Create session factory."""
        return async_sessionmaker(engine, **self.session_kwargs)

# Global database configuration
db_config = DatabaseConfig()
```

### Redis Configuration

**File**: `api/config/redis.py`

```python
import redis.asyncio as redis
from .settings import settings
import ssl
from typing import Optional

class RedisConfig:
    """Redis configuration and connection management."""
    
    def __init__(self):
        self.connection_kwargs = self._build_connection_kwargs()
        self.pool_kwargs = self._build_pool_kwargs()
    
    def _build_connection_kwargs(self) -> dict:
        """Build Redis connection configuration."""
        # Parse Redis URL
        url_parts = redis.from_url(settings.redis_url, decode_responses=True)
        
        kwargs = {
            "socket_timeout": settings.redis_socket_timeout,
            "socket_connect_timeout": settings.redis_socket_connect_timeout,
            "retry_on_timeout": settings.redis_retry_on_timeout,
            "health_check_interval": settings.redis_health_check_interval,
            "decode_responses": True
        }
        
        # SSL configuration for production
        if settings.is_production:
            kwargs.update({
                "ssl": True,
                "ssl_cert_reqs": ssl.CERT_REQUIRED,
                "ssl_ca_certs": None,  # Use system CA certificates
            })
        
        return kwargs
    
    def _build_pool_kwargs(self) -> dict:
        """Build Redis connection pool configuration."""
        return {
            "max_connections": 20,
            "retry_on_timeout": True
        }
    
    def create_connection_pool(self):
        """Create Redis connection pool."""
        return redis.ConnectionPool.from_url(
            settings.redis_url,
            **self.pool_kwargs,
            **self.connection_kwargs
        )
    
    def create_redis_client(self, connection_pool=None):
        """Create Redis client."""
        if connection_pool:
            return redis.Redis(connection_pool=connection_pool)
        else:
            return redis.from_url(settings.redis_url, **self.connection_kwargs)

# Global Redis configuration
redis_config = RedisConfig()
```

### Logging Configuration

**File**: `api/config/logging.py`

```python
import logging
import logging.config
from pythonjsonlogger import jsonlogger
from .settings import settings
import os

class LoggingConfig:
    """Centralized logging configuration."""
    
    def __init__(self):
        self.log_format = self._get_log_format()
        self.log_config = self._build_log_config()
    
    def _get_log_format(self) -> str:
        """Get log format based on environment."""
        if settings.environment == "development":
            return "%(asctime)s - %(name)s - %(levelname)s - %(message)s"
        else:
            # JSON format for production
            return "%(asctime)s %(name)s %(levelname)s %(message)s"
    
    def _build_log_config(self) -> dict:
        """Build comprehensive logging configuration."""
        
        # Ensure log directory exists
        log_dir = "/app/logs" if os.path.exists("/app") else "./logs"
        os.makedirs(log_dir, exist_ok=True)
        
        config = {
            "version": 1,
            "disable_existing_loggers": False,
            "formatters": {
                "standard": {
                    "format": self.log_format
                },
                "json": {
                    "()": jsonlogger.JsonFormatter,
                    "format": "%(asctime)s %(name)s %(levelname)s %(message)s"
                }
            },
            "handlers": {
                "console": {
                    "level": settings.log_level,
                    "class": "logging.StreamHandler",
                    "formatter": "json" if settings.is_production else "standard",
                    "stream": "ext://sys.stdout"
                }
            },
            "loggers": {
                "": {  # Root logger
                    "handlers": ["console"],
                    "level": settings.log_level,
                    "propagate": False
                },
                "uvicorn": {
                    "handlers": ["console"],
                    "level": "INFO",
                    "propagate": False
                },
                "sqlalchemy.engine": {
                    "handlers": ["console"],
                    "level": "WARNING" if not settings.database_echo else "INFO",
                    "propagate": False
                },
                "redis": {
                    "handlers": ["console"],
                    "level": "WARNING",
                    "propagate": False
                }
            }
        }
        
        # Add file handler for production
        if settings.is_production or settings.environment == "staging":
            config["handlers"]["file"] = {
                "level": settings.log_level,
                "class": "logging.handlers.RotatingFileHandler",
                "formatter": "json",
                "filename": f"{log_dir}/api.log",
                "maxBytes": 10 * 1024 * 1024,  # 10MB
                "backupCount": 5
            }
            
            # Add file handler to all loggers
            for logger_config in config["loggers"].values():
                logger_config["handlers"].append("file")
        
        return config
    
    def setup_logging(self):
        """Setup logging configuration."""
        logging.config.dictConfig(self.log_config)
        
        # Set up Logfire if enabled
        if settings.logfire_enabled:
            try:
                import logfire
                logfire.configure(
                    service_name=settings.logfire_project_name,
                    service_version="1.0.0",
                    environment=settings.environment,
                    send_to_logfire=settings.logfire_token is not None,
                    console=settings.is_development,
                    token=settings.logfire_token
                )
            except ImportError:
                logging.warning("Logfire not available, skipping setup")

# Global logging configuration
logging_config = LoggingConfig()
```

## Environment-Specific Configurations

### Development Configuration

**File**: `.env.development`

```bash
# Development Environment Configuration
ENVIRONMENT=development
DEBUG=true
LOG_LEVEL=DEBUG

# Database (Local PostgreSQL)
DATABASE_URL=postgresql://postgres:dev_password@localhost:5432/build_dev
DATABASE_ECHO=true
DATABASE_POOL_SIZE=5
DATABASE_MAX_OVERFLOW=10

# Redis (Local Redis)
REDIS_URL=redis://:dev_password@localhost:6379/0

# Security (Weak for development)
JWT_SECRET=development-secret-key-not-for-production
BCRYPT_ROUNDS=4

# CORS (Permissive for development)
ALLOWED_ORIGINS=["http://localhost:3000", "http://127.0.0.1:3000", "http://localhost:8080"]

# Features (All enabled for development)
ENABLE_SWAGGER_UI=true
ENABLE_REDOC=true

# Monitoring
LOGFIRE_ENABLED=false
PROMETHEUS_ENABLED=true

# Rate Limiting (Relaxed for development)
RATE_LIMIT_PER_MINUTE=1000
RATE_LIMIT_BURST=100
```

### Staging Configuration

**File**: `.env.staging`

```bash
# Staging Environment Configuration
ENVIRONMENT=staging
DEBUG=false
LOG_LEVEL=INFO

# Database (Staging RDS)
DATABASE_URL=postgresql://build_user:${DB_PASSWORD}@staging-db.internal:5432/build_staging
DATABASE_ECHO=false
DATABASE_POOL_SIZE=15
DATABASE_MAX_OVERFLOW=25

# Redis (Staging ElastiCache)
REDIS_URL=redis://:${REDIS_PASSWORD}@staging-redis.internal:6379/0

# Security (Production-like)
JWT_SECRET=${JWT_SECRET}
BCRYPT_ROUNDS=12

# CORS (Restricted)
ALLOWED_ORIGINS=["https://staging.getbuild.ing"]

# Features
ENABLE_SWAGGER_UI=true
ENABLE_REDOC=true

# Monitoring
LOGFIRE_ENABLED=true
LOGFIRE_TOKEN=${LOGFIRE_TOKEN}
PROMETHEUS_ENABLED=true

# Rate Limiting (Production-like)
RATE_LIMIT_PER_MINUTE=100
RATE_LIMIT_BURST=30
```

### Production Configuration

**File**: `.env.production`

```bash
# Production Environment Configuration
ENVIRONMENT=production
DEBUG=false
LOG_LEVEL=WARNING

# Database (Production RDS)
DATABASE_URL=postgresql://build_user:${DB_PASSWORD}@prod-db.internal:5432/build_production
DATABASE_ECHO=false
DATABASE_POOL_SIZE=20
DATABASE_MAX_OVERFLOW=30

# Redis (Production ElastiCache)
REDIS_URL=redis://:${REDIS_PASSWORD}@prod-redis.internal:6379/0

# Security (Maximum security)
JWT_SECRET=${JWT_SECRET}
BCRYPT_ROUNDS=12

# CORS (Strict)
ALLOWED_ORIGINS=["https://getbuild.ing", "https://app.getbuild.ing"]

# Features (Minimal)
ENABLE_SWAGGER_UI=false
ENABLE_REDOC=false

# Monitoring
LOGFIRE_ENABLED=true
LOGFIRE_TOKEN=${LOGFIRE_TOKEN}
PROMETHEUS_ENABLED=true

# Rate Limiting (Strict)
RATE_LIMIT_PER_MINUTE=60
RATE_LIMIT_BURST=20
```

## Security Configuration

### JWT Configuration

```python
# api/config/security.py
from datetime import timedelta
from .settings import settings

class SecurityConfig:
    """Security configuration settings."""
    
    # JWT Settings
    JWT_SECRET_KEY = settings.jwt_secret
    JWT_ALGORITHM = settings.jwt_algorithm
    JWT_ACCESS_TOKEN_EXPIRE = timedelta(minutes=settings.jwt_access_token_expire_minutes)
    JWT_REFRESH_TOKEN_EXPIRE = timedelta(days=settings.jwt_refresh_token_expire_days)
    
    # Password Settings
    PASSWORD_MIN_LENGTH = 12
    PASSWORD_REQUIRE_UPPERCASE = True
    PASSWORD_REQUIRE_LOWERCASE = True
    PASSWORD_REQUIRE_NUMBERS = True
    PASSWORD_REQUIRE_SPECIAL = True
    PASSWORD_SPECIAL_CHARS = "!@#$%^&*()_+-=[]{}|;:,.<>?"
    
    # Account Security
    MAX_FAILED_LOGIN_ATTEMPTS = 5
    ACCOUNT_LOCKOUT_DURATION = timedelta(minutes=15)
    
    # Session Security
    SESSION_TIMEOUT = timedelta(hours=24)
    SESSION_REFRESH_THRESHOLD = timedelta(hours=6)
    
    # Rate Limiting
    RATE_LIMITS = {
        "auth": {"requests": settings.rate_limit_auth_per_minute, "window": 60},
        "api": {"requests": settings.rate_limit_per_minute, "window": 60},
        "health": {"requests": 1000, "window": 60}
    }
    
    # Security Headers
    SECURITY_HEADERS = {
        "Strict-Transport-Security": "max-age=31536000; includeSubDomains",
        "X-Content-Type-Options": "nosniff",
        "X-Frame-Options": "DENY",
        "X-XSS-Protection": "1; mode=block",
        "Referrer-Policy": "strict-origin-when-cross-origin",
        "Content-Security-Policy": (
            "default-src 'self'; "
            "script-src 'self' 'unsafe-inline'; "
            "style-src 'self' 'unsafe-inline'; "
            "img-src 'self' data: https:; "
            "connect-src 'self' wss: ws:; "
            "object-src 'none'; "
            "base-uri 'self'; "
            "frame-ancestors 'none';"
        )
    }

security_config = SecurityConfig()
```

### CORS Configuration

```python
# api/config/cors.py
from fastapi.middleware.cors import CORSMiddleware
from .settings import settings

class CORSConfig:
    """CORS configuration for different environments."""
    
    def __init__(self):
        self.cors_kwargs = self._build_cors_config()
    
    def _build_cors_config(self) -> dict:
        """Build CORS configuration based on environment."""
        if settings.is_development:
            # Permissive CORS for development
            return {
                "allow_origins": settings.allowed_origins + ["http://localhost:*"],
                "allow_credentials": True,
                "allow_methods": ["*"],
                "allow_headers": ["*"],
            }
        elif settings.environment == "staging":
            # Moderate CORS for staging
            return {
                "allow_origins": settings.allowed_origins,
                "allow_credentials": True,
                "allow_methods": ["GET", "POST", "PUT", "DELETE", "OPTIONS"],
                "allow_headers": [
                    "Accept",
                    "Accept-Language",
                    "Content-Language",
                    "Content-Type",
                    "Authorization",
                    "X-Request-ID"
                ],
            }
        else:
            # Strict CORS for production
            return {
                "allow_origins": settings.allowed_origins,
                "allow_credentials": True,
                "allow_methods": ["GET", "POST", "PUT", "DELETE"],
                "allow_headers": [
                    "Accept",
                    "Content-Type",
                    "Authorization",
                    "X-Request-ID"
                ],
            }
    
    def add_cors_middleware(self, app):
        """Add CORS middleware to FastAPI app."""
        if settings.cors_enabled:
            app.add_middleware(CORSMiddleware, **self.cors_kwargs)

cors_config = CORSConfig()
```

## Middleware Configuration

### Request Logging Configuration

```python
# api/config/middleware.py
from typing import Dict, Any

class MiddlewareConfig:
    """Configuration for application middleware."""
    
    # Rate Limiting Configuration
    RATE_LIMITING = {
        "enabled": True,
        "storage_backend": "redis",
        "rules": {
            "default": {"rate": "100/minute", "burst": 20},
            "auth": {"rate": "10/minute", "burst": 5},
            "health": {"rate": "1000/minute", "burst": 100}
        },
        "ban_duration": {
            "light": 300,    # 5 minutes
            "moderate": 900, # 15 minutes
            "severe": 3600   # 1 hour
        }
    }
    
    # Security Monitoring
    SECURITY_MONITORING = {
        "enabled": True,
        "track_failed_logins": True,
        "track_suspicious_patterns": True,
        "auto_ban_threshold": 10,
        "suspicious_patterns": {
            "sql_injection": [
                r"(?i)(union\s+select|select\s+.*\s+from)",
                r"(?i)(drop\s+table|delete\s+from|insert\s+into)"
            ],
            "xss": [
                r"<script[^>]*>.*?</script>",
                r"javascript:",
                r"on\w+\s*="
            ],
            "path_traversal": [
                r"\.\.\/",
                r"\/etc\/passwd",
                r"\/proc\/"
            ]
        }
    }
    
    # Request Logging
    REQUEST_LOGGING = {
        "enabled": True,
        "log_headers": settings.is_development,
        "log_body": settings.is_development,
        "exclude_paths": ["/health", "/metrics"],
        "max_body_size": 1024,  # Log first 1KB of body
        "sensitive_headers": [
            "authorization",
            "cookie",
            "x-api-key"
        ]
    }
    
    # Performance Monitoring
    PERFORMANCE_MONITORING = {
        "enabled": True,
        "slow_request_threshold": 1.0,  # 1 second
        "track_db_queries": True,
        "track_redis_operations": True
    }

middleware_config = MiddlewareConfig()
```

## Validation and Type Checking

### Pydantic Models for Configuration

```python
# api/config/models.py
from pydantic import BaseModel, validator, Field
from typing import List, Dict, Any, Optional
from enum import Enum

class Environment(str, Enum):
    DEVELOPMENT = "development"
    STAGING = "staging"
    PRODUCTION = "production"

class LogLevel(str, Enum):
    DEBUG = "DEBUG"
    INFO = "INFO"
    WARNING = "WARNING"
    ERROR = "ERROR"
    CRITICAL = "CRITICAL"

class DatabaseConfig(BaseModel):
    """Database configuration model."""
    url: str = Field(..., description="Database connection URL")
    echo: bool = Field(False, description="Enable SQL query logging")
    pool_size: int = Field(20, ge=1, le=100, description="Connection pool size")
    max_overflow: int = Field(30, ge=0, le=100, description="Max overflow connections")
    pool_timeout: int = Field(30, ge=1, le=300, description="Pool timeout in seconds")
    pool_recycle: int = Field(3600, ge=60, description="Pool recycle time in seconds")
    
    @validator('url')
    def validate_database_url(cls, v):
        if not v.startswith('postgresql://') and not v.startswith('postgresql+asyncpg://'):
            raise ValueError('Database URL must be a PostgreSQL connection string')
        return v

class RedisConfig(BaseModel):
    """Redis configuration model."""
    url: str = Field(..., description="Redis connection URL")
    socket_timeout: int = Field(5, ge=1, le=60, description="Socket timeout in seconds")
    socket_connect_timeout: int = Field(5, ge=1, le=30, description="Connection timeout")
    retry_on_timeout: bool = Field(True, description="Retry on timeout")
    health_check_interval: int = Field(30, ge=10, le=300, description="Health check interval")
    
    @validator('url')
    def validate_redis_url(cls, v):
        if not v.startswith('redis://'):
            raise ValueError('Redis URL must be a redis:// connection string')
        return v

class SecurityConfig(BaseModel):
    """Security configuration model."""
    jwt_secret: str = Field(..., min_length=32, description="JWT secret key")
    jwt_algorithm: str = Field("HS256", description="JWT algorithm")
    jwt_access_token_expire_minutes: int = Field(15, ge=5, le=60, description="Access token expiry")
    jwt_refresh_token_expire_days: int = Field(7, ge=1, le=30, description="Refresh token expiry")
    bcrypt_rounds: int = Field(12, ge=10, le=15, description="Bcrypt rounds")
    
    @validator('jwt_secret')
    def validate_jwt_secret(cls, v):
        if len(v) < 32:
            raise ValueError('JWT secret must be at least 32 characters long')
        return v

class MonitoringConfig(BaseModel):
    """Monitoring configuration model."""
    logfire_enabled: bool = Field(True, description="Enable Logfire monitoring")
    logfire_token: Optional[str] = Field(None, description="Logfire token")
    logfire_project_name: str = Field("build-platform", description="Logfire project name")
    prometheus_enabled: bool = Field(True, description="Enable Prometheus metrics")
    
    @validator('logfire_token')
    def validate_logfire_token(cls, v, values):
        if values.get('logfire_enabled') and not v:
            raise ValueError('Logfire token required when Logfire is enabled')
        return v

class ApplicationConfig(BaseModel):
    """Complete application configuration."""
    environment: Environment = Environment.DEVELOPMENT
    debug: bool = False
    log_level: LogLevel = LogLevel.INFO
    
    database: DatabaseConfig
    redis: RedisConfig
    security: SecurityConfig
    monitoring: MonitoringConfig
    
    # API Configuration
    api_host: str = Field("0.0.0.0", description="API host")
    api_port: int = Field(8000, ge=1000, le=65535, description="API port")
    
    # CORS
    cors_enabled: bool = Field(True, description="Enable CORS")
    allowed_origins: List[str] = Field(["http://localhost:3000"], description="Allowed origins")
    
    class Config:
        use_enum_values = True
```

## Configuration Validation

### Startup Configuration Check

```python
# api/config/validation.py
import os
import sys
from typing import List, Tuple
from .settings import settings

class ConfigurationValidator:
    """Validate configuration at startup."""
    
    def __init__(self):
        self.errors: List[str] = []
        self.warnings: List[str] = []
    
    def validate_all(self) -> Tuple[bool, List[str], List[str]]:
        """Validate all configuration settings."""
        self._validate_required_env_vars()
        self._validate_database_config()
        self._validate_redis_config()
        self._validate_security_config()
        self._validate_environment_specific()
        
        return len(self.errors) == 0, self.errors, self.warnings
    
    def _validate_required_env_vars(self):
        """Validate required environment variables."""
        required_vars = [
            "DATABASE_URL",
            "REDIS_URL",
            "JWT_SECRET"
        ]
        
        for var in required_vars:
            if not os.getenv(var):
                self.errors.append(f"Required environment variable {var} is not set")
    
    def _validate_database_config(self):
        """Validate database configuration."""
        try:
            # Test database URL format
            if not settings.database_url.startswith(('postgresql://', 'postgresql+asyncpg://')):
                self.errors.append("DATABASE_URL must be a PostgreSQL connection string")
            
            # Validate pool settings
            if settings.database_pool_size <= 0:
                self.errors.append("DATABASE_POOL_SIZE must be positive")
            
            if settings.database_max_overflow < 0:
                self.errors.append("DATABASE_MAX_OVERFLOW must be non-negative")
                
        except Exception as e:
            self.errors.append(f"Database configuration error: {e}")
    
    def _validate_redis_config(self):
        """Validate Redis configuration."""
        try:
            if not settings.redis_url.startswith('redis://'):
                self.errors.append("REDIS_URL must be a redis:// connection string")
                
        except Exception as e:
            self.errors.append(f"Redis configuration error: {e}")
    
    def _validate_security_config(self):
        """Validate security configuration."""
        # JWT secret strength
        if len(settings.jwt_secret) < 32:
            self.errors.append("JWT_SECRET must be at least 32 characters long")
        
        # Production security checks
        if settings.is_production:
            if settings.jwt_secret == "development-secret-key-not-for-production":
                self.errors.append("Production JWT_SECRET must not use development default")
            
            if settings.debug:
                self.warnings.append("DEBUG should be false in production")
            
            if settings.database_echo:
                self.warnings.append("DATABASE_ECHO should be false in production")
    
    def _validate_environment_specific(self):
        """Validate environment-specific configuration."""
        if settings.environment == "production":
            # Production-specific validations
            production_checks = [
                (settings.debug == False, "DEBUG must be false in production"),
                (settings.log_level in ["WARNING", "ERROR"], "LOG_LEVEL should be WARNING or ERROR in production"),
                ("localhost" not in settings.allowed_origins, "ALLOWED_ORIGINS should not include localhost in production"),
                (not settings.enable_swagger_ui, "Swagger UI should be disabled in production")
            ]
            
            for check, message in production_checks:
                if not check:
                    self.warnings.append(message)
        
        elif settings.environment == "development":
            # Development-specific warnings
            if not settings.debug:
                self.warnings.append("Consider enabling DEBUG in development")

def validate_configuration() -> bool:
    """Validate configuration and exit if invalid."""
    validator = ConfigurationValidator()
    is_valid, errors, warnings = validator.validate_all()
    
    # Print warnings
    for warning in warnings:
        print(f"WARNING: {warning}", file=sys.stderr)
    
    # Print errors and exit if invalid
    if not is_valid:
        print("Configuration validation failed:", file=sys.stderr)
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        sys.exit(1)
    
    print(f"Configuration validated successfully for {settings.environment} environment")
    return True
```

## Configuration Management Scripts

### Environment Setup Script

**File**: `scripts/setup-config.sh`

```bash
#!/bin/bash

# Configuration setup script

set -e

ENVIRONMENT=${1:-development}
CONFIG_DIR="config"
SECRETS_DIR="secrets"

# Create directories
mkdir -p "$CONFIG_DIR" "$SECRETS_DIR"

# Function to generate random password
generate_password() {
    openssl rand -base64 32 | tr -d "=+/" | cut -c1-25
}

# Function to generate JWT secret
generate_jwt_secret() {
    openssl rand -base64 64 | tr -d "=+/"
}

setup_development() {
    echo "Setting up development configuration..."
    
    cat > .env.development << EOF
# Development Environment
ENVIRONMENT=development
DEBUG=true
LOG_LEVEL=DEBUG

# Database
DATABASE_URL=postgresql://postgres:dev_password@localhost:5432/build_dev
DATABASE_ECHO=true

# Redis
REDIS_URL=redis://:dev_password@localhost:6379/0

# Security
JWT_SECRET=$(generate_jwt_secret)
BCRYPT_ROUNDS=4

# CORS
ALLOWED_ORIGINS=["http://localhost:3000", "http://127.0.0.1:3000"]

# Features
ENABLE_SWAGGER_UI=true
ENABLE_REDOC=true

# Monitoring
LOGFIRE_ENABLED=false
PROMETHEUS_ENABLED=true
EOF

    echo "Development configuration created: .env.development"
}

setup_staging() {
    echo "Setting up staging configuration..."
    
    DB_PASSWORD=$(generate_password)
    REDIS_PASSWORD=$(generate_password)
    JWT_SECRET=$(generate_jwt_secret)
    
    cat > .env.staging << EOF
# Staging Environment
ENVIRONMENT=staging
DEBUG=false
LOG_LEVEL=INFO

# Database
DATABASE_URL=postgresql://build_user:\${DB_PASSWORD}@staging-db.internal:5432/build_staging

# Redis
REDIS_URL=redis://:\${REDIS_PASSWORD}@staging-redis.internal:6379/0

# Security
JWT_SECRET=\${JWT_SECRET}
BCRYPT_ROUNDS=12

# CORS
ALLOWED_ORIGINS=["https://staging.getbuild.ing"]

# Monitoring
LOGFIRE_ENABLED=true
LOGFIRE_TOKEN=\${LOGFIRE_TOKEN}
EOF

    # Save secrets
    echo "$DB_PASSWORD" > "$SECRETS_DIR/staging_db_password.txt"
    echo "$REDIS_PASSWORD" > "$SECRETS_DIR/staging_redis_password.txt"
    echo "$JWT_SECRET" > "$SECRETS_DIR/staging_jwt_secret.txt"
    
    chmod 600 "$SECRETS_DIR"/*
    
    echo "Staging configuration created: .env.staging"
    echo "Secrets saved in: $SECRETS_DIR/"
}

setup_production() {
    echo "Setting up production configuration template..."
    
    cat > .env.production.template << EOF
# Production Environment Template
# Replace \${VARIABLE} with actual values

ENVIRONMENT=production
DEBUG=false
LOG_LEVEL=WARNING

# Database (from AWS Secrets Manager)
DATABASE_URL=\${DATABASE_URL}

# Redis (from AWS Secrets Manager)
REDIS_URL=\${REDIS_URL}

# Security (from AWS Secrets Manager)
JWT_SECRET=\${JWT_SECRET}
BCRYPT_ROUNDS=12

# CORS
ALLOWED_ORIGINS=["https://getbuild.ing"]

# Features
ENABLE_SWAGGER_UI=false
ENABLE_REDOC=false

# Monitoring
LOGFIRE_ENABLED=true
LOGFIRE_TOKEN=\${LOGFIRE_TOKEN}
EOF

    echo "Production configuration template created: .env.production.template"
    echo "Use AWS Secrets Manager for actual production secrets"
}

case "$ENVIRONMENT" in
    development|dev)
        setup_development
        ;;
    staging)
        setup_staging
        ;;
    production|prod)
        setup_production
        ;;
    *)
        echo "Usage: $0 {development|staging|production}"
        echo "Available environments:"
        echo "  development - Local development setup"
        echo "  staging     - Staging environment setup"
        echo "  production  - Production template"
        exit 1
        ;;
esac

echo "Configuration setup completed for $ENVIRONMENT environment"
```

This configuration reference provides comprehensive guidance for setting up and managing all aspects of the Build Platform Session 1 infrastructure with proper validation, security, and environment-specific settings.