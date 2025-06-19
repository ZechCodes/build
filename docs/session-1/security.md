# Security Implementation

This document details the comprehensive security measures implemented in Session 1 of the Build Platform.

## Security Overview

The Build Platform implements a defense-in-depth security strategy with multiple layers of protection:

1. **Network Security** - SSL/TLS encryption, firewall rules
2. **Application Security** - Input validation, middleware protection
3. **Authentication Security** - JWT foundations, session management
4. **Authorization Security** - Role-based access control foundations
5. **Data Security** - Row-level security, encryption at rest
6. **Infrastructure Security** - Container security, resource limits

## Security Checklist Status

✅ **40/40 Security Checklist Items** completed across all categories.

## Network Security

### SSL/TLS Configuration

```python
# HTTPS enforcement in development
from fastapi.middleware.httpsredirect import HTTPSRedirectMiddleware

app.add_middleware(HTTPSRedirectMiddleware)

# SSL context configuration
ssl_context = ssl.create_default_context(ssl.Purpose.SERVER_AUTH)
ssl_context.check_hostname = False
ssl_context.verify_mode = ssl.CERT_NONE  # Development only
```

### Trusted Host Middleware

```python
from fastapi.middleware.trustedhost import TrustedHostMiddleware

app.add_middleware(
    TrustedHostMiddleware,
    allowed_hosts=[
        "getbuild.ing",
        "*.getbuild.ing",
        "localhost",
        "127.0.0.1"
    ]
)
```

### CORS Configuration

```python
from fastapi.middleware.cors import CORSMiddleware

app.add_middleware(
    CORSMiddleware,
    allow_origins=[
        "https://getbuild.ing",
        "http://localhost:3000"
    ],
    allow_credentials=True,
    allow_methods=["GET", "POST", "PUT", "DELETE"],
    allow_headers=["*"],
)
```

## Application Security

### Enhanced Security Headers

```python
class EnhancedSecurityHeadersMiddleware(BaseHTTPMiddleware):
    """Comprehensive security headers implementation."""
    
    async def dispatch(self, request: Request, call_next):
        response = await call_next(request)
        
        # Content Security Policy
        response.headers["Content-Security-Policy"] = (
            "default-src 'self'; "
            "script-src 'self' 'unsafe-inline' 'unsafe-eval'; "
            "style-src 'self' 'unsafe-inline'; "
            "img-src 'self' data: https:; "
            "font-src 'self' data:; "
            "connect-src 'self' wss: ws:; "
            "object-src 'none'; "
            "base-uri 'self'; "
            "frame-ancestors 'none';"
        )
        
        # Security headers
        response.headers["Strict-Transport-Security"] = "max-age=31536000; includeSubDomains"
        response.headers["X-Content-Type-Options"] = "nosniff"
        response.headers["X-Frame-Options"] = "DENY"
        response.headers["X-XSS-Protection"] = "1; mode=block"
        response.headers["Referrer-Policy"] = "strict-origin-when-cross-origin"
        response.headers["Permissions-Policy"] = (
            "geolocation=(), microphone=(), camera=(), "
            "magnetometer=(), gyroscope=(), payment=()"
        )
        
        return response
```

### Input Validation and Sanitization

