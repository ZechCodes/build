"""
TLS Configuration for Production Deployment
Implements Redis TLS and WebSocket SSL security
"""
import ssl
import os
from dataclasses import dataclass
from typing import Optional, List
import redis.asyncio as redis
import structlog

logger = structlog.get_logger()

@dataclass
class TLSConfig:
    """TLS configuration for production security"""
    # Redis TLS Configuration
    redis_tls_enabled: bool = True
    redis_ca_cert_path: Optional[str] = None
    redis_client_cert_path: Optional[str] = None
    redis_client_key_path: Optional[str] = None
    
    # WebSocket SSL Configuration
    enforce_wss: bool = True
    allowed_origins: List[str] = None
    validate_user_agents: bool = True
    
    # Network Security
    bind_internal_only: bool = True
    max_connections_per_ip: int = 100
    
    def __post_init__(self):
        """Initialize configuration from environment"""
        if self.redis_ca_cert_path is None:
            self.redis_ca_cert_path = os.getenv('REDIS_TLS_CA_CERT', '/certs/ca.crt')
        if self.redis_client_cert_path is None:
            self.redis_client_cert_path = os.getenv('REDIS_TLS_CLIENT_CERT', '/certs/client.crt')
        if self.redis_client_key_path is None:
            self.redis_client_key_path = os.getenv('REDIS_TLS_CLIENT_KEY', '/certs/client.key')
        if self.allowed_origins is None:
            origins = os.getenv('WEBSOCKET_ALLOWED_ORIGINS', '')
            self.allowed_origins = [o.strip() for o in origins.split(',') if o.strip()]

class RedisSecureClient:
    """Secure Redis client with TLS support"""
    
    def __init__(self, config: TLSConfig):
        self.config = config
        self.ssl_context = None
        if config.redis_tls_enabled:
            self.ssl_context = self._create_ssl_context()
    
    def _create_ssl_context(self) -> ssl.SSLContext:
        """Create SSL context for Redis TLS connections"""
        try:
            context = ssl.create_default_context(ssl.Purpose.SERVER_AUTH)
            context.check_hostname = False  # Configure based on certificate setup
            context.verify_mode = ssl.CERT_REQUIRED
            
            # Load CA certificate
            if os.path.exists(self.config.redis_ca_cert_path):
                context.load_verify_locations(self.config.redis_ca_cert_path)
                logger.info("Loaded Redis CA certificate", 
                           path=self.config.redis_ca_cert_path)
            
            # Load client certificate and key
            if (os.path.exists(self.config.redis_client_cert_path) and 
                os.path.exists(self.config.redis_client_key_path)):
                context.load_cert_chain(
                    self.config.redis_client_cert_path,
                    self.config.redis_client_key_path
                )
                logger.info("Loaded Redis client certificates")
            
            return context
            
        except Exception as e:
            logger.error("Failed to create Redis SSL context", error=str(e))
            raise
    
    async def create_redis_client(self, redis_url: str) -> redis.Redis:
        """Create Redis client with TLS encryption"""
        try:
            # Convert redis:// to rediss:// for TLS
            if self.config.redis_tls_enabled and redis_url.startswith('redis://'):
                redis_url = redis_url.replace('redis://', 'rediss://', 1)
                logger.info("Converted Redis URL to use TLS", url=redis_url.split('@')[-1])
            
            if self.ssl_context:
                client = redis.from_url(
                    redis_url,
                    ssl=self.ssl_context,
                    ssl_cert_reqs=ssl.CERT_REQUIRED,
                    ssl_check_hostname=False,
                    decode_responses=False
                )
            else:
                client = redis.from_url(redis_url, decode_responses=False)
            
            # Test connection
            await client.ping()
            logger.info("Redis TLS connection established successfully")
            return client
            
        except Exception as e:
            logger.error("Failed to create Redis client", error=str(e))
            raise

