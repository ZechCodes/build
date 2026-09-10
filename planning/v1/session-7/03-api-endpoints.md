# Session 7.3: API Endpoints & Scheduling System

## Objective
Implement comprehensive REST API endpoints for snapshot management with automated scheduling, user quota management, and snapshot lifecycle operations.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for API request monitoring and performance tracking
- **Session 2**: Integrates with authentication middleware for API security
- **Session 5**: Uses database models for snapshot metadata and user quota tracking
- **Session 6**: Coordinates with session management for active session snapshots

## Core Implementation

### Snapshot API Endpoints
**Location**: `snapshot-manager/api/endpoints.py`

```python
# snapshot-manager/api/endpoints.py
import asyncio
from typing import Dict, Any, List, Optional
from datetime import datetime, timedelta
from fastapi import APIRouter, Depends, HTTPException, status, BackgroundTasks
from fastapi.security import HTTPBearer
from pydantic import BaseModel, Field
import structlog
import logfire

from ..core.snapshot_manager import SnapshotManager, SnapshotType, SnapshotState
from ..models.snapshot_models import SnapshotModel, SnapshotQuotaModel
from ..auth.dependencies import get_current_user, require_permission
from ..scheduling.scheduler import SnapshotScheduler

logger = structlog.get_logger()
router = APIRouter()
security = HTTPBearer()

# Pydantic models for API
class CreateSnapshotRequest(BaseModel):
    vm_id: str = Field(..., description="VM ID to snapshot")
    name: str = Field(..., min_length=1, max_length=100, description="Snapshot name")
    description: str = Field("", max_length=500, description="Snapshot description")
    snapshot_type: SnapshotType = Field(SnapshotType.MANUAL, description="Snapshot type")
    tags: List[str] = Field(default_factory=list, max_items=10, description="Snapshot tags")
    encrypt: bool = Field(True, description="Encrypt snapshot data")

class SnapshotResponse(BaseModel):
    snapshot_id: str
    vm_id: str
    name: str
    description: str
    snapshot_type: str
    state: str
    created_at: datetime
    size_bytes: int
    compressed_size_bytes: int
    version: int
    tags: List[str]
    is_encrypted: bool

class CreateScheduleRequest(BaseModel):
    vm_id: str = Field(..., description="VM ID to schedule snapshots for")
    name: str = Field(..., min_length=1, max_length=100, description="Schedule name")
    cron_expression: str = Field(..., description="Cron expression for schedule")
    retention_days: int = Field(7, ge=1, le=365, description="Days to retain snapshots")
    max_snapshots: int = Field(10, ge=1, le=100, description="Maximum snapshots to keep")
    enabled: bool = Field(True, description="Whether schedule is enabled")

class SnapshotQuotaResponse(BaseModel):
    user_id: str
    max_snapshots: int
    used_snapshots: int
    max_storage_gb: int
    used_storage_gb: float
    quota_remaining: int
    storage_remaining_gb: float

@router.post("/snapshots", response_model=SnapshotResponse, status_code=201)
async def create_snapshot(
    request: CreateSnapshotRequest,
    background_tasks: BackgroundTasks,
    current_user: dict = Depends(get_current_user),
    snapshot_manager: SnapshotManager = Depends(),
    _: dict = Depends(require_permission("snapshot:create"))
):
    """Create a new VM snapshot"""
    try:
        user_id = current_user["user_id"]
        
        # Log API request
        logfire.info("Snapshot creation API request",
                    user_id=user_id,
                    vm_id=request.vm_id,
                    name=request.name,
                    snapshot_type=request.snapshot_type.value)
        
        # Validate quota
        await _validate_user_quota(user_id)
        
        # Create snapshot
        snapshot_id = await snapshot_manager.create_snapshot(
            vm_id=request.vm_id,
            user_id=user_id,
            name=request.name,
            description=request.description,
            snapshot_type=request.snapshot_type,
            tags=request.tags,
            encrypt=request.encrypt
        )
        
        # Get snapshot metadata for response
        metadata = snapshot_manager.snapshots[snapshot_id]
        
        logger.info("Snapshot creation requested via API", 
                   snapshot_id=snapshot_id,
                   user_id=user_id,
                   vm_id=request.vm_id)
        
        return SnapshotResponse(
            snapshot_id=metadata.snapshot_id,
            vm_id=metadata.vm_id,
            name=metadata.name,
            description=metadata.description,
            snapshot_type=metadata.snapshot_type.value,
            state=metadata.state.value,
            created_at=datetime.fromtimestamp(metadata.created_at),
            size_bytes=metadata.size_bytes,
            compressed_size_bytes=metadata.compressed_size_bytes,
            version=metadata.version,
            tags=metadata.tags,
            is_encrypted=metadata.is_encrypted
        )
        
    except Exception as e:
        logger.error("Snapshot creation API failed", 
                    user_id=current_user.get("user_id"),
                    error=str(e))
        logfire.error("Snapshot creation API error",
                     user_id=current_user.get("user_id"),
                     error=str(e))
        
        if "quota" in str(e).lower():
            raise HTTPException(
                status_code=status.HTTP_402_PAYMENT_REQUIRED,
                detail="Snapshot quota exceeded"
            )
        elif "permission" in str(e).lower():
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="VM access denied"
            )
        else:
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail="Snapshot creation failed"
            )

@router.get("/snapshots", response_model=List[SnapshotResponse])
async def list_snapshots(
    vm_id: Optional[str] = None,
    limit: int = Field(default=50, ge=1, le=1000),
    offset: int = Field(default=0, ge=0),
    current_user: dict = Depends(get_current_user),
    snapshot_manager: SnapshotManager = Depends(),
    _: dict = Depends(require_permission("snapshot:read"))
):
    """List user's snapshots"""
    try:
        user_id = current_user["user_id"]
        
        # Filter snapshots by user and optionally by VM
        user_snapshots = [
            metadata for metadata in snapshot_manager.snapshots.values()
            if metadata.user_id == user_id and 
               (vm_id is None or metadata.vm_id == vm_id)
        ]
        
        # Sort by creation time (newest first)
        user_snapshots.sort(key=lambda x: x.created_at, reverse=True)
        
        # Apply pagination
        paginated_snapshots = user_snapshots[offset:offset + limit]
        
        # Convert to response models
        response_snapshots = [
            SnapshotResponse(
                snapshot_id=metadata.snapshot_id,
                vm_id=metadata.vm_id,
                name=metadata.name,
                description=metadata.description,
                snapshot_type=metadata.snapshot_type.value,
                state=metadata.state.value,
                created_at=datetime.fromtimestamp(metadata.created_at),
                size_bytes=metadata.size_bytes,
                compressed_size_bytes=metadata.compressed_size_bytes,
                version=metadata.version,
                tags=metadata.tags,
                is_encrypted=metadata.is_encrypted
            )
            for metadata in paginated_snapshots
        ]
        
        logger.info("Snapshots listed via API", 
                   user_id=user_id,
                   vm_id=vm_id,
                   count=len(response_snapshots))
        
        logfire.info("Snapshot list API request",
                    user_id=user_id,
                    vm_id=vm_id,
                    results_count=len(response_snapshots),
                    total_snapshots=len(user_snapshots))
        
        return response_snapshots
        
    except Exception as e:
        logger.error("Snapshot listing API failed", 
                    user_id=current_user.get("user_id"),
                    error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to list snapshots"
        )

@router.get("/snapshots/{snapshot_id}", response_model=SnapshotResponse)
async def get_snapshot(
    snapshot_id: str,
    current_user: dict = Depends(get_current_user),
    snapshot_manager: SnapshotManager = Depends(),
    _: dict = Depends(require_permission("snapshot:read"))
):
    """Get specific snapshot details"""
    try:
        user_id = current_user["user_id"]
        
        # Get snapshot metadata
        metadata = snapshot_manager.snapshots.get(snapshot_id)
        if not metadata:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Snapshot not found"
            )
        
        # Verify ownership
        if metadata.user_id != user_id:
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Snapshot access denied"
            )
        
        logger.info("Snapshot details retrieved via API", 
                   snapshot_id=snapshot_id,
                   user_id=user_id)
        
        return SnapshotResponse(
            snapshot_id=metadata.snapshot_id,
            vm_id=metadata.vm_id,
            name=metadata.name,
            description=metadata.description,
            snapshot_type=metadata.snapshot_type.value,
            state=metadata.state.value,
            created_at=datetime.fromtimestamp(metadata.created_at),
            size_bytes=metadata.size_bytes,
            compressed_size_bytes=metadata.compressed_size_bytes,
            version=metadata.version,
            tags=metadata.tags,
            is_encrypted=metadata.is_encrypted
        )
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Snapshot details API failed", 
                    snapshot_id=snapshot_id,
                    error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to get snapshot details"
        )

@router.post("/snapshots/{snapshot_id}/restore")
async def restore_snapshot(
    snapshot_id: str,
    target_vm_id: Optional[str] = None,
    background_tasks: BackgroundTasks = BackgroundTasks(),
    current_user: dict = Depends(get_current_user),
    snapshot_manager: SnapshotManager = Depends(),
    _: dict = Depends(require_permission("snapshot:restore"))
):
    """Restore a snapshot to a VM"""
    try:
        user_id = current_user["user_id"]
        
        # Log restoration request
        logfire.info("Snapshot restoration API request",
                    user_id=user_id,
                    snapshot_id=snapshot_id,
                    target_vm_id=target_vm_id)
        
        # Start restoration
        restored_vm_id = await snapshot_manager.restore_snapshot(
            snapshot_id=snapshot_id,
            user_id=user_id,
            target_vm_id=target_vm_id
        )
        
        logger.info("Snapshot restoration started via API", 
                   snapshot_id=snapshot_id,
                   target_vm_id=restored_vm_id,
                   user_id=user_id)
        
        return {
            "message": "Snapshot restoration started",
            "snapshot_id": snapshot_id,
            "target_vm_id": restored_vm_id,
            "status": "restoring"
        }
        
    except Exception as e:
        logger.error("Snapshot restoration API failed", 
                    snapshot_id=snapshot_id,
                    error=str(e))
        logfire.error("Snapshot restoration API error",
                     snapshot_id=snapshot_id,
                     error=str(e))
        
        if "not found" in str(e).lower():
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Snapshot not found"
            )
        elif "access denied" in str(e).lower():
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Snapshot access denied"
            )
        else:
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail="Snapshot restoration failed"
            )

@router.delete("/snapshots/{snapshot_id}")
async def delete_snapshot(
    snapshot_id: str,
    current_user: dict = Depends(get_current_user),
    snapshot_manager: SnapshotManager = Depends(),
    _: dict = Depends(require_permission("snapshot:delete"))
):
    """Delete a snapshot"""
    try:
        user_id = current_user["user_id"]
        
        # Verify snapshot exists and ownership
        metadata = snapshot_manager.snapshots.get(snapshot_id)
        if not metadata:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Snapshot not found"
            )
        
        if metadata.user_id != user_id:
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Snapshot access denied"
            )
        
        # Delete snapshot
        success = await snapshot_manager.delete_snapshot(snapshot_id, user_id)
        
        if success:
            logger.info("Snapshot deleted via API", 
                       snapshot_id=snapshot_id,
                       user_id=user_id)
            
            logfire.info("Snapshot deletion completed",
                        snapshot_id=snapshot_id,
                        user_id=user_id)
            
            return {"message": "Snapshot deleted successfully"}
        else:
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail="Failed to delete snapshot"
            )
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Snapshot deletion API failed", 
                    snapshot_id=snapshot_id,
                    error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Snapshot deletion failed"
        )

@router.get("/quota", response_model=SnapshotQuotaResponse)
async def get_user_quota(
    current_user: dict = Depends(get_current_user),
    snapshot_manager: SnapshotManager = Depends()
):
    """Get user's snapshot quota information"""
    try:
        user_id = current_user["user_id"]
        
        # Get quota information
        quota_info = await _get_user_quota_info(user_id, snapshot_manager)
        
        logger.info("User quota retrieved via API", 
                   user_id=user_id,
                   used_snapshots=quota_info["used_snapshots"],
                   used_storage_gb=quota_info["used_storage_gb"])
        
        return SnapshotQuotaResponse(**quota_info)
        
    except Exception as e:
        logger.error("User quota API failed", 
                    user_id=current_user.get("user_id"),
                    error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to get quota information"
        )
```

