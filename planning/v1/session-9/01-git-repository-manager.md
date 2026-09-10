# Session 9.1: Git Repository Manager & Core Operations

## Objective
Implement comprehensive Git repository management system with Soft-serve integration, providing users with secure, isolated Git repositories with full lifecycle management, access controls, and quota enforcement.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for Git operation monitoring and repository analytics
- **Session 2**: Integrates with authentication system for repository ownership and access control
- **Session 5**: Uses database models for repository metadata and user quota tracking
- **Session 8**: Enables Git operations directly from terminal sessions

## Core Implementation

### Git Repository Manager
**Location**: `git-manager/core/repository_manager.py`

```python
# git-manager/core/repository_manager.py
import asyncio
import os
import time
import subprocess
import tempfile
import hashlib
from typing import Dict, Any, Optional, List, Tuple
from dataclasses import dataclass, asdict
from enum import Enum
from pathlib import Path
import aiofiles
import structlog
import logfire

logger = structlog.get_logger()

class RepositoryState(Enum):
    CREATING = "creating"
    ACTIVE = "active"
    ARCHIVED = "archived"
    DELETING = "deleting"
    ERROR = "error"
    MAINTENANCE = "maintenance"

class RepositoryVisibility(Enum):
    PRIVATE = "private"
    PUBLIC = "public"
    INTERNAL = "internal"

class RepositoryType(Enum):
    REGULAR = "regular"
    TEMPLATE = "template"
    FORK = "fork"
    MIRROR = "mirror"

@dataclass
class RepositoryMetadata:
    id: str
    name: str
    user_id: str
    description: str
    visibility: RepositoryVisibility
    repo_type: RepositoryType
    state: RepositoryState
    size_bytes: int
    clone_url_ssh: str
    clone_url_https: str
    default_branch: str
    created_at: float
    updated_at: float
    last_push_at: Optional[float]
    last_activity_at: Optional[float]
    push_count: int
    clone_count: int
    fork_count: int
    collaborators: List[str]
    protected_branches: List[str]
    tags: List[str]
    topics: List[str]
    is_template: bool
    is_archived: bool
    template_source_id: Optional[str] = None
    fork_source_id: Optional[str] = None
    mirror_url: Optional[str] = None

@dataclass
class RepositoryStats:
    total_commits: int
    total_branches: int
    total_tags: int
    total_contributors: int
    languages: Dict[str, int]
    activity_score: float
    health_score: float

class GitRepositoryManager:
    def __init__(self, softserve_client, database, ssh_key_manager, storage_backend):
        self.softserve = softserve_client
        self.db = database
        self.ssh_keys = ssh_key_manager
        self.storage = storage_backend
        
        # Configuration
        self.max_repos_per_user = 50
        self.max_repo_size = 10 * 1024 * 1024 * 1024  # 10GB
        self.max_total_size_per_user = 50 * 1024 * 1024 * 1024  # 50GB
        self.max_collaborators_per_repo = 20
        self.max_file_size = 100 * 1024 * 1024  # 100MB
        
        # Cache for repository metadata
        self.repo_cache = {}
        self.cache_ttl = 300  # 5 minutes
        
        # Active operations tracking
        self.active_operations = {}
        
    async def create_repository(
        self, 
        user_id: str, 
        name: str, 
        description: str = "",
        visibility: RepositoryVisibility = RepositoryVisibility.PRIVATE,
        repo_type: RepositoryType = RepositoryType.REGULAR,
        is_template: bool = False,
        template_source_id: Optional[str] = None,
        topics: List[str] = None,
        auto_init: bool = True,
        gitignore_template: Optional[str] = None,
        license_template: Optional[str] = None
    ) -> str:
        """Create a new Git repository with comprehensive configuration"""
        try:
            logfire.info("Repository creation initiated", 
                        user_id=user_id, name=name, visibility=visibility.value)
            
            # Input validation
            await self._validate_repository_creation(user_id, name, template_source_id)
            
            # Generate repository ID
            repo_id = await self._generate_repo_id(user_id, name)
            
            # Create repository metadata
            metadata = RepositoryMetadata(
                id=repo_id,
                name=name,
                user_id=user_id,
                description=description,
                visibility=visibility,
                repo_type=repo_type,
                state=RepositoryState.CREATING,
                size_bytes=0,
                clone_url_ssh="",
                clone_url_https="",
                default_branch="main",
                created_at=time.time(),
                updated_at=time.time(),
                last_push_at=None,
                last_activity_at=None,
                push_count=0,
                clone_count=0,
                fork_count=0,
                collaborators=[],
                protected_branches=["main"] if auto_init else [],
                tags=[],
                topics=topics or [],
                is_template=is_template,
                is_archived=False,
                template_source_id=template_source_id
            )
            
            # Store initial metadata
            await self._store_metadata(metadata)
            
            # Track operation
            operation_id = f"create_{repo_id}"
            self.active_operations[operation_id] = asyncio.create_task(
                self._create_repository_task(metadata, auto_init, gitignore_template, license_template)
            )
            
            logger.info("Repository creation task started", 
                       repo_id=repo_id, operation_id=operation_id)
            
            return repo_id
            
        except Exception as e:
            logger.error("Failed to initiate repository creation", 
                        name=name, user_id=user_id, error=str(e))
            logfire.error("Repository creation failed", 
                         user_id=user_id, name=name, error=str(e))
            raise

    async def _create_repository_task(
        self, 
        metadata: RepositoryMetadata,
        auto_init: bool,
        gitignore_template: Optional[str],
        license_template: Optional[str]
    ):
        """Background task to create repository with full initialization"""
        repo_id = metadata.id
        
        try:
            # Create repository in Soft-serve
            repo_path = f"{metadata.user_id}/{metadata.name}"
            
            await self.softserve.create_repository(
                path=repo_path,
                description=metadata.description,
                visibility=metadata.visibility.value,
                default_branch=metadata.default_branch
            )
            
            # Set up repository access controls
            await self.softserve.set_repository_permissions(
                repo_path, metadata.user_id, "admin"
            )
            
            # Generate clone URLs
            ssh_url = await self.softserve.get_ssh_clone_url(repo_path)
            https_url = await self.softserve.get_https_clone_url(repo_path)
            
            # Initialize repository content
            if auto_init:
                await self._initialize_repository_content(
                    metadata, repo_path, gitignore_template, license_template
                )
            elif metadata.template_source_id:
                await self._initialize_from_template(metadata, metadata.template_source_id)
            
            # Update metadata with successful creation
            metadata.state = RepositoryState.ACTIVE
            metadata.clone_url_ssh = ssh_url
            metadata.clone_url_https = https_url
            metadata.updated_at = time.time()
            metadata.last_activity_at = time.time()
            
            # Calculate initial repository size
            metadata.size_bytes = await self._calculate_repository_size(repo_path)
            
            await self._store_metadata(metadata)
            await self._invalidate_cache(repo_id)
            
            # Setup repository hooks
            await self._setup_repository_hooks(repo_path)
            
            # Create initial repository statistics
            await self._initialize_repository_stats(repo_id)
            
            logger.info("Repository creation completed successfully", 
                       repo_id=repo_id, size_bytes=metadata.size_bytes)
            
            logfire.info("Repository created", 
                        repo_id=repo_id,
                        user_id=metadata.user_id,
                        name=metadata.name,
                        size_bytes=metadata.size_bytes,
                        auto_init=auto_init)
            
        except Exception as e:
            logger.error("Repository creation failed", repo_id=repo_id, error=str(e))
            
            # Update metadata to error state
            metadata.state = RepositoryState.ERROR
            metadata.updated_at = time.time()
            await self._store_metadata(metadata)
            
            logfire.error("Repository creation task failed", 
                         repo_id=repo_id, error=str(e))
        
        finally:
            # Cleanup operation tracking
            operation_id = f"create_{repo_id}"
            self.active_operations.pop(operation_id, None)

    async def _initialize_repository_content(
        self,
        metadata: RepositoryMetadata,
        repo_path: str,
        gitignore_template: Optional[str],
        license_template: Optional[str]
    ):
        """Initialize repository with default content"""
        try:
            # Create temporary directory for initialization
            with tempfile.TemporaryDirectory() as temp_dir:
                local_repo_path = Path(temp_dir) / "repo"
                
                # Clone the empty repository
                await self._clone_repository_for_init(metadata.clone_url_ssh, local_repo_path)
                
                # Create README.md
                readme_content = self._generate_readme_content(metadata)
                readme_path = local_repo_path / "README.md"
                async with aiofiles.open(readme_path, 'w') as f:
                    await f.write(readme_content)
                
                # Add .gitignore if template specified
                if gitignore_template:
                    gitignore_content = await self._get_gitignore_template(gitignore_template)
                    if gitignore_content:
                        gitignore_path = local_repo_path / ".gitignore"
                        async with aiofiles.open(gitignore_path, 'w') as f:
                            await f.write(gitignore_content)
                
                # Add LICENSE if template specified
                if license_template:
                    license_content = await self._get_license_template(license_template)
                    if license_content:
                        license_path = local_repo_path / "LICENSE"
                        async with aiofiles.open(license_path, 'w') as f:
                            await f.write(license_content)
                
                # Stage and commit initial files
                await self._commit_initial_files(local_repo_path, metadata)
                
                # Push to remote
                await self._push_initial_commit(local_repo_path, metadata)
                
                logger.info("Repository initialized with content", 
                           repo_id=metadata.id)
                
        except Exception as e:
            logger.error("Failed to initialize repository content", 
                        repo_id=metadata.id, error=str(e))
            raise

    async def clone_repository(
        self, 
        repo_id: str, 
        user_id: str, 
        destination_path: str,
        branch: Optional[str] = None,
        depth: Optional[int] = None,
        recursive: bool = False
    ) -> bool:
        """Clone repository to VM file system with advanced options"""
        try:
            logfire.info("Repository clone initiated", 
                        repo_id=repo_id, user_id=user_id, destination=destination_path)
            
            # Get repository metadata
            metadata = await self._get_metadata(repo_id)
            if not metadata:
                raise ValueError("Repository not found")
            
            # Check access permissions
            if not await self._check_access_permission(repo_id, user_id, "read"):
                raise ValueError("Access denied")
            
            # Validate destination path
            if not await self._validate_clone_destination(destination_path, user_id):
                raise ValueError("Invalid destination path")
            
            # Get user's SSH key for cloning
            ssh_key_path = await self.ssh_keys.get_user_private_key_path(user_id)
            if not ssh_key_path:
                raise ValueError("No SSH key configured for user")
            
            # Perform clone operation
            clone_success = await self._perform_clone_operation(
                metadata.clone_url_ssh, 
                destination_path, 
                ssh_key_path,
                branch=branch,
                depth=depth,
                recursive=recursive
            )
            
            if clone_success:
                # Update clone count
                metadata.clone_count += 1
                metadata.last_activity_at = time.time()
                metadata.updated_at = time.time()
                await self._store_metadata(metadata)
                await self._invalidate_cache(repo_id)
                
                # Log successful clone
                logger.info("Repository cloned successfully", 
                           repo_id=repo_id, user_id=user_id, 
                           destination=destination_path)
                
                logfire.info("Repository cloned", 
                            repo_id=repo_id,
                            user_id=user_id,
                            destination=destination_path,
                            branch=branch,
                            depth=depth)
            
            return clone_success
            
        except Exception as e:
            logger.error("Repository clone failed", 
                        repo_id=repo_id, user_id=user_id, error=str(e))
            
            logfire.error("Repository clone failed", 
                         repo_id=repo_id, user_id=user_id, error=str(e))
            return False

    async def _perform_clone_operation(
        self,
        clone_url: str,
        destination: str,
        ssh_key_path: str,
        branch: Optional[str] = None,
        depth: Optional[int] = None,
        recursive: bool = False
    ) -> bool:
        """Perform the actual git clone operation with advanced options"""
        try:
            # Set up SSH configuration for git
            ssh_config = f"""
Host git-server
    HostName {self.softserve.hostname}
    Port {self.softserve.ssh_port}
    User git
    IdentityFile {ssh_key_path}
    StrictHostKeyChecking no
    UserKnownHostsFile /dev/null
    ServerAliveInterval 60
    ServerAliveCountMax 3
"""
            
            with tempfile.NamedTemporaryFile(mode='w', suffix='.ssh_config', delete=False) as f:
                f.write(ssh_config)
                ssh_config_path = f.name
            
            try:
                # Build git clone command
                git_cmd = ['git', 'clone']
                
                # Add clone options
                if branch:
                    git_cmd.extend(['--branch', branch])
                
                if depth:
                    git_cmd.extend(['--depth', str(depth)])
                
                if recursive:
                    git_cmd.append('--recursive')
                
                # Add progress reporting
                git_cmd.append('--progress')
                
                # Add URL and destination
                git_cmd.extend([clone_url, destination])
                
                # Set up environment
                env = {
                    **os.environ,
                    'GIT_SSH_COMMAND': f'ssh -F {ssh_config_path}',
                    'GIT_TERMINAL_PROMPT': '0'  # Disable interactive prompts
                }
                
                # Execute clone with timeout
                process = await asyncio.create_subprocess_exec(
                    *git_cmd,
                    env=env,
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE
                )
                
                try:
                    stdout, stderr = await asyncio.wait_for(
                        process.communicate(), timeout=300  # 5 minute timeout
                    )
                except asyncio.TimeoutError:
                    process.kill()
                    logger.error("Git clone timed out")
                    return False
                
                if process.returncode == 0:
                    logger.info("Git clone completed successfully", 
                               destination=destination)
                    return True
                else:
                    logger.error("Git clone failed", 
                               returncode=process.returncode,
                               stdout=stdout.decode(),
                               stderr=stderr.decode())
                    return False
                    
            finally:
                # Cleanup SSH config file
                Path(ssh_config_path).unlink(missing_ok=True)
                
        except Exception as e:
            logger.error("Clone operation failed", error=str(e))
            return False

    async def fork_repository(
        self,
        source_repo_id: str,
        user_id: str,
        fork_name: Optional[str] = None,
        fork_description: Optional[str] = None
    ) -> str:
        """Create a fork of an existing repository"""
        try:
            # Get source repository metadata
            source_metadata = await self._get_metadata(source_repo_id)
            if not source_metadata:
                raise ValueError("Source repository not found")
            
            # Check if source repository allows forking
            if not await self._check_access_permission(source_repo_id, user_id, "read"):
                raise ValueError("Cannot fork private repository without access")
            
            # Generate fork name and description
            if not fork_name:
                fork_name = source_metadata.name
            
            if not fork_description:
                fork_description = f"Fork of {source_metadata.user_id}/{source_metadata.name}"
            
            # Create fork repository
            fork_repo_id = await self.create_repository(
                user_id=user_id,
                name=fork_name,
                description=fork_description,
                visibility=RepositoryVisibility.PRIVATE,  # Forks start as private
                repo_type=RepositoryType.FORK,
                auto_init=False
            )
            
            # Get fork metadata and update with fork information
            fork_metadata = await self._get_metadata(fork_repo_id)
            fork_metadata.fork_source_id = source_repo_id
            await self._store_metadata(fork_metadata)
            
            # Clone source repository content to fork
            await self._clone_repository_content(source_metadata, fork_metadata)
            
            # Update source repository fork count
            source_metadata.fork_count += 1
            source_metadata.updated_at = time.time()
            await self._store_metadata(source_metadata)
            
            logger.info("Repository forked successfully", 
                       source_repo_id=source_repo_id,
                       fork_repo_id=fork_repo_id,
                       user_id=user_id)
            
            logfire.info("Repository forked", 
                        source_repo_id=source_repo_id,
                        fork_repo_id=fork_repo_id,
                        user_id=user_id,
                        fork_name=fork_name)
            
            return fork_repo_id
            
        except Exception as e:
            logger.error("Repository fork failed", 
                        source_repo_id=source_repo_id,
                        user_id=user_id,
                        error=str(e))
            raise

    async def delete_repository(self, repo_id: str, user_id: str) -> bool:
        """Delete a repository with comprehensive cleanup"""
        try:
            # Get repository metadata
            metadata = await self._get_metadata(repo_id)
            if not metadata or metadata.user_id != user_id:
                return False
            
            # Update state to deleting
            metadata.state = RepositoryState.DELETING
            metadata.updated_at = time.time()
            await self._store_metadata(metadata)
            
            # Create deletion task
            operation_id = f"delete_{repo_id}"
            self.active_operations[operation_id] = asyncio.create_task(
                self._delete_repository_task(metadata)
            )
            
            logger.info("Repository deletion initiated", 
                       repo_id=repo_id, user_id=user_id)
            
            logfire.info("Repository deletion started", 
                        repo_id=repo_id, user_id=user_id)
            
            return True
            
        except Exception as e:
            logger.error("Failed to initiate repository deletion", 
                        repo_id=repo_id, error=str(e))
            return False

    async def _delete_repository_task(self, metadata: RepositoryMetadata):
        """Background task to delete repository with cleanup"""
        repo_id = metadata.id
        
        try:
            repo_path = f"{metadata.user_id}/{metadata.name}"
            
            # Remove from Soft-serve
            await self.softserve.delete_repository(repo_path)
            
            # Clean up any associated data
            await self._cleanup_repository_data(repo_id)
            
            # Remove repository statistics
            await self._delete_repository_stats(repo_id)
            
            # Remove metadata
            await self._delete_metadata(repo_id)
            await self._invalidate_cache(repo_id)
            
            logger.info("Repository deletion completed", repo_id=repo_id)
            
            logfire.info("Repository deleted", 
                        repo_id=repo_id,
                        user_id=metadata.user_id,
                        name=metadata.name)
            
        except Exception as e:
            logger.error("Repository deletion failed", repo_id=repo_id, error=str(e))
            
            # Restore repository state on failure
            metadata.state = RepositoryState.ERROR
            metadata.updated_at = time.time()
            await self._store_metadata(metadata)
            
            logfire.error("Repository deletion failed", 
                         repo_id=repo_id, error=str(e))
        
        finally:
            # Cleanup operation tracking
            operation_id = f"delete_{repo_id}"
            self.active_operations.pop(operation_id, None)

    async def get_repository_stats(self, repo_id: str) -> Optional[RepositoryStats]:
        """Get comprehensive repository statistics"""
        try:
            metadata = await self._get_metadata(repo_id)
            if not metadata:
                return None
            
            # Get cached stats or calculate new ones
            stats = await self._get_cached_stats(repo_id)
            if not stats:
                stats = await self._calculate_repository_stats(metadata)
                await self._cache_stats(repo_id, stats)
            
            return stats
            
        except Exception as e:
            logger.error("Failed to get repository stats", repo_id=repo_id, error=str(e))
            return None

    # Helper methods for validation, caching, and operations
    async def _validate_repository_creation(self, user_id: str, name: str, template_source_id: Optional[str]):
        """Validate repository creation parameters"""
        # Check user quotas
        if not await self._check_user_quotas(user_id):
            raise ValueError("User repository quota exceeded")
        
        # Validate repository name
        if not self._validate_repo_name(name):
            raise ValueError("Invalid repository name")
        
        # Check for name conflicts
        if await self._repo_name_exists(user_id, name):
            raise ValueError("Repository name already exists")
        
        # Validate template if specified
        if template_source_id:
            template_metadata = await self._get_metadata(template_source_id)
            if not template_metadata or not template_metadata.is_template:
                raise ValueError("Invalid template repository")

    def _validate_repo_name(self, name: str) -> bool:
        """Validate repository name format"""
        import re
        # Allow alphanumeric, hyphens, underscores, periods
        # Must start and end with alphanumeric
        pattern = r'^[a-zA-Z0-9][a-zA-Z0-9._-]{0,98}[a-zA-Z0-9]$'
        return bool(re.match(pattern, name)) and len(name) >= 2

    async def _check_user_quotas(self, user_id: str) -> bool:
        """Check if user is within repository quotas"""
        try:
            user_repos = await self.list_user_repositories(user_id)
            
            # Check repository count
            if len(user_repos) >= self.max_repos_per_user:
                return False
            
            # Check total storage usage
            total_size = sum(repo.size_bytes for repo in user_repos)
            if total_size >= self.max_total_size_per_user:
                return False
            
            return True
            
        except Exception as e:
            logger.error("Failed to check user quotas", user_id=user_id, error=str(e))
            return False

    async def _generate_repo_id(self, user_id: str, name: str) -> str:
        """Generate unique repository ID"""
        import uuid
        base_string = f"{user_id}:{name}:{time.time()}"
        hash_digest = hashlib.sha256(base_string.encode()).hexdigest()[:12]
        return f"repo_{hash_digest}"

    # Cache management methods
    async def _get_cached_metadata(self, repo_id: str) -> Optional[RepositoryMetadata]:
        """Get repository metadata from cache"""
        cache_entry = self.repo_cache.get(repo_id)
        if cache_entry and time.time() - cache_entry['timestamp'] < self.cache_ttl:
            return cache_entry['metadata']
        return None

    async def _cache_metadata(self, metadata: RepositoryMetadata):
        """Cache repository metadata"""
        self.repo_cache[metadata.id] = {
            'metadata': metadata,
            'timestamp': time.time()
        }

    async def _invalidate_cache(self, repo_id: str):
        """Invalidate cache for repository"""
        self.repo_cache.pop(repo_id, None)

    # Database operations (to be implemented with actual database)
    async def _store_metadata(self, metadata: RepositoryMetadata):
        """Store repository metadata in database"""
        # Implementation depends on database choice
        pass

    async def _get_metadata(self, repo_id: str) -> Optional[RepositoryMetadata]:
        """Get repository metadata from database"""
        # Check cache first
        cached = await self._get_cached_metadata(repo_id)
        if cached:
            return cached
        
        # Fetch from database and cache
        # Implementation depends on database choice
        pass

    async def _delete_metadata(self, repo_id: str):
        """Delete repository metadata from database"""
        # Implementation depends on database choice
        pass

    async def list_user_repositories(self, user_id: str) -> List[RepositoryMetadata]:
        """List all repositories for a user"""
        # Implementation depends on database choice
        pass
```

