# Infrastructure Security Enhancements

**Priority**: High  
**Effort**: Medium  
**Timeline**: Next Sprint  
**Security Impact**: Critical for production deployment

## Overview

Infrastructure-level security enhancements focus on securing the communication channels and storage mechanisms used by the Session Management system. These features address the foundation security requirements that are typically handled at the deployment and infrastructure level.

## Moved from Session 6

The following items were identified during Session 6 development but moved to future implementation due to their infrastructure-focused nature:

### 1. Redis TLS/SSL Encryption Configuration
- **Original requirement**: "Secure session storage with encrypted Redis connection"
- **Current status**: Redis connections use standard TCP without encryption
- **Security gap**: Session data transmitted to Redis in plaintext

### 2. WebSocket Secure (WSS) Enforcement  
- **Original requirement**: "Recovery data transmission uses encrypted channels"
- **Current status**: WebSocket connections support both WS and WSS
- **Security gap**: No enforcement of encrypted WebSocket connections

### 3. Network Security Configuration
- **Original requirement**: Implied by secure storage and transmission requirements
- **Current status**: Network isolation and firewall rules not specified
- **Security gap**: No network-level access controls defined

## Technical Implementation

### Redis TLS Configuration

**Objective**: Encrypt all Redis communications to protect session data in transit.

**Implementation Approach**:

```python
# Redis TLS Configuration
import redis.asyncio as redis
import ssl

# Create SSL context for Redis connections
def create_redis_ssl_context():
    context = ssl.create_default_context()
    context.check_hostname = False  # For development with self-signed certs
    context.verify_mode = ssl.CERT_REQUIRED
    
    # Configure certificates
    context.load_verify_locations('/path/to/redis-ca.crt')
    context.load_cert_chain('/path/to/redis-client.crt', '/path/to/redis-client.key')
    
    return context

# Updated Redis client initialization
class RedisConfig:
    def __init__(self):
        self.ssl_context = create_redis_ssl_context()
        
    async def create_redis_client(self, redis_url: str):
        """Create Redis client with TLS encryption"""
        # Parse Redis URL and add SSL parameters
        if redis_url.startswith('redis://'):
            redis_url = redis_url.replace('redis://', 'rediss://', 1)
        
        return redis.from_url(
            redis_url,
            ssl=self.ssl_context,
            ssl_cert_reqs=ssl.CERT_REQUIRED,
            ssl_check_hostname=False,  # Configure based on cert setup
            decode_responses=False  # Keep binary mode for session data
        )

# Configuration updates for existing managers
class SessionBufferManager:
    def __init__(self, redis_client: redis.Redis, encryption_key: str = None):
        self.redis = redis_client
        self.encryption_key = encryption_key
        # ... existing initialization
```

**Security Benefits**:
- Encrypts session data in transit to Redis
- Prevents network-level interception of sensitive session information
- Enables mutual TLS authentication between application and Redis

**Testing Strategy**:
```python
async def test_redis_tls_connection():
    """Test Redis TLS connectivity and certificate validation"""
    config = RedisConfig()
    client = await config.create_redis_client("rediss://localhost:6380")
    
    # Test connection
    await client.ping()
    
    # Test data encryption in transit
    await client.set("test_key", "test_value")
    result = await client.get("test_key")
    assert result == b"test_value"
```

### WebSocket Secure (WSS) Enforcement

**Objective**: Ensure all WebSocket connections use encryption in production.

**Implementation Approach**:

