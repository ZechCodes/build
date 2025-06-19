"""JWT token management system according to Session 2 requirements."""

import secrets
from datetime import datetime, timedelta, timezone
from typing import Dict, Any, Optional
from jose import JWTError, jwt
from fastapi import HTTPException, status


class JWTManager:
    """JWT token manager with enhanced security features."""
    
    def __init__(self, secret_key: str, algorithm: str = "HS256"):
        """Initialize JWT manager with configuration."""
        self.secret_key = secret_key
        self.algorithm = algorithm
        self.access_token_expire_minutes = 15  # Session 2 requirement: 15 minutes
        self.refresh_token_expire_days = 7     # Session 2 requirement: 7 days
    
    def create_access_token(self, data: Dict[Any, Any]) -> str:
        """Create access token with 15-minute expiration."""
        to_encode = data.copy()
        expire = datetime.now(timezone.utc) + timedelta(minutes=self.access_token_expire_minutes)
        to_encode.update({"exp": expire, "type": "access"})
        return jwt.encode(to_encode, self.secret_key, algorithm=self.algorithm)
    
    def create_refresh_token(self, data: Dict[Any, Any]) -> str:
        """Create refresh token with 7-day expiration and JTI for tracking."""
        to_encode = data.copy()
        expire = datetime.now(timezone.utc) + timedelta(days=self.refresh_token_expire_days)
        jti = secrets.token_urlsafe(32)  # JWT ID for token tracking/revocation
        to_encode.update({"exp": expire, "type": "refresh", "jti": jti})
        return jwt.encode(to_encode, self.secret_key, algorithm=self.algorithm)
    
    def verify_token(self, token: str, token_type: str = "access") -> Dict[Any, Any]:
        """Verify token and return payload with enhanced security checks."""
        try:
            payload = jwt.decode(token, self.secret_key, algorithms=[self.algorithm])
            
            # Verify token type matches expected type
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
    
    def get_token_expiry_seconds(self, token_type: str = "access") -> int:
        """Get token expiry time in seconds."""
        if token_type == "access":
            return self.access_token_expire_minutes * 60
        elif token_type == "refresh":
            return self.refresh_token_expire_days * 24 * 60 * 60
        else:
            raise ValueError(f"Unknown token type: {token_type}")
    
    def extract_jti(self, refresh_token: str) -> Optional[str]:
        """Extract JWT ID from refresh token for tracking."""
        try:
            payload = self.verify_token(refresh_token, token_type="refresh")
            return payload.get("jti")
        except HTTPException:
            return None