## TDD Implementation Cycle

### Test-Driven Development Process

1. **Red Phase**: Write failing repository tests
   ```bash
   # Create repository test file
   touch git-manager/tests/test_repository_manager.py
   
   # Run failing test
   pytest git-manager/tests/test_repository_manager.py::test_create_repository -v
   ```

2. **Green Phase**: Implement basic repository management
   ```bash
   # Implement repository creation and basic operations
   pytest git-manager/tests/test_repository_manager.py::test_create_repository -v
   ```

3. **Refactor Phase**: Add advanced features and optimizations
   ```bash
   # Add comprehensive repository management features
   pytest git-manager/tests/ -v
   ```

4. **Commit**: Commit repository management functionality
   ```bash
   git add git-manager/core/ git-manager/tests/
   git commit -m "feat: implement comprehensive Git repository manager

   - Add GitRepositoryManager with full lifecycle management
   - Implement repository creation with auto-initialization and templates
   - Add repository cloning with advanced options (branch, depth, recursive)
   - Include repository forking with content synchronization
   - Add repository deletion with comprehensive cleanup
   - Integrate with Logfire for operation monitoring and analytics
   
   Tests: Added comprehensive test suite for all repository operations
   Security: Access control validation and quota enforcement
   Performance: Async operations and metadata caching for efficiency"
   ```