```python
# WebSocket Security Configuration
from fastapi import FastAPI, WebSocket, HTTPException
from fastapi.middleware.https import HTTPSRedirectMiddleware

class WebSocketSecurityConfig:
    def __init__(self, enforce_wss: bool = True, allowed_origins: list = None):
        self.enforce_wss = enforce_wss
        self.allowed_origins = allowed_origins or []
    
    def validate_websocket_security(self, websocket: WebSocket):
        """Validate WebSocket connection security requirements"""
        
        # Enforce WSS in production
        if self.enforce_wss:
            if not websocket.url.scheme == 'wss':
                raise HTTPException(
                    status_code=426,
                    detail="Secure WebSocket (WSS) required"
                )
        
        # Validate origin if configured
        origin = websocket.headers.get('origin')
        if self.allowed_origins and origin not in self.allowed_origins:
            raise HTTPException(
                status_code=403,
                detail="Origin not allowed"
            )
        
        # Additional security headers validation
        self._validate_security_headers(websocket)
    
    def _validate_security_headers(self, websocket: WebSocket):
        """Validate security-related headers"""
        headers = websocket.headers
        
        # Check for suspicious user agents
        user_agent = headers.get('user-agent', '')
        if not user_agent or len(user_agent) < 10:
            logger.warning("Suspicious WebSocket connection - minimal user agent",
                         user_agent=user_agent,
                         client_ip=self._extract_client_ip(websocket))

# Integration with existing WebSocket gateway
class WebSocketGateway:
    def __init__(self, session_manager, auth_service, jwt_secret: str,
                 security_config: WebSocketSecurityConfig = None):
        # ... existing initialization
        self.security_config = security_config or WebSocketSecurityConfig()
    
    async def handle_connection(self, websocket: WebSocket, token: str):
        """Enhanced connection handler with security validation"""
        try:
            # Validate security requirements first
            self.security_config.validate_websocket_security(websocket)
            
            # Continue with existing connection logic
            # ... existing implementation
            
        except HTTPException as e:
            await websocket.close(code=e.status_code, reason=e.detail)
            return
```

**Security Benefits**:
- Prevents session data interception via encrypted WebSocket connections
- Enforces origin validation to prevent cross-site WebSocket hijacking
- Provides configurable security policies for different environments

**Configuration Example**:
```python
# Production configuration
security_config = WebSocketSecurityConfig(
    enforce_wss=True,
    allowed_origins=[
        "https://app.example.com",
        "https://admin.example.com"
    ]
)

# Development configuration  
dev_security_config = WebSocketSecurityConfig(
    enforce_wss=False,  # Allow WS for local development
    allowed_origins=["http://localhost:3000"]
)
```

### Network Security Configuration

**Objective**: Define network-level security controls for the session management system.

**Implementation Approach**:

```yaml
# Docker Compose network configuration
version: '3.8'
services:
  session-manager:
    image: session-manager:latest
    networks:
      - session_internal
      - session_external
    environment:
      - REDIS_URL=rediss://redis:6379
      - ENABLE_WSS=true
      - ALLOWED_ORIGINS=https://app.example.com
      
  redis:
    image: redis:latest
    command: >
      redis-server 
      --tls-port 6379 
      --port 0
      --tls-cert-file /tls/redis.crt
      --tls-key-file /tls/redis.key
      --tls-ca-cert-file /tls/ca.crt
      --tls-auth-clients yes
    networks:
      - session_internal
    volumes:
      - ./tls:/tls:ro

networks:
  session_internal:
    driver: bridge
    internal: true  # No external access
  session_external:
    driver: bridge
```

**Firewall Rules** (iptables example):
```bash
# Allow only necessary ports
iptables -A INPUT -p tcp --dport 443 -j ACCEPT  # HTTPS
iptables -A INPUT -p tcp --dport 80 -j ACCEPT   # HTTP (redirect to HTTPS)

# Block direct Redis access from external networks
iptables -A INPUT -p tcp --dport 6379 -s 10.0.0.0/8 -j ACCEPT
iptables -A INPUT -p tcp --dport 6379 -j DROP

# Default deny policy
iptables -P INPUT DROP
iptables -P FORWARD DROP
```

## Security Analysis

### Threat Mitigation

**Before Implementation**:
- **Man-in-the-middle attacks**: Possible on Redis and WebSocket connections
- **Network sniffing**: Session data visible in transit  
- **Lateral movement**: Compromised services can access Redis directly

