# Session 1: Core Infrastructure & Database Layer

## Objective
Establish the foundational infrastructure and database schema that will support all other components of the Build platform.

## Overview
This session creates the backbone of the Build platform by implementing the core database schema, Redis configuration, basic FastAPI structure, and observability foundations. All subsequent sessions depend on the successful completion of this infrastructure layer.

## Prerequisites
- Session 0 completed successfully
- Development environment verified and functional
- All security checklist items from Session 0 completed

## Components to Implement

### 1. PostgreSQL Database Schema
**Location**: `api/database/`

#### Core Tables
```sql
-- Users and Authentication
CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email VARCHAR(255) UNIQUE NOT NULL,
    username VARCHAR(50) UNIQUE NOT NULL,
    password_hash VARCHAR(255) NOT NULL,
    is_active BOOLEAN DEFAULT true,
    is_verified BOOLEAN DEFAULT false,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    last_login TIMESTAMP WITH TIME ZONE
);

-- VM Instances
CREATE TABLE vm_instances (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    state VARCHAR(20) NOT NULL DEFAULT 'stopped',
    firecracker_id VARCHAR(50) UNIQUE,
    config JSONB NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    started_at TIMESTAMP WITH TIME ZONE,
    stopped_at TIMESTAMP WITH TIME ZONE
);

-- Sessions
CREATE TABLE sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    vm_instance_id UUID NOT NULL REFERENCES vm_instances(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    session_token VARCHAR(255) UNIQUE NOT NULL,
    state VARCHAR(20) NOT NULL DEFAULT 'active',
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    last_activity TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    expires_at TIMESTAMP WITH TIME ZONE NOT NULL
);

-- Snapshots
CREATE TABLE snapshots (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    vm_instance_id UUID NOT NULL REFERENCES vm_instances(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    description TEXT,
    storage_path VARCHAR(500) NOT NULL,
    size_bytes BIGINT,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    metadata JSONB
);

-- Audit Log
CREATE TABLE audit_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID REFERENCES users(id),
    action VARCHAR(100) NOT NULL,
    resource_type VARCHAR(50),
    resource_id UUID,
    ip_address INET,
    user_agent TEXT,
    details JSONB,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);
```

#### Indexes and Constraints
```sql
-- Performance indexes
CREATE INDEX idx_users_email ON users(email);
CREATE INDEX idx_users_username ON users(username);
CREATE INDEX idx_vm_instances_user_id ON vm_instances(user_id);
CREATE INDEX idx_vm_instances_state ON vm_instances(state);
CREATE INDEX idx_sessions_user_id ON sessions(user_id);
CREATE INDEX idx_sessions_vm_instance_id ON sessions(vm_instance_id);
CREATE INDEX idx_sessions_expires_at ON sessions(expires_at);
CREATE INDEX idx_snapshots_user_id ON snapshots(user_id);
CREATE INDEX idx_snapshots_vm_instance_id ON snapshots(vm_instance_id);
CREATE INDEX idx_audit_logs_user_id ON audit_logs(user_id);
CREATE INDEX idx_audit_logs_created_at ON audit_logs(created_at);

-- Row Level Security
ALTER TABLE vm_instances ENABLE ROW LEVEL SECURITY;
ALTER TABLE sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE snapshots ENABLE ROW LEVEL SECURITY;

-- RLS Policies
CREATE POLICY user_vm_instances ON vm_instances FOR ALL USING (user_id = current_setting('app.current_user_id')::UUID);
CREATE POLICY user_sessions ON sessions FOR ALL USING (user_id = current_setting('app.current_user_id')::UUID);
CREATE POLICY user_snapshots ON snapshots FOR ALL USING (user_id = current_setting('app.current_user_id')::UUID);
```

### 2. Redis Configuration with Sentinel
**Location**: `infrastructure/redis/`

#### Redis Configuration
```yaml
# redis-primary.conf
port 6379
bind 127.0.0.1
protected-mode yes
requirepass your_redis_password

# Memory and persistence
maxmemory 2gb
maxmemory-policy allkeys-lru
save 900 1
save 300 10
save 60 10000

# Logging
loglevel notice
logfile /var/log/redis/redis-server.log

# Security
rename-command FLUSHDB ""
rename-command FLUSHALL ""
rename-command CONFIG "CONFIG_9f2dd0cb8f9b8fc5"
```

#### Sentinel Configuration
```yaml
# sentinel.conf
port 26379
sentinel monitor mymaster 127.0.0.1 6379 2
sentinel auth-pass mymaster your_redis_password
sentinel down-after-milliseconds mymaster 5000
sentinel parallel-syncs mymaster 1
sentinel failover-timeout mymaster 10000
```

### 3. FastAPI Application Structure
**Location**: `api/`

