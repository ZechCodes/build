# Session 2: Authentication & Authorization System

## Objective
Implement a secure, robust authentication and authorization system that protects all platform resources and provides a seamless user experience.

## Overview
This session builds upon the core infrastructure from Session 1 to create a comprehensive security layer. It implements JWT-based authentication, role-based access control, and all necessary security measures to protect user data and platform resources.

## Prerequisites
- Session 1 completed successfully
- Database schema operational
- Redis cluster functional
- Basic FastAPI application running

## Components to Implement

### 1. User Management System
**Location**: `auth-service/`

#### User Registration & Login
```python
# auth-service/models/user.py
from sqlalchemy import Column, String, Boolean, DateTime, Enum
from sqlalchemy.orm import relationship
from passlib.context import CryptContext
from .base import Base, UUIDMixin, TimestampMixin
import enum

class UserRole(enum.Enum):
    USER = "user"
    ADMIN = "admin"
    MODERATOR = "moderator"

class User(Base, UUIDMixin, TimestampMixin):
    __tablename__ = "users"
    
    email = Column(String(255), unique=True, nullable=False, index=True)
    username = Column(String(50), unique=True, nullable=False, index=True)
    password_hash = Column(String(255), nullable=False)
    is_active = Column(Boolean, default=True)
    is_verified = Column(Boolean, default=False)
    role = Column(Enum(UserRole), default=UserRole.USER)
    last_login = Column(DateTime(timezone=True))
    failed_login_attempts = Column(Integer, default=0)
    locked_until = Column(DateTime(timezone=True))
    
    # Relationships
    vm_instances = relationship("VMInstance", back_populates="user")
    sessions = relationship("Session", back_populates="user")

pwd_context = CryptContext(schemes=["bcrypt"], deprecated="auto")

def hash_password(password: str) -> str:
    return pwd_context.hash(password)

def verify_password(plain_password: str, hashed_password: str) -> bool:
    return pwd_context.verify(plain_password, hashed_password)
```

#### Authentication Schemas
```python
# auth-service/schemas/auth.py
from pydantic import BaseModel, EmailStr, validator
import re

class UserRegistration(BaseModel):
    email: EmailStr
    username: str
    password: str
    
    @validator('username')
    def validate_username(cls, v):
        if not re.match(r'^[a-zA-Z0-9_-]{3,30}$', v):
            raise ValueError('Username must be 3-30 characters, alphanumeric, underscore, or dash only')
        return v
    
    @validator('password')
    def validate_password(cls, v):
        if len(v) < 8:
            raise ValueError('Password must be at least 8 characters')
        if not re.search(r'[A-Z]', v):
            raise ValueError('Password must contain at least one uppercase letter')
        if not re.search(r'[a-z]', v):
            raise ValueError('Password must contain at least one lowercase letter')
        if not re.search(r'\d', v):
            raise ValueError('Password must contain at least one digit')
        if not re.search(r'[!@#$%^&*(),.?":{}|<>]', v):
            raise ValueError('Password must contain at least one special character')
        return v

class UserLogin(BaseModel):
    email: EmailStr
    password: str

class TokenResponse(BaseModel):
    access_token: str
    refresh_token: str
    token_type: str = "bearer"
    expires_in: int

class UserProfile(BaseModel):
    id: str
    email: str
    username: str
    role: str
    is_verified: bool
    created_at: datetime
```

### 2. JWT Token Management
**Location**: `auth-service/security/`