### Snapshot Scheduler
**Location**: `snapshot-manager/scheduling/scheduler.py`

```python
# snapshot-manager/scheduling/scheduler.py
import asyncio
from typing import Dict, List, Optional
from datetime import datetime, timedelta
from dataclasses import dataclass
from croniter import croniter
import structlog
import logfire

logger = structlog.get_logger()

@dataclass
class SnapshotSchedule:
    schedule_id: str
    user_id: str
    vm_id: str
    name: str
    cron_expression: str
    retention_days: int
    max_snapshots: int
    enabled: bool
    created_at: datetime
    last_run: Optional[datetime]
    next_run: datetime

class SnapshotScheduler:
    def __init__(self, snapshot_manager):
        self.snapshot_manager = snapshot_manager
        self.schedules: Dict[str, SnapshotSchedule] = {}
        self.scheduler_task: Optional[asyncio.Task] = None
        self.check_interval = 60  # Check every minute
        
    async def initialize(self):
        """Initialize the snapshot scheduler"""
        # Load existing schedules from database
        await self._load_schedules()
        
        # Start scheduler task
        self.scheduler_task = asyncio.create_task(self._scheduler_loop())
        
        logger.info("Snapshot scheduler initialized", 
                   schedules_count=len(self.schedules))
        logfire.info("Snapshot scheduler started",
                    active_schedules=len(self.schedules))
    
    async def create_schedule(self, user_id: str, vm_id: str, name: str,
                            cron_expression: str, retention_days: int,
                            max_snapshots: int, enabled: bool = True) -> str:
        """Create a new snapshot schedule"""
        try:
            # Validate cron expression
            if not croniter.is_valid(cron_expression):
                raise ValueError("Invalid cron expression")
            
            schedule_id = self._generate_schedule_id(user_id, vm_id)
            
            # Calculate next run time
            cron = croniter(cron_expression, datetime.now())
            next_run = cron.get_next(datetime)
            
            schedule = SnapshotSchedule(
                schedule_id=schedule_id,
                user_id=user_id,
                vm_id=vm_id,
                name=name,
                cron_expression=cron_expression,
                retention_days=retention_days,
                max_snapshots=max_snapshots,
                enabled=enabled,
                created_at=datetime.now(),
                last_run=None,
                next_run=next_run
            )
            
            self.schedules[schedule_id] = schedule
            
            # Persist to database
            await self._persist_schedule(schedule)
            
            logger.info("Snapshot schedule created", 
                       schedule_id=schedule_id,
                       user_id=user_id,
                       vm_id=vm_id,
                       cron_expression=cron_expression)
            
            logfire.info("Snapshot schedule created",
                        schedule_id=schedule_id,
                        user_id=user_id,
                        vm_id=vm_id,
                        next_run=next_run.isoformat())
            
            return schedule_id
            
        except Exception as e:
            logger.error("Failed to create snapshot schedule", 
                        user_id=user_id,
                        vm_id=vm_id,
                        error=str(e))
            raise
    
    async def _scheduler_loop(self):
        """Main scheduler loop"""
        while True:
            try:
                await asyncio.sleep(self.check_interval)
                await self._check_and_execute_schedules()
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Scheduler loop error", error=str(e))
    
    async def _check_and_execute_schedules(self):
        """Check and execute due schedules"""
        current_time = datetime.now()
        
        for schedule in self.schedules.values():
            if not schedule.enabled:
                continue
                
            if current_time >= schedule.next_run:
                # Execute schedule
                await self._execute_schedule(schedule)
                
                # Update next run time
                cron = croniter(schedule.cron_expression, current_time)
                schedule.next_run = cron.get_next(datetime)
                schedule.last_run = current_time
                
                # Persist updates
                await self._persist_schedule(schedule)
    
    async def _execute_schedule(self, schedule: SnapshotSchedule):
        """Execute a scheduled snapshot"""
        try:
            logger.info("Executing scheduled snapshot", 
                       schedule_id=schedule.schedule_id,
                       vm_id=schedule.vm_id)
            
            # Create automatic snapshot
            snapshot_name = f"{schedule.name}_{datetime.now().strftime('%Y%m%d_%H%M%S')}"
            
            snapshot_id = await self.snapshot_manager.create_snapshot(
                vm_id=schedule.vm_id,
                user_id=schedule.user_id,
                name=snapshot_name,
                description=f"Automatic snapshot from schedule {schedule.name}",
                snapshot_type=SnapshotType.AUTOMATIC
            )
            
            # Clean up old snapshots based on retention policy
            await self._cleanup_old_snapshots(schedule)
            
            logger.info("Scheduled snapshot created", 
                       schedule_id=schedule.schedule_id,
                       snapshot_id=snapshot_id)
            
            logfire.info("Scheduled snapshot executed",
                        schedule_id=schedule.schedule_id,
                        snapshot_id=snapshot_id,
                        vm_id=schedule.vm_id)
            
        except Exception as e:
            logger.error("Failed to execute scheduled snapshot", 
                        schedule_id=schedule.schedule_id,
                        error=str(e))
            logfire.error("Scheduled snapshot execution failed",
                         schedule_id=schedule.schedule_id,
                         error=str(e))
```

