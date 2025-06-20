"""
JWT Token Refresh Strategy for Long-Running Sessions

Handles token lifecycle management for:
- WebSocket connections that may last hours
- API sessions with automatic refresh
- Secure token rotation
- Graceful handling of token expiration
"""

import os
import asyncio
from datetime import datetime, timedelta
from typing import Dict, Optional, Tuple, Any
from enum import Enum
import json
import secrets

from jose import JWTError, jwt
from pydantic import BaseModel, Field
import redis.asyncio as redis


class TokenType(str, Enum):
    """Types of tokens in the system."""
    ACCESS = "access"
    REFRESH = "refresh"
    WEBSOCKET = "websocket"


class TokenStatus(str, Enum):
    """Token validation status."""
    VALID = "valid"
    EXPIRED = "expired"
    INVALID = "invalid"
    REVOKED = "revoked"


class TokenClaims(BaseModel):
    """Standard JWT claims."""
    sub: str = Field(..., description="Subject (user ID)")
    iat: datetime = Field(..., description="Issued at")
    exp: datetime = Field(..., description="Expires at")
    jti: str = Field(..., description="JWT ID")
    type: TokenType = Field(..., description="Token type")
    scope: list[str] = Field(default_factory=list, description="Token scopes")
    session_id: Optional[str] = Field(None, description="Session ID for WebSocket tokens")


class TokenPair(BaseModel):
    """Access and refresh token pair."""
    access_token: str = Field(..., description="JWT access token")
    refresh_token: str = Field(..., description="JWT refresh token")
    token_type: str = Field(default="Bearer")
    expires_in: int = Field(..., description="Access token expiry in seconds")
    refresh_expires_in: int = Field(..., description="Refresh token expiry in seconds")


class TokenRefreshResult(BaseModel):
    """Result of token refresh operation."""
    success: bool
    token_pair: Optional[TokenPair] = None
    error: Optional[str] = None
    requires_reauth: bool = False