### Repository Management Test Cases

```python
# git-manager/tests/test_repository_manager.py
import pytest
import asyncio
import tempfile
from unittest.mock import AsyncMock, Mock, patch
from git_manager.core.repository_manager import (
    GitRepositoryManager, RepositoryMetadata, RepositoryState, 
    RepositoryVisibility, RepositoryType
)

@pytest.fixture
async def repository_manager():
    """Create repository manager with mocked dependencies"""
    softserve_mock = AsyncMock()
    database_mock = AsyncMock()
    ssh_key_mock = AsyncMock()
    storage_mock = AsyncMock()
    
    manager = GitRepositoryManager(
        softserve_client=softserve_mock,
        database=database_mock,
        ssh_key_manager=ssh_key_mock,
        storage_backend=storage_mock
    )
    
    return manager, softserve_mock, database_mock, ssh_key_mock

class TestRepositoryCreation:
    async def test_create_repository_success(self, repository_manager):
        """Test successful repository creation"""
        manager, softserve_mock, db_mock, ssh_mock = repository_manager
        
        # Mock quota check
        manager._check_user_quotas = AsyncMock(return_value=True)
        manager._repo_name_exists = AsyncMock(return_value=False)
        manager._store_metadata = AsyncMock()
        
        # Test repository creation
        repo_id = await manager.create_repository(
            user_id="user123",
            name="test-repo",
            description="Test repository",
            visibility=RepositoryVisibility.PRIVATE
        )
        
        assert repo_id.startswith("repo_")
        manager._check_user_quotas.assert_called_once_with("user123")
        manager._store_metadata.assert_called_once()

    async def test_create_repository_quota_exceeded(self, repository_manager):
        """Test repository creation with quota exceeded"""
        manager, _, _, _ = repository_manager
        
        # Mock quota check to fail
        manager._check_user_quotas = AsyncMock(return_value=False)
        
        with pytest.raises(ValueError, match="quota exceeded"):
            await manager.create_repository(
                user_id="user123",
                name="test-repo"
            )

    async def test_create_repository_invalid_name(self, repository_manager):
        """Test repository creation with invalid name"""
        manager, _, _, _ = repository_manager
        
        manager._check_user_quotas = AsyncMock(return_value=True)
        
        with pytest.raises(ValueError, match="Invalid repository name"):
            await manager.create_repository(
                user_id="user123",
                name="@invalid-name!"
            )

    async def test_create_repository_name_conflict(self, repository_manager):
        """Test repository creation with name conflict"""
        manager, _, _, _ = repository_manager
        
        manager._check_user_quotas = AsyncMock(return_value=True)
        manager._repo_name_exists = AsyncMock(return_value=True)
        
        with pytest.raises(ValueError, match="already exists"):
            await manager.create_repository(
                user_id="user123",
                name="existing-repo"
            )

class TestRepositoryCloning:
    async def test_clone_repository_success(self, repository_manager):
        """Test successful repository cloning"""
        manager, softserve_mock, db_mock, ssh_mock = repository_manager
        
        # Mock repository metadata
        metadata = RepositoryMetadata(
            id="repo123",
            name="test-repo",
            user_id="user123",
            description="Test repo",
            visibility=RepositoryVisibility.PRIVATE,
            repo_type=RepositoryType.REGULAR,
            state=RepositoryState.ACTIVE,
            size_bytes=1024,
            clone_url_ssh="git@example.com:user123/test-repo.git",
            clone_url_https="https://example.com/user123/test-repo.git",
            default_branch="main",
            created_at=1234567890,
            updated_at=1234567890,
            last_push_at=None,
            last_activity_at=None,
            push_count=0,
            clone_count=0,
            fork_count=0,
            collaborators=[],
            protected_branches=[],
            tags=[],
            topics=[],
            is_template=False,
            is_archived=False
        )
        
        manager._get_metadata = AsyncMock(return_value=metadata)
        manager._check_access_permission = AsyncMock(return_value=True)
        manager._validate_clone_destination = AsyncMock(return_value=True)
        ssh_mock.get_user_private_key_path.return_value = "/path/to/key"
        manager._perform_clone_operation = AsyncMock(return_value=True)
        manager._store_metadata = AsyncMock()
        manager._invalidate_cache = AsyncMock()
        
        result = await manager.clone_repository(
            repo_id="repo123",
            user_id="user123",
            destination_path="/tmp/test-repo"
        )
        
        assert result is True
        manager._perform_clone_operation.assert_called_once()
        assert metadata.clone_count == 1

    async def test_clone_repository_access_denied(self, repository_manager):
        """Test repository cloning with access denied"""
        manager, _, _, _ = repository_manager
        
        metadata = Mock()
        manager._get_metadata = AsyncMock(return_value=metadata)
        manager._check_access_permission = AsyncMock(return_value=False)
        
        result = await manager.clone_repository(
            repo_id="repo123",
            user_id="user456",
            destination_path="/tmp/test-repo"
        )
        
        assert result is False

    async def test_clone_repository_no_ssh_key(self, repository_manager):
        """Test repository cloning without SSH key"""
        manager, _, _, ssh_mock = repository_manager
        
        metadata = Mock()
        manager._get_metadata = AsyncMock(return_value=metadata)
        manager._check_access_permission = AsyncMock(return_value=True)
        manager._validate_clone_destination = AsyncMock(return_value=True)
        ssh_mock.get_user_private_key_path.return_value = None
        
        result = await manager.clone_repository(
            repo_id="repo123",
            user_id="user123",
            destination_path="/tmp/test-repo"
        )
        
        assert result is False

class TestRepositoryForking:
    async def test_fork_repository_success(self, repository_manager):
        """Test successful repository forking"""
        manager, _, _, _ = repository_manager
        
        # Mock source repository
        source_metadata = RepositoryMetadata(
            id="source123",
            name="original-repo",
            user_id="owner123",
            description="Original repo",
            visibility=RepositoryVisibility.PUBLIC,
            repo_type=RepositoryType.REGULAR,
            state=RepositoryState.ACTIVE,
            size_bytes=2048,
            clone_url_ssh="git@example.com:owner123/original-repo.git",
            clone_url_https="https://example.com/owner123/original-repo.git",
            default_branch="main",
            created_at=1234567890,
            updated_at=1234567890,
            last_push_at=None,
            last_activity_at=None,
            push_count=5,
            clone_count=10,
            fork_count=0,
            collaborators=[],
            protected_branches=["main"],
            tags=[],
            topics=["python", "web"],
            is_template=False,
            is_archived=False
        )
        
        manager._get_metadata = AsyncMock(return_value=source_metadata)
        manager._check_access_permission = AsyncMock(return_value=True)
        manager.create_repository = AsyncMock(return_value="fork123")
        manager._clone_repository_content = AsyncMock()
        manager._store_metadata = AsyncMock()
        
        fork_repo_id = await manager.fork_repository(
            source_repo_id="source123",
            user_id="user123",
            fork_name="my-fork"
        )
        
        assert fork_repo_id == "fork123"
        manager.create_repository.assert_called_once()
        manager._clone_repository_content.assert_called_once()
        assert source_metadata.fork_count == 1

class TestRepositoryDeletion:
    async def test_delete_repository_success(self, repository_manager):
        """Test successful repository deletion"""
        manager, _, _, _ = repository_manager
        
        metadata = RepositoryMetadata(
            id="repo123",
            name="test-repo",
            user_id="user123",
            description="Test repo",
            visibility=RepositoryVisibility.PRIVATE,
            repo_type=RepositoryType.REGULAR,
            state=RepositoryState.ACTIVE,
            size_bytes=1024,
            clone_url_ssh="git@example.com:user123/test-repo.git",
            clone_url_https="https://example.com/user123/test-repo.git",
            default_branch="main",
            created_at=1234567890,
            updated_at=1234567890,
            last_push_at=None,
            last_activity_at=None,
            push_count=0,
            clone_count=0,
            fork_count=0,
            collaborators=[],
            protected_branches=[],
            tags=[],
            topics=[],
            is_template=False,
            is_archived=False
        )
        
        manager._get_metadata = AsyncMock(return_value=metadata)
        manager._store_metadata = AsyncMock()
        
        result = await manager.delete_repository(
            repo_id="repo123",
            user_id="user123"
        )
        
        assert result is True
        assert metadata.state == RepositoryState.DELETING

    async def test_delete_repository_not_owner(self, repository_manager):
        """Test repository deletion by non-owner"""
        manager, _, _, _ = repository_manager
        
        metadata = Mock()
        metadata.user_id = "owner123"
        manager._get_metadata = AsyncMock(return_value=metadata)
        
        result = await manager.delete_repository(
            repo_id="repo123",
            user_id="user456"
        )
        
        assert result is False

class TestRepositoryValidation:
    def test_validate_repo_name_valid(self, repository_manager):
        """Test repository name validation with valid names"""
        manager, _, _, _ = repository_manager
        
        valid_names = [
            "my-repo",
            "project_name",
            "web-app.v2",
            "123project",
            "project123"
        ]
        
        for name in valid_names:
            assert manager._validate_repo_name(name) is True

    def test_validate_repo_name_invalid(self, repository_manager):
        """Test repository name validation with invalid names"""
        manager, _, _, _ = repository_manager
        
        invalid_names = [
            "",
            "a",
            "-invalid",
            "invalid-",
            ".invalid",
            "invalid.",
            "inv@lid",
            "inv lid",
            "a" * 101  # Too long
        ]
        
        for name in invalid_names:
            assert manager._validate_repo_name(name) is False

class TestRepositoryQuotas:
    async def test_check_user_quotas_within_limits(self, repository_manager):
        """Test quota check when user is within limits"""
        manager, _, _, _ = repository_manager
        
        # Mock user repositories
        repos = [Mock(size_bytes=1000) for _ in range(10)]
        manager.list_user_repositories = AsyncMock(return_value=repos)
        
        result = await manager._check_user_quotas("user123")
        assert result is True

    async def test_check_user_quotas_repo_count_exceeded(self, repository_manager):
        """Test quota check when repository count is exceeded"""
        manager, _, _, _ = repository_manager
        
        # Mock too many repositories
        repos = [Mock(size_bytes=1000) for _ in range(manager.max_repos_per_user + 1)]
        manager.list_user_repositories = AsyncMock(return_value=repos)
        
        result = await manager._check_user_quotas("user123")
        assert result is False

    async def test_check_user_quotas_storage_exceeded(self, repository_manager):
        """Test quota check when storage limit is exceeded"""
        manager, _, _, _ = repository_manager
        
        # Mock repositories that exceed storage limit
        large_size = manager.max_total_size_per_user + 1
        repos = [Mock(size_bytes=large_size)]
        manager.list_user_repositories = AsyncMock(return_value=repos)
        
        result = await manager._check_user_quotas("user123")
        assert result is False
```