#### Token Generation & Validation
```python
# auth-service/security/jwt.py
from jose import JWTError, jwt
from datetime import datetime, timedelta
from typing import Optional, Dict, Any
from fastapi import HTTPException, status
import secrets

class JWTManager:
    def __init__(self, secret_key: str, algorithm: str = "HS256"):
        self.secret_key = secret_key
        self.algorithm = algorithm
        self.access_token_expire_minutes = 15
        self.refresh_token_expire_days = 7
    
    def create_access_token(self, data: Dict[Any, Any]) -> str:
        to_encode = data.copy()
        expire = datetime.utcnow() + timedelta(minutes=self.access_token_expire_minutes)
        to_encode.update({"exp": expire, "type": "access"})
        return jwt.encode(to_encode, self.secret_key, algorithm=self.algorithm)
    
    def create_refresh_token(self, data: Dict[Any, Any]) -> str:
        to_encode = data.copy()
        expire = datetime.utcnow() + timedelta(days=self.refresh_token_expire_days)
        to_encode.update({"exp": expire, "type": "refresh", "jti": secrets.token_urlsafe(32)})
        return jwt.encode(to_encode, self.secret_key, algorithm=self.algorithm)
    
    def verify_token(self, token: str, token_type: str = "access") -> Dict[Any, Any]:
        try:
            payload = jwt.decode(token, self.secret_key, algorithms=[self.algorithm])
            if payload.get("type") != token_type:
                raise HTTPException(
                    status_code=status.HTTP_401_UNAUTHORIZED,
                    detail="Invalid token type"
                )
            return payload
        except JWTError:
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Could not validate credentials"
            )

# auth-service/security/dependencies.py
from fastapi import Depends, HTTPException, status
from fastapi.security import HTTPBearer, HTTPAuthorizationCredentials
from sqlalchemy.ext.asyncio import AsyncSession
from .jwt import JWTManager
from ..database import get_db
from ..models.user import User

security = HTTPBearer()
jwt_manager = JWTManager(secret_key=settings.JWT_SECRET)

async def get_current_user(
    credentials: HTTPAuthorizationCredentials = Depends(security),
    db: AsyncSession = Depends(get_db)
) -> User:
    token = credentials.credentials
    payload = jwt_manager.verify_token(token)
    
    user_id = payload.get("sub")
    if user_id is None:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Invalid authentication credentials"
        )
    
    user = await db.get(User, user_id)
    if user is None or not user.is_active:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="User not found or inactive"
        )
    
    return user

async def get_current_active_user(current_user: User = Depends(get_current_user)) -> User:
    if not current_user.is_active:
        raise HTTPException(status_code=400, detail="Inactive user")
    return current_user
```

### 3. Rate Limiting & Security Middleware
**Location**: `auth-service/middleware/`

#### Rate Limiting Implementation
```python
# auth-service/middleware/rate_limiting.py
from fastapi import HTTPException, Request, status
from starlette.middleware.base import BaseHTTPMiddleware
import redis.asyncio as redis
import json
from datetime import datetime, timedelta

class RateLimitMiddleware(BaseHTTPMiddleware):
    def __init__(self, app, redis_url: str):
        super().__init__(app)
        self.redis = redis.from_url(redis_url)
        self.rate_limits = {
            "/auth/login": {"max_requests": 5, "window_minutes": 15},
            "/auth/register": {"max_requests": 3, "window_minutes": 60},
            "/auth/reset-password": {"max_requests": 3, "window_minutes": 60},
        }
    
    async def dispatch(self, request: Request, call_next):
        client_ip = self.get_client_ip(request)
        endpoint = request.url.path
        
        if endpoint in self.rate_limits:
            if await self.is_rate_limited(client_ip, endpoint):
                raise HTTPException(
                    status_code=status.HTTP_429_TOO_MANY_REQUESTS,
                    detail="Rate limit exceeded"
                )
        
        response = await call_next(request)
        
        if endpoint in self.rate_limits and response.status_code < 400:
            await self.record_request(client_ip, endpoint)
        
        return response
    
    async def is_rate_limited(self, client_ip: str, endpoint: str) -> bool:
        limit_config = self.rate_limits[endpoint]
        key = f"rate_limit:{client_ip}:{endpoint}"
        
        current_requests = await self.redis.get(key)
        if current_requests is None:
            return False
        
        return int(current_requests) >= limit_config["max_requests"]
    
    async def record_request(self, client_ip: str, endpoint: str):
        limit_config = self.rate_limits[endpoint]
        key = f"rate_limit:{client_ip}:{endpoint}"
        window_seconds = limit_config["window_minutes"] * 60
        
        await self.redis.incr(key)
        await self.redis.expire(key, window_seconds)
```