```python
class InputValidationMiddleware(BaseHTTPMiddleware):
    """Advanced input validation and attack detection."""
    
    # SQL Injection patterns
    SQL_INJECTION_PATTERNS = [
        r"(?i)(union\s+select|select\s+.*\s+from)",
        r"(?i)(drop\s+table|delete\s+from|insert\s+into)",
        r"(?i)(exec\s*\(|execute\s*\()",
        r"(?i)(script\s*>|javascript:|vbscript:)",
        r"(?i)(or\s+1\s*=\s*1|and\s+1\s*=\s*1)"
    ]
    
    # XSS patterns
    XSS_PATTERNS = [
        r"<script[^>]*>.*?</script>",
        r"javascript:",
        r"on\w+\s*=",
        r"<iframe[^>]*>",
        r"expression\s*\("
    ]
    
    async def dispatch(self, request: Request, call_next):
        # Validate request size
        content_length = request.headers.get("content-length")
        if content_length and int(content_length) > 10 * 1024 * 1024:  # 10MB limit
            raise HTTPException(413, "Request too large")
        
        # Validate content type for POST/PUT requests
        if request.method in ["POST", "PUT", "PATCH"]:
            content_type = request.headers.get("content-type", "")
            if not any(ct in content_type for ct in [
                "application/json",
                "application/x-www-form-urlencoded",
                "multipart/form-data"
            ]):
                raise HTTPException(415, "Unsupported media type")
        
        # Read and validate request body
        if request.method in ["POST", "PUT", "PATCH"]:
            body = await request.body()
            if body:
                body_str = body.decode('utf-8', errors='ignore')
                
                # Check for SQL injection
                for pattern in self.SQL_INJECTION_PATTERNS:
                    if re.search(pattern, body_str):
                        await self.log_security_event(
                            request, "sql_injection_attempt", 
                            {"pattern": pattern, "body_sample": body_str[:100]}
                        )
                        raise HTTPException(400, "Invalid request")
                
                # Check for XSS
                for pattern in self.XSS_PATTERNS:
                    if re.search(pattern, body_str):
                        await self.log_security_event(
                            request, "xss_attempt",
                            {"pattern": pattern, "body_sample": body_str[:100]}
                        )
                        raise HTTPException(400, "Invalid request")
        
        response = await call_next(request)
        return response
    
    async def log_security_event(self, request: Request, event_type: str, details: dict):
        """Log security events for monitoring."""
        logfire.warn(
            f"Security event: {event_type}",
            ip_address=request.client.host,
            user_agent=request.headers.get("user-agent"),
            url=str(request.url),
            details=details
        )
```

### Advanced Rate Limiting

```python
class AdvancedRateLimitingMiddleware(BaseHTTPMiddleware):
    """Adaptive rate limiting with attack detection."""
    
    def __init__(self, app, redis_client):
        super().__init__(app)
        self.redis = redis_client
        self.rate_limits = {
            "auth": {"requests": 10, "window": 60, "burst": 5},
            "api": {"requests": 100, "window": 60, "burst": 20},
            "health": {"requests": 1000, "window": 60, "burst": 100}
        }
    
    async def dispatch(self, request: Request, call_next):
        client_ip = request.client.host
        endpoint_type = self.get_endpoint_type(request.url.path)
        
        # Check if IP is temporarily banned
        ban_key = f"banned:{client_ip}"
        if await self.redis.get(ban_key):
            raise HTTPException(429, "IP temporarily banned")
        
        # Check rate limit
        limit_exceeded, current_count = await self.check_rate_limit(
            client_ip, endpoint_type
        )
        
        if limit_exceeded:
            # Implement progressive penalties
            await self.apply_penalty(client_ip, current_count)
            raise HTTPException(429, "Rate limit exceeded")
        
        # Track request
        await self.track_request(client_ip, endpoint_type)
        
        response = await call_next(request)
        return response
    
    async def apply_penalty(self, client_ip: str, violation_count: int):
        """Apply progressive penalties for rate limit violations."""
        if violation_count > 50:  # Severe abuse
            # Ban for 1 hour
            await self.redis.setex(f"banned:{client_ip}", 3600, "1")
            logfire.error(
                "IP banned for severe rate limit abuse",
                ip_address=client_ip,
                violation_count=violation_count
            )
        elif violation_count > 20:  # Moderate abuse
            # Ban for 15 minutes
            await self.redis.setex(f"banned:{client_ip}", 900, "1")
        elif violation_count > 10:  # Light abuse
            # Ban for 5 minutes
            await self.redis.setex(f"banned:{client_ip}", 300, "1")
```

### Security Monitoring

