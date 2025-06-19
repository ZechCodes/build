# Database Documentation

This document provides comprehensive documentation for the PostgreSQL database schema, models, and operations implemented in Session 1.

## Database Overview

### Database Configuration

- **Database**: PostgreSQL 16+
- **Driver**: asyncpg (async Python driver)
- **ORM**: SQLAlchemy 2.0+ with async support
- **Migrations**: Alembic
- **Connection Pooling**: SQLAlchemy async engine with connection pooling

### Key Features

- **UUID Primary Keys** - All models use UUID for better security and distribution
- **Row-Level Security (RLS)** - Database-level access control
- **Comprehensive Indexing** - Optimized for query performance
- **Audit Logging** - Complete audit trail for all operations
- **Async Operations** - Non-blocking database operations
- **Connection Pooling** - Efficient connection management

## Database Schema

### Base Models

#### UUIDMixin

```python
class UUIDMixin:
    """Provides UUID primary key for all models."""
    
    id: Mapped[UUID] = mapped_column(
        UUID(as_uuid=True), 
        primary_key=True, 
        default=uuid.uuid4,
        index=True
    )
```

#### TimestampMixin

```python
class TimestampMixin:
    """Provides created_at and updated_at timestamps."""
    
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        default=func.now(),
        index=True
    )
    
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        default=func.now(),
        onupdate=func.now(),
        index=True
    )
```

### Core Models

#### User Model

```python
class User(Base, UUIDMixin, TimestampMixin):
    """User account model with comprehensive security features."""
    
    __tablename__ = "users"
    
    # Basic Information
    email: Mapped[str] = mapped_column(String(255), unique=True, nullable=False, index=True)
    username: Mapped[str] = mapped_column(String(50), unique=True, nullable=False, index=True)
    full_name: Mapped[Optional[str]] = mapped_column(String(100))
    
    # Authentication
    password_hash: Mapped[str] = mapped_column(String(255), nullable=False)
    is_active: Mapped[bool] = mapped_column(Boolean, default=True, index=True)
    is_verified: Mapped[bool] = mapped_column(Boolean, default=False, index=True)
    
    # Security
    last_login: Mapped[Optional[datetime]] = mapped_column(DateTime(timezone=True))
    failed_login_attempts: Mapped[int] = mapped_column(Integer, default=0)
    locked_until: Mapped[Optional[datetime]] = mapped_column(DateTime(timezone=True))
    
    # Relationships
    sessions: Mapped[List["Session"]] = relationship("Session", back_populates="user")
    audit_logs: Mapped[List["AuditLog"]] = relationship("AuditLog", back_populates="user")
```

#### Session Model

```python
class Session(Base, UUIDMixin, TimestampMixin):
    """User session model for authentication and activity tracking."""
    
    __tablename__ = "sessions"
    
    # Session Information
    user_id: Mapped[UUID] = mapped_column(UUID(as_uuid=True), ForeignKey("users.id"), nullable=False, index=True)
    session_token: Mapped[str] = mapped_column(String(255), unique=True, nullable=False, index=True)
    
    # Session Metadata
    ip_address: Mapped[Optional[str]] = mapped_column(String(45))  # IPv4/IPv6
    user_agent: Mapped[Optional[str]] = mapped_column(Text)
    
    # Session State
    is_active: Mapped[bool] = mapped_column(Boolean, default=True, index=True)
    expires_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False, index=True)
    last_accessed: Mapped[Optional[datetime]] = mapped_column(DateTime(timezone=True))
    
    # Additional Metadata
    metadata: Mapped[Optional[dict]] = mapped_column(JSON)
    
    # Relationships
    user: Mapped["User"] = relationship("User", back_populates="sessions")
```

#### AuditLog Model

```python
class AuditLog(Base, UUIDMixin, TimestampMixin):
    """Comprehensive audit logging for security and compliance."""
    
    __tablename__ = "audit_logs"
    
    # Operation Information
    user_id: Mapped[Optional[UUID]] = mapped_column(UUID(as_uuid=True), ForeignKey("users.id"), index=True)
    action: Mapped[str] = mapped_column(String(100), nullable=False, index=True)
    
    # Resource Information
    resource_type: Mapped[Optional[str]] = mapped_column(String(50), index=True)
    resource_id: Mapped[Optional[UUID]] = mapped_column(UUID(as_uuid=True), index=True)
    
    # Request Context
    ip_address: Mapped[Optional[str]] = mapped_column(String(45), index=True)
    user_agent: Mapped[Optional[str]] = mapped_column(Text)
    
    # Additional Details
    details: Mapped[Optional[dict]] = mapped_column(JSON)
    
    # Relationships
    user: Mapped[Optional["User"]] = relationship("User", back_populates="audit_logs")
```