#### Account Lockout Protection
```python
# auth-service/security/lockout.py
from datetime import datetime, timedelta
from sqlalchemy.ext.asyncio import AsyncSession
from ..models.user import User

class AccountLockoutManager:
    def __init__(self, max_attempts: int = 5, lockout_duration_minutes: int = 30):
        self.max_attempts = max_attempts
        self.lockout_duration = timedelta(minutes=lockout_duration_minutes)
    
    async def record_failed_attempt(self, user: User, db: AsyncSession):
        user.failed_login_attempts += 1
        
        if user.failed_login_attempts >= self.max_attempts:
            user.locked_until = datetime.utcnow() + self.lockout_duration
        
        await db.commit()
    
    async def reset_failed_attempts(self, user: User, db: AsyncSession):
        user.failed_login_attempts = 0
        user.locked_until = None
        await db.commit()
    
    def is_locked(self, user: User) -> bool:
        if user.locked_until is None:
            return False
        return datetime.utcnow() < user.locked_until
```

### 4. WebSocket Authentication
**Location**: `websocket-gateway/auth/`

#### WebSocket Authentication Handler
```python
# websocket-gateway/auth/websocket_auth.py
from fastapi import WebSocket, HTTPException, status
from jose import JWTError
from ..security.jwt import JWTManager
from ..models.user import User
from ..database import get_db

class WebSocketAuthenticator:
    def __init__(self, jwt_manager: JWTManager):
        self.jwt_manager = jwt_manager
    
    async def authenticate_websocket(self, websocket: WebSocket) -> User:
        # Extract token from query parameters or headers
        token = websocket.query_params.get("token")
        if not token:
            # Try to get from headers
            token = websocket.headers.get("authorization")
            if token and token.startswith("Bearer "):
                token = token[7:]
        
        if not token:
            await websocket.close(code=status.WS_1008_POLICY_VIOLATION)
            raise HTTPException(status_code=401, detail="No token provided")
        
        try:
            payload = self.jwt_manager.verify_token(token)
            user_id = payload.get("sub")
            
            async with get_db() as db:
                user = await db.get(User, user_id)
                if not user or not user.is_active:
                    await websocket.close(code=status.WS_1008_POLICY_VIOLATION)
                    raise HTTPException(status_code=401, detail="Invalid user")
                
                return user
                
        except JWTError:
            await websocket.close(code=status.WS_1008_POLICY_VIOLATION)
            raise HTTPException(status_code=401, detail="Invalid token")
```

### 5. Authorization & Permissions
**Location**: `auth-service/authorization/`

#### Permission System
```python
# auth-service/authorization/permissions.py
from enum import Enum
from typing import List, Set
from fastapi import HTTPException, status
from ..models.user import User, UserRole

class Permission(Enum):
    # VM Management
    VM_CREATE = "vm:create"
    VM_DELETE = "vm:delete"
    VM_MODIFY = "vm:modify"
    VM_VIEW = "vm:view"
    
    # Session Management
    SESSION_CREATE = "session:create"
    SESSION_VIEW = "session:view"
    SESSION_TERMINATE = "session:terminate"
    
    # Snapshot Management
    SNAPSHOT_CREATE = "snapshot:create"
    SNAPSHOT_DELETE = "snapshot:delete"
    SNAPSHOT_RESTORE = "snapshot:restore"
    SNAPSHOT_VIEW = "snapshot:view"
    
    # Admin Functions
    USER_MANAGE = "user:manage"
    SYSTEM_ADMIN = "system:admin"

class PermissionChecker:
    role_permissions = {
        UserRole.USER: {
            Permission.VM_CREATE, Permission.VM_DELETE, Permission.VM_MODIFY, Permission.VM_VIEW,
            Permission.SESSION_CREATE, Permission.SESSION_VIEW, Permission.SESSION_TERMINATE,
            Permission.SNAPSHOT_CREATE, Permission.SNAPSHOT_DELETE, Permission.SNAPSHOT_RESTORE, Permission.SNAPSHOT_VIEW
        },
        UserRole.MODERATOR: {
            # Include all user permissions plus some admin ones
            *role_permissions.get(UserRole.USER, set()),
            Permission.USER_MANAGE
        },
        UserRole.ADMIN: {
            # Include all permissions
            *[perm for perm in Permission]
        }
    }
    
    @classmethod
    def user_has_permission(cls, user: User, permission: Permission) -> bool:
        user_permissions = cls.role_permissions.get(user.role, set())
        return permission in user_permissions
    
    @classmethod
    def require_permission(cls, permission: Permission):
        def decorator(func):
            async def wrapper(*args, **kwargs):
                # Extract user from dependencies
                current_user = kwargs.get('current_user')
                if not current_user:
                    raise HTTPException(
                        status_code=status.HTTP_401_UNAUTHORIZED,
                        detail="Authentication required"
                    )
                
                if not cls.user_has_permission(current_user, permission):
                    raise HTTPException(
                        status_code=status.HTTP_403_FORBIDDEN,
                        detail="Insufficient permissions"
                    )
                
                return await func(*args, **kwargs)
            return wrapper
        return decorator
```