## Security Checklist for Repository Management

### Repository Access Control
- [ ] Repository ownership validation on all operations with strict user ID matching
- [ ] Cross-user repository access prevention with authorization checks
- [ ] Repository enumeration protection with user-scoped queries
- [ ] Collaborator permission enforcement with role-based access
- [ ] Repository visibility controls preventing unauthorized discovery
- [ ] Branch protection rules with push restrictions and required reviews
- [ ] Force push restrictions for protected branches
- [ ] Access audit logging for all repository operations
- [ ] Rate limiting on repository creation and cloning (5 ops/minute)
- [ ] Secure repository deletion with complete data wiping

### Repository Content Security
- [ ] File size limits to prevent large file abuse (100MB per file)
- [ ] Repository size monitoring and limits (10GB per repository)
- [ ] Binary file scanning for malware and suspicious content
- [ ] Git hook validation and sandboxing for security
- [ ] Commit content validation to prevent sensitive data exposure
- [ ] Large file detection and prevention in pushes
- [ ] Repository backup verification and integrity checks
- [ ] Protection against Git bomb attacks (malicious repositories)
- [ ] Filename validation to prevent path traversal attacks
- [ ] Content type validation for uploaded files

### Clone and Fork Security
- [ ] Clone operation authentication with SSH key validation
- [ ] Clone rate limiting to prevent DoS attacks (10 clones/hour per user)
- [ ] Fork permission validation for private repositories
- [ ] Source repository integrity verification before forking
- [ ] Clone destination path validation to prevent directory traversal
- [ ] SSH key access monitoring and suspicious activity detection
- [ ] Fork relationship tracking for security incident response
- [ ] Clone audit logging with IP address and timestamp tracking
- [ ] Bandwidth limiting for large repository transfers
- [ ] Protection against clone bombing attacks

