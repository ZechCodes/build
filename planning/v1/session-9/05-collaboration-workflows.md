# Session 9.5: Collaboration Workflows & Access Management

## Objective
Implement comprehensive Git collaboration workflows, access control management, and team-based development features, enabling secure multi-user Git repository collaboration with fine-grained permissions and workflow automation.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for collaboration activity monitoring and workflow analytics
- **Session 2**: Integrates with authentication system for user verification and team management
- **Session 9.1**: Provides repository foundation for collaboration features
- **Session 9.2**: Uses SSH key management for secure team member access
- **Session 9.3**: Leverages Soft-serve integration for advanced Git server features

## Core Implementation

### Git Collaboration Manager
**Location**: `git-manager/collaboration/collaboration_manager.py`

```python
# git-manager/collaboration/collaboration_manager.py
import asyncio
import time
import json
from typing import Dict, Any, List, Optional, Set
from dataclasses import dataclass, asdict
from enum import Enum
import structlog
import logfire
from datetime import datetime, timedelta

logger = structlog.get_logger()

class PermissionLevel(Enum):
    READ = "read"
    WRITE = "write"
    MAINTAIN = "maintain"
    ADMIN = "admin"
    OWNER = "owner"

class WorkflowAction(Enum):
    CREATE_BRANCH = "create_branch"
    DELETE_BRANCH = "delete_branch"
    MERGE_REQUEST = "merge_request"
    APPROVE_MERGE = "approve_merge"
    FORCE_PUSH = "force_push"
    CREATE_TAG = "create_tag"
    DELETE_TAG = "delete_tag"
    MODIFY_SETTINGS = "modify_settings"

class TeamRole(Enum):
    MEMBER = "member"
    MAINTAINER = "maintainer"
    ADMIN = "admin"
    OWNER = "owner"

@dataclass
class RepositoryTeam:
    id: str
    repository_id: str
    name: str
    description: str
    permission_level: PermissionLevel
    members: List[str]
    created_at: float
    updated_at: float
    created_by: str

@dataclass
class CollaboratorPermission:
    user_id: str
    repository_id: str
    permission_level: PermissionLevel
    granted_by: str
    granted_at: float
    expires_at: Optional[float]
    is_active: bool
    restrictions: List[str]

@dataclass
class BranchProtectionRule:
    id: str
    repository_id: str
    branch_pattern: str
    required_reviews: int
    dismiss_stale_reviews: bool
    require_code_owner_review: bool
    required_status_checks: List[str]
    enforce_admins: bool
    allow_force_pushes: bool
    allow_deletions: bool
    created_by: str
    created_at: float

@dataclass
class MergeRequest:
    id: str
    repository_id: str
    source_branch: str
    target_branch: str
    title: str
    description: str
    author_id: str
    assignees: List[str]
    reviewers: List[str]
    labels: List[str]
    status: str  # open, merged, closed, draft
    created_at: float
    updated_at: float
    merged_at: Optional[float]
    merged_by: Optional[str]
    conflicts: bool
    reviews: List[Dict[str, Any]]

@dataclass
class WorkflowRule:
    id: str
    repository_id: str
    name: str
    trigger_events: List[str]
    conditions: Dict[str, Any]
    actions: List[Dict[str, Any]]
    is_active: bool
    created_by: str
    created_at: float

class GitCollaborationManager:
    def __init__(self, repository_manager, ssh_key_manager, softserve_client, database):
        self.repo_manager = repository_manager
        self.ssh_manager = ssh_key_manager
        self.softserve = softserve_client
        self.db = database
        
        # Permission matrix
        self.permission_matrix = {
            PermissionLevel.READ: {
                WorkflowAction.CREATE_BRANCH,
            },
            PermissionLevel.WRITE: {
                WorkflowAction.CREATE_BRANCH,
                WorkflowAction.DELETE_BRANCH,
                WorkflowAction.MERGE_REQUEST,
            },
            PermissionLevel.MAINTAIN: {
                WorkflowAction.CREATE_BRANCH,
                WorkflowAction.DELETE_BRANCH,
                WorkflowAction.MERGE_REQUEST,
                WorkflowAction.APPROVE_MERGE,
                WorkflowAction.CREATE_TAG,
                WorkflowAction.DELETE_TAG,
            },
            PermissionLevel.ADMIN: {
                WorkflowAction.CREATE_BRANCH,
                WorkflowAction.DELETE_BRANCH,
                WorkflowAction.MERGE_REQUEST,
                WorkflowAction.APPROVE_MERGE,
                WorkflowAction.FORCE_PUSH,
                WorkflowAction.CREATE_TAG,
                WorkflowAction.DELETE_TAG,
                WorkflowAction.MODIFY_SETTINGS,
            },
            PermissionLevel.OWNER: set(WorkflowAction)  # All permissions
        }

    async def add_collaborator(self, repository_id: str, owner_id: str, 
                             collaborator_id: str, permission_level: PermissionLevel,
                             expires_at: Optional[float] = None,
                             restrictions: List[str] = None) -> bool:
        """Add collaborator to repository with specific permissions"""
        try:
            # Verify repository ownership
            repo_metadata = await self.repo_manager._get_metadata(repository_id)
            if not repo_metadata or repo_metadata.user_id != owner_id:
                raise ValueError("Not authorized to manage collaborators")
            
            # Check if user already has access
            existing_permission = await self._get_collaborator_permission(
                repository_id, collaborator_id
            )
            if existing_permission and existing_permission.is_active:
                raise ValueError("User is already a collaborator")
            
            # Create permission record
            permission = CollaboratorPermission(
                user_id=collaborator_id,
                repository_id=repository_id,
                permission_level=permission_level,
                granted_by=owner_id,
                granted_at=time.time(),
                expires_at=expires_at,
                is_active=True,
                restrictions=restrictions or []
            )
            
            # Store permission in database
            await self._store_collaborator_permission(permission)
            
            # Update Soft-serve permissions
            repo_path = f"{repo_metadata.user_id}/{repo_metadata.name}"
            await self.softserve.set_repository_permissions(
                repo_path, collaborator_id, permission_level.value
            )
            
            # Log collaboration event
            logfire.info("Collaborator added to repository",
                       repository_id=repository_id,
                       collaborator_id=collaborator_id,
                       permission_level=permission_level.value,
                       granted_by=owner_id)
            
            # Trigger workflow automation
            await self._trigger_workflow_event(
                repository_id, "collaborator_added", {
                    "collaborator_id": collaborator_id,
                    "permission_level": permission_level.value,
                    "granted_by": owner_id
                }
            )
            
            return True
            
        except Exception as e:
            logger.error("Failed to add collaborator",
                        repository_id=repository_id,
                        collaborator_id=collaborator_id,
                        error=str(e))
            return False

    async def create_repository_team(self, repository_id: str, owner_id: str,
                                   team_name: str, description: str,
                                   permission_level: PermissionLevel,
                                   initial_members: List[str] = None) -> str:
        """Create a team for repository collaboration"""
        try:
            # Verify repository ownership
            repo_metadata = await self.repo_manager._get_metadata(repository_id)
            if not repo_metadata or repo_metadata.user_id != owner_id:
                raise ValueError("Not authorized to create teams")
            
            # Generate team ID
            team_id = f"team_{repository_id}_{int(time.time())}"
            
            # Create team
            team = RepositoryTeam(
                id=team_id,
                repository_id=repository_id,
                name=team_name,
                description=description,
                permission_level=permission_level,
                members=initial_members or [],
                created_at=time.time(),
                updated_at=time.time(),
                created_by=owner_id
            )
            
            # Store team in database
            await self._store_repository_team(team)
            
            # Add initial members with team permissions
            for member_id in initial_members or []:
                await self.add_collaborator(
                    repository_id, owner_id, member_id, permission_level
                )
            
            logfire.info("Repository team created",
                       repository_id=repository_id,
                       team_id=team_id,
                       team_name=team_name,
                       members_count=len(initial_members or []))
            
            return team_id
            
        except Exception as e:
            logger.error("Failed to create repository team",
                        repository_id=repository_id,
                        team_name=team_name,
                        error=str(e))
            raise

    async def create_branch_protection_rule(self, repository_id: str, user_id: str,
                                          branch_pattern: str,
                                          required_reviews: int = 1,
                                          require_code_owner_review: bool = False,
                                          required_status_checks: List[str] = None,
                                          enforce_admins: bool = False) -> str:
        """Create branch protection rule"""
        try:
            # Verify permission to create protection rules
            if not await self._check_workflow_permission(
                repository_id, user_id, WorkflowAction.MODIFY_SETTINGS
            ):
                raise ValueError("Not authorized to create branch protection rules")
            
            # Generate rule ID
            rule_id = f"protection_{repository_id}_{int(time.time())}"
            
            # Create protection rule
            rule = BranchProtectionRule(
                id=rule_id,
                repository_id=repository_id,
                branch_pattern=branch_pattern,
                required_reviews=required_reviews,
                dismiss_stale_reviews=True,
                require_code_owner_review=require_code_owner_review,
                required_status_checks=required_status_checks or [],
                enforce_admins=enforce_admins,
                allow_force_pushes=False,
                allow_deletions=False,
                created_by=user_id,
                created_at=time.time()
            )
            
            # Store rule in database
            await self._store_branch_protection_rule(rule)
            
            # Apply rule to Soft-serve
            await self._apply_branch_protection_to_softserve(rule)
            
            logfire.info("Branch protection rule created",
                       repository_id=repository_id,
                       rule_id=rule_id,
                       branch_pattern=branch_pattern,
                       required_reviews=required_reviews)
            
            return rule_id
            
        except Exception as e:
            logger.error("Failed to create branch protection rule",
                        repository_id=repository_id,
                        branch_pattern=branch_pattern,
                        error=str(e))
            raise

    async def create_merge_request(self, repository_id: str, author_id: str,
                                 source_branch: str, target_branch: str,
                                 title: str, description: str,
                                 assignees: List[str] = None,
                                 reviewers: List[str] = None,
                                 labels: List[str] = None) -> str:
        """Create a merge request"""
        try:
            # Verify write permission
            if not await self._check_workflow_permission(
                repository_id, author_id, WorkflowAction.MERGE_REQUEST
            ):
                raise ValueError("Not authorized to create merge requests")
            
            # Check for branch conflicts
            conflicts = await self._check_branch_conflicts(
                repository_id, source_branch, target_branch
            )
            
            # Generate merge request ID
            mr_id = f"mr_{repository_id}_{int(time.time())}"
            
            # Create merge request
            merge_request = MergeRequest(
                id=mr_id,
                repository_id=repository_id,
                source_branch=source_branch,
                target_branch=target_branch,
                title=title,
                description=description,
                author_id=author_id,
                assignees=assignees or [],
                reviewers=reviewers or [],
                labels=labels or [],
                status="open",
                created_at=time.time(),
                updated_at=time.time(),
                merged_at=None,
                merged_by=None,
                conflicts=conflicts,
                reviews=[]
            )
            
            # Store merge request
            await self._store_merge_request(merge_request)
            
            # Auto-assign reviewers based on CODEOWNERS
            await self._auto_assign_reviewers(merge_request)
            
            # Trigger notifications
            await self._send_merge_request_notifications(merge_request)
            
            logfire.info("Merge request created",
                       repository_id=repository_id,
                       mr_id=mr_id,
                       source_branch=source_branch,
                       target_branch=target_branch,
                       author_id=author_id)
            
            return mr_id
            
        except Exception as e:
            logger.error("Failed to create merge request",
                        repository_id=repository_id,
                        source_branch=source_branch,
                        target_branch=target_branch,
                        error=str(e))
            raise

    async def review_merge_request(self, mr_id: str, reviewer_id: str,
                                 approval: bool, comments: str = "",
                                 requested_changes: List[str] = None) -> bool:
        """Review a merge request"""
        try:
            # Get merge request
            merge_request = await self._get_merge_request(mr_id)
            if not merge_request:
                raise ValueError("Merge request not found")
            
            # Verify reviewer permission
            if not await self._check_workflow_permission(
                merge_request.repository_id, reviewer_id, WorkflowAction.APPROVE_MERGE
            ):
                raise ValueError("Not authorized to review merge requests")
            
            # Create review
            review = {
                "reviewer_id": reviewer_id,
                "approval": approval,
                "comments": comments,
                "requested_changes": requested_changes or [],
                "timestamp": time.time()
            }
            
            # Add review to merge request
            merge_request.reviews.append(review)
            merge_request.updated_at = time.time()
            
            # Store updated merge request
            await self._store_merge_request(merge_request)
            
            # Check if merge request can be auto-merged
            if approval and await self._can_auto_merge(merge_request):
                await self._auto_merge_request(merge_request)
            
            logfire.info("Merge request reviewed",
                       mr_id=mr_id,
                       reviewer_id=reviewer_id,
                       approval=approval)
            
            return True
            
        except Exception as e:
            logger.error("Failed to review merge request",
                        mr_id=mr_id,
                        reviewer_id=reviewer_id,
                        error=str(e))
            return False

    async def create_workflow_automation(self, repository_id: str, creator_id: str,
                                       workflow_name: str,
                                       trigger_events: List[str],
                                       conditions: Dict[str, Any],
                                       actions: List[Dict[str, Any]]) -> str:
        """Create workflow automation rule"""
        try:
            # Verify admin permission
            if not await self._check_workflow_permission(
                repository_id, creator_id, WorkflowAction.MODIFY_SETTINGS
            ):
                raise ValueError("Not authorized to create workflows")
            
            # Generate workflow ID
            workflow_id = f"workflow_{repository_id}_{int(time.time())}"
            
            # Create workflow rule
            workflow = WorkflowRule(
                id=workflow_id,
                repository_id=repository_id,
                name=workflow_name,
                trigger_events=trigger_events,
                conditions=conditions,
                actions=actions,
                is_active=True,
                created_by=creator_id,
                created_at=time.time()
            )
            
            # Store workflow
            await self._store_workflow_rule(workflow)
            
            logfire.info("Workflow automation created",
                       repository_id=repository_id,
                       workflow_id=workflow_id,
                       workflow_name=workflow_name)
            
            return workflow_id
            
        except Exception as e:
            logger.error("Failed to create workflow automation",
                        repository_id=repository_id,
                        workflow_name=workflow_name,
                        error=str(e))
            raise

    async def get_repository_activity_feed(self, repository_id: str, user_id: str,
                                         limit: int = 50,
                                         since: Optional[float] = None) -> List[Dict[str, Any]]:
        """Get repository activity feed for user"""
        try:
            # Verify read access
            if not await self._check_workflow_permission(
                repository_id, user_id, WorkflowAction.CREATE_BRANCH
            ):
                raise ValueError("Not authorized to view repository activity")
            
            # Get activity events
            activities = await self._get_repository_activities(
                repository_id, limit, since
            )
            
            # Filter activities based on user permissions
            filtered_activities = []
            for activity in activities:
                if await self._can_user_see_activity(user_id, activity):
                    filtered_activities.append(activity)
            
            return filtered_activities[:limit]
            
        except Exception as e:
            logger.error("Failed to get repository activity feed",
                        repository_id=repository_id,
                        user_id=user_id,
                        error=str(e))
            return []

    async def _check_workflow_permission(self, repository_id: str, user_id: str,
                                       action: WorkflowAction) -> bool:
        """Check if user has permission for workflow action"""
        try:
            # Get user permission for repository
            permission = await self._get_collaborator_permission(repository_id, user_id)
            
            # Check if repository owner
            repo_metadata = await self.repo_manager._get_metadata(repository_id)
            if repo_metadata and repo_metadata.user_id == user_id:
                return True  # Owner has all permissions
            
            # Check permission level
            if not permission or not permission.is_active:
                return False
            
            # Check if permission level allows action
            allowed_actions = self.permission_matrix.get(permission.permission_level, set())
            return action in allowed_actions
            
        except Exception as e:
            logger.error("Failed to check workflow permission",
                        repository_id=repository_id,
                        user_id=user_id,
                        action=action.value,
                        error=str(e))
            return False

    async def _trigger_workflow_event(self, repository_id: str, event_type: str,
                                    event_data: Dict[str, Any]) -> None:
        """Trigger workflow automation for repository event"""
        try:
            # Get active workflows for repository
            workflows = await self._get_active_workflows(repository_id)
            
            for workflow in workflows:
                if event_type in workflow.trigger_events:
                    # Check workflow conditions
                    if await self._evaluate_workflow_conditions(workflow, event_data):
                        # Execute workflow actions
                        await self._execute_workflow_actions(workflow, event_data)
            
        except Exception as e:
            logger.error("Failed to trigger workflow event",
                        repository_id=repository_id,
                        event_type=event_type,
                        error=str(e))

    async def _auto_assign_reviewers(self, merge_request: MergeRequest) -> None:
        """Auto-assign reviewers based on CODEOWNERS file"""
        try:
            # Get CODEOWNERS rules for affected files
            codeowners = await self._get_codeowners_rules(merge_request.repository_id)
            if not codeowners:
                return
            
            # Get list of changed files in merge request
            changed_files = await self._get_changed_files(merge_request)
            
            # Find applicable reviewers
            required_reviewers = set()
            for file_path in changed_files:
                reviewers = await self._find_reviewers_for_file(codeowners, file_path)
                required_reviewers.update(reviewers)
            
            # Add reviewers to merge request
            if required_reviewers:
                merge_request.reviewers.extend(list(required_reviewers))
                merge_request.updated_at = time.time()
                await self._store_merge_request(merge_request)
            
        except Exception as e:
            logger.error("Failed to auto-assign reviewers",
                        mr_id=merge_request.id,
                        error=str(e))

    # Database operations (implement based on your database choice)
    async def _store_collaborator_permission(self, permission: CollaboratorPermission) -> None:
        """Store collaborator permission in database"""
        # Implementation depends on database backend
        pass
    
    async def _get_collaborator_permission(self, repository_id: str, user_id: str) -> Optional[CollaboratorPermission]:
        """Get collaborator permission from database"""
        # Implementation depends on database backend
        pass
    
    async def _store_repository_team(self, team: RepositoryTeam) -> None:
        """Store repository team in database"""
        # Implementation depends on database backend
        pass
    
    async def _store_branch_protection_rule(self, rule: BranchProtectionRule) -> None:
        """Store branch protection rule in database"""
        # Implementation depends on database backend
        pass
    
    async def _store_merge_request(self, merge_request: MergeRequest) -> None:
        """Store merge request in database"""
        # Implementation depends on database backend
        pass
    
    async def _get_merge_request(self, mr_id: str) -> Optional[MergeRequest]:
        """Get merge request from database"""
        # Implementation depends on database backend
        pass
    
    async def _store_workflow_rule(self, workflow: WorkflowRule) -> None:
        """Store workflow rule in database"""
        # Implementation depends on database backend
        pass
```