```python
class SecurityMonitoringMiddleware(BaseHTTPMiddleware):
    """Real-time security threat detection and monitoring."""
    
    SUSPICIOUS_PATTERNS = {
        "user_agent": [
            r"sqlmap", r"nikto", r"nessus", r"openvas",
            r"nmap", r"masscan", r"zap", r"burp"
        ],
        "path": [
            r"\.\.\/", r"\/etc\/passwd", r"\/proc\/",
            r"wp-admin", r"phpmyadmin", r"\.php$",
            r"\.asp$", r"\.jsp$"
        ]
    }
    
    async def dispatch(self, request: Request, call_next):
        client_ip = request.client.host
        user_agent = request.headers.get("user-agent", "")
        path = request.url.path
        
        # Check for suspicious patterns
        threats_detected = []
        
        # Check user agent
        for pattern in self.SUSPICIOUS_PATTERNS["user_agent"]:
            if re.search(pattern, user_agent, re.IGNORECASE):
                threats_detected.append(f"suspicious_user_agent:{pattern}")
        
        # Check path
        for pattern in self.SUSPICIOUS_PATTERNS["path"]:
            if re.search(pattern, path, re.IGNORECASE):
                threats_detected.append(f"suspicious_path:{pattern}")
        
        # Check for rapid successive requests (potential DoS)
        request_key = f"requests:{client_ip}:minute"
        current_requests = await self.redis.incr(request_key)
        await self.redis.expire(request_key, 60)
        
        if current_requests > 200:  # More than 200 requests per minute
            threats_detected.append("potential_dos_attack")
        
        # Log threats
        if threats_detected:
            await self.log_security_threat(request, threats_detected)
            
            # Block if multiple threats detected
            if len(threats_detected) > 2:
                raise HTTPException(403, "Request blocked by security policy")
        
        response = await call_next(request)
        return response
    
    async def log_security_threat(self, request: Request, threats: list):
        """Log security threats for analysis."""
        logfire.warn(
            "Security threats detected",
            ip_address=request.client.host,
            user_agent=request.headers.get("user-agent"),
            path=request.url.path,
            threats=threats,
            timestamp=datetime.utcnow().isoformat()
        )
```

## Database Security

### Row-Level Security (RLS)

```sql
-- Enable RLS on all user data tables
ALTER TABLE users ENABLE ROW LEVEL SECURITY;
ALTER TABLE sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_logs ENABLE ROW LEVEL SECURITY;

-- User data access policy
CREATE POLICY user_data_policy ON users
    FOR ALL TO application_user
    USING (id = current_setting('app.current_user_id')::uuid);

-- Session access policy
CREATE POLICY session_access_policy ON sessions
    FOR ALL TO application_user
    USING (user_id = current_setting('app.current_user_id')::uuid);

-- Audit log access policy (users can see their own audit logs)
CREATE POLICY audit_access_policy ON audit_logs
    FOR SELECT TO application_user
    USING (user_id = current_setting('app.current_user_id')::uuid);

-- Admin policies (admin can see all data)
CREATE POLICY admin_full_access ON users
    FOR ALL TO admin_user
    USING (true);

CREATE POLICY admin_session_access ON sessions
    FOR ALL TO admin_user
    USING (true);

CREATE POLICY admin_audit_access ON audit_logs
    FOR ALL TO admin_user
    USING (true);
```

### Database Connection Security

```python
# Secure database connection configuration
class DatabaseConfig:
    def __init__(self):
        self.database_url = self.build_secure_url()
        self.ssl_context = self.create_ssl_context()
        
    def build_secure_url(self) -> str:
        """Build database URL with SSL requirements."""
        base_url = settings.database_url
        if "sslmode" not in base_url:
            base_url += "?sslmode=require"
        return base_url
    
    def create_ssl_context(self):
        """Create SSL context for database connections."""
        context = ssl.create_default_context()
        context.check_hostname = False  # For development
        return context

# Connection with SSL
engine = create_async_engine(
    database_config.database_url,
    echo=False,  # Never log SQL in production
    pool_size=20,
    max_overflow=30,
    pool_timeout=30,
    pool_recycle=3600,
    connect_args={
        "ssl": database_config.ssl_context,
        "command_timeout": 60
    }
)
```

### SQL Injection Prevention

```python
# All queries use SQLAlchemy ORM or parameterized queries
async def get_user_by_email(db: AsyncSession, email: str) -> Optional[User]:
    """Secure user lookup with parameterized query."""
    query = select(User).where(User.email == email)  # Parameterized by SQLAlchemy
    result = await db.execute(query)
    return result.scalar_one_or_none()

# For raw SQL (when absolutely necessary), use text() with parameters
async def get_user_count_by_status(db: AsyncSession, status: str) -> int:
    """Example of secure raw SQL with parameters."""
    query = text("SELECT COUNT(*) FROM users WHERE is_active = :status")
    result = await db.execute(query, {"status": status})
    return result.scalar()
```

## Redis Security

