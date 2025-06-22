"""
Authentication and authorization middleware for snapshot API.
"""

import jwt
import time
from typing import Optional, Dict, Any
from fastapi import HTTPException, Request, status
from fastapi.security import HTTPBearer, HTTPAuthorizationCredentials
import structlog
import logfire

logger = structlog.get_logger()


class AuthenticationMiddleware:
    """
    JWT-based authentication middleware for snapshot API.
    
    Provides user authentication and authorization with comprehensive
    security logging and monitoring.
    """
    
    def __init__(self, jwt_secret: str, jwt_algorithm: str = "HS256"):
        """Initialize authentication middleware."""
        self.jwt_secret = jwt_secret
        self.jwt_algorithm = jwt_algorithm
        self.bearer_scheme = HTTPBearer()
    
    async def authenticate_user(self, credentials: HTTPAuthorizationCredentials) -> Dict[str, Any]:
        """
        Authenticate user from JWT token.
        
        Args:
            credentials: Bearer token credentials
            
        Returns:
            Dict containing user information
            
        Raises:
            HTTPException: If authentication fails
        """
        try:
            token = credentials.credentials
            
            # Decode and validate JWT token
            payload = jwt.decode(
                token, 
                self.jwt_secret, 
                algorithms=[self.jwt_algorithm]
            )
            
            # Validate required claims
            required_claims = ['user_id', 'exp', 'iat']
            for claim in required_claims:
                if claim not in payload:
                    logger.warning("Missing required JWT claim", claim=claim)
                    raise HTTPException(
                        status_code=status.HTTP_401_UNAUTHORIZED,
                        detail=f"Invalid token: missing {claim}"
                    )
            
            # Check token expiration
            if payload['exp'] < time.time():
                logger.warning("Expired JWT token", user_id=payload.get('user_id'))
                raise HTTPException(
                    status_code=status.HTTP_401_UNAUTHORIZED,
                    detail="Token has expired"
                )
            
            # Extract user information
            user_info = {
                'user_id': payload['user_id'],
                'permissions': payload.get('permissions', []),
                'roles': payload.get('roles', []),
                'session_id': payload.get('session_id'),
                'exp': payload['exp'],
                'iat': payload['iat']
            }
            
            logger.info("User authenticated successfully", 
                       user_id=user_info['user_id'],
                       roles=user_info['roles'])
            
            # Log authentication to Logfire
            logfire.info("User authentication successful",
                        user_id=user_info['user_id'],
                        roles=user_info['roles'],
                        permissions=user_info['permissions'],
                        token_exp=payload['exp'])
            
            return user_info
            
        except jwt.ExpiredSignatureError:
            logger.warning("JWT token expired")
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Token has expired"
            )
        except jwt.InvalidTokenError as e:
            logger.warning("Invalid JWT token", error=str(e))
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Invalid authentication token"
            )
        except Exception as e:
            logger.error("Authentication error", error=str(e))
            logfire.error("Authentication system error", error=str(e))
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail="Authentication system error"
            )
    
    def check_permission(self, user_info: Dict[str, Any], required_permission: str) -> bool:
        """
        Check if user has required permission.
        
        Args:
            user_info: User information from authentication
            required_permission: Permission to check
            
        Returns:
            bool: True if user has permission
        """
        user_permissions = user_info.get('permissions', [])
        user_roles = user_info.get('roles', [])
        
        # Check direct permission
        if required_permission in user_permissions:
            return True
        
        # Check role-based permissions
        role_permissions = {
            'admin': ['*'],  # Admin has all permissions
            'snapshot_manager': [
                'snapshot:create', 'snapshot:read', 'snapshot:delete',
                'snapshot:restore', 'snapshot:list'
            ],
            'snapshot_user': ['snapshot:create', 'snapshot:read', 'snapshot:list'],
            'viewer': ['snapshot:read', 'snapshot:list']
        }
        
        for role in user_roles:
            role_perms = role_permissions.get(role, [])
            if '*' in role_perms or required_permission in role_perms:
                return True
        
        return False
    
    def check_vm_access(self, user_info: Dict[str, Any], vm_id: str) -> bool:
        """
        Check if user has access to specific VM.
        
        Args:
            user_info: User information from authentication
            vm_id: VM ID to check access for
            
        Returns:
            bool: True if user has access
        """
        user_id = user_info['user_id']
        user_roles = user_info.get('roles', [])
        
        # Admin has access to all VMs
        if 'admin' in user_roles:
            return True
        
        # For now, simple ownership check based on VM ID format
        # In real implementation, this would query VM ownership database
        if vm_id.startswith(f"vm_{user_id}_"):
            return True
        
        # Check if VM is shared with user (would query database)
        # This is a placeholder for actual VM access control
        
        logger.warning("VM access denied", user_id=user_id, vm_id=vm_id)
        return False
    
    async def get_current_user(self, request: Request) -> Dict[str, Any]:
        """
        Extract current user from request.
        
        Args:
            request: FastAPI request object
            
        Returns:
            Dict containing user information
        """
        # Try to get credentials from Authorization header
        try:
            credentials = await self.bearer_scheme(request)
            return await self.authenticate_user(credentials)
        except HTTPException:
            raise
        except Exception as e:
            logger.error("Failed to extract user from request", error=str(e))
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Authentication required"
            )


class RateLimiter:
    """
    Rate limiting for API endpoints.
    """
    
    def __init__(self, max_requests: int = 100, window_seconds: int = 3600):
        """Initialize rate limiter."""
        self.max_requests = max_requests
        self.window_seconds = window_seconds
        self.request_counts = {}  # In production, use Redis
    
    def check_rate_limit(self, user_id: str) -> bool:
        """
        Check if user is within rate limits.
        
        Args:
            user_id: User ID to check
            
        Returns:
            bool: True if within limits
        """
        current_time = time.time()
        window_start = current_time - self.window_seconds
        
        # Clean old entries
        if user_id in self.request_counts:
            self.request_counts[user_id] = [
                req_time for req_time in self.request_counts[user_id]
                if req_time > window_start
            ]
        else:
            self.request_counts[user_id] = []
        
        # Check current count
        current_count = len(self.request_counts[user_id])
        
        if current_count >= self.max_requests:
            logger.warning("Rate limit exceeded", 
                          user_id=user_id, 
                          current_count=current_count,
                          max_requests=self.max_requests)
            return False
        
        # Record this request
        self.request_counts[user_id].append(current_time)
        return True