## Database Indexes

### Automatic Indexes

All models include automatic indexes on:
- Primary keys (UUID)
- Foreign keys
- Created/updated timestamps
- Unique constraints

### Custom Indexes

```sql
-- User model indexes
CREATE INDEX idx_users_email ON users(email);
CREATE INDEX idx_users_username ON users(username);
CREATE INDEX idx_users_is_active ON users(is_active);
CREATE INDEX idx_users_last_login ON users(last_login);

-- Session model indexes
CREATE INDEX idx_sessions_user_id ON sessions(user_id);
CREATE INDEX idx_sessions_token ON sessions(session_token);
CREATE INDEX idx_sessions_expires_at ON sessions(expires_at);
CREATE INDEX idx_sessions_is_active ON sessions(is_active);

-- Audit log indexes
CREATE INDEX idx_audit_logs_user_id ON audit_logs(user_id);
CREATE INDEX idx_audit_logs_action ON audit_logs(action);
CREATE INDEX idx_audit_logs_resource_type ON audit_logs(resource_type);
CREATE INDEX idx_audit_logs_ip_address ON audit_logs(ip_address);
CREATE INDEX idx_audit_logs_created_at ON audit_logs(created_at);

-- Composite indexes for common queries
CREATE INDEX idx_sessions_user_active ON sessions(user_id, is_active);
CREATE INDEX idx_audit_logs_user_action ON audit_logs(user_id, action);
```

## Row-Level Security (RLS)

### RLS Policies

```sql
-- Enable RLS on all tables
ALTER TABLE users ENABLE ROW LEVEL SECURITY;
ALTER TABLE sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_logs ENABLE ROW LEVEL SECURITY;

-- User access policy (users can only see their own data)
CREATE POLICY user_access_policy ON users
    FOR ALL TO application_user
    USING (id = current_setting('app.current_user_id')::uuid);

-- Session access policy (users can only see their own sessions)
CREATE POLICY session_access_policy ON sessions
    FOR ALL TO application_user
    USING (user_id = current_setting('app.current_user_id')::uuid);

-- Audit log access policy (users can only see their own audit logs)
CREATE POLICY audit_log_access_policy ON audit_logs
    FOR ALL TO application_user
    USING (user_id = current_setting('app.current_user_id')::uuid);

-- Admin policy (admins can see all data)
CREATE POLICY admin_access_policy ON users
    FOR ALL TO admin_user
    USING (true);

CREATE POLICY admin_session_policy ON sessions
    FOR ALL TO admin_user
    USING (true);

CREATE POLICY admin_audit_policy ON audit_logs
    FOR ALL TO admin_user
    USING (true);
```

### RLS Context Setting

```python
async def set_rls_context(db: AsyncSession, user_id: UUID, is_admin: bool = False):
    """Set RLS context for database operations."""
    await db.execute(text(f"SET app.current_user_id = '{user_id}'"))
    if is_admin:
        await db.execute(text("SET ROLE admin_user"))
    else:
        await db.execute(text("SET ROLE application_user"))
```

## Database Operations

### Connection Management

```python
# Database configuration
class DatabaseConfig:
    def __init__(self):
        self.database_url = settings.database_url
        self.echo = settings.database_echo
        self.pool_size = 20
        self.max_overflow = 30
        self.pool_timeout = 30
        self.pool_recycle = 3600

# Engine creation
engine = create_async_engine(
    database_url,
    echo=echo,
    pool_size=pool_size,
    max_overflow=max_overflow,
    pool_timeout=pool_timeout,
    pool_recycle=pool_recycle
)

# Session factory
AsyncSessionLocal = async_sessionmaker(
    engine,
    class_=AsyncSession,
    expire_on_commit=False
)
```

### Database Session Management

```python
async def get_db_session() -> AsyncSession:
    """Get database session with proper cleanup."""
    async with AsyncSessionLocal() as session:
        try:
            yield session
        except Exception:
            await session.rollback()
            raise
        finally:
            await session.close()
```

### CRUD Operations

#### User Operations

```python
class UserRepository:
    def __init__(self, db: AsyncSession):
        self.db = db
    
    async def create_user(self, user_data: UserCreate) -> User:
        """Create new user with secure password hashing."""
        password_hash = hash_password(user_data.password)
        
        user = User(
            email=user_data.email,
            username=user_data.username,
            full_name=user_data.full_name,
            password_hash=password_hash
        )
        
        self.db.add(user)
        await self.db.commit()
        await self.db.refresh(user)
        
        # Log user creation
        await self.audit_log(
            user_id=user.id,
            action="user.created",
            resource_type="user",
            resource_id=user.id
        )
        
        return user
    
    async def get_user_by_email(self, email: str) -> Optional[User]:
        """Get user by email address."""
        query = select(User).where(User.email == email)
        result = await self.db.execute(query)
        return result.scalar_one_or_none()
    
    async def update_user(self, user_id: UUID, user_data: UserUpdate) -> User:
        """Update user information."""
        query = select(User).where(User.id == user_id)
        result = await self.db.execute(query)
        user = result.scalar_one()
        
        for field, value in user_data.dict(exclude_unset=True).items():
            setattr(user, field, value)
        
        await self.db.commit()
        await self.db.refresh(user)
        
        # Log user update
        await self.audit_log(
            user_id=user.id,
            action="user.updated",
            resource_type="user",
            resource_id=user.id,
            details=user_data.dict(exclude_unset=True)
        )
        
        return user
```