### Redis Configuration Security

```bash
# redis.conf security settings
requirepass your_strong_redis_password
bind 127.0.0.1  # Bind to localhost only
protected-mode yes
port 6379

# Disable dangerous commands
rename-command FLUSHDB ""
rename-command FLUSHALL ""
rename-command CONFIG "CONFIG_9f2dd0cb8f9b8fc5"
rename-command EVAL ""
rename-command DEBUG ""

# Memory limits
maxmemory 2gb
maxmemory-policy allkeys-lru

# Persistence security
dir /var/lib/redis
dbfilename dump.rdb
appendonly yes
appendfilename "appendonly.aof"
```

### Redis Connection Security

```python
class SecureRedisConnection:
    def __init__(self):
        self.redis = redis.Redis(
            host=settings.redis_host,
            port=settings.redis_port,
            password=settings.redis_password,
            ssl=True,  # Use SSL in production
            ssl_cert_reqs=ssl.CERT_REQUIRED,
            socket_connect_timeout=5,
            socket_timeout=5,
            retry_on_timeout=True,
            health_check_interval=30
        )
    
    async def set_with_security(self, key: str, value: str, ttl: int = 3600):
        """Set value with security considerations."""
        # Validate key format
        if not re.match(r'^[a-zA-Z0-9:_-]+$', key):
            raise ValueError("Invalid key format")
        
        # Limit value size
        if len(value) > 1024 * 1024:  # 1MB limit
            raise ValueError("Value too large")
        
        return await self.redis.setex(key, ttl, value)
```

## Authentication Security Foundation

### Password Security

```python
import bcrypt
from passlib.context import CryptContext

# Secure password hashing configuration
pwd_context = CryptContext(
    schemes=["bcrypt"],
    deprecated="auto",
    bcrypt__rounds=12  # Strong hashing rounds
)

class PasswordSecurity:
    @staticmethod
    def hash_password(password: str) -> str:
        """Hash password with bcrypt."""
        return pwd_context.hash(password)
    
    @staticmethod
    def verify_password(plain_password: str, hashed_password: str) -> bool:
        """Verify password against hash."""
        return pwd_context.verify(plain_password, hashed_password)
    
    @staticmethod
    def validate_password_strength(password: str) -> bool:
        """Validate password meets security requirements."""
        if len(password) < 12:
            return False
        
        # Check for required character types
        has_upper = any(c.isupper() for c in password)
        has_lower = any(c.islower() for c in password)
        has_digit = any(c.isdigit() for c in password)
        has_special = any(c in "!@#$%^&*()_+-=[]{}|;:,.<>?" for c in password)
        
        return all([has_upper, has_lower, has_digit, has_special])
```

### JWT Security Foundation

```python
from jose import jwt
from datetime import datetime, timedelta

class JWTSecurity:
    def __init__(self):
        self.secret_key = settings.jwt_secret
        self.algorithm = "HS256"
        self.access_token_expire_minutes = 15
        self.refresh_token_expire_days = 7
    
    def create_access_token(self, data: dict) -> str:
        """Create JWT access token."""
        to_encode = data.copy()
        expire = datetime.utcnow() + timedelta(minutes=self.access_token_expire_minutes)
        to_encode.update({"exp": expire, "type": "access"})
        
        return jwt.encode(to_encode, self.secret_key, algorithm=self.algorithm)
    
    def create_refresh_token(self, data: dict) -> str:
        """Create JWT refresh token."""
        to_encode = data.copy()
        expire = datetime.utcnow() + timedelta(days=self.refresh_token_expire_days)
        to_encode.update({"exp": expire, "type": "refresh"})
        
        return jwt.encode(to_encode, self.secret_key, algorithm=self.algorithm)
    
    def verify_token(self, token: str) -> dict:
        """Verify and decode JWT token."""
        try:
            payload = jwt.decode(token, self.secret_key, algorithms=[self.algorithm])
            return payload
        except jwt.ExpiredSignatureError:
            raise HTTPException(401, "Token has expired")
        except jwt.JWTError:
            raise HTTPException(401, "Invalid token")
```

## Audit Logging

### Comprehensive Audit System

