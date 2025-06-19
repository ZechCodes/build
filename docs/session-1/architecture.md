# Session 1: Architecture Overview

This document provides a comprehensive overview of the core infrastructure and database layer implemented in Session 1.

## System Architecture

### High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    Build Platform Session 1                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌─────────────────┐    ┌─────────────────┐    ┌─────────────┐  │
│  │   FastAPI App   │    │   PostgreSQL    │    │    Redis    │  │
│  │                 │    │                 │    │             │  │
│  │ • REST API      │◄──►│ • User data     │    │ • Sessions  │  │
│  │ • WebSocket     │    │ • Audit logs    │◄──►│ • Cache     │  │
│  │ • Middleware    │    │ • VM metadata   │    │ • Rate      │  │
│  │ • Security      │    │ • Sessions      │    │   limiting  │  │
│  └─────────────────┘    └─────────────────┘    └─────────────┘  │
│           │                       │                      │      │
│           └───────────────────────┼──────────────────────┘      │
│                                   │                             │
│  ┌─────────────────────────────────┼─────────────────────────┐   │
│  │           Monitoring Layer      │                         │   │
│  │                                 │                         │   │
│  │ ┌─────────────┐  ┌─────────────┐│┌─────────────────────┐  │   │
│  │ │   Logfire   │  │  Structured ││││   Security         │  │   │
│  │ │ Observability│  │   Logging   ││││   Monitoring       │  │   │
│  │ └─────────────┘  └─────────────┘│└─────────────────────┘  │   │
│  └─────────────────────────────────┼─────────────────────────┘   │
└─────────────────────────────────────┼─────────────────────────────┘
                                      │
                              ┌───────▼──────┐
                              │   MinIO      │
                              │  (Future)    │
                              └──────────────┘
```

## Core Components

### 1. FastAPI Application

**Location**: `api/app/main.py`

The FastAPI application serves as the central API gateway with:

- **REST API endpoints** for health checking and future authentication
- **Middleware stack** for security, monitoring, and request processing
- **WebSocket support** for real-time communication (foundation)
- **OpenAPI documentation** with Swagger UI and ReDoc

#### Middleware Stack (Execution Order)

1. **Logfire Tracking** - Request tracing and observability
2. **Logfire Performance** - Performance monitoring and slow request detection
3. **Logfire User Activity** - User activity tracking
4. **Enhanced Security Headers** - Comprehensive security policies
5. **Security Monitoring** - Threat detection and suspicious activity monitoring
6. **Input Validation** - Attack detection and input sanitization
7. **Advanced Rate Limiting** - Adaptive rate limiting with attack detection
8. **IP Whitelist** - Admin endpoint protection
9. **Request Logging** - Structured request/response logging
10. **Performance Monitoring** - Request performance tracking
11. **Error Tracking** - Exception monitoring and reporting
12. **Health Metrics** - System health data collection
13. **Prometheus Metrics** - Metrics exposure
14. **User Context** - Authentication context extraction
15. **CORS** - Cross-origin resource sharing
16. **Trusted Host** - Host validation

### 2. Database Layer (PostgreSQL)

**Location**: `api/app/models/`

#### Database Features

- **UUID Primary Keys** - All models use UUID for security and scalability
- **Row-Level Security (RLS)** - Data isolation and access control
- **Async Operations** - Non-blocking database operations with asyncpg
- **Connection Pooling** - Efficient connection management
- **Comprehensive Indexing** - Optimized query performance
- **Audit Logging** - Complete audit trail for all operations

#### Core Models

```python
# Base Models
class Base:
    """SQLAlchemy declarative base with common functionality"""

class UUIDMixin:
    """UUID primary key mixin"""
    id: UUID = Column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)

class TimestampMixin:
    """Created/updated timestamp mixin"""
    created_at: datetime
    updated_at: datetime

# Core Domain Models
class User(Base, UUIDMixin, TimestampMixin):
    """User account model with security features"""
    
class Session(Base, UUIDMixin, TimestampMixin):
    """User session model for authentication"""
    
class AuditLog(Base, UUIDMixin, TimestampMixin):
    """Comprehensive audit logging model"""
```

### 3. Redis Layer

**Location**: `api/app/core/redis.py`

#### Redis Configuration

- **Authentication Required** - Password-protected Redis instance
- **Connection Pooling** - Efficient connection management
- **Health Monitoring** - Continuous health checks
- **Persistence** - Data durability configuration
- **Memory Management** - Eviction policies and limits

#### Redis Usage

```python
# Session Management
session_data = {
    "user_id": user.id,
    "created_at": datetime.utcnow(),
    "last_accessed": datetime.utcnow()
}
await redis.setex(f"session:{session_id}", 3600, json.dumps(session_data))

# Rate Limiting
rate_limit_key = f"rate_limit:{user_id}:{endpoint}"
current_count = await redis.incr(rate_limit_key)
await redis.expire(rate_limit_key, window_seconds)