## TDD Implementation Cycle

### Test-Driven Development Process

1. **Red Phase**: Write failing API tests
   ```bash
   # Create API test file
   touch snapshot-manager/tests/test_api_endpoints.py
   
   # Run failing test
   pytest snapshot-manager/tests/test_api_endpoints.py::test_create_snapshot_api -v
   ```

2. **Green Phase**: Implement basic API functionality
   ```bash
   # Implement API endpoints
   pytest snapshot-manager/tests/test_api_endpoints.py::test_create_snapshot_api -v
   ```

3. **Refactor Phase**: Add validation and error handling
   ```bash
   # Add comprehensive validation and security
   pytest snapshot-manager/tests/ -v
   ```

4. **Commit**: Commit API functionality
   ```bash
   git add snapshot-manager/api/ snapshot-manager/scheduling/ snapshot-manager/tests/test_api_endpoints.py
   git commit -m "feat: implement snapshot REST API with scheduling system
   
   - Add comprehensive REST API endpoints for snapshot CRUD operations
   - Implement automated snapshot scheduling with cron expressions
   - Add user quota management and validation
   - Include comprehensive request/response validation with Pydantic
   - Integrate with Logfire for API request monitoring and analytics
   
   Tests: Added comprehensive API test suite with authentication scenarios
   Security: JWT authentication, authorization, and input validation
   Performance: Pagination, filtering, and efficient quota checking"
   ```