### Collaboration API Endpoints
**Location**: `api/endpoints/collaboration.py`

```python
# api/endpoints/collaboration.py
from fastapi import APIRouter, HTTPException, Depends, status
from typing import List, Optional
from pydantic import BaseModel
import structlog
import logfire

from ..auth import get_current_user
from ..dependencies import get_collaboration_manager
from git_manager.collaboration.collaboration_manager import PermissionLevel

logger = structlog.get_logger()
router = APIRouter(prefix="/api/v1/collaboration", tags=["collaboration"])

class AddCollaboratorRequest(BaseModel):
    repository_id: str
    collaborator_id: str
    permission_level: str
    expires_at: Optional[float] = None
    restrictions: Optional[List[str]] = None

class CreateTeamRequest(BaseModel):
    repository_id: str
    team_name: str
    description: str
    permission_level: str
    initial_members: Optional[List[str]] = None

class CreateMergeRequestRequest(BaseModel):
    repository_id: str
    source_branch: str
    target_branch: str
    title: str
    description: str
    assignees: Optional[List[str]] = None
    reviewers: Optional[List[str]] = None
    labels: Optional[List[str]] = None

class ReviewMergeRequestRequest(BaseModel):
    approval: bool
    comments: str = ""
    requested_changes: Optional[List[str]] = None

@router.post("/collaborators", status_code=status.HTTP_201_CREATED)
async def add_collaborator(
    request: AddCollaboratorRequest,
    current_user = Depends(get_current_user),
    collaboration_manager = Depends(get_collaboration_manager)
):
    """Add collaborator to repository"""
    try:
        # Validate permission level
        try:
            permission_level = PermissionLevel(request.permission_level)
        except ValueError:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Invalid permission level"
            )
        
        success = await collaboration_manager.add_collaborator(
            repository_id=request.repository_id,
            owner_id=current_user.id,
            collaborator_id=request.collaborator_id,
            permission_level=permission_level,
            expires_at=request.expires_at,
            restrictions=request.restrictions
        )
        
        if not success:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Failed to add collaborator"
            )
        
        logfire.info("Collaborator added via API",
                   repository_id=request.repository_id,
                   collaborator_id=request.collaborator_id,
                   owner_id=current_user.id)
        
        return {"message": "Collaborator added successfully"}
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Failed to add collaborator via API", error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Internal server error"
        )

@router.post("/teams", status_code=status.HTTP_201_CREATED)
async def create_team(
    request: CreateTeamRequest,
    current_user = Depends(get_current_user),
    collaboration_manager = Depends(get_collaboration_manager)
):
    """Create repository team"""
    try:
        # Validate permission level
        try:
            permission_level = PermissionLevel(request.permission_level)
        except ValueError:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Invalid permission level"
            )
        
        team_id = await collaboration_manager.create_repository_team(
            repository_id=request.repository_id,
            owner_id=current_user.id,
            team_name=request.team_name,
            description=request.description,
            permission_level=permission_level,
            initial_members=request.initial_members
        )
        
        logfire.info("Team created via API",
                   repository_id=request.repository_id,
                   team_id=team_id,
                   owner_id=current_user.id)
        
        return {"team_id": team_id, "message": "Team created successfully"}
        
    except Exception as e:
        logger.error("Failed to create team via API", error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Internal server error"
        )

@router.post("/merge-requests", status_code=status.HTTP_201_CREATED)
async def create_merge_request(
    request: CreateMergeRequestRequest,
    current_user = Depends(get_current_user),
    collaboration_manager = Depends(get_collaboration_manager)
):
    """Create merge request"""
    try:
        mr_id = await collaboration_manager.create_merge_request(
            repository_id=request.repository_id,
            author_id=current_user.id,
            source_branch=request.source_branch,
            target_branch=request.target_branch,
            title=request.title,
            description=request.description,
            assignees=request.assignees,
            reviewers=request.reviewers,
            labels=request.labels
        )
        
        logfire.info("Merge request created via API",
                   repository_id=request.repository_id,
                   mr_id=mr_id,
                   author_id=current_user.id)
        
        return {"merge_request_id": mr_id, "message": "Merge request created successfully"}
        
    except Exception as e:
        logger.error("Failed to create merge request via API", error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Internal server error"
        )

@router.post("/merge-requests/{mr_id}/reviews", status_code=status.HTTP_201_CREATED)
async def review_merge_request(
    mr_id: str,
    request: ReviewMergeRequestRequest,
    current_user = Depends(get_current_user),
    collaboration_manager = Depends(get_collaboration_manager)
):
    """Review merge request"""
    try:
        success = await collaboration_manager.review_merge_request(
            mr_id=mr_id,
            reviewer_id=current_user.id,
            approval=request.approval,
            comments=request.comments,
            requested_changes=request.requested_changes
        )
        
        if not success:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Failed to review merge request"
            )
        
        logfire.info("Merge request reviewed via API",
                   mr_id=mr_id,
                   reviewer_id=current_user.id,
                   approval=request.approval)
        
        return {"message": "Review submitted successfully"}
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Failed to review merge request via API", error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Internal server error"
        )

@router.get("/repositories/{repository_id}/activity")
async def get_repository_activity(
    repository_id: str,
    limit: int = 50,
    since: Optional[float] = None,
    current_user = Depends(get_current_user),
    collaboration_manager = Depends(get_collaboration_manager)
):
    """Get repository activity feed"""
    try:
        activities = await collaboration_manager.get_repository_activity_feed(
            repository_id=repository_id,
            user_id=current_user.id,
            limit=limit,
            since=since
        )
        
        return {"activities": activities}
        
    except Exception as e:
        logger.error("Failed to get repository activity via API", error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Internal server error"
        )
```