## Performance Requirements

### Repository Operations
- Repository creation < 5 seconds (including initialization)
- Repository deletion < 3 seconds (metadata removal)
- Repository listing < 500ms (with pagination)
- Repository metadata retrieval < 100ms (with caching)
- Repository stats calculation < 2 seconds
- Cache hit ratio > 80% for metadata operations

### Git Operations
- Small repository clone (< 10MB) < 30 seconds
- Medium repository clone (< 100MB) < 2 minutes
- Large repository clone (< 1GB) < 10 minutes
- Repository fork operation < 1 minute
- Repository size calculation < 5 seconds
- Branch/tag operations < 2 seconds

### Quota and Validation
- Quota checking < 100ms (with caching)
- Repository name validation < 10ms
- Access permission checking < 50ms
- SSH key validation < 100ms
- File size validation < 20ms per file
- Content scanning < 1 second per MB

## Integration Testing

### Repository Lifecycle Testing
```python
async def test_repository_complete_lifecycle():
    """Test complete repository lifecycle"""
    manager = GitRepositoryManager(...)
    
    # Create repository
    repo_id = await manager.create_repository(
        user_id="user123",
        name="lifecycle-test",
        auto_init=True
    )
    
    # Verify creation
    metadata = await manager._get_metadata(repo_id)
    assert metadata.state == RepositoryState.ACTIVE
    
    # Clone repository
    success = await manager.clone_repository(
        repo_id=repo_id,
        user_id="user123",
        destination_path="/tmp/lifecycle-test"
    )
    assert success is True
    
    # Fork repository
    fork_id = await manager.fork_repository(
        source_repo_id=repo_id,
        user_id="user456"
    )
    assert fork_id is not None
    
    # Delete repositories
    fork_deleted = await manager.delete_repository(fork_id, "user456")
    repo_deleted = await manager.delete_repository(repo_id, "user123")
    
    assert fork_deleted is True
    assert repo_deleted is True
```