### API Test Cases

```python
# snapshot-manager/tests/test_api_endpoints.py
import pytest
from fastapi.testclient import TestClient
from unittest.mock import AsyncMock, patch
from snapshot_manager.api.app import app

@pytest.fixture
def client():
    """Test client for API"""
    return TestClient(app)

@pytest.fixture
def auth_headers():
    """Authentication headers for testing"""
    return {"Authorization": "Bearer test_jwt_token"}

class TestSnapshotAPI:
    def test_create_snapshot_success(self, client, auth_headers):
        """Test successful snapshot creation via API"""
        with patch('snapshot_manager.api.endpoints.get_current_user') as mock_user:
            mock_user.return_value = {"user_id": "user123"}
            
            request_data = {
                "vm_id": "vm123",
                "name": "test-snapshot",
                "description": "Test snapshot",
                "encrypt": True
            }
            
            response = client.post(
                "/snapshots",
                json=request_data,
                headers=auth_headers
            )
            
            assert response.status_code == 201
            data = response.json()
            assert data["name"] == "test-snapshot"
            assert data["vm_id"] == "vm123"
    
    def test_create_snapshot_unauthorized(self, client):
        """Test snapshot creation without authentication"""
        request_data = {
            "vm_id": "vm123",
            "name": "test-snapshot"
        }
        
        response = client.post("/snapshots", json=request_data)
        assert response.status_code == 401
    
    def test_list_snapshots_with_pagination(self, client, auth_headers):
        """Test snapshot listing with pagination"""
        with patch('snapshot_manager.api.endpoints.get_current_user') as mock_user:
            mock_user.return_value = {"user_id": "user123"}
            
            response = client.get(
                "/snapshots?limit=10&offset=0",
                headers=auth_headers
            )
            
            assert response.status_code == 200
            data = response.json()
            assert isinstance(data, list)
            assert len(data) <= 10
```