## TDD Implementation Cycle

### Red Phase: Collaboration Test Creation
```python
# git-manager/tests/test_collaboration.py
import pytest
from git_manager.collaboration.collaboration_manager import GitCollaborationManager, PermissionLevel

@pytest.mark.asyncio
async def test_add_collaborator_unauthorized():
    """Test that unauthorized users cannot add collaborators"""
    # This test should initially fail (Red phase)
    assert False, "Collaboration authorization not implemented yet"

@pytest.mark.asyncio
async def test_merge_request_creation():
    """Test merge request creation workflow"""
    # This test should initially fail (Red phase)
    assert False, "Merge request workflow not implemented yet"

@pytest.mark.asyncio
async def test_branch_protection_enforcement():
    """Test that branch protection rules are enforced"""
    # This test should initially fail (Red phase)
    assert False, "Branch protection not implemented yet"
```

### Green Phase: Collaboration Implementation
```python
# Implement collaboration features to make tests pass
# This involves adding permission checks, workflow automation, and team management
```

### Refactor Phase: Collaboration Optimization
```python
# Optimize collaboration implementations for performance and usability
# Add comprehensive activity logging and notifications
# Enhance workflow automation and rule engine
```

## Security Checklist ✅

### Collaboration Access Control
- [ ] Repository collaboration permission validation for all operations
- [ ] Team membership verification and authorization
- [ ] Cross-repository collaboration prevention
- [ ] Collaborator enumeration protection
- [ ] Permission escalation prevention through team management
- [ ] Repository owner privilege protection
- [ ] Guest user access restriction enforcement
- [ ] Collaboration audit logging and monitoring
- [ ] Permission expiration enforcement
- [ ] Unauthorized team creation prevention

