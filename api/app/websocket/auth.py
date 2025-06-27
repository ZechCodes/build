"""WebSocket authentication handler according to Session 2 requirements."""

from fastapi import WebSocket, HTTPException, status
import structlog

from app.core.database import AsyncSessionLocal
from app.models.user import User
from app.security.jwt import JWTManager

logger = structlog.get_logger(__name__)


class WebSocketAuthenticator:
    """WebSocket authentication handler for secure real-time connections."""

    def __init__(self, jwt_manager: JWTManager):
        """Initialize WebSocket authenticator."""
        self.jwt_manager = jwt_manager

    async def authenticate_websocket(self, websocket: WebSocket) -> User:
        """Authenticate WebSocket connection using JWT token."""
        try:
            # Extract token from query parameters or headers
            token = self._extract_token(websocket)
            
            if not token:
                logger.warning("WebSocket authentication failed - no token provided")
                await websocket.close(code=status.WS_1008_POLICY_VIOLATION)
                raise HTTPException(status_code=401, detail="No token provided")
            
            
            # Verify and decode token
            try:
                payload = self.jwt_manager.verify_token(token)
                user_id = payload.get("sub")
                
                if not user_id:
                    logger.warning("WebSocket authentication failed - invalid token payload")
                    await websocket.close(code=status.WS_1008_POLICY_VIOLATION)
                    raise HTTPException(status_code=401, detail="Invalid token")
                
            except HTTPException:
                logger.warning("WebSocket authentication failed - token verification failed")
                await websocket.close(code=status.WS_1008_POLICY_VIOLATION)
                raise HTTPException(status_code=401, detail="Invalid token")
            
            # Get user from database
            async with AsyncSessionLocal() as db:
                user = await db.get(User, user_id)
                
                if not user or not user.is_active:
                    logger.warning(
                        "WebSocket authentication failed - user not found or inactive",
                        user_id=user_id,
                        user_found=user is not None,
                        user_active=user.is_active if user else None
                    )
                    await websocket.close(code=status.WS_1008_POLICY_VIOLATION)
                    raise HTTPException(status_code=401, detail="Invalid user")
                
                logger.info(
                    "WebSocket authentication successful",
                    user_id=str(user.id),
                    email=user.email,
                    role=user.role.value
                )
                
                return user
                
        except HTTPException:
            # Re-raise HTTP exceptions
            raise
        except Exception as e:
            # Handle unexpected errors
            logger.error(
                "WebSocket authentication error",
                error=str(e),
                error_type=type(e).__name__
            )
            await websocket.close(code=status.WS_1011_INTERNAL_ERROR)
            raise HTTPException(status_code=500, detail="Authentication error")

    def _extract_token(self, websocket: WebSocket) -> str | None:
        """Extract JWT token from WebSocket connection."""
        # First, try query parameters
        token = websocket.query_params.get("token")
        if token:
            return token
        
        # Then, try authorization header
        auth_header = websocket.headers.get("authorization")
        if auth_header and auth_header.startswith("Bearer "):
            return auth_header[7:]  # Remove "Bearer " prefix
        
        # No token found
        return None

    async def authenticate_and_authorize(self, websocket: WebSocket, required_permissions: list = None) -> User:
        """Authenticate WebSocket and check permissions."""
        user = await self.authenticate_websocket(websocket)
        
        if required_permissions:
            from app.authorization.permissions import PermissionChecker
            
            for permission in required_permissions:
                if not PermissionChecker.user_has_permission(user, permission):
                    logger.warning(
                        "WebSocket authorization failed",
                        user_id=str(user.id),
                        required_permission=permission.value
                    )
                    await websocket.close(code=status.WS_1008_POLICY_VIOLATION)
                    raise HTTPException(
                        status_code=403, 
                        detail=f"Permission required: {permission.value}"
                    )
        
        return user


class WebSocketConnectionManager:
    """Manage WebSocket connections with authentication."""

    def __init__(self, authenticator: WebSocketAuthenticator):
        """Initialize connection manager."""
        self.authenticator = authenticator
        self.active_connections: dict[str, dict] = {}  # user_id -> connection info

    async def connect(self, websocket: WebSocket, required_permissions: list = None) -> User:
        """Accept WebSocket connection and authenticate user."""
        await websocket.accept()
        
        try:
            user = await self.authenticator.authenticate_and_authorize(
                websocket, required_permissions
            )
            
            # Store connection
            connection_info = {
                "websocket": websocket,
                "user": user,
                "permissions": required_permissions or []
            }
            
            self.active_connections[str(user.id)] = connection_info
            
            logger.info(
                "WebSocket connection established",
                user_id=str(user.id),
                email=user.email,
                total_connections=len(self.active_connections)
            )
            
            return user
            
        except HTTPException:
            # Authentication failed, connection should already be closed
            await websocket.close(code=status.WS_1008_POLICY_VIOLATION)
            raise

    async def disconnect(self, user_id: str):
        """Disconnect and remove user connection."""
        if user_id in self.active_connections:
            connection_info = self.active_connections[user_id]
            websocket = connection_info["websocket"]
            
            try:
                await websocket.close()
            except Exception as e:
                logger.warning(
                    "Error closing WebSocket connection",
                    user_id=user_id,
                    error=str(e)
                )
            
            del self.active_connections[user_id]
            
            logger.info(
                "WebSocket connection closed",
                user_id=user_id,
                total_connections=len(self.active_connections)
            )

    async def send_personal_message(self, message: str, user_id: str):
        """Send message to specific user."""
        if user_id in self.active_connections:
            websocket = self.active_connections[user_id]["websocket"]
            try:
                await websocket.send_text(message)
            except Exception as e:
                logger.error(
                    "Failed to send WebSocket message",
                    user_id=user_id,
                    error=str(e)
                )
                # Remove broken connection
                await self.disconnect(user_id)

    async def broadcast(self, message: str, exclude_user_id: str = None):
        """Broadcast message to all connected users."""
        disconnected_users = []
        
        for user_id, connection_info in self.active_connections.items():
            if exclude_user_id and user_id == exclude_user_id:
                continue
                
            websocket = connection_info["websocket"]
            try:
                await websocket.send_text(message)
            except Exception as e:
                logger.error(
                    "Failed to broadcast WebSocket message",
                    user_id=user_id,
                    error=str(e)
                )
                disconnected_users.append(user_id)
        
        # Clean up broken connections
        for user_id in disconnected_users:
            await self.disconnect(user_id)

    def get_user_connection(self, user_id: str) -> dict | None:
        """Get connection info for user."""
        return self.active_connections.get(user_id)

    def is_user_connected(self, user_id: str) -> bool:
        """Check if user is connected."""
        return user_id in self.active_connections

    def get_connected_users(self) -> list[str]:
        """Get list of connected user IDs."""
        return list(self.active_connections.keys())

    def get_connection_count(self) -> int:
        """Get total number of active connections."""
        return len(self.active_connections)