## Security Checklist for API Endpoints

### API Security Controls
- [ ] JWT authentication required for all snapshot operations
- [ ] Authorization validation based on user permissions and roles
- [ ] Input validation with Pydantic models for all request data
- [ ] Output sanitization to prevent information disclosure
- [ ] Rate limiting on API endpoints (100 requests/minute per user)
- [ ] Request size limits to prevent DoS attacks (10MB max request)
- [ ] SQL injection prevention with parameterized queries
- [ ] Cross-site scripting (XSS) prevention in API responses
- [ ] Cross-origin resource sharing (CORS) configuration
- [ ] API versioning for backward compatibility and security updates

### Resource Access Control
- [ ] Snapshot ownership validation on all operations
- [ ] VM ownership verification before snapshot creation
- [ ] Cross-user resource access prevention with strict authorization
- [ ] Administrative API access controls with elevated permissions
- [ ] API key management for service-to-service communication
- [ ] Audit logging for all API operations with request/response data
- [ ] Error handling that doesn't leak sensitive information
- [ ] Resource enumeration protection with user-scoped queries
- [ ] Quota enforcement with proper error responses
- [ ] Session management integration for API access control

### Scheduling Security
- [ ] Schedule ownership validation for all schedule operations
- [ ] Cron expression validation to prevent malicious schedules
- [ ] Schedule execution authorization with user context validation
- [ ] Rate limiting on schedule creation (10 schedules per user max)
- [ ] Schedule enumeration protection with user-scoped access
- [ ] Automated snapshot cleanup with secure deletion procedures
- [ ] Schedule modification audit logging with change tracking
- [ ] Resource usage monitoring for scheduled operations
- [ ] Schedule execution error handling with proper alerting
- [ ] Schedule permissions inheritance from parent resources