### Merge Request Security
- [ ] Merge request author verification
- [ ] Branch protection rule enforcement
- [ ] Required review validation
- [ ] Code owner approval verification
- [ ] Malicious merge prevention
- [ ] Force push protection on protected branches
- [ ] Auto-merge security validation
- [ ] Review bypass prevention
- [ ] Merge conflict security validation
- [ ] Branch deletion protection

### Workflow Automation Security
- [ ] Workflow creation authorization validation
- [ ] Automated action security validation
- [ ] Workflow trigger event verification
- [ ] Malicious workflow prevention
- [ ] Workflow execution isolation
- [ ] Automated notification security
- [ ] External integration security
- [ ] Workflow audit logging
- [ ] Resource limit enforcement for workflows
- [ ] Privilege escalation prevention through automation

### Team Management Security
- [ ] Team creation authorization validation
- [ ] Team membership management security
- [ ] Team permission inheritance validation
- [ ] Cross-team access prevention
- [ ] Team enumeration protection
- [ ] Team hierarchy security validation
- [ ] Team deletion authorization
- [ ] Team activity audit logging
- [ ] Bulk permission change validation
- [ ] Team-based notification security

### API Security for Collaboration
- [ ] Collaboration API authentication validation
- [ ] API endpoint authorization checking
- [ ] Input validation for all collaboration requests
- [ ] Rate limiting for collaboration operations
- [ ] API audit logging for collaboration events
- [ ] Bulk operation security validation
- [ ] API response data filtering
- [ ] Cross-origin request security
- [ ] API versioning security
- [ ] Error message security for collaboration APIs