#### Core Application Structure
```python
# api/main.py
from fastapi import FastAPI, Depends, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.middleware.trustedhost import TrustedHostMiddleware
from contextlib import asynccontextmanager
import uvicorn

from .database import database
from .auth import auth_router
from .vms import vm_router
from .sessions import session_router
from .monitoring import setup_monitoring

@asynccontextmanager
async def lifespan(app: FastAPI):
    # Startup
    await database.connect()
    setup_monitoring(app)
    yield
    # Shutdown
    await database.disconnect()

app = FastAPI(
    title="Build Platform API",
    description="Cloud development environment platform",
    version="1.0.0",
    lifespan=lifespan
)

# Security middleware
app.add_middleware(TrustedHostMiddleware, allowed_hosts=["getbuild.ing", "*.getbuild.ing", "localhost"])
app.add_middleware(
    CORSMiddleware,
    allow_origins=["https://getbuild.ing", "http://localhost:3000"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

# Include routers
app.include_router(auth_router, prefix="/auth", tags=["authentication"])
app.include_router(vm_router, prefix="/vms", tags=["virtual-machines"])
app.include_router(session_router, prefix="/sessions", tags=["sessions"])

@app.get("/health")
async def health_check():
    return {"status": "healthy", "service": "build-api"}

if __name__ == "__main__":
    uvicorn.run("main:app", host="0.0.0.0", port=8000, reload=True)
```

#### Database Models with SQLAlchemy
```python
# api/models/base.py
from sqlalchemy import Column, DateTime, UUID
from sqlalchemy.ext.declarative import declarative_base
from sqlalchemy.sql import func
import uuid

Base = declarative_base()

class TimestampMixin:
    created_at = Column(DateTime(timezone=True), server_default=func.now())
    updated_at = Column(DateTime(timezone=True), server_default=func.now(), onupdate=func.now())

class UUIDMixin:
    id = Column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)

# api/models/user.py
from sqlalchemy import Column, String, Boolean, DateTime
from .base import Base, TimestampMixin, UUIDMixin

class User(Base, UUIDMixin, TimestampMixin):
    __tablename__ = "users"
    
    email = Column(String(255), unique=True, nullable=False, index=True)
    username = Column(String(50), unique=True, nullable=False, index=True)
    password_hash = Column(String(255), nullable=False)
    is_active = Column(Boolean, default=True)
    is_verified = Column(Boolean, default=False)
    last_login = Column(DateTime(timezone=True))
```

### 4. Pydantic Logfire Integration
**Location**: `api/monitoring/`

#### Observability Setup
```python
# api/monitoring/logfire_setup.py
import logfire
from fastapi import FastAPI
import structlog
from typing import Any, Dict

def setup_monitoring(app: FastAPI) -> None:
    """Configure Pydantic Logfire for comprehensive observability"""
    
    # Configure Logfire
    logfire.configure(
        service_name="build-api",
        service_version="1.0.0",
        environment="development"  # Change per environment
    )
    
    # Instrument FastAPI
    logfire.instrument_fastapi(app)
    
    # Configure structured logging
    structlog.configure(
        processors=[
            structlog.stdlib.filter_by_level,
            structlog.stdlib.add_logger_name,
            structlog.stdlib.add_log_level,
            structlog.stdlib.PositionalArgumentsFormatter(),
            structlog.processors.TimeStamper(fmt="iso"),
            structlog.processors.StackInfoRenderer(),
            structlog.processors.format_exc_info,
            structlog.processors.UnicodeDecoder(),
            structlog.processors.JSONRenderer()
        ],
        context_class=dict,
        logger_factory=structlog.stdlib.LoggerFactory(),
        wrapper_class=structlog.stdlib.BoundLogger,
        cache_logger_on_first_use=True,
    )

# api/monitoring/middleware.py
from fastapi import Request, Response
from starlette.middleware.base import BaseHTTPMiddleware
import time
import logfire

class LogfireMiddleware(BaseHTTPMiddleware):
    async def dispatch(self, request: Request, call_next):
        start_time = time.time()
        
        with logfire.span(
            "HTTP Request",
            method=request.method,
            url=str(request.url),
            user_agent=request.headers.get("user-agent")
        ) as span:
            response = await call_next(request)
            
            process_time = time.time() - start_time
            span.set_attribute("response.status_code", response.status_code)
            span.set_attribute("response.process_time", process_time)
            
            return response
```

## Critical Decisions

### Database Connection Strategy
- **Decision**: Use asyncpg with SQLAlchemy async for connection pooling
- **Rationale**: Better performance under load, non-blocking operations
- **Configuration**: Max 20 connections per instance, 5 min connections

### Redis Memory Management
- **Decision**: allkeys-lru eviction policy with 2GB limit
- **Rationale**: Automatic cleanup of least recently used keys
- **Monitoring**: Memory usage alerts at 80% capacity

### Backup Strategy
- **Decision**: Continuous WAL archiving + daily full backups
- **Retention**: 30 days for WAL, 90 days for full backups
- **Location**: S3-compatible storage with versioning

### Environment Separation
- **Decision**: Complete isolation between dev/staging/prod
- **Implementation**: Separate databases, Redis instances, secrets

## Security Checklist ✅

### Database Security
- [ ] All connections use SSL/TLS encryption
- [ ] Database passwords stored in environment variables only
- [ ] Row-level security (RLS) policies implemented and tested
- [ ] Prepared statements used for all queries (SQLAlchemy default)
- [ ] Connection limits configured (max 20 per service)
- [ ] Database audit logging enabled for all DDL/DML operations
- [ ] Regular backup encryption verified
- [ ] Database user follows principle of least privilege
- [ ] SQL injection protection via ORM (no raw SQL without validation)
- [ ] Database monitoring for suspicious activity patterns