# Caching
cache_key = f"cache:{resource_type}:{resource_id}"
await redis.setex(cache_key, ttl, serialized_data)
```

### 4. Monitoring & Observability

#### Pydantic Logfire Integration

**Location**: `api/app/monitoring/`

- **Distributed Tracing** - Request tracing across components
- **Structured Logging** - JSON-formatted logs with context
- **Performance Monitoring** - Request/response time tracking
- **Database Tracking** - Query performance and slow query detection
- **Security Event Logging** - Comprehensive security monitoring
- **Metrics Collection** - System health and performance metrics

#### Monitoring Components

```python
# Logfire Setup
logfire.configure(
    service_name="build-api",
    service_version="1.0.0",
    environment=settings.environment
)

# Database Tracking
class DatabaseTracker:
    """Tracks all database operations with Logfire spans"""
    
# Security Monitoring
class SecurityMonitoringMiddleware:
    """Real-time security threat detection"""
    
# Metrics Collection
class MetricsCollector:
    """Comprehensive metrics collection and reporting"""
```

## Security Architecture

### Defense-in-Depth Strategy

1. **Network Layer**
   - Host-based firewalls
   - Network segmentation
   - SSL/TLS encryption

2. **Application Layer**
   - Input validation and sanitization
   - SQL injection prevention
   - XSS protection
   - CSRF protection

3. **Authentication Layer**
   - JWT-based authentication (foundation)
   - Session management
   - Account lockout protection

4. **Authorization Layer**
   - Role-based access control (RBAC) foundation
   - Resource ownership validation
   - Permission-based access control

5. **Data Layer**
   - Row-level security (RLS)
   - Data encryption at rest
   - Audit logging
   - Backup encryption

### Security Controls Implemented

- ✅ **40/40 Security Checklist Items** completed
- ✅ **Enhanced Security Headers** (CSP, HSTS, X-Frame-Options)
- ✅ **Advanced Rate Limiting** with attack detection
- ✅ **Input Validation** with injection protection
- ✅ **Security Monitoring** with real-time threat detection
- ✅ **Audit Logging** for all operations
- ✅ **Database Security** with SSL and RLS
- ✅ **Redis Security** with authentication and monitoring

## Data Flow

### Request Processing Flow

```
1. Client Request
   ↓
2. Load Balancer (Future)
   ↓
3. FastAPI Application
   ├─ Middleware Stack (16 layers)
   ├─ Route Handler
   ├─ Business Logic
   └─ Response Generation
   ↓
4. Database/Redis Operations
   ├─ PostgreSQL (persistent data)
   └─ Redis (cache/sessions)
   ↓
5. Monitoring & Logging
   ├─ Logfire Tracing
   ├─ Audit Logging
   └─ Metrics Collection
   ↓
6. Client Response
```

### Database Operations Flow

```
1. API Request
   ↓
2. Authentication/Authorization Check
   ↓
3. Input Validation
   ↓
4. Database Operation
   ├─ SQL Query Generation
   ├─ RLS Policy Application
   └─ Query Execution
   ↓
5. Audit Logging
   ├─ Operation Details
   ├─ User Context
   └─ Result Status
   ↓
6. Response Generation
```

## Configuration Management

### Environment-Based Configuration

```python
class Settings(BaseSettings):
    # Environment
    environment: str = "development"
    debug: bool = False
    
    # Database
    database_url: str
    database_echo: bool = False
    
    # Redis
    redis_url: str
    redis_password: str
    
    # Security
    jwt_secret: str
    bcrypt_rounds: int = 12
    rate_limit_per_minute: int = 60
    
    # Features
    enable_swagger_ui: bool = True
    enable_cors: bool = True
    allowed_origins: List[str] = ["http://localhost:3000"]
```

### Security Configuration

- **Environment Variables** - All secrets stored in environment variables
- **SSL/TLS Enforcement** - All connections encrypted
- **CORS Configuration** - Specific allowed origins
- **Rate Limiting** - Configurable limits per endpoint
- **Session Management** - Secure session configuration

## Deployment Architecture

### Local Development

- **Podman Compose** - Container orchestration
- **PostgreSQL** - Database service
- **Redis** - Caching and session storage
- **MinIO** - Object storage (planned)
- **Soft-serve** - Git server (planned)

### Development Services

```yaml
services:
  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_DB: build_dev
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: dev_password
    
  redis:
    image: redis:7-alpine
    command: redis-server --appendonly yes --requirepass dev_password
    
  api:
    build: ./api
    environment:
      DATABASE_URL: postgresql://postgres:dev_password@postgres:5432/build_dev
      REDIS_URL: redis://:dev_password@redis:6379/0
```

## Scalability Considerations

### Horizontal Scaling Points

1. **API Servers** - Stateless design enables easy scaling
2. **Database Read Replicas** - Read scaling for queries
3. **Redis Cluster** - Distributed caching and sessions
4. **Load Balancers** - Traffic distribution

### Resource Management

- **Connection Pooling** - Efficient database connections
- **Caching Strategy** - Redis for frequently accessed data
- **Async Operations** - Non-blocking I/O for better concurrency
- **Resource Limits** - Configurable limits per service

## Future Extensions

Session 1 provides the foundation for:

- **Session 2**: Authentication & Authorization
- **Session 3**: VM Management with Firecracker
- **Session 4**: PTY/Terminal Connections
- **Session 5**: WebSocket Communication
- **Session 6**: Session Management
- **Session 7**: Snapshot System
- **And beyond...**

The architecture is designed to be modular and extensible, allowing each session to build upon the solid foundation established in Session 1.