class TokenManager:
    """JWT token lifecycle manager."""
    
    def __init__(
        self,
        secret_key: str,
        algorithm: str = "HS256",
        access_token_expire_minutes: int = 15,
        refresh_token_expire_days: int = 7,
        websocket_token_expire_hours: int = 24,
        redis_client: Optional[redis.Redis] = None
    ):
        self.secret_key = secret_key
        self.algorithm = algorithm
        self.access_token_expire = timedelta(minutes=access_token_expire_minutes)
        self.refresh_token_expire = timedelta(days=refresh_token_expire_days)
        self.websocket_token_expire = timedelta(hours=websocket_token_expire_hours)
        self.redis = redis_client
        
        # Token refresh thresholds
        self.refresh_threshold = timedelta(minutes=2)  # Refresh 2 min before expiry
        self.websocket_refresh_threshold = timedelta(minutes=5)  # WS tokens refresh earlier
        
    def _generate_jti(self) -> str:
        """Generate unique JWT ID."""
        return secrets.token_urlsafe(32)
    
    def _create_token(
        self,
        user_id: str,
        token_type: TokenType,
        expires_delta: timedelta,
        scope: list[str] = None,
        session_id: str = None
    ) -> str:
        """Create a JWT token."""
        now = datetime.utcnow()
        claims = TokenClaims(
            sub=user_id,
            iat=now,
            exp=now + expires_delta,
            jti=self._generate_jti(),
            type=token_type,
            scope=scope or [],
            session_id=session_id
        )
        
        return jwt.encode(
            claims.model_dump(mode='json'),
            self.secret_key,
            algorithm=self.algorithm
        )
    
    async def create_token_pair(
        self,
        user_id: str,
        scope: list[str] = None
    ) -> TokenPair:
        """Create access and refresh token pair."""
        access_token = self._create_token(
            user_id=user_id,
            token_type=TokenType.ACCESS,
            expires_delta=self.access_token_expire,
            scope=scope
        )
        
        refresh_token = self._create_token(
            user_id=user_id,
            token_type=TokenType.REFRESH,
            expires_delta=self.refresh_token_expire,
            scope=scope
        )
        
        # Store refresh token in Redis for tracking
        if self.redis:
            refresh_claims = self.decode_token(refresh_token)
            if refresh_claims:
                await self._store_refresh_token(refresh_claims.jti, user_id, refresh_token)
        
        return TokenPair(
            access_token=access_token,
            refresh_token=refresh_token,
            expires_in=int(self.access_token_expire.total_seconds()),
            refresh_expires_in=int(self.refresh_token_expire.total_seconds())
        )
    
    async def create_websocket_token(
        self,
        user_id: str,
        session_id: str,
        scope: list[str] = None
    ) -> str:
        """Create long-lived WebSocket token."""
        return self._create_token(
            user_id=user_id,
            token_type=TokenType.WEBSOCKET,
            expires_delta=self.websocket_token_expire,
            scope=scope,
            session_id=session_id
        )
    
    def decode_token(self, token: str) -> Optional[TokenClaims]:
        """Decode and validate JWT token."""
        try:
            payload = jwt.decode(
                token,
                self.secret_key,
                algorithms=[self.algorithm]
            )
            return TokenClaims(**payload)
        except JWTError:
            return None
    
    async def validate_token(self, token: str) -> Tuple[TokenStatus, Optional[TokenClaims]]:
        """Validate token and return status."""
        claims = self.decode_token(token)
        if not claims:
            return TokenStatus.INVALID, None
        
        # Check if token is expired
        if datetime.utcnow() > claims.exp:
            return TokenStatus.EXPIRED, claims
        
        # Check if refresh token is revoked (only for refresh tokens)
        if claims.type == TokenType.REFRESH and self.redis:
            is_revoked = await self._is_refresh_token_revoked(claims.jti)
            if is_revoked:
                return TokenStatus.REVOKED, claims
        
        return TokenStatus.VALID, claims
    
    async def refresh_access_token(self, refresh_token: str) -> TokenRefreshResult:
        """Refresh access token using refresh token."""
        # Validate refresh token
        status, claims = await self.validate_token(refresh_token)
        
        if status != TokenStatus.VALID:
            return TokenRefreshResult(
                success=False,
                error=f"Invalid refresh token: {status.value}",
                requires_reauth=True
            )
        
        if claims.type != TokenType.REFRESH:
            return TokenRefreshResult(
                success=False,
                error="Token is not a refresh token",
                requires_reauth=True
            )
        
        # Create new token pair
        new_token_pair = await self.create_token_pair(
            user_id=claims.sub,
            scope=claims.scope
        )
        
        # Revoke old refresh token and store new one
        if self.redis:
            await self._revoke_refresh_token(claims.jti)
        
        return TokenRefreshResult(
            success=True,
            token_pair=new_token_pair
        )
    
    def needs_refresh(self, token: str) -> bool:
        """Check if token needs refresh based on expiry threshold."""
        claims = self.decode_token(token)
        if not claims:
            return True
        
        time_until_expiry = claims.exp - datetime.utcnow()
        
        if claims.type == TokenType.WEBSOCKET:
            return time_until_expiry <= self.websocket_refresh_threshold
        else:
            return time_until_expiry <= self.refresh_threshold
    
    async def revoke_user_tokens(self, user_id: str) -> int:
        """Revoke all refresh tokens for a user."""
        if not self.redis:
            return 0
        
        pattern = f"refresh_token:{user_id}:*"
        keys = await self.redis.keys(pattern)
        
        if keys:
            await self.redis.delete(*keys)
        
        return len(keys)
    
    async def _store_refresh_token(self, jti: str, user_id: str, token: str):
        """Store refresh token in Redis."""
        key = f"refresh_token:{user_id}:{jti}"
        await self.redis.setex(
            key,
            int(self.refresh_token_expire.total_seconds()),
            token
        )
    
    async def _is_refresh_token_revoked(self, jti: str) -> bool:
        """Check if refresh token is revoked."""
        # Try to find the token in Redis
        pattern = f"refresh_token:*:{jti}"
        keys = await self.redis.keys(pattern)
        return len(keys) == 0
    
    async def _revoke_refresh_token(self, jti: str):
        """Revoke a specific refresh token."""
        pattern = f"refresh_token:*:{jti}"
        keys = await self.redis.keys(pattern)
        if keys:
            await self.redis.delete(*keys)