## Critical Decisions

### Token Expiration Strategy
- **Access Token**: 15 minutes (short-lived for security)
- **Refresh Token**: 7 days (balance between security and UX)
- **Automatic Refresh**: Frontend handles token refresh transparently

### Password Security
- **Hashing**: bcrypt with 12 rounds minimum
- **Complexity**: 8+ characters, mixed case, numbers, special characters
- **Reset Flow**: Time-limited tokens, email verification required

### Rate Limiting Thresholds
- **Login Attempts**: 5 per 15 minutes per IP
- **Registration**: 3 per hour per IP
- **Password Reset**: 3 per hour per IP
- **Account Lockout**: 5 failed attempts = 30 minute lockout

### Session Management
- **Session Storage**: Redis for fast access
- **Session Timeout**: 30 minutes of inactivity
- **Concurrent Sessions**: Unlimited (user preference)

## Security Checklist ✅

### Authentication Security
- [ ] Passwords hashed with bcrypt (minimum 12 rounds)
- [ ] Strong password requirements enforced (complexity, length)
- [ ] JWT secrets cryptographically secure and rotated regularly
- [ ] Token expiration times appropriate (15min access, 7 days refresh)
- [ ] Secure token storage (HTTP-only cookies for web, secure storage for mobile)
- [ ] Account lockout after failed attempts (5 attempts = 30min lockout)
- [ ] Rate limiting on authentication endpoints (5 attempts/15min per IP)
- [ ] Timing attack prevention (constant-time password verification)
- [ ] User enumeration prevention (consistent responses for valid/invalid users)
- [ ] Secure password reset flow with time-limited tokens

### Authorization Security
- [ ] Role-based access control (RBAC) properly implemented
- [ ] Permission checks on all protected endpoints
- [ ] Resource ownership validation (users can only access their resources)
- [ ] SQL injection prevention via ORM and parameterized queries
- [ ] Cross-user data isolation enforced
- [ ] Administrative functions properly protected
- [ ] WebSocket connections authenticated and authorized
- [ ] Session hijacking prevention (secure session tokens)
- [ ] Privilege escalation protection
- [ ] Authorization bypass testing completed

### Transport Security
- [ ] HTTPS enforced for all authentication endpoints
- [ ] Secure WebSocket connections (WSS) required
- [ ] CORS properly configured with specific allowed origins
- [ ] Security headers implemented (HSTS, CSP, X-Frame-Options)
- [ ] Certificate validation in production environment
- [ ] HTTP Strict Transport Security (HSTS) enabled
- [ ] Secure cookie attributes set (Secure, HttpOnly, SameSite)
- [ ] Content Security Policy (CSP) configured
- [ ] X-Content-Type-Options: nosniff header
- [ ] Referrer-Policy configured appropriately

### Session Security
- [ ] Session tokens cryptographically random
- [ ] Session fixation prevention (new token on login)
- [ ] Secure session storage (Redis with authentication)
- [ ] Session timeout implementation (30 minutes inactivity)
- [ ] Concurrent session management
- [ ] Session invalidation on logout and password change
- [ ] WebSocket session authentication
- [ ] Session replay attack prevention
- [ ] Cross-session contamination prevention
- [ ] Session enumeration prevention

### Input Validation & Data Protection
- [ ] Input validation on all authentication endpoints
- [ ] Email validation and sanitization
- [ ] Username validation (alphanumeric + safe characters)
- [ ] Password strength validation
- [ ] Request size limits enforced
- [ ] File upload restrictions (if applicable)
- [ ] Output encoding to prevent XSS
- [ ] Error messages don't expose sensitive information
- [ ] Logging excludes sensitive data (passwords, tokens)
- [ ] Data retention policies implemented

## Testing Requirements