## Performance Requirements

### Collaboration Operations
- Collaborator addition/removal < 2 seconds
- Team creation/management < 3 seconds
- Permission validation < 100ms
- Activity feed generation < 1 second
- Merge request creation < 2 seconds
- Review submission < 1 second

### Workflow Performance
- Workflow trigger evaluation < 500ms
- Automated action execution < 5 seconds
- Branch protection validation < 200ms
- Code owner resolution < 300ms
- Notification delivery < 2 seconds
- Activity event processing < 100ms

### Scalability Requirements
- Support 1000+ collaborators per repository
- Handle 100+ concurrent merge requests
- Process 500+ workflow triggers per minute
- Support 50+ teams per repository
- Manage 10000+ activity events per day
- Scale to 100+ repositories per user

## Commit Instructions

After implementing each collaboration feature:

```bash
git add git-manager/collaboration/
git add api/endpoints/collaboration.py
git commit -m "Add Git collaboration workflows with comprehensive team management

- Implement GitCollaborationManager with advanced permission system
- Add repository team management with role-based access control
- Implement merge request workflow with automated review assignment
- Add branch protection rules with customizable enforcement
- Implement workflow automation with event-driven triggers
- Add collaboration API endpoints with RESTful interface
- Include activity feed with permission-filtered events
- Add TDD cycle with Red-Green-Refactor for collaboration features
- Ensure >90% collaboration test coverage with integration tests

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete collaboration test suite:

```bash
# Run all collaboration tests
pytest git-manager/tests/collaboration/ -v --timeout=300

# Run specific collaboration test categories
pytest git-manager/tests/collaboration/ -k "team_management" -v
pytest git-manager/tests/collaboration/ -k "merge_request" -v
pytest git-manager/tests/collaboration/ -k "workflow_automation" -v

# Run collaboration API tests
pytest api/tests/test_collaboration_endpoints.py -v

# Run collaboration integration tests
pytest git-manager/tests/integration/test_collaboration_integration.py -v
```

Validate collaboration test coverage:
```bash
pytest git-manager/tests/collaboration/ --cov=git_manager.collaboration --cov-report=html --cov-fail-under=90
```

## Integration Testing

Test collaboration integration with previous sessions:
```bash
# Test integration with Session 1 (Logfire monitoring)
pytest git-manager/tests/integration/test_collaboration_logfire_integration.py -v

# Test integration with Session 2 (Authentication)
pytest git-manager/tests/integration/test_collaboration_auth_integration.py -v

# Test collaboration with Session 9.1-9.3 (Git components)
pytest git-manager/tests/integration/test_collaboration_git_integration.py -v
```