class WebSocketTokenManager:
    """Specialized token manager for WebSocket connections."""
    
    def __init__(self, token_manager: TokenManager):
        self.token_manager = token_manager
        self.active_connections: Dict[str, 'WSTokenContext'] = {}
    
    async def authenticate_websocket(
        self,
        token: str,
        session_id: str
    ) -> Tuple[bool, Optional[TokenClaims], Optional[str]]:
        """Authenticate WebSocket connection."""
        status, claims = await self.token_manager.validate_token(token)
        
        if status != TokenStatus.VALID:
            return False, None, f"Authentication failed: {status.value}"
        
        # For WebSocket tokens, verify session ID matches
        if claims.type == TokenType.WEBSOCKET and claims.session_id != session_id:
            return False, None, "Session ID mismatch"
        
        # Create token context for this connection
        context = WSTokenContext(
            connection_id=f"{session_id}_{secrets.token_urlsafe(8)}",
            user_id=claims.sub,
            session_id=session_id,
            current_token=token,
            token_manager=self.token_manager
        )
        
        self.active_connections[context.connection_id] = context
        
        # Start background refresh task
        asyncio.create_task(self._token_refresh_loop(context))
        
        return True, claims, None
    
    async def get_valid_token(self, connection_id: str) -> Optional[str]:
        """Get current valid token for connection."""
        context = self.active_connections.get(connection_id)
        if not context:
            return None
        
        return await context.get_valid_token()
    
    async def disconnect_websocket(self, connection_id: str):
        """Clean up WebSocket token context."""
        context = self.active_connections.pop(connection_id, None)
        if context:
            context.should_stop = True
    
    async def _token_refresh_loop(self, context: 'WSTokenContext'):
        """Background task to refresh tokens proactively."""
        while not context.should_stop:
            try:
                # Check if token needs refresh
                if self.token_manager.needs_refresh(context.current_token):
                    await context.refresh_token()
                
                # Sleep for 30 seconds before checking again
                await asyncio.sleep(30)
                
            except Exception as e:
                # Log error but continue
                print(f"Token refresh loop error: {e}")
                await asyncio.sleep(60)  # Wait longer on error


class WSTokenContext:
    """Token context for WebSocket connection."""
    
    def __init__(
        self,
        connection_id: str,
        user_id: str,
        session_id: str,
        current_token: str,
        token_manager: TokenManager
    ):
        self.connection_id = connection_id
        self.user_id = user_id
        self.session_id = session_id
        self.current_token = current_token
        self.token_manager = token_manager
        self.should_stop = False
        self.last_refresh = datetime.utcnow()
        self._lock = asyncio.Lock()
    
    async def get_valid_token(self) -> Optional[str]:
        """Get current valid token, refreshing if necessary."""
        async with self._lock:
            # Check current token validity
            status, claims = await self.token_manager.validate_token(self.current_token)
            
            if status == TokenStatus.VALID:
                return self.current_token
            
            # Try to refresh
            if await self.refresh_token():
                return self.current_token
            
            return None
    
    async def refresh_token(self) -> bool:
        """Refresh the current token."""
        try:
            # For WebSocket tokens, we create a new WebSocket token
            # In practice, you might want to use a different refresh strategy
            new_token = await self.token_manager.create_websocket_token(
                user_id=self.user_id,
                session_id=self.session_id,
                scope=[]  # Would get from current token
            )
            
            self.current_token = new_token
            self.last_refresh = datetime.utcnow()
            return True
            
        except Exception as e:
            print(f"Token refresh failed: {e}")
            return False