## Performance Requirements

### API Performance
- API response time < 200ms for metadata operations
- API response time < 500ms for snapshot creation initiation
- Pagination efficiency for large result sets
- Concurrent API request handling (100+ simultaneous)
- Database query optimization for listing operations
- Caching for frequently accessed metadata

### Scheduling Performance
- Schedule check interval every 60 seconds
- Schedule execution latency < 30 seconds
- Concurrent schedule execution (10+ simultaneous)
- Retention cleanup efficiency for large snapshot sets
- Memory usage < 100MB for scheduler service
- Schedule database query optimization

## Error Handling and Validation

### API Error Responses
```python
# Standardized error responses
class APIError(HTTPException):
    def __init__(self, status_code: int, detail: str, error_code: str = None):
        super().__init__(status_code=status_code, detail=detail)
        self.error_code = error_code

# Example error handling
try:
    await snapshot_manager.create_snapshot(...)
except PermissionError:
    raise APIError(
        status_code=403,
        detail="VM access denied",
        error_code="VM_ACCESS_DENIED"
    )
except QuotaExceededError:
    raise APIError(
        status_code=402,
        detail="Snapshot quota exceeded",
        error_code="QUOTA_EXCEEDED"
    )
```

### Input Validation
```python
# Pydantic validation with custom validators
class CreateSnapshotRequest(BaseModel):
    vm_id: str = Field(..., regex=r'^vm_[a-zA-Z0-9_-]+$')
    name: str = Field(..., min_length=1, max_length=100)
    
    @validator('name')
    def validate_name(cls, v):
        if not v.replace('-', '').replace('_', '').isalnum():
            raise ValueError('Name must be alphanumeric with dashes and underscores')
        return v
```

## Quota Management System