#### Session Operations

```python
class SessionRepository:
    def __init__(self, db: AsyncSession):
        self.db = db
    
    async def create_session(self, user_id: UUID, session_data: SessionCreate) -> Session:
        """Create new user session."""
        session = Session(
            user_id=user_id,
            session_token=secrets.token_urlsafe(32),
            ip_address=session_data.ip_address,
            user_agent=session_data.user_agent,
            expires_at=datetime.utcnow() + timedelta(hours=24),
            metadata=session_data.metadata
        )
        
        self.db.add(session)
        await self.db.commit()
        await self.db.refresh(session)
        
        return session
    
    async def get_active_session(self, session_token: str) -> Optional[Session]:
        """Get active session by token."""
        query = (
            select(Session)
            .where(Session.session_token == session_token)
            .where(Session.is_active == True)
            .where(Session.expires_at > func.now())
        )
        result = await self.db.execute(query)
        return result.scalar_one_or_none()
    
    async def deactivate_session(self, session_token: str) -> bool:
        """Deactivate session."""
        query = (
            update(Session)
            .where(Session.session_token == session_token)
            .values(is_active=False)
        )
        result = await self.db.execute(query)
        await self.db.commit()
        
        return result.rowcount > 0
```

### Audit Logging

```python
async def audit_log(
    db: AsyncSession,
    user_id: Optional[UUID] = None,
    action: str = "",
    resource_type: Optional[str] = None,
    resource_id: Optional[UUID] = None,
    ip_address: Optional[str] = None,
    user_agent: Optional[str] = None,
    details: Optional[dict] = None
) -> AuditLog:
    """Create audit log entry."""
    
    audit_entry = AuditLog(
        user_id=user_id,
        action=action,
        resource_type=resource_type,
        resource_id=resource_id,
        ip_address=ip_address,
        user_agent=user_agent,
        details=details or {}
    )
    
    db.add(audit_entry)
    await db.commit()
    await db.refresh(audit_entry)
    
    return audit_entry
```

## Database Migrations

### Alembic Configuration

```python
# alembic.ini configuration
[alembic]
script_location = alembic
sqlalchemy.url = driver://user:pass@localhost/dbname

# Migration environment
from alembic import context
from app.models import Base
from app.core.database import engine

target_metadata = Base.metadata

async def run_migrations_online():
    """Run migrations in 'online' mode."""
    async with engine.connect() as connection:
        await connection.run_sync(do_run_migrations)

def do_run_migrations(connection):
    context.configure(
        connection=connection,
        target_metadata=target_metadata,
        compare_type=True,
        compare_server_default=True
    )
    
    with context.begin_transaction():
        context.run_migrations()
```

### Migration Commands

```bash
# Create new migration
alembic revision --autogenerate -m "Create user and session tables"

# Run migrations
alembic upgrade head

# Rollback migration
alembic downgrade -1

# Show current revision
alembic current

# Show migration history
alembic history
```

### Sample Migration

```python
"""Create user and session tables

Revision ID: 001_initial_schema
Revises: 
Create Date: 2024-01-01 00:00:00.000000

"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql

# revision identifiers
revision = '001_initial_schema'
down_revision = None
branch_labels = None
depends_on = None

def upgrade() -> None:
    # Create users table
    op.create_table('users',
        sa.Column('id', postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column('email', sa.String(length=255), nullable=False),
        sa.Column('username', sa.String(length=50), nullable=False),
        sa.Column('full_name', sa.String(length=100), nullable=True),
        sa.Column('password_hash', sa.String(length=255), nullable=False),
        sa.Column('is_active', sa.Boolean(), nullable=True, default=True),
        sa.Column('is_verified', sa.Boolean(), nullable=True, default=False),
        sa.Column('last_login', sa.DateTime(timezone=True), nullable=True),
        sa.Column('failed_login_attempts', sa.Integer(), nullable=True, default=0),
        sa.Column('locked_until', sa.DateTime(timezone=True), nullable=True),
        sa.Column('created_at', sa.DateTime(timezone=True), nullable=True),
        sa.Column('updated_at', sa.DateTime(timezone=True), nullable=True),
        sa.PrimaryKeyConstraint('id')
    )
    
    # Create indexes
    op.create_index('idx_users_email', 'users', ['email'], unique=True)
    op.create_index('idx_users_username', 'users', ['username'], unique=True)
    op.create_index('idx_users_is_active', 'users', ['is_active'])
    
    # Enable RLS
    op.execute('ALTER TABLE users ENABLE ROW LEVEL SECURITY')

def downgrade() -> None:
    op.drop_table('users')
```