**After Implementation**:
- **Encrypted communications**: All data protected by TLS/SSL
- **Certificate-based authentication**: Mutual TLS prevents unauthorized access
- **Network isolation**: Internal services protected from external access

### Compliance Benefits

- **SOC 2 Type II**: Encryption in transit requirements
- **PCI DSS**: Data protection standards (if applicable)
- **GDPR**: Data protection by design principles
- **HIPAA**: Technical safeguards for healthcare data (if applicable)

## Integration with Existing Session 6 Code

### No Breaking Changes Required

The infrastructure security enhancements are designed to be **additive** and require no changes to existing Session 6 application logic:

- **Buffer Manager**: Works unchanged with TLS-enabled Redis
- **Recovery Manager**: Automatically benefits from encrypted communications  
- **WebSocket Gateway**: Enhanced with optional security validation
- **State Manager**: No modifications required

### Configuration-Driven Implementation

All security features are controlled via configuration, allowing:
- **Development flexibility**: Can disable for local development
- **Production enforcement**: Strict security in production environments
- **Gradual rollout**: Enable features incrementally during deployment

### Current Integration Points

```python
# session-manager/config/security.py
from dataclasses import dataclass
from typing import List, Optional

@dataclass
class SecurityConfig:
    # Redis Security
    redis_tls_enabled: bool = True
    redis_ca_cert_path: Optional[str] = None
    redis_client_cert_path: Optional[str] = None
    redis_client_key_path: Optional[str] = None
    
    # WebSocket Security
    enforce_wss: bool = True
    allowed_origins: List[str] = None
    validate_user_agents: bool = True
    
    # Network Security
    bind_internal_only: bool = True
    max_connections_per_ip: int = 100

# Integration in main application
async def create_session_manager(config: SecurityConfig):
    # Create secure Redis client
    redis_client = await create_redis_client(
        url=os.getenv('REDIS_URL'),
        tls_enabled=config.redis_tls_enabled,
        ca_cert_path=config.redis_ca_cert_path
    )
    
    # Create WebSocket security config
    ws_security = WebSocketSecurityConfig(
        enforce_wss=config.enforce_wss,
        allowed_origins=config.allowed_origins
    )
    
    # Initialize existing managers with secure configuration
    buffer_manager = SessionBufferManager(redis_client)
    gateway = WebSocketGateway(
        session_manager, auth_service, jwt_secret,
        security_config=ws_security
    )
    
    return session_manager, gateway
```

## Testing and Validation

### Infrastructure Testing

```python
async def test_infrastructure_security():
    """Comprehensive infrastructure security testing"""
    
    # Test Redis TLS connection
    await test_redis_tls_connectivity()
    await test_redis_certificate_validation() 
    await test_redis_encryption_in_transit()
    
    # Test WebSocket security
    await test_wss_enforcement()
    await test_origin_validation()
    await test_security_header_validation()
    
    # Test network security
    await test_port_accessibility()
    await test_network_isolation()

async def test_redis_tls_connectivity():
    """Test Redis TLS connection establishment"""
    config = RedisConfig()
    client = await config.create_redis_client("rediss://redis:6379")
    
    # Test basic connectivity
    pong = await client.ping()
    assert pong == True
    
    # Test encrypted data transmission
    test_data = "sensitive_session_data"
    await client.set("test:tls", test_data)
    retrieved = await client.get("test:tls")
    assert retrieved.decode() == test_data

async def test_wss_enforcement():
    """Test WebSocket Secure enforcement"""
    security_config = WebSocketSecurityConfig(enforce_wss=True)
    
    # Test WSS rejection
    with pytest.raises(HTTPException) as exc_info:
        mock_ws = create_mock_websocket(scheme='ws')
        security_config.validate_websocket_security(mock_ws)
    
    assert exc_info.value.status_code == 426
    
    # Test WSS acceptance
    mock_wss = create_mock_websocket(scheme='wss')
    security_config.validate_websocket_security(mock_wss)  # Should not raise
```