### Redis Security
- [ ] Redis authentication (requirepass) configured
- [ ] Redis bound to localhost/private network only
- [ ] Dangerous commands (FLUSHDB, CONFIG) renamed/disabled
- [ ] Redis Sentinel authentication configured
- [ ] Memory limits enforced to prevent DoS
- [ ] Redis logs configured for security monitoring
- [ ] Network isolation between Redis and external services
- [ ] Regular Redis configuration security review
- [ ] Backup encryption for Redis persistence files
- [ ] Rate limiting on Redis operations

### Application Security
- [ ] CORS properly configured with specific origins
- [ ] Trusted Host middleware configured
- [ ] Security headers implemented (HSTS, CSP, etc.)
- [ ] Request rate limiting implemented
- [ ] Input validation on all endpoints
- [ ] Error handling doesn't expose sensitive information
- [ ] Health check endpoints don't expose sensitive data
- [ ] Dependency vulnerability scanning configured
- [ ] Secret scanning in CI/CD pipeline
- [ ] Structured logging excludes sensitive data

### Infrastructure Security
- [ ] Network segmentation between tiers
- [ ] Firewall rules restrict database access
- [ ] Container security scanning enabled
- [ ] Non-root container execution verified
- [ ] Resource limits configured for all services
- [ ] Security updates automated where possible
- [ ] Monitoring and alerting for security events
- [ ] Backup integrity verification procedures
- [ ] Disaster recovery procedures documented and tested
- [ ] Access controls for infrastructure management

## Testing Requirements

### Database Testing
- [ ] Connection pool exhaustion handling
- [ ] Database migration rollback procedures
- [ ] Row-level security policy verification
- [ ] Backup and restore procedures
- [ ] Performance under concurrent load
- [ ] Failover scenarios with Redis Sentinel

### Integration Testing
- [ ] API endpoint functionality
- [ ] Database connection resilience
- [ ] Redis connectivity and failover
- [ ] Monitoring and logging integration
- [ ] Authentication flow end-to-end
- [ ] Error handling and recovery

### Security Testing
- [ ] SQL injection attempts (automated tools)
- [ ] Authentication bypass attempts
- [ ] Authorization boundary testing
- [ ] Rate limiting effectiveness
- [ ] Input validation boundary testing
- [ ] Container escape attempts

## Performance Targets

### Database Performance
- Query response time < 100ms (95th percentile)
- Connection establishment < 50ms
- Migration execution < 5 minutes
- Backup completion < 30 minutes

### API Performance
- Health check response < 10ms
- Authentication request < 200ms
- CRUD operations < 300ms
- Concurrent request handling: 1000 req/sec

### Redis Performance
- Cache hit ratio > 95%
- Operation latency < 1ms
- Memory usage < 80% of allocated
- Sentinel failover < 10 seconds

## Monitoring & Alerting

### Key Metrics
- Database connection pool utilization
- Query execution times and slow query detection
- Redis memory usage and hit ratios
- API response times and error rates
- Failed authentication attempts
- Resource utilization (CPU, memory, disk)

### Alert Conditions
- Database connection pool > 80% utilized
- Query execution time > 1 second
- Redis memory usage > 80%
- API error rate > 1%
- Failed login attempts > 10/minute
- Disk space usage > 85%

## Documentation Deliverables

### Technical Documentation
- [ ] Database schema documentation with ER diagrams
- [ ] API documentation with OpenAPI spec
- [ ] Redis configuration and topology documentation
- [ ] Monitoring and alerting runbook
- [ ] Backup and recovery procedures
- [ ] Performance tuning guide

### Operational Documentation
- [ ] Service startup and shutdown procedures
- [ ] Troubleshooting guide for common issues
- [ ] Security incident response procedures
- [ ] Maintenance and upgrade procedures
- [ ] Capacity planning guidelines
- [ ] Development environment setup guide

## Next Steps

Upon successful completion of Session 1:
1. All database tables created and tested
2. Redis cluster functional with Sentinel
3. FastAPI application serving basic endpoints
4. Monitoring and logging operational
5. Security checklist 100% completed
6. Proceed to Session 2: Authentication & Authorization System

## Risk Mitigation

### High-Risk Areas
1. **Database corruption**: Regular integrity checks, point-in-time recovery
2. **Performance degradation**: Query optimization, indexing strategy
3. **Security vulnerabilities**: Regular security audits, penetration testing
4. **Data loss**: Multi-tier backup strategy, replication
5. **Service unavailability**: Health checks, automated recovery

### Contingency Plans
- Database failover procedures documented
- Redis cluster recovery steps defined
- API service restart automation
- Data corruption recovery procedures
- Security incident response plan

---

**Session 1 Success Criteria:**
- All core infrastructure components operational
- Security checklist 100% complete
- Performance benchmarks met
- Documentation complete
- Ready for Session 2 authentication implementation