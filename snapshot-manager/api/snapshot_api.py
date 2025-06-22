"""
FastAPI-based REST API for VM snapshot management.

Provides secure, high-performance snapshot operations with comprehensive
monitoring, rate limiting, and security controls.
"""

import time
import asyncio
from typing import Optional, List, Dict, Any
from fastapi import FastAPI, HTTPException, Depends, Query, status, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
import structlog
import logfire

from .models import (
    CreateSnapshotRequest, RestoreSnapshotRequest, SnapshotResponse,
    SnapshotListResponse, SnapshotStatsResponse, ErrorResponse,
    HealthCheckResponse, SnapshotState, SnapshotType
)
from .auth import AuthenticationMiddleware, RateLimiter

logger = structlog.get_logger()


class SnapshotAPI:
    """
    REST API for VM snapshot management.
    
    Provides comprehensive snapshot operations with security, monitoring,
    and performance optimization.
    """
    
    def __init__(self, snapshot_manager, 
                 auth_middleware: AuthenticationMiddleware,
                 cors_origins: List[str] = None):
        """Initialize snapshot API."""
        self.snapshot_manager = snapshot_manager
        self.auth = auth_middleware
        self.rate_limiter = RateLimiter(max_requests=100, window_seconds=3600)
        self.app = FastAPI(
            title="VM Snapshot Manager API",
            description="Secure REST API for VM snapshot management",
            version="1.0.0",
            docs_url="/docs",
            redoc_url="/redoc"
        )
        
        # Add CORS middleware
        self.app.add_middleware(
            CORSMiddleware,
            allow_origins=cors_origins or ["*"],
            allow_credentials=True,
            allow_methods=["*"],
            allow_headers=["*"],
        )
        
        # Initialize metrics
        self.start_time = time.time()
        self.request_count = 0
        
        # Set up routes
        self._setup_routes()
        self._setup_error_handlers()
    
    def _setup_routes(self):
        """Set up API routes."""
        
        @self.app.middleware("http")
        async def log_requests(request: Request, call_next):
            """Log all incoming requests for monitoring."""
            start_time = time.time()
            self.request_count += 1
            
            try:
                response = await call_next(request)
                duration_ms = (time.time() - start_time) * 1000
                
                # Log request details
                logger.info("API request completed",
                           method=request.method,
                           path=request.url.path,
                           status_code=response.status_code,
                           duration_ms=duration_ms)
                
                # Log to Logfire
                logfire.info("API request",
                            method=request.method,
                            path=request.url.path,
                            status_code=response.status_code,
                            duration_ms=duration_ms,
                            user_agent=request.headers.get("user-agent"))
                
                return response
                
            except Exception as e:
                duration_ms = (time.time() - start_time) * 1000
                logger.error("API request failed",
                           method=request.method,
                           path=request.url.path,
                           error=str(e),
                           duration_ms=duration_ms)
                
                logfire.error("API request failed",
                             method=request.method,
                             path=request.url.path,
                             error=str(e),
                             duration_ms=duration_ms)
                raise
        
        @self.app.get("/health", response_model=HealthCheckResponse)
        async def health_check():
            """Health check endpoint."""
            uptime = time.time() - self.start_time
            
            # Check component health
            storage_status = "healthy"  # Would check actual storage
            database_status = "healthy"  # Would check actual database
            
            return HealthCheckResponse(
                status="healthy",
                version="1.0.0",
                uptime_seconds=uptime,
                storage_backend="s3",
                storage_status=storage_status,
                database_status=database_status,
                components={
                    "snapshot_manager": "healthy",
                    "storage_backend": storage_status,
                    "database": database_status,
                    "authentication": "healthy"
                }
            )
        
        @self.app.post("/snapshots", response_model=SnapshotResponse)
        async def create_snapshot(
            request: CreateSnapshotRequest,
            current_user: Dict[str, Any] = Depends(self.auth.get_current_user)
        ):
            """Create a new VM snapshot."""
            await self._check_rate_limit(current_user['user_id'])
            await self._check_permission(current_user, 'snapshot:create')
            await self._check_vm_access(current_user, request.vm_id)
            
            try:
                logger.info("Creating snapshot",
                           user_id=current_user['user_id'],
                           vm_id=request.vm_id,
                           name=request.name)
                
                # Create snapshot
                snapshot_id = await self.snapshot_manager.create_snapshot(
                    vm_id=request.vm_id,
                    user_id=current_user['user_id'],
                    name=request.name,
                    description=request.description,
                    snapshot_type=request.snapshot_type.value,
                    tags=request.tags or {}
                )
                
                # Get snapshot metadata
                metadata = await self.snapshot_manager.get_snapshot_metadata(
                    snapshot_id, current_user['user_id']
                )
                
                # Convert to response model
                response = self._metadata_to_response(metadata)
                
                logger.info("Snapshot created successfully",
                           snapshot_id=snapshot_id,
                           user_id=current_user['user_id'])
                
                logfire.info("Snapshot creation completed",
                            snapshot_id=snapshot_id,
                            user_id=current_user['user_id'],
                            vm_id=request.vm_id,
                            snapshot_type=request.snapshot_type.value)
                
                return response
                
            except Exception as e:
                logger.error("Failed to create snapshot",
                           user_id=current_user['user_id'],
                           vm_id=request.vm_id,
                           error=str(e))
                
                logfire.error("Snapshot creation failed",
                             user_id=current_user['user_id'],
                             vm_id=request.vm_id,
                             error=str(e))
                
                raise HTTPException(
                    status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                    detail=f"Failed to create snapshot: {str(e)}"
                )
        
        @self.app.get("/snapshots", response_model=SnapshotListResponse)
        async def list_snapshots(
            current_user: Dict[str, Any] = Depends(self.auth.get_current_user),
            vm_id: Optional[str] = Query(None, description="Filter by VM ID"),
            state: Optional[SnapshotState] = Query(None, description="Filter by state"),
            page: int = Query(1, ge=1, description="Page number"),
            page_size: int = Query(20, ge=1, le=100, description="Page size")
        ):
            """List user's snapshots with filtering and pagination."""
            await self._check_rate_limit(current_user['user_id'])
            await self._check_permission(current_user, 'snapshot:list')
            
            try:
                # Get user's snapshots
                snapshots = await self.snapshot_manager.list_user_snapshots(
                    current_user['user_id']
                )
                
                # Apply filters
                if vm_id:
                    if not await self._check_vm_access_silent(current_user, vm_id):
                        snapshots = []  # No access to this VM
                    else:
                        snapshots = [s for s in snapshots if s.vm_id == vm_id]
                
                if state:
                    snapshots = [s for s in snapshots if s.state == state.value]
                
                # Apply pagination
                total_count = len(snapshots)
                start_idx = (page - 1) * page_size
                end_idx = start_idx + page_size
                page_snapshots = snapshots[start_idx:end_idx]
                
                # Convert to response models
                response_snapshots = [
                    self._metadata_to_response(snapshot) 
                    for snapshot in page_snapshots
                ]
                
                return SnapshotListResponse(
                    snapshots=response_snapshots,
                    total_count=total_count,
                    page=page,
                    page_size=page_size,
                    has_next=end_idx < total_count
                )
                
            except Exception as e:
                logger.error("Failed to list snapshots",
                           user_id=current_user['user_id'],
                           error=str(e))
                
                raise HTTPException(
                    status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                    detail=f"Failed to list snapshots: {str(e)}"
                )
        
        @self.app.get("/snapshots/{snapshot_id}", response_model=SnapshotResponse)
        async def get_snapshot(
            snapshot_id: str,
            current_user: Dict[str, Any] = Depends(self.auth.get_current_user)
        ):
            """Get specific snapshot details."""
            await self._check_rate_limit(current_user['user_id'])
            await self._check_permission(current_user, 'snapshot:read')
            
            try:
                metadata = await self.snapshot_manager.get_snapshot_metadata(
                    snapshot_id, current_user['user_id']
                )
                
                await self._check_vm_access(current_user, metadata.vm_id)
                
                return self._metadata_to_response(metadata)
                
            except ValueError as e:
                if "not found" in str(e).lower():
                    raise HTTPException(
                        status_code=status.HTTP_404_NOT_FOUND,
                        detail="Snapshot not found"
                    )
                raise HTTPException(
                    status_code=status.HTTP_400_BAD_REQUEST,
                    detail=str(e)
                )
            except Exception as e:
                logger.error("Failed to get snapshot",
                           snapshot_id=snapshot_id,
                           user_id=current_user['user_id'],
                           error=str(e))
                
                raise HTTPException(
                    status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                    detail=f"Failed to get snapshot: {str(e)}"
                )
        
        @self.app.post("/snapshots/{snapshot_id}/restore")
        async def restore_snapshot(
            snapshot_id: str,
            request: RestoreSnapshotRequest,
            current_user: Dict[str, Any] = Depends(self.auth.get_current_user)
        ):
            """Restore a snapshot to a VM."""
            await self._check_rate_limit(current_user['user_id'])
            await self._check_permission(current_user, 'snapshot:restore')
            
            try:
                # Get snapshot metadata first
                metadata = await self.snapshot_manager.get_snapshot_metadata(
                    snapshot_id, current_user['user_id']
                )
                
                # Check VM access
                source_vm_id = metadata.vm_id
                target_vm_id = request.target_vm_id or source_vm_id
                
                await self._check_vm_access(current_user, source_vm_id)
                await self._check_vm_access(current_user, target_vm_id)
                
                logger.info("Restoring snapshot",
                           snapshot_id=snapshot_id,
                           source_vm_id=source_vm_id,
                           target_vm_id=target_vm_id,
                           user_id=current_user['user_id'])
                
                # Restore snapshot
                result = await self.snapshot_manager.restore_snapshot(
                    snapshot_id=snapshot_id,
                    user_id=current_user['user_id'],
                    target_vm_id=target_vm_id,
                    restore_options=request.restore_options or {}
                )
                
                logger.info("Snapshot restored successfully",
                           snapshot_id=snapshot_id,
                           target_vm_id=target_vm_id)
                
                logfire.info("Snapshot restoration completed",
                            snapshot_id=snapshot_id,
                            source_vm_id=source_vm_id,
                            target_vm_id=target_vm_id,
                            user_id=current_user['user_id'])
                
                return {"message": "Snapshot restored successfully", "result": result}
                
            except ValueError as e:
                if "not found" in str(e).lower():
                    raise HTTPException(
                        status_code=status.HTTP_404_NOT_FOUND,
                        detail="Snapshot not found"
                    )
                raise HTTPException(
                    status_code=status.HTTP_400_BAD_REQUEST,
                    detail=str(e)
                )
            except Exception as e:
                logger.error("Failed to restore snapshot",
                           snapshot_id=snapshot_id,
                           user_id=current_user['user_id'],
                           error=str(e))
                
                logfire.error("Snapshot restoration failed",
                             snapshot_id=snapshot_id,
                             user_id=current_user['user_id'],
                             error=str(e))
                
                raise HTTPException(
                    status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                    detail=f"Failed to restore snapshot: {str(e)}"
                )
        
        @self.app.delete("/snapshots/{snapshot_id}")
        async def delete_snapshot(
            snapshot_id: str,
            current_user: Dict[str, Any] = Depends(self.auth.get_current_user)
        ):
            """Delete a snapshot."""
            await self._check_rate_limit(current_user['user_id'])
            await self._check_permission(current_user, 'snapshot:delete')
            
            try:
                # Get snapshot metadata to check VM access
                metadata = await self.snapshot_manager.get_snapshot_metadata(
                    snapshot_id, current_user['user_id']
                )
                
                await self._check_vm_access(current_user, metadata.vm_id)
                
                logger.info("Deleting snapshot",
                           snapshot_id=snapshot_id,
                           user_id=current_user['user_id'])
                
                # Delete snapshot
                result = await self.snapshot_manager.delete_snapshot(
                    snapshot_id, current_user['user_id']
                )
                
                if result:
                    logger.info("Snapshot deleted successfully",
                               snapshot_id=snapshot_id)
                    
                    logfire.info("Snapshot deletion completed",
                                snapshot_id=snapshot_id,
                                user_id=current_user['user_id'])
                    
                    return {"message": "Snapshot deleted successfully"}
                else:
                    raise HTTPException(
                        status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                        detail="Failed to delete snapshot"
                    )
                    
            except ValueError as e:
                if "not found" in str(e).lower():
                    raise HTTPException(
                        status_code=status.HTTP_404_NOT_FOUND,
                        detail="Snapshot not found"
                    )
                raise HTTPException(
                    status_code=status.HTTP_400_BAD_REQUEST,
                    detail=str(e)
                )
            except Exception as e:
                logger.error("Failed to delete snapshot",
                           snapshot_id=snapshot_id,
                           user_id=current_user['user_id'],
                           error=str(e))
                
                raise HTTPException(
                    status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                    detail=f"Failed to delete snapshot: {str(e)}"
                )
        
        @self.app.get("/snapshots/stats", response_model=SnapshotStatsResponse)
        async def get_snapshot_stats(
            current_user: Dict[str, Any] = Depends(self.auth.get_current_user)
        ):
            """Get snapshot statistics for the user."""
            await self._check_rate_limit(current_user['user_id'])
            await self._check_permission(current_user, 'snapshot:read')
            
            try:
                # Get user's snapshots
                snapshots = await self.snapshot_manager.list_user_snapshots(
                    current_user['user_id']
                )
                
                # Calculate statistics
                total_snapshots = len(snapshots)
                total_size_bytes = sum(s.size_bytes or 0 for s in snapshots)
                
                # Group by state
                snapshots_by_state = {}
                for snapshot in snapshots:
                    state = snapshot.state
                    snapshots_by_state[state] = snapshots_by_state.get(state, 0) + 1
                
                # Group by type
                snapshots_by_type = {}
                for snapshot in snapshots:
                    snap_type = snapshot.snapshot_type
                    snapshots_by_type[snap_type] = snapshots_by_type.get(snap_type, 0) + 1
                
                # Calculate average size
                available_snapshots = [s for s in snapshots if s.size_bytes is not None]
                average_size = (
                    sum(s.size_bytes for s in available_snapshots) / len(available_snapshots)
                    if available_snapshots else 0.0
                )
                
                return SnapshotStatsResponse(
                    total_snapshots=total_snapshots,
                    total_size_bytes=total_size_bytes,
                    snapshots_by_state=snapshots_by_state,
                    snapshots_by_type=snapshots_by_type,
                    average_size_bytes=average_size,
                    storage_efficiency=None  # Would calculate from deduplication stats
                )
                
            except Exception as e:
                logger.error("Failed to get snapshot stats",
                           user_id=current_user['user_id'],
                           error=str(e))
                
                raise HTTPException(
                    status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                    detail=f"Failed to get snapshot statistics: {str(e)}"
                )
    
    def _setup_error_handlers(self):
        """Set up global error handlers."""
        
        @self.app.exception_handler(HTTPException)
        async def http_exception_handler(request: Request, exc: HTTPException):
            """Handle HTTP exceptions with consistent error format."""
            return JSONResponse(
                status_code=exc.status_code,
                content=ErrorResponse(
                    error=exc.detail,
                    error_code=str(exc.status_code)
                ).model_dump()
            )
        
        @self.app.exception_handler(ValueError)
        async def value_error_handler(request: Request, exc: ValueError):
            """Handle validation errors."""
            logger.warning("Validation error", error=str(exc))
            return JSONResponse(
                status_code=status.HTTP_400_BAD_REQUEST,
                content=ErrorResponse(
                    error="Invalid request parameters",
                    details=str(exc),
                    error_code="VALIDATION_ERROR"
                ).model_dump()
            )
        
        @self.app.exception_handler(Exception)
        async def general_exception_handler(request: Request, exc: Exception):
            """Handle unexpected errors."""
            logger.error("Unexpected API error", error=str(exc))
            logfire.error("Unexpected API error",
                         error=str(exc),
                         path=request.url.path,
                         method=request.method)
            
            return JSONResponse(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                content=ErrorResponse(
                    error="Internal server error",
                    error_code="INTERNAL_ERROR"
                ).model_dump()
            )
    
    # Helper methods
    
    async def _check_rate_limit(self, user_id: str):
        """Check rate limits for user."""
        if not self.rate_limiter.check_rate_limit(user_id):
            raise HTTPException(
                status_code=status.HTTP_429_TOO_MANY_REQUESTS,
                detail="Rate limit exceeded"
            )
    
    async def _check_permission(self, user_info: Dict[str, Any], permission: str):
        """Check user permissions."""
        if not self.auth.check_permission(user_info, permission):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail=f"Permission denied: {permission}"
            )
    
    async def _check_vm_access(self, user_info: Dict[str, Any], vm_id: str):
        """Check VM access permissions."""
        if not self.auth.check_vm_access(user_info, vm_id):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Access denied to VM"
            )
    
    async def _check_vm_access_silent(self, user_info: Dict[str, Any], vm_id: str) -> bool:
        """Check VM access without raising exception."""
        return self.auth.check_vm_access(user_info, vm_id)
    
    def _metadata_to_response(self, metadata) -> SnapshotResponse:
        """Convert snapshot metadata to API response."""
        from datetime import datetime
        
        # Convert core enums to API enums
        api_snapshot_type = SnapshotType.FULL  # Default mapping
        if hasattr(metadata.snapshot_type, 'value'):
            snapshot_type_str = metadata.snapshot_type.value
        else:
            snapshot_type_str = str(metadata.snapshot_type)
        
        # Map core snapshot types to API types
        if "manual" in snapshot_type_str.lower():
            api_snapshot_type = SnapshotType.FULL
        
        api_state = SnapshotState.AVAILABLE  # Default mapping
        if hasattr(metadata.state, 'value'):
            state_str = metadata.state.value
        else:
            state_str = str(metadata.state)
        
        # Map core states to API states
        state_mapping = {
            "creating": SnapshotState.CREATING,
            "available": SnapshotState.AVAILABLE,
            "deleting": SnapshotState.DELETING,
            "error": SnapshotState.ERROR
        }
        api_state = state_mapping.get(state_str.lower(), SnapshotState.AVAILABLE)
        
        # Convert tags list to dict for API
        tags_dict = {}
        if metadata.tags:
            for tag in metadata.tags:
                if ':' in tag:
                    key, value = tag.split(':', 1)
                    tags_dict[key] = value
                else:
                    tags_dict[tag] = ""
        
        return SnapshotResponse(
            snapshot_id=metadata.snapshot_id,
            vm_id=metadata.vm_id,
            user_id=metadata.user_id,
            name=metadata.name,
            description=metadata.description,
            snapshot_type=api_snapshot_type,
            state=api_state,
            size_bytes=metadata.size_bytes,
            tags=tags_dict,
            created_at=datetime.fromtimestamp(metadata.created_at),
            updated_at=datetime.fromtimestamp(metadata.updated_at),
            expires_at=None  # Not in core metadata
        )