### Performance Impact Assessment

**Expected Impact**:
- **TLS Overhead**: ~5-10% CPU increase for encryption/decryption
- **Memory Usage**: Minimal increase for SSL contexts and certificates
- **Latency**: ~1-2ms additional latency for TLS handshakes
- **Throughput**: 95-98% of non-encrypted performance

**Monitoring Recommendations**:
```python
# Performance monitoring for infrastructure security
import time
import structlog

async def monitor_tls_performance():
    """Monitor TLS performance impact"""
    start_time = time.time()
    
    # Perform Redis operation
    await redis_client.set("perf_test", "data")
    result = await redis_client.get("perf_test")
    
    duration = time.time() - start_time
    
    # Log performance metrics
    logger.info("Redis TLS operation completed",
                duration_ms=duration * 1000,
                operation="set_get",
                tls_enabled=True)
    
    # Alert if performance degrades significantly
    if duration > 0.1:  # 100ms threshold
        logger.warning("Redis TLS performance degradation detected",
                      duration_ms=duration * 1000)
```

## Deployment Considerations

### Certificate Management

```bash
# Certificate generation for Redis TLS
openssl genrsa -out ca.key 4096
openssl req -new -x509 -days 365 -key ca.key -out ca.crt

openssl genrsa -out redis.key 2048
openssl req -new -key redis.key -out redis.csr
openssl x509 -req -days 365 -in redis.csr -CA ca.crt -CAkey ca.key -out redis.crt

# Client certificates
openssl genrsa -out client.key 2048
openssl req -new -key client.key -out client.csr  
openssl x509 -req -days 365 -in client.csr -CA ca.crt -CAkey ca.key -out client.crt
```

### Environment Configuration

```bash
# Production environment variables
REDIS_URL=rediss://redis:6379
REDIS_TLS_CA_CERT=/certs/ca.crt
REDIS_TLS_CLIENT_CERT=/certs/client.crt
REDIS_TLS_CLIENT_KEY=/certs/client.key

WEBSOCKET_ENFORCE_WSS=true
WEBSOCKET_ALLOWED_ORIGINS=https://app.example.com,https://admin.example.com

NETWORK_BIND_INTERNAL=true
SECURITY_MAX_CONNECTIONS_PER_IP=100
```

### Health Checks

```python
async def infrastructure_health_check():
    """Health check for infrastructure security components"""
    checks = {}
    
    # Redis TLS connectivity
    try:
        await redis_client.ping()
        checks['redis_tls'] = 'healthy'
    except Exception as e:
        checks['redis_tls'] = f'unhealthy: {str(e)}'
    
    # Certificate expiration
    try:
        cert_expiry = check_certificate_expiration('/certs/redis.crt')
        if cert_expiry < 30:  # 30 days warning
            checks['certificate'] = f'warning: expires in {cert_expiry} days'
        else:
            checks['certificate'] = 'healthy'
    except Exception as e:
        checks['certificate'] = f'error: {str(e)}'
    
    return checks
```

## Success Criteria

### Functional Requirements
- ✅ Redis connections use TLS encryption
- ✅ WebSocket connections enforced to use WSS in production
- ✅ Certificate-based authentication for Redis access
- ✅ Origin validation for WebSocket connections
- ✅ Network isolation between internal and external services

### Security Requirements  
- ✅ All session data encrypted in transit
- ✅ Mutual TLS authentication prevents unauthorized Redis access
- ✅ WebSocket hijacking prevention via origin validation
- ✅ Certificate rotation and management procedures
- ✅ Network-level access controls implemented

### Performance Requirements
- ✅ Less than 10% performance impact from TLS overhead
- ✅ Sub-5ms additional latency for secure connections
- ✅ Monitoring and alerting for security performance metrics
- ✅ Graceful degradation if security services unavailable

This infrastructure security implementation provides the foundational encryption and network security required for production deployment while maintaining compatibility with the existing Session 6 codebase.