class TokenRefreshMiddleware:
    """ASGI middleware for automatic token refresh."""
    
    def __init__(self, app, token_manager: TokenManager):
        self.app = app
        self.token_manager = token_manager
    
    async def __call__(self, scope, receive, send):
        """ASGI middleware entry point."""
        if scope["type"] == "http":
            # Extract authorization header
            headers = dict(scope.get("headers", []))
            auth_header = headers.get(b"authorization", b"").decode()
            
            if auth_header.startswith("Bearer "):
                token = auth_header[7:]
                
                # Check if token needs refresh
                if self.token_manager.needs_refresh(token):
                    # Add header to indicate refresh needed
                    scope["token_refresh_needed"] = True
        
        await self.app(scope, receive, send)


# Configuration
class TokenConfig:
    """Token configuration settings."""
    
    # Token lifetimes
    ACCESS_TOKEN_EXPIRE_MINUTES = 15
    REFRESH_TOKEN_EXPIRE_DAYS = 7
    WEBSOCKET_TOKEN_EXPIRE_HOURS = 24
    
    # Refresh thresholds
    ACCESS_TOKEN_REFRESH_THRESHOLD_MINUTES = 2
    WEBSOCKET_TOKEN_REFRESH_THRESHOLD_MINUTES = 5
    
    # Security
    JWT_ALGORITHM = "HS256"
    TOKEN_ROTATION_ENABLED = True
    REVOKE_ON_REFRESH = True
    
    # Rate limiting
    MAX_REFRESH_ATTEMPTS_PER_HOUR = 60
    
    @classmethod
    def from_env(cls):
        """Load configuration from environment variables."""
        return cls(
            ACCESS_TOKEN_EXPIRE_MINUTES=int(os.getenv("JWT_ACCESS_TOKEN_EXPIRE_MINUTES", "15")),
            REFRESH_TOKEN_EXPIRE_DAYS=int(os.getenv("JWT_REFRESH_TOKEN_EXPIRE_DAYS", "7")),
            WEBSOCKET_TOKEN_EXPIRE_HOURS=int(os.getenv("JWT_WEBSOCKET_TOKEN_EXPIRE_HOURS", "24")),
            JWT_ALGORITHM=os.getenv("JWT_ALGORITHM", "HS256")
        )


# Factory function
def create_token_manager(redis_url: str = None, secret_key: str = None) -> TokenManager:
    """Create token manager with Redis backend."""
    if not secret_key:
        secret_key = os.getenv("JWT_SECRET")
        if not secret_key:
            raise ValueError("JWT_SECRET environment variable required")
    
    redis_client = None
    if redis_url:
        redis_client = redis.from_url(redis_url)
    
    config = TokenConfig.from_env()
    
    return TokenManager(
        secret_key=secret_key,
        algorithm=config.JWT_ALGORITHM,
        access_token_expire_minutes=config.ACCESS_TOKEN_EXPIRE_MINUTES,
        refresh_token_expire_days=config.REFRESH_TOKEN_EXPIRE_DAYS,
        websocket_token_expire_hours=config.WEBSOCKET_TOKEN_EXPIRE_HOURS,
        redis_client=redis_client
    )


# Example usage
async def example_usage():
    """Example of token refresh functionality."""
    # Create token manager
    token_manager = create_token_manager()
    
    # Create initial token pair
    token_pair = await token_manager.create_token_pair("user123")
    print(f"Access token: {token_pair.access_token[:50]}...")
    
    # Simulate token refresh
    refresh_result = await token_manager.refresh_access_token(token_pair.refresh_token)
    if refresh_result.success:
        print("Token refreshed successfully")
    
    # WebSocket token management
    ws_token_manager = WebSocketTokenManager(token_manager)
    ws_token = await token_manager.create_websocket_token("user123", "session456")
    
    success, claims, error = await ws_token_manager.authenticate_websocket(ws_token, "session456")
    if success:
        print(f"WebSocket authenticated for user: {claims.sub}")


if __name__ == "__main__":
    asyncio.run(example_usage())