class WebSocketSecurityValidator:
    """WebSocket security validation for production"""
    
    def __init__(self, config: TLSConfig):
        self.config = config
    
    def validate_websocket_security(self, websocket):
        """Validate WebSocket connection security requirements"""
        from fastapi import HTTPException
        
        # Enforce WSS in production
        if self.config.enforce_wss:
            scheme = getattr(websocket.url, 'scheme', 'ws')
            if scheme != 'wss':
                logger.warning("WebSocket connection rejected - WSS required",
                             scheme=scheme, 
                             client_ip=self._extract_client_ip(websocket))
                raise HTTPException(
                    status_code=426,
                    detail="Secure WebSocket (WSS) required in production"
                )
        
        # Validate origin
        if self.config.allowed_origins:
            origin = websocket.headers.get('origin')
            if origin not in self.config.allowed_origins:
                logger.warning("WebSocket connection rejected - invalid origin",
                             origin=origin,
                             allowed=self.config.allowed_origins)
                raise HTTPException(
                    status_code=403,
                    detail="Origin not allowed"
                )
        
        # Validate user agent
        if self.config.validate_user_agents:
            user_agent = websocket.headers.get('user-agent', '')
            if not user_agent or len(user_agent) < 10:
                logger.warning("Suspicious WebSocket connection - minimal user agent",
                             user_agent=user_agent,
                             client_ip=self._extract_client_ip(websocket))
    
    def _extract_client_ip(self, websocket) -> str:
        """Extract client IP from WebSocket connection"""
        # Try X-Forwarded-For first (load balancer)
        forwarded_for = websocket.headers.get('x-forwarded-for')
        if forwarded_for:
            return forwarded_for.split(',')[0].strip()
        
        # Try X-Real-IP
        real_ip = websocket.headers.get('x-real-ip')
        if real_ip:
            return real_ip
        
        # Fall back to client address
        client = getattr(websocket, 'client', None)
        if client:
            return client.host
        
        return 'unknown'

# Production configuration factory
def create_production_tls_config() -> TLSConfig:
    """Create production TLS configuration"""
    return TLSConfig(
        redis_tls_enabled=True,
        enforce_wss=True,
        allowed_origins=[
            'https://app.buildplatform.dev',
            'https://admin.buildplatform.dev'
        ],
        validate_user_agents=True,
        bind_internal_only=True,
        max_connections_per_ip=100
    )

# Development configuration factory
def create_development_tls_config() -> TLSConfig:
    """Create development TLS configuration"""
    return TLSConfig(
        redis_tls_enabled=False,  # Disable for local development
        enforce_wss=False,        # Allow WS for local development
        allowed_origins=['http://localhost:3000', 'http://127.0.0.1:3000'],
        validate_user_agents=False,
        bind_internal_only=False,
        max_connections_per_ip=1000
    )

# Health check for TLS infrastructure
async def check_tls_infrastructure(config: TLSConfig) -> dict:
    """Health check for TLS infrastructure"""
    checks = {}
    
    # Check Redis TLS connectivity
    if config.redis_tls_enabled:
        try:
            redis_client_factory = RedisSecureClient(config)
            redis_url = os.getenv('REDIS_URL', 'redis://localhost:6379')
            redis_client = await redis_client_factory.create_redis_client(redis_url)
            await redis_client.ping()
            await redis_client.close()
            checks['redis_tls'] = 'healthy'
        except Exception as e:
            checks['redis_tls'] = f'unhealthy: {str(e)}'
    else:
        checks['redis_tls'] = 'disabled'
    
    # Check certificate expiration
    if config.redis_tls_enabled and config.redis_client_cert_path:
        try:
            import ssl
            import datetime
            
            cert = ssl.load_certificate(ssl.FILETYPE_PEM, 
                                      open(config.redis_client_cert_path, 'rb').read())
            expiry_date = datetime.datetime.strptime(
                cert.get_notAfter().decode('ascii'), '%Y%m%d%H%M%SZ'
            )
            days_until_expiry = (expiry_date - datetime.datetime.utcnow()).days
            
            if days_until_expiry < 30:
                checks['certificate'] = f'warning: expires in {days_until_expiry} days'
            else:
                checks['certificate'] = f'healthy: {days_until_expiry} days remaining'
                
        except Exception as e:
            checks['certificate'] = f'error: {str(e)}'
    else:
        checks['certificate'] = 'not_applicable'
    
    return checks