## Database Monitoring

### Query Performance Monitoring

```python
# SQLAlchemy event listeners for query tracking
from sqlalchemy import event
from app.monitoring.database_tracking import db_tracker

@event.listens_for(engine, "before_cursor_execute")
def before_cursor_execute(conn, cursor, statement, parameters, context, executemany):
    """Track query start."""
    query_id = f"{id(conn)}_{id(cursor)}_{time.time()}"
    context._logfire_query_id = query_id
    db_tracker.track_query_start(query_id, statement, parameters)

@event.listens_for(engine, "after_cursor_execute")
def after_cursor_execute(conn, cursor, statement, parameters, context, executemany):
    """Track query completion."""
    query_id = getattr(context, "_logfire_query_id", None)
    if query_id:
        db_tracker.track_query_end(query_id, success=True)
```

### Database Health Checks

```python
async def check_database_health() -> dict:
    """Check database connectivity and performance."""
    try:
        async with get_db_session() as db:
            # Test basic connectivity
            result = await db.execute(text("SELECT 1"))
            
            # Test query performance
            start_time = time.time()
            await db.execute(text("SELECT COUNT(*) FROM users"))
            query_time = time.time() - start_time
            
            # Check connection pool status
            pool = db.bind.pool
            
            return {
                "status": "healthy",
                "connectivity": "ok",
                "query_time_ms": round(query_time * 1000, 2),
                "pool_size": pool.size(),
                "checked_out_connections": pool.checkedout(),
                "checked_in_connections": pool.checkedin()
            }
    except Exception as e:
        return {
            "status": "unhealthy",
            "error": str(e)
        }
```

## Security Considerations

### Data Protection

1. **Encryption at Rest** - Database encryption enabled
2. **SSL/TLS Connections** - All connections encrypted
3. **Password Hashing** - bcrypt with 12+ rounds
4. **Row-Level Security** - Database-level access control
5. **Input Validation** - Prevents SQL injection
6. **Audit Logging** - Complete operation tracking

### Access Control

1. **Database Users** - Separate users for application and admin access
2. **Connection Limits** - Prevents connection exhaustion
3. **Query Timeouts** - Prevents long-running queries
4. **Prepared Statements** - SQLAlchemy uses prepared statements by default

### Backup & Recovery

```bash
# Database backup
pg_dump -h localhost -U postgres -d build_dev > backup.sql

# Point-in-time recovery setup
# Enable WAL archiving in postgresql.conf
archive_mode = on
archive_command = 'cp %p /path/to/archive/%f'

# Create base backup
pg_basebackup -h localhost -U postgres -D /path/to/backup
```

## Troubleshooting

### Common Issues

1. **Connection Pool Exhaustion**
   ```python
   # Check pool status
   pool = engine.pool
   print(f"Pool size: {pool.size()}")
   print(f"Checked out: {pool.checkedout()}")
   ```

2. **Slow Queries**
   ```sql
   -- Enable slow query logging
   log_min_duration_statement = 1000  -- Log queries > 1 second
   
   -- Check slow queries
   SELECT query, total_time, calls 
   FROM pg_stat_statements 
   ORDER BY total_time DESC;
   ```

3. **RLS Issues**
   ```sql
   -- Check RLS policies
   SELECT schemaname, tablename, policyname, cmd, qual 
   FROM pg_policies;
   
   -- Disable RLS for debugging
   SET row_security = off;
   ```

### Performance Optimization

1. **Index Usage**
   ```sql
   -- Check index usage
   SELECT schemaname, tablename, indexname, idx_scan, idx_tup_read
   FROM pg_stat_user_indexes
   ORDER BY idx_scan DESC;
   ```

2. **Query Optimization**
   ```sql
   -- Analyze query plans
   EXPLAIN (ANALYZE, BUFFERS) SELECT * FROM users WHERE email = 'user@example.com';
   ```

3. **Connection Tuning**
   ```python
   # Adjust connection pool settings
   engine = create_async_engine(
       database_url,
       pool_size=20,        # Base connection pool size
       max_overflow=30,     # Maximum overflow connections
       pool_timeout=30,     # Connection timeout
       pool_recycle=3600    # Recycle connections after 1 hour
   )
   ```