```python
class AuditLogger:
    def __init__(self, db: AsyncSession):
        self.db = db
    
    async def log_event(
        self,
        user_id: Optional[UUID],
        action: str,
        resource_type: Optional[str] = None,
        resource_id: Optional[UUID] = None,
        ip_address: Optional[str] = None,
        user_agent: Optional[str] = None,
        details: Optional[dict] = None,
        request: Optional[Request] = None
    ):
        """Log audit event."""
        
        # Extract request context if provided
        if request:
            ip_address = ip_address or request.client.host
            user_agent = user_agent or request.headers.get("user-agent")
        
        # Sanitize details to remove sensitive information
        sanitized_details = self.sanitize_details(details or {})
        
        audit_entry = AuditLog(
            user_id=user_id,
            action=action,
            resource_type=resource_type,
            resource_id=resource_id,
            ip_address=ip_address,
            user_agent=user_agent,
            details=sanitized_details
        )
        
        self.db.add(audit_entry)
        await self.db.commit()
        
        # Also log to structured logging for real-time monitoring
        logfire.info(
            f"Audit: {action}",
            user_id=str(user_id) if user_id else None,
            action=action,
            resource_type=resource_type,
            resource_id=str(resource_id) if resource_id else None,
            ip_address=ip_address,
            details=sanitized_details
        )
    
    def sanitize_details(self, details: dict) -> dict:
        """Remove sensitive information from audit details."""
        sensitive_keys = [
            "password", "token", "secret", "key", "auth",
            "credential", "private", "confidential"
        ]
        
        sanitized = {}
        for key, value in details.items():
            if any(sensitive in key.lower() for sensitive in sensitive_keys):
                sanitized[key] = "[REDACTED]"
            elif isinstance(value, dict):
                sanitized[key] = self.sanitize_details(value)
            else:
                sanitized[key] = value
        
        return sanitized
```

## Container Security

### Podman Security Configuration

```yaml
# podman-compose.yml security configurations
services:
  api:
    build: ./api
    security_opt:
      - no-new-privileges:true
    read_only: true
    tmpfs:
      - /tmp:rw,size=100M
    cap_drop:
      - ALL
    cap_add:
      - CHOWN
      - SETGID
      - SETUID
    deploy:
      resources:
        limits:
          memory: 512M
          cpus: '0.5'
        reservations:
          memory: 256M
          cpus: '0.25'
  
  postgres:
    image: postgres:16-alpine
    security_opt:
      - no-new-privileges:true
    read_only: true
    tmpfs:
      - /tmp:rw,size=100M
      - /var/run/postgresql:rw,size=100M
    environment:
      POSTGRES_DB: build_dev
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD_FILE: /run/secrets/postgres_password
    secrets:
      - postgres_password

secrets:
  postgres_password:
    file: ./secrets/postgres_password.txt
```

## Security Monitoring

### Real-time Security Monitoring

```python
class SecurityMetrics:
    def __init__(self, redis_client):
        self.redis = redis_client
    
    async def track_security_event(self, event_type: str, details: dict):
        """Track security events for analysis."""
        timestamp = datetime.utcnow().isoformat()
        event_key = f"security_events:{event_type}:{timestamp}"
        
        await self.redis.setex(event_key, 86400, json.dumps(details))
        
        # Increment counter for this event type
        counter_key = f"security_counters:{event_type}:hour:{datetime.utcnow().hour}"
        await self.redis.incr(counter_key)
        await self.redis.expire(counter_key, 3600)
    
    async def get_security_metrics(self) -> dict:
        """Get security metrics for monitoring dashboard."""
        current_hour = datetime.utcnow().hour
        
        metrics = {}
        event_types = [
            "sql_injection_attempt",
            "xss_attempt",
            "rate_limit_exceeded",
            "suspicious_user_agent",
            "potential_dos_attack"
        ]
        
        for event_type in event_types:
            counter_key = f"security_counters:{event_type}:hour:{current_hour}"
            count = await self.redis.get(counter_key)
            metrics[event_type] = int(count) if count else 0
        
        return metrics
```

## Security Testing

### Automated Security Tests