### Soft-serve Integration Testing
```python
async def test_softserve_integration():
    """Test integration with Soft-serve Git server"""
    manager = GitRepositoryManager(...)
    
    # Test repository creation in Soft-serve
    repo_id = await manager.create_repository(
        user_id="user123",
        name="integration-test"
    )
    
    # Verify repository exists in Soft-serve
    repo_exists = await manager.softserve.repository_exists("user123/integration-test")
    assert repo_exists is True
    
    # Test clone URL generation
    metadata = await manager._get_metadata(repo_id)
    assert metadata.clone_url_ssh.endswith("/integration-test.git")
    assert metadata.clone_url_https.endswith("/integration-test.git")
```

## Next Implementation Steps

1. **Complete repository manager implementation** with all lifecycle operations
2. **Add repository template system** with predefined project structures
3. **Implement repository statistics** with language detection and activity metrics
4. **Add repository hooks system** for custom validation and automation
5. **Create repository backup system** with automated backup and recovery
6. **Add repository mirroring** for external repository synchronization
7. **Implement advanced search** and filtering for repository discovery

## Commit Guidelines

Repository management commits should include:
- **Comprehensive operation support** for create, read, update, delete operations
- **Security validation** with access controls and input sanitization
- **Performance optimization** with caching and async operations
- **Test coverage** for all repository operations and edge cases (>80%)
- **Integration verification** with Soft-serve and authentication systems
- **Documentation updates** with API examples and usage guidelines