### User Quota Implementation
```python
async def _validate_user_quota(user_id: str):
    """Validate user snapshot quota"""
    quota_info = await _get_user_quota_info(user_id)
    
    if quota_info["used_snapshots"] >= quota_info["max_snapshots"]:
        raise QuotaExceededError("Maximum snapshot count exceeded")
    
    if quota_info["used_storage_gb"] >= quota_info["max_storage_gb"]:
        raise QuotaExceededError("Storage quota exceeded")

async def _get_user_quota_info(user_id: str, snapshot_manager) -> Dict[str, Any]:
    """Get comprehensive user quota information"""
    # Get user's snapshots
    user_snapshots = [
        s for s in snapshot_manager.snapshots.values()
        if s.user_id == user_id
    ]
    
    # Calculate usage
    used_snapshots = len(user_snapshots)
    used_storage_bytes = sum(s.compressed_size_bytes for s in user_snapshots)
    used_storage_gb = used_storage_bytes / (1024 ** 3)
    
    # Get user limits (from database or config)
    max_snapshots = 50  # Default limit
    max_storage_gb = 100.0  # Default limit
    
    return {
        "user_id": user_id,
        "max_snapshots": max_snapshots,
        "used_snapshots": used_snapshots,
        "max_storage_gb": max_storage_gb,
        "used_storage_gb": round(used_storage_gb, 2),
        "quota_remaining": max_snapshots - used_snapshots,
        "storage_remaining_gb": round(max_storage_gb - used_storage_gb, 2)
    }
```

## Integration Testing

### API Integration Tests
```python
async def test_api_integration_flow():
    """Test complete API integration flow"""
    # Create snapshot
    create_response = await client.post("/snapshots", json=snapshot_data)
    snapshot_id = create_response.json()["snapshot_id"]
    
    # List snapshots
    list_response = await client.get("/snapshots")
    assert any(s["snapshot_id"] == snapshot_id for s in list_response.json())
    
    # Get snapshot details
    details_response = await client.get(f"/snapshots/{snapshot_id}")
    assert details_response.json()["snapshot_id"] == snapshot_id
    
    # Restore snapshot
    restore_response = await client.post(f"/snapshots/{snapshot_id}/restore")
    assert restore_response.status_code == 200
    
    # Delete snapshot
    delete_response = await client.delete(f"/snapshots/{snapshot_id}")
    assert delete_response.status_code == 200
```

## API Documentation

### OpenAPI Schema Generation
```python
# FastAPI automatic OpenAPI generation with custom descriptions
app = FastAPI(
    title="Build Platform Snapshot API",
    description="Comprehensive VM snapshot management API",
    version="1.0.0",
    openapi_tags=[
        {
            "name": "snapshots",
            "description": "VM snapshot operations"
        },
        {
            "name": "schedules", 
            "description": "Automated snapshot scheduling"
        },
        {
            "name": "quota",
            "description": "User quota management"
        }
    ]
)

# Include comprehensive examples in schema
@router.post("/snapshots", response_model=SnapshotResponse)
async def create_snapshot(
    request: CreateSnapshotRequest = Body(
        ...,
        example={
            "vm_id": "vm_abc123",
            "name": "my-snapshot",
            "description": "Snapshot before deployment",
            "tags": ["deployment", "backup"],
            "encrypt": True
        }
    )
):
    """Create a new VM snapshot with optional encryption and tagging"""
    pass
```

## Next Implementation Steps

1. **Complete API endpoint implementation** with all CRUD operations
2. **Implement comprehensive input validation** with Pydantic models
3. **Add automated scheduling system** with cron expressions
4. **Create quota management system** with user limits
5. **Add comprehensive error handling** with structured responses
6. **Implement API rate limiting** and security controls
7. **Create API documentation** with examples and integration guides

## Commit Guidelines

API commits should include:
- **Comprehensive input validation** for all endpoints
- **Security controls** with authentication and authorization
- **Error handling** with appropriate HTTP status codes
- **Test coverage** for API endpoints and edge cases (>80%)
- **Integration verification** with snapshot manager and authentication
- **Documentation updates** with API examples and usage guidelines