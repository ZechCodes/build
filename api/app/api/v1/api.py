"""API v1 router."""

from fastapi import APIRouter

from app.api.v1.endpoints import auth, health, users, security

api_router = APIRouter()

# Include endpoint routers
api_router.include_router(health.router, prefix="/health", tags=["health"])
api_router.include_router(auth.router, prefix="/auth", tags=["authentication"])
api_router.include_router(users.router, prefix="/users", tags=["users"])
api_router.include_router(security.router, prefix="/security", tags=["security"])