### Authentication Testing
- [ ] Valid user registration flow
- [ ] Invalid registration attempts (weak passwords, duplicate users)
- [ ] Valid login flow with correct credentials
- [ ] Invalid login attempts (wrong password, non-existent user)
- [ ] Account lockout functionality
- [ ] Password reset flow (request, validation, completion)
- [ ] Token refresh mechanism
- [ ] Token expiration handling
- [ ] Logout functionality

### Authorization Testing
- [ ] Role-based permission enforcement
- [ ] Resource ownership validation
- [ ] Cross-user access prevention
- [ ] Permission escalation attempts
- [ ] WebSocket authorization
- [ ] API endpoint authorization
- [ ] Admin function protection
- [ ] Unauthorized access attempts

### Security Testing
- [ ] Brute force attack simulation (login, registration)
- [ ] Rate limiting effectiveness
- [ ] Session fixation prevention
- [ ] Token manipulation attempts
- [ ] SQL injection attempts on auth endpoints
- [ ] Cross-site scripting (XSS) prevention
- [ ] Cross-site request forgery (CSRF) protection
- [ ] Man-in-the-middle attack prevention
- [ ] Timing attack resistance
- [ ] Password complexity enforcement

### Functionality Testing
- [ ] Authentication functionality verification
- [ ] Token generation/validation functionality
- [ ] Rate limiting accuracy
- [ ] Redis session storage functionality
- [ ] Database query functionality for auth operations

## Monitoring & Alerting

### Security Metrics
- Failed login attempts per minute/hour
- Account lockouts triggered
- Suspicious IP addresses (multiple failed attempts)
- Token validation failures
- Permission denied events
- Password reset requests
- New user registrations

### Operational Metrics
- Authentication request counts
- Token generation/validation counts
- Rate limiting trigger counts
- Redis connection status
- Database query success rates for auth operations

### Alert Conditions
- Failed login attempts > 100/hour
- Account lockouts > 10/hour
- Token validation failures > 5% of total requests
- Authentication failure rate > 5%
- Redis connection failures
- Suspicious activity patterns (rapid requests from single IP)

## API Endpoints

### Authentication Endpoints
```python
POST /auth/register          # User registration
POST /auth/login             # User login
POST /auth/logout            # User logout
POST /auth/refresh           # Token refresh
POST /auth/reset-password    # Request password reset
POST /auth/confirm-reset     # Confirm password reset
GET  /auth/me               # Get current user profile
PUT  /auth/me               # Update user profile
```

### Authorization Endpoints
```python
GET  /auth/permissions      # Get user permissions
POST /auth/check-permission # Check specific permission
```

## Documentation Deliverables

### API Documentation
- [ ] OpenAPI specification for all authentication endpoints
- [ ] Authentication flow diagrams
- [ ] Error response documentation
- [ ] Rate limiting documentation
- [ ] WebSocket authentication guide

### Security Documentation
- [ ] Security architecture overview
- [ ] Threat model for authentication system
- [ ] Security controls documentation
- [ ] Incident response procedures
- [ ] Security testing procedures

### Developer Documentation
- [ ] Integration guide for frontend developers
- [ ] Authentication middleware usage
- [ ] Permission system guide
- [ ] Testing authentication in development
- [ ] Troubleshooting common issues

## Next Steps

Upon successful completion of Session 2:
1. All authentication endpoints functional and secure
2. Authorization system enforcing permissions correctly
3. Rate limiting and account lockout working
4. WebSocket authentication operational
5. Security testing completed with no critical issues
6. Proceed to Session 3: Firecracker VM Management Core

## Risk Mitigation

### Security Risks
1. **Credential theft**: Multi-factor authentication (future), secure storage
2. **Brute force attacks**: Rate limiting, account lockout, monitoring
3. **Session hijacking**: Secure tokens, HTTPS enforcement, session validation
4. **Privilege escalation**: Strict permission checks, regular audits
5. **Token manipulation**: Secure signing, validation, expiration

### Operational Risks
1. **Account lockout abuse**: Administrative unlock procedures
2. **Password reset abuse**: Rate limiting, email verification
3. **Service degradation**: Monitoring, redundancy, scaling
4. **Service unavailability**: Health checks, redundancy, monitoring

---

**Session 2 Success Criteria:**
- Secure authentication system operational
- Authorization properly enforcing permissions
- Rate limiting and security measures active
- WebSocket authentication functional
- Security testing passed with no critical vulnerabilities
- Ready for Session 3 VM management implementation