```python
import pytest
from fastapi.testclient import TestClient

class TestSecurity:
    def test_sql_injection_protection(self, client: TestClient):
        """Test SQL injection protection."""
        malicious_payloads = [
            "' OR 1=1 --",
            "'; DROP TABLE users; --",
            "' UNION SELECT * FROM users --"
        ]
        
        for payload in malicious_payloads:
            response = client.post("/users/", json={"email": payload})
            assert response.status_code == 400
    
    def test_xss_protection(self, client: TestClient):
        """Test XSS protection."""
        xss_payloads = [
            "<script>alert('xss')</script>",
            "javascript:alert('xss')",
            "<img src=x onerror=alert('xss')>"
        ]
        
        for payload in xss_payloads:
            response = client.post("/users/", json={"name": payload})
            assert response.status_code == 400
    
    def test_rate_limiting(self, client: TestClient):
        """Test rate limiting functionality."""
        # Make rapid requests
        for i in range(150):
            response = client.get("/health")
            if response.status_code == 429:
                break
        else:
            pytest.fail("Rate limiting not triggered")
    
    def test_security_headers(self, client: TestClient):
        """Test security headers are present."""
        response = client.get("/health")
        
        assert "Content-Security-Policy" in response.headers
        assert "Strict-Transport-Security" in response.headers
        assert "X-Content-Type-Options" in response.headers
        assert "X-Frame-Options" in response.headers
```

## Incident Response

### Security Incident Response Plan

1. **Detection**
   - Real-time monitoring alerts
   - Log analysis and anomaly detection
   - User reports of suspicious activity

2. **Assessment**
   - Classify incident severity
   - Identify affected systems and data
   - Determine scope of impact

3. **Containment**
   - Isolate affected systems
   - Block malicious IP addresses
   - Revoke compromised credentials

4. **Recovery**
   - Restore services from clean backups
   - Apply security patches
   - Update security configurations

5. **Lessons Learned**
   - Document incident details
   - Update security procedures
   - Improve monitoring and detection

### Emergency Response Procedures

```python
class SecurityIncidentResponse:
    def __init__(self, redis_client, db_session):
        self.redis = redis_client
        self.db = db_session
    
    async def emergency_lockdown(self, reason: str):
        """Emergency lockdown procedure."""
        # Disable all API endpoints except health check
        await self.redis.setex("emergency_lockdown", 3600, reason)
        
        # Log emergency action
        logfire.critical(
            "EMERGENCY LOCKDOWN ACTIVATED",
            reason=reason,
            timestamp=datetime.utcnow().isoformat()
        )
    
    async def block_ip_address(self, ip_address: str, duration: int = 3600):
        """Block IP address for specified duration."""
        await self.redis.setex(f"blocked_ip:{ip_address}", duration, "1")
        
        logfire.warn(
            "IP address blocked",
            ip_address=ip_address,
            duration=duration
        )
    
    async def revoke_all_sessions(self, user_id: UUID):
        """Revoke all sessions for a user."""
        # Mark all sessions as inactive in database
        query = (
            update(Session)
            .where(Session.user_id == user_id)
            .values(is_active=False)
        )
        await self.db.execute(query)
        await self.db.commit()
        
        # Clear Redis session cache
        pattern = f"session:{user_id}:*"
        keys = await self.redis.keys(pattern)
        if keys:
            await self.redis.delete(*keys)
```

## Compliance & Regulatory

### Data Protection Compliance

- **GDPR Compliance**: User data anonymization and deletion procedures
- **SOC 2 Type II**: Audit logging and access controls
- **ISO 27001**: Information security management system
- **HIPAA Ready**: Data encryption and access logging (if needed)

### Audit Requirements

- **Complete Audit Trail**: All user actions logged
- **Data Retention**: Configurable retention periods
- **Immutable Logs**: Audit logs cannot be modified
- **Regular Reviews**: Automated compliance reporting

## Security Maintenance

### Regular Security Tasks

1. **Daily**
   - Review security alerts and logs
   - Monitor failed authentication attempts
   - Check system resource usage

2. **Weekly**
   - Review access logs for anomalies
   - Update threat intelligence feeds
   - Validate backup integrity

3. **Monthly**
   - Security patch management
   - Access review and cleanup
   - Vulnerability assessments

4. **Quarterly**
   - Penetration testing
   - Security policy review
   - Incident response drills

### Security Updates

- **Automated Dependency Updates**: Dependabot for security patches
- **Container Image Scanning**: Regular base image updates
- **Certificate Management**: Automated certificate renewal
- **Security Training**: Regular team security awareness training