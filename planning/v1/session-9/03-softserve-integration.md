# Session 9.3: Soft-serve Integration & Git Server Management

## Objective
Implement comprehensive integration with Charm's Soft-serve Git server, providing seamless Git hosting functionality, repository management, and secure access control through API integration.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for Git server operation monitoring and API analytics
- **Session 2**: Integrates with authentication system for user verification
- **Session 9.1**: Provides Git server backend for repository operations
- **Session 9.2**: Manages SSH keys for Git authentication

## Core Implementation

### Soft-serve Client Integration
**Location**: `git-manager/integrations/softserve_client.py`

```python
# git-manager/integrations/softserve_client.py
import aiohttp
import asyncio
import json
import time
import ssl
from typing import Dict, Any, Optional, List, Union
from dataclasses import dataclass
from enum import Enum
import structlog
import logfire
from urllib.parse import quote, urljoin

logger = structlog.get_logger()

class RepositoryVisibility(Enum):
    PRIVATE = "private"
    PUBLIC = "public"
    INTERNAL = "internal"

class PermissionLevel(Enum):
    READ = "read"
    WRITE = "write"
    ADMIN = "admin"

@dataclass
class SoftServeRepository:
    name: str
    full_name: str
    description: str
    visibility: str
    default_branch: str
    clone_url_ssh: str
    clone_url_https: str
    created_at: str
    updated_at: str
    size: int
    is_empty: bool
    is_archived: bool

@dataclass
class SoftServeUser:
    username: str
    display_name: str
    email: str
    is_admin: bool
    created_at: str
    last_active: str

@dataclass
class SoftServeSSHKey:
    id: int
    title: str
    key: str
    fingerprint: str
    created_at: str
    last_used: Optional[str]

class SoftServeClient:
    def __init__(
        self, 
        base_url: str, 
        admin_token: str, 
        ssh_hostname: str, 
        ssh_port: int = 22,
        https_port: int = 443,
        timeout: int = 30,
        max_retries: int = 3,
        verify_ssl: bool = True
    ):
        self.base_url = base_url.rstrip('/')
        self.admin_token = admin_token
        self.ssh_hostname = ssh_hostname
        self.ssh_port = ssh_port
        self.https_port = https_port
        self.timeout = timeout
        self.max_retries = max_retries
        self.verify_ssl = verify_ssl
        
        # Session management
        self.session: Optional[aiohttp.ClientSession] = None
        self.connector: Optional[aiohttp.TCPConnector] = None
        
        # Rate limiting
        self.rate_limit_delay = 0.1  # 100ms between requests
        self.last_request_time = 0
        
        # Health monitoring
        self.health_status = True
        self.last_health_check = 0
        self.health_check_interval = 300  # 5 minutes

    async def __aenter__(self):
        await self.initialize_session()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        await self.close_session()

    async def initialize_session(self):
        """Initialize HTTP session with proper configuration"""
        try:
            # Create SSL context if needed
            ssl_context = None
            if self.verify_ssl:
                ssl_context = ssl.create_default_context()
            elif not self.verify_ssl:
                ssl_context = False
            
            # Create connector with connection pooling
            self.connector = aiohttp.TCPConnector(
                limit=100,
                limit_per_host=10,
                ttl_dns_cache=300,
                use_dns_cache=True,
                ssl=ssl_context
            )
            
            # Create session with timeout and headers
            timeout = aiohttp.ClientTimeout(total=self.timeout)
            headers = {
                'Authorization': f'Bearer {self.admin_token}',
                'Content-Type': 'application/json',
                'Accept': 'application/json',
                'User-Agent': 'BuildPlatform-GitManager/1.0'
            }
            
            self.session = aiohttp.ClientSession(
                connector=self.connector,
                timeout=timeout,
                headers=headers
            )
            
            # Perform initial health check
            await self.health_check()
            
            logger.info("Soft-serve client session initialized", 
                       base_url=self.base_url)
            
        except Exception as e:
            logger.error("Failed to initialize Soft-serve session", error=str(e))
            raise

    async def close_session(self):
        """Close HTTP session and cleanup resources"""
        if self.session:
            await self.session.close()
            self.session = None
        
        if self.connector:
            await self.connector.close()
            self.connector = None

    async def health_check(self) -> bool:
        """Check Soft-serve server health"""
        try:
            current_time = time.time()
            
            # Skip if recently checked
            if current_time - self.last_health_check < self.health_check_interval:
                return self.health_status
            
            async with self.session.get(f"{self.base_url}/health") as response:
                self.health_status = response.status == 200
                self.last_health_check = current_time
                
                if self.health_status:
                    logger.debug("Soft-serve health check passed")
                else:
                    logger.warning("Soft-serve health check failed", 
                                 status=response.status)
                
                logfire.info("Soft-serve health check", 
                            healthy=self.health_status,
                            response_time_ms=(time.time() - current_time) * 1000)
                
                return self.health_status
                
        except Exception as e:
            self.health_status = False
            logger.error("Soft-serve health check error", error=str(e))
            logfire.error("Soft-serve health check failed", error=str(e))
            return False

    async def _make_request(
        self, 
        method: str, 
        endpoint: str, 
        data: Optional[Dict] = None,
        params: Optional[Dict] = None,
        retry_count: int = 0
    ) -> aiohttp.ClientResponse:
        """Make HTTP request with retry logic and rate limiting"""
        try:
            # Rate limiting
            current_time = time.time()
            time_since_last = current_time - self.last_request_time
            if time_since_last < self.rate_limit_delay:
                await asyncio.sleep(self.rate_limit_delay - time_since_last)
            
            self.last_request_time = time.time()
            
            # Ensure session is available
            if not self.session:
                await self.initialize_session()
            
            # Build URL
            url = urljoin(self.base_url, endpoint)
            
            # Prepare request kwargs
            kwargs = {}
            if data:
                kwargs['json'] = data
            if params:
                kwargs['params'] = params
            
            # Make request
            start_time = time.time()
            
            async with self.session.request(method, url, **kwargs) as response:
                response_time = (time.time() - start_time) * 1000
                
                # Log request
                logfire.info("Soft-serve API request", 
                            method=method,
                            endpoint=endpoint,
                            status_code=response.status,
                            response_time_ms=response_time)
                
                # Handle rate limiting
                if response.status == 429:
                    retry_after = int(response.headers.get('Retry-After', 60))
                    logger.warning("Rate limited by Soft-serve", 
                                 retry_after=retry_after)
                    
                    if retry_count < self.max_retries:
                        await asyncio.sleep(retry_after)
                        return await self._make_request(
                            method, endpoint, data, params, retry_count + 1
                        )
                
                # Handle server errors with retry
                if response.status >= 500 and retry_count < self.max_retries:
                    delay = 2 ** retry_count  # Exponential backoff
                    logger.warning("Server error, retrying", 
                                 status=response.status, delay=delay)
                    await asyncio.sleep(delay)
                    return await self._make_request(
                        method, endpoint, data, params, retry_count + 1
                    )
                
                return response
                
        except Exception as e:
            logger.error("Request to Soft-serve failed", 
                        method=method, endpoint=endpoint, error=str(e))
            
            if retry_count < self.max_retries:
                delay = 2 ** retry_count
                await asyncio.sleep(delay)
                return await self._make_request(
                    method, endpoint, data, params, retry_count + 1
                )
            
            raise

    async def create_repository(
        self, 
        path: str, 
        description: str = "",
        visibility: str = "private",
        default_branch: str = "main",
        auto_init: bool = True,
        gitignore_template: Optional[str] = None,
        license_template: Optional[str] = None
    ) -> bool:
        """Create a new repository in Soft-serve"""
        try:
            data = {
                "name": path,
                "description": description,
                "visibility": visibility,
                "default_branch": default_branch,
                "auto_init": auto_init
            }
            
            if gitignore_template:
                data["gitignore_template"] = gitignore_template
            
            if license_template:
                data["license_template"] = license_template
            
            response = await self._make_request("POST", "/api/v1/repos", data=data)
            
            if response.status == 201:
                repo_data = await response.json()
                logger.info("Repository created in Soft-serve", 
                           path=path, repo_id=repo_data.get('id'))
                
                logfire.info("Soft-serve repository created", 
                            path=path,
                            visibility=visibility,
                            auto_init=auto_init)
                return True
            else:
                error_text = await response.text()
                logger.error("Failed to create repository", 
                           path=path, status=response.status, error=error_text)
                
                logfire.error("Soft-serve repository creation failed", 
                             path=path, status=response.status, error=error_text)
                return False
                
        except Exception as e:
            logger.error("Soft-serve API error during repository creation", 
                        path=path, error=str(e))
            logfire.error("Soft-serve repository creation error", 
                         path=path, error=str(e))
            return False

    async def get_repository(self, path: str) -> Optional[SoftServeRepository]:
        """Get repository information from Soft-serve"""
        try:
            encoded_path = quote(path, safe='')
            response = await self._make_request("GET", f"/api/v1/repos/{encoded_path}")
            
            if response.status == 200:
                repo_data = await response.json()
                return SoftServeRepository(
                    name=repo_data['name'],
                    full_name=repo_data['full_name'],
                    description=repo_data.get('description', ''),
                    visibility=repo_data['visibility'],
                    default_branch=repo_data['default_branch'],
                    clone_url_ssh=repo_data['clone_url_ssh'],
                    clone_url_https=repo_data['clone_url_https'],
                    created_at=repo_data['created_at'],
                    updated_at=repo_data['updated_at'],
                    size=repo_data.get('size', 0),
                    is_empty=repo_data.get('is_empty', True),
                    is_archived=repo_data.get('is_archived', False)
                )
            elif response.status == 404:
                return None
            else:
                error_text = await response.text()
                logger.error("Failed to get repository", 
                           path=path, status=response.status, error=error_text)
                return None
                
        except Exception as e:
            logger.error("Error getting repository from Soft-serve", 
                        path=path, error=str(e))
            return None

    async def delete_repository(self, path: str) -> bool:
        """Delete a repository from Soft-serve"""
        try:
            encoded_path = quote(path, safe='')
            response = await self._make_request("DELETE", f"/api/v1/repos/{encoded_path}")
            
            if response.status == 204:
                logger.info("Repository deleted from Soft-serve", path=path)
                logfire.info("Soft-serve repository deleted", path=path)
                return True
            else:
                error_text = await response.text()
                logger.error("Failed to delete repository", 
                           path=path, status=response.status, error=error_text)
                logfire.error("Soft-serve repository deletion failed", 
                             path=path, status=response.status, error=error_text)
                return False
                
        except Exception as e:
            logger.error("Soft-serve API error during repository deletion", 
                        path=path, error=str(e))
            logfire.error("Soft-serve repository deletion error", 
                         path=path, error=str(e))
            return False

    async def set_repository_permissions(
        self, 
        repo_path: str, 
        username: str, 
        permission: str
    ) -> bool:
        """Set user permissions for a repository"""
        try:
            # Validate permission level
            if permission not in ["read", "write", "admin"]:
                raise ValueError(f"Invalid permission level: {permission}")
            
            data = {
                "username": username,
                "permission": permission
            }
            
            encoded_path = quote(repo_path, safe='')
            endpoint = f"/api/v1/repos/{encoded_path}/collaborators"
            
            response = await self._make_request("POST", endpoint, data=data)
            
            if response.status in [200, 201]:
                logger.info("Repository permissions set", 
                           repo_path=repo_path, username=username, permission=permission)
                
                logfire.info("Soft-serve repository permissions set", 
                            repo_path=repo_path,
                            username=username,
                            permission=permission)
                return True
            else:
                error_text = await response.text()
                logger.error("Failed to set repository permissions", 
                           repo_path=repo_path, username=username,
                           status=response.status, error=error_text)
                
                logfire.error("Soft-serve repository permissions failed", 
                             repo_path=repo_path, username=username,
                             status=response.status, error=error_text)
                return False
                
        except Exception as e:
            logger.error("Soft-serve API error setting permissions", 
                        repo_path=repo_path, username=username, error=str(e))
            logfire.error("Soft-serve permissions error", 
                         repo_path=repo_path, username=username, error=str(e))
            return False

    async def remove_repository_permissions(
        self, 
        repo_path: str, 
        username: str
    ) -> bool:
        """Remove user permissions from a repository"""
        try:
            encoded_path = quote(repo_path, safe='')
            encoded_username = quote(username, safe='')
            endpoint = f"/api/v1/repos/{encoded_path}/collaborators/{encoded_username}"
            
            response = await self._make_request("DELETE", endpoint)
            
            if response.status == 204:
                logger.info("Repository permissions removed", 
                           repo_path=repo_path, username=username)
                
                logfire.info("Soft-serve repository permissions removed", 
                            repo_path=repo_path, username=username)
                return True
            else:
                error_text = await response.text()
                logger.error("Failed to remove repository permissions", 
                           repo_path=repo_path, username=username,
                           status=response.status, error=error_text)
                return False
                
        except Exception as e:
            logger.error("Error removing repository permissions", 
                        repo_path=repo_path, username=username, error=str(e))
            return False

    async def add_user_ssh_key(
        self, 
        username: str, 
        public_key: str, 
        title: str
    ) -> bool:
        """Add SSH key for a user"""
        try:
            data = {
                "key": public_key.strip(),
                "title": title
            }
            
            encoded_username = quote(username, safe='')
            endpoint = f"/api/v1/users/{encoded_username}/keys"
            
            response = await self._make_request("POST", endpoint, data=data)
            
            if response.status == 201:
                key_data = await response.json()
                logger.info("SSH key added for user", 
                           username=username, title=title, key_id=key_data.get('id'))
                
                logfire.info("Soft-serve SSH key added", 
                            username=username,
                            title=title,
                            fingerprint=key_data.get('fingerprint'))
                return True
            else:
                error_text = await response.text()
                logger.error("Failed to add SSH key", 
                           username=username, title=title,
                           status=response.status, error=error_text)
                
                logfire.error("Soft-serve SSH key addition failed", 
                             username=username, title=title,
                             status=response.status, error=error_text)
                return False
                
        except Exception as e:
            logger.error("Soft-serve API error adding SSH key", 
                        username=username, title=title, error=str(e))
            logfire.error("Soft-serve SSH key error", 
                         username=username, title=title, error=str(e))
            return False

    async def remove_user_ssh_key(
        self, 
        username: str, 
        key_fingerprint: str
    ) -> bool:
        """Remove SSH key for a user by fingerprint"""
        try:
            # First, get the key ID by fingerprint
            key_id = await self._get_ssh_key_id_by_fingerprint(username, key_fingerprint)
            if not key_id:
                logger.warning("SSH key not found for removal", 
                             username=username, fingerprint=key_fingerprint)
                return False
            
            encoded_username = quote(username, safe='')
            endpoint = f"/api/v1/users/{encoded_username}/keys/{key_id}"
            
            response = await self._make_request("DELETE", endpoint)
            
            if response.status == 204:
                logger.info("SSH key removed for user", 
                           username=username, key_id=key_id, fingerprint=key_fingerprint)
                
                logfire.info("Soft-serve SSH key removed", 
                            username=username,
                            fingerprint=key_fingerprint)
                return True
            else:
                error_text = await response.text()
                logger.error("Failed to remove SSH key", 
                           username=username, key_id=key_id,
                           status=response.status, error=error_text)
                return False
                
        except Exception as e:
            logger.error("Error removing SSH key", 
                        username=username, fingerprint=key_fingerprint, error=str(e))
            return False

    async def _get_ssh_key_id_by_fingerprint(
        self, 
        username: str, 
        fingerprint: str
    ) -> Optional[int]:
        """Get SSH key ID by fingerprint"""
        try:
            keys = await self.list_user_ssh_keys(username)
            for key in keys:
                if key.fingerprint == fingerprint:
                    return key.id
            return None
            
        except Exception as e:
            logger.error("Error finding SSH key by fingerprint", 
                        username=username, fingerprint=fingerprint, error=str(e))
            return None

    async def list_user_ssh_keys(self, username: str) -> List[SoftServeSSHKey]:
        """List all SSH keys for a user"""
        try:
            encoded_username = quote(username, safe='')
            endpoint = f"/api/v1/users/{encoded_username}/keys"
            
            response = await self._make_request("GET", endpoint)
            
            if response.status == 200:
                keys_data = await response.json()
                return [
                    SoftServeSSHKey(
                        id=key['id'],
                        title=key['title'],
                        key=key['key'],
                        fingerprint=key['fingerprint'],
                        created_at=key['created_at'],
                        last_used=key.get('last_used')
                    )
                    for key in keys_data
                ]
            else:
                logger.error("Failed to list SSH keys", 
                           username=username, status=response.status)
                return []
                
        except Exception as e:
            logger.error("Error listing SSH keys", 
                        username=username, error=str(e))
            return []

    async def get_ssh_clone_url(self, repo_path: str) -> str:
        """Generate SSH clone URL for repository"""
        if self.ssh_port == 22:
            return f"git@{self.ssh_hostname}:{repo_path}.git"
        else:
            return f"ssh://git@{self.ssh_hostname}:{self.ssh_port}/{repo_path}.git"

    async def get_https_clone_url(self, repo_path: str) -> str:
        """Generate HTTPS clone URL for repository"""
        if self.https_port == 443:
            return f"https://{self.ssh_hostname}/{repo_path}.git"
        else:
            return f"https://{self.ssh_hostname}:{self.https_port}/{repo_path}.git"

    async def repository_exists(self, repo_path: str) -> bool:
        """Check if repository exists"""
        repo = await self.get_repository(repo_path)
        return repo is not None

    async def create_user(
        self, 
        username: str, 
        email: str, 
        display_name: str = "",
        is_admin: bool = False
    ) -> bool:
        """Create a new user in Soft-serve"""
        try:
            data = {
                "username": username,
                "email": email,
                "display_name": display_name or username,
                "is_admin": is_admin
            }
            
            response = await self._make_request("POST", "/api/v1/users", data=data)
            
            if response.status == 201:
                user_data = await response.json()
                logger.info("User created in Soft-serve", 
                           username=username, user_id=user_data.get('id'))
                
                logfire.info("Soft-serve user created", 
                            username=username,
                            email=email,
                            is_admin=is_admin)
                return True
            else:
                error_text = await response.text()
                logger.error("Failed to create user", 
                           username=username, status=response.status, error=error_text)
                
                logfire.error("Soft-serve user creation failed", 
                             username=username, status=response.status, error=error_text)
                return False
                
        except Exception as e:
            logger.error("Error creating user in Soft-serve", 
                        username=username, error=str(e))
            logfire.error("Soft-serve user creation error", 
                         username=username, error=str(e))
            return False

    async def get_user(self, username: str) -> Optional[SoftServeUser]:
        """Get user information from Soft-serve"""
        try:
            encoded_username = quote(username, safe='')
            response = await self._make_request("GET", f"/api/v1/users/{encoded_username}")
            
            if response.status == 200:
                user_data = await response.json()
                return SoftServeUser(
                    username=user_data['username'],
                    display_name=user_data.get('display_name', ''),
                    email=user_data.get('email', ''),
                    is_admin=user_data.get('is_admin', False),
                    created_at=user_data['created_at'],
                    last_active=user_data.get('last_active', '')
                )
            elif response.status == 404:
                return None
            else:
                logger.error("Failed to get user", 
                           username=username, status=response.status)
                return None
                
        except Exception as e:
            logger.error("Error getting user from Soft-serve", 
                        username=username, error=str(e))
            return None

    async def update_repository_visibility(
        self, 
        repo_path: str, 
        visibility: str
    ) -> bool:
        """Update repository visibility"""
        try:
            if visibility not in ["private", "public", "internal"]:
                raise ValueError(f"Invalid visibility: {visibility}")
            
            data = {"visibility": visibility}
            encoded_path = quote(repo_path, safe='')
            endpoint = f"/api/v1/repos/{encoded_path}"
            
            response = await self._make_request("PATCH", endpoint, data=data)
            
            if response.status == 200:
                logger.info("Repository visibility updated", 
                           repo_path=repo_path, visibility=visibility)
                
                logfire.info("Soft-serve repository visibility updated", 
                            repo_path=repo_path, visibility=visibility)
                return True
            else:
                error_text = await response.text()
                logger.error("Failed to update repository visibility", 
                           repo_path=repo_path, visibility=visibility,
                           status=response.status, error=error_text)
                return False
                
        except Exception as e:
            logger.error("Error updating repository visibility", 
                        repo_path=repo_path, visibility=visibility, error=str(e))
            return False

    async def get_repository_statistics(self, repo_path: str) -> Optional[Dict[str, Any]]:
        """Get repository statistics from Soft-serve"""
        try:
            encoded_path = quote(repo_path, safe='')
            endpoint = f"/api/v1/repos/{encoded_path}/stats"
            
            response = await self._make_request("GET", endpoint)
            
            if response.status == 200:
                return await response.json()
            else:
                logger.error("Failed to get repository statistics", 
                           repo_path=repo_path, status=response.status)
                return None
                
        except Exception as e:
            logger.error("Error getting repository statistics", 
                        repo_path=repo_path, error=str(e))
            return None

    async def archive_repository(self, repo_path: str, archive: bool = True) -> bool:
        """Archive or unarchive a repository"""
        try:
            data = {"archived": archive}
            encoded_path = quote(repo_path, safe='')
            endpoint = f"/api/v1/repos/{encoded_path}"
            
            response = await self._make_request("PATCH", endpoint, data=data)
            
            if response.status == 200:
                action = "archived" if archive else "unarchived"
                logger.info(f"Repository {action}", repo_path=repo_path)
                
                logfire.info(f"Soft-serve repository {action}", 
                            repo_path=repo_path, archived=archive)
                return True
            else:
                error_text = await response.text()
                logger.error(f"Failed to {'archive' if archive else 'unarchive'} repository", 
                           repo_path=repo_path, status=response.status, error=error_text)
                return False
                
        except Exception as e:
            logger.error(f"Error {'archiving' if archive else 'unarchiving'} repository", 
                        repo_path=repo_path, error=str(e))
            return False

    async def get_server_info(self) -> Optional[Dict[str, Any]]:
        """Get Soft-serve server information"""
        try:
            response = await self._make_request("GET", "/api/v1/info")
            
            if response.status == 200:
                return await response.json()
            else:
                logger.error("Failed to get server info", status=response.status)
                return None
                
        except Exception as e:
            logger.error("Error getting server info", error=str(e))
            return None
```

## TDD Implementation Cycle

### Test-Driven Development Process

1. **Red Phase**: Write failing Soft-serve integration tests
   ```bash
   # Create Soft-serve integration test file
   touch git-manager/tests/test_softserve_client.py
   
   # Run failing test
   pytest git-manager/tests/test_softserve_client.py::test_create_repository -v
   ```

2. **Green Phase**: Implement basic Soft-serve client functionality
   ```bash
   # Implement basic API client operations
   pytest git-manager/tests/test_softserve_client.py::test_create_repository -v
   ```

3. **Refactor Phase**: Add advanced features and error handling
   ```bash
   # Add comprehensive Soft-serve integration features
   pytest git-manager/tests/ -v
   ```

4. **Commit**: Commit Soft-serve integration functionality
   ```bash
   git add git-manager/integrations/ git-manager/tests/
   git commit -m "feat: implement comprehensive Soft-serve Git server integration

   - Add SoftServeClient with full API integration
   - Implement repository management with create, delete, and permissions
   - Add SSH key management with user authentication
   - Include comprehensive error handling and retry logic
   - Add health monitoring and rate limiting for API requests
   - Integrate with Logfire for API operation monitoring and analytics
   
   Tests: Added comprehensive test suite for Soft-serve API operations
   Security: API authentication, input validation, and secure communication
   Performance: Connection pooling, retry logic, and rate limiting"
   ```

### Soft-serve Integration Test Cases

```python
# git-manager/tests/test_softserve_client.py
import pytest
import asyncio
import aioresponses
from unittest.mock import AsyncMock, Mock, patch
from git_manager.integrations.softserve_client import (
    SoftServeClient, SoftServeRepository, RepositoryVisibility
)

@pytest.fixture
async def softserve_client():
    """Create Soft-serve client for testing"""
    client = SoftServeClient(
        base_url="https://git.example.com",
        admin_token="test_token",
        ssh_hostname="git.example.com",
        ssh_port=22
    )
    
    # Mock session initialization
    client.session = AsyncMock()
    client.connector = AsyncMock()
    
    return client

class TestSoftServeClient:
    async def test_create_repository_success(self, softserve_client):
        """Test successful repository creation"""
        client = softserve_client
        
        # Mock successful response
        mock_response = AsyncMock()
        mock_response.status = 201
        mock_response.json.return_value = {"id": 123, "name": "test-repo"}
        
        client._make_request = AsyncMock(return_value=mock_response)
        
        result = await client.create_repository(
            path="user123/test-repo",
            description="Test repository",
            visibility="private"
        )
        
        assert result is True
        client._make_request.assert_called_once_with(
            "POST", 
            "/api/v1/repos",
            data={
                "name": "user123/test-repo",
                "description": "Test repository",
                "visibility": "private",
                "default_branch": "main",
                "auto_init": True
            }
        )

    async def test_create_repository_failure(self, softserve_client):
        """Test repository creation failure"""
        client = softserve_client
        
        # Mock error response
        mock_response = AsyncMock()
        mock_response.status = 400
        mock_response.text.return_value = "Invalid repository name"
        
        client._make_request = AsyncMock(return_value=mock_response)
        
        result = await client.create_repository(
            path="invalid/repo/name",
            description="Invalid repo"
        )
        
        assert result is False

    async def test_get_repository_success(self, softserve_client):
        """Test successful repository retrieval"""
        client = softserve_client
        
        # Mock successful response
        mock_response = AsyncMock()
        mock_response.status = 200
        mock_response.json.return_value = {
            "name": "test-repo",
            "full_name": "user123/test-repo",
            "description": "Test repository",
            "visibility": "private",
            "default_branch": "main",
            "clone_url_ssh": "git@git.example.com:user123/test-repo.git",
            "clone_url_https": "https://git.example.com/user123/test-repo.git",
            "created_at": "2023-01-01T00:00:00Z",
            "updated_at": "2023-01-01T00:00:00Z",
            "size": 1024,
            "is_empty": False,
            "is_archived": False
        }
        
        client._make_request = AsyncMock(return_value=mock_response)
        
        repo = await client.get_repository("user123/test-repo")
        
        assert repo is not None
        assert repo.name == "test-repo"
        assert repo.full_name == "user123/test-repo"
        assert repo.visibility == "private"

    async def test_get_repository_not_found(self, softserve_client):
        """Test repository retrieval when not found"""
        client = softserve_client
        
        # Mock 404 response
        mock_response = AsyncMock()
        mock_response.status = 404
        
        client._make_request = AsyncMock(return_value=mock_response)
        
        repo = await client.get_repository("user123/nonexistent")
        
        assert repo is None

    async def test_delete_repository_success(self, softserve_client):
        """Test successful repository deletion"""
        client = softserve_client
        
        # Mock successful response
        mock_response = AsyncMock()
        mock_response.status = 204
        
        client._make_request = AsyncMock(return_value=mock_response)
        
        result = await client.delete_repository("user123/test-repo")
        
        assert result is True
        client._make_request.assert_called_once_with(
            "DELETE", 
            "/api/v1/repos/user123%2Ftest-repo"
        )

    async def test_set_repository_permissions_success(self, softserve_client):
        """Test successful repository permission setting"""
        client = softserve_client
        
        # Mock successful response
        mock_response = AsyncMock()
        mock_response.status = 201
        
        client._make_request = AsyncMock(return_value=mock_response)
        
        result = await client.set_repository_permissions(
            repo_path="user123/test-repo",
            username="collaborator",
            permission="write"
        )
        
        assert result is True
        client._make_request.assert_called_once_with(
            "POST",
            "/api/v1/repos/user123%2Ftest-repo/collaborators",
            data={
                "username": "collaborator",
                "permission": "write"
            }
        )

    async def test_set_repository_permissions_invalid_permission(self, softserve_client):
        """Test repository permission setting with invalid permission"""
        client = softserve_client
        
        with pytest.raises(ValueError, match="Invalid permission level"):
            await client.set_repository_permissions(
                repo_path="user123/test-repo",
                username="collaborator",
                permission="invalid"
            )

    async def test_add_user_ssh_key_success(self, softserve_client):
        """Test successful SSH key addition"""
        client = softserve_client
        
        # Mock successful response
        mock_response = AsyncMock()
        mock_response.status = 201
        mock_response.json.return_value = {
            "id": 456,
            "fingerprint": "SHA256:abcd1234"
        }
        
        client._make_request = AsyncMock(return_value=mock_response)
        
        result = await client.add_user_ssh_key(
            username="user123",
            public_key="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAID user@example.com",
            title="My SSH Key"
        )
        
        assert result is True
        client._make_request.assert_called_once_with(
            "POST",
            "/api/v1/users/user123/keys",
            data={
                "key": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAID user@example.com",
                "title": "My SSH Key"
            }
        )

    async def test_remove_user_ssh_key_success(self, softserve_client):
        """Test successful SSH key removal"""
        client = softserve_client
        
        # Mock key lookup
        client._get_ssh_key_id_by_fingerprint = AsyncMock(return_value=456)
        
        # Mock successful deletion response
        mock_response = AsyncMock()
        mock_response.status = 204
        
        client._make_request = AsyncMock(return_value=mock_response)
        
        result = await client.remove_user_ssh_key(
            username="user123",
            key_fingerprint="SHA256:abcd1234"
        )
        
        assert result is True
        client._get_ssh_key_id_by_fingerprint.assert_called_once_with(
            "user123", "SHA256:abcd1234"
        )

    async def test_health_check_success(self, softserve_client):
        """Test successful health check"""
        client = softserve_client
        
        # Mock successful health response
        mock_response = AsyncMock()
        mock_response.status = 200
        
        client.session.get = AsyncMock()
        client.session.get.return_value.__aenter__.return_value = mock_response
        
        # Reset health check timer to force check
        client.last_health_check = 0
        
        result = await client.health_check()
        
        assert result is True
        assert client.health_status is True

    async def test_health_check_failure(self, softserve_client):
        """Test health check failure"""
        client = softserve_client
        
        # Mock failed health response
        mock_response = AsyncMock()
        mock_response.status = 500
        
        client.session.get = AsyncMock()
        client.session.get.return_value.__aenter__.return_value = mock_response
        
        # Reset health check timer to force check
        client.last_health_check = 0
        
        result = await client.health_check()
        
        assert result is False
        assert client.health_status is False

class TestRequestRetryLogic:
    async def test_make_request_retry_on_server_error(self, softserve_client):
        """Test request retry on server error"""
        client = softserve_client
        
        # Mock server error followed by success
        error_response = AsyncMock()
        error_response.status = 500
        
        success_response = AsyncMock()
        success_response.status = 200
        
        client.session = AsyncMock()
        client.session.request = AsyncMock()
        client.session.request.return_value.__aenter__.side_effect = [
            error_response, success_response
        ]
        
        with patch('asyncio.sleep') as mock_sleep:
            response = await client._make_request("GET", "/test")
            
            assert response.status == 200
            assert client.session.request.call_count == 2
            mock_sleep.assert_called_once_with(1)  # 2^0 = 1 second delay

    async def test_make_request_rate_limiting(self, softserve_client):
        """Test request rate limiting handling"""
        client = softserve_client
        
        # Mock rate limit response followed by success
        rate_limit_response = AsyncMock()
        rate_limit_response.status = 429
        rate_limit_response.headers = {"Retry-After": "2"}
        
        success_response = AsyncMock()
        success_response.status = 200
        
        client.session = AsyncMock()
        client.session.request = AsyncMock()
        client.session.request.return_value.__aenter__.side_effect = [
            rate_limit_response, success_response
        ]
        
        with patch('asyncio.sleep') as mock_sleep:
            response = await client._make_request("GET", "/test")
            
            assert response.status == 200
            mock_sleep.assert_called_once_with(2)  # Retry-After value

class TestCloneUrlGeneration:
    async def test_get_ssh_clone_url_default_port(self, softserve_client):
        """Test SSH clone URL generation with default port"""
        client = softserve_client
        client.ssh_port = 22
        
        url = await client.get_ssh_clone_url("user123/test-repo")
        
        assert url == "git@git.example.com:user123/test-repo.git"

    async def test_get_ssh_clone_url_custom_port(self, softserve_client):
        """Test SSH clone URL generation with custom port"""
        client = softserve_client
        client.ssh_port = 2222
        
        url = await client.get_ssh_clone_url("user123/test-repo")
        
        assert url == "ssh://git@git.example.com:2222/user123/test-repo.git"

    async def test_get_https_clone_url_default_port(self, softserve_client):
        """Test HTTPS clone URL generation with default port"""
        client = softserve_client
        client.https_port = 443
        
        url = await client.get_https_clone_url("user123/test-repo")
        
        assert url == "https://git.example.com/user123/test-repo.git"

    async def test_get_https_clone_url_custom_port(self, softserve_client):
        """Test HTTPS clone URL generation with custom port"""
        client = softserve_client
        client.https_port = 8443
        
        url = await client.get_https_clone_url("user123/test-repo")
        
        assert url == "https://git.example.com:8443/user123/test-repo.git"

class TestUserManagement:
    async def test_create_user_success(self, softserve_client):
        """Test successful user creation"""
        client = softserve_client
        
        # Mock successful response
        mock_response = AsyncMock()
        mock_response.status = 201
        mock_response.json.return_value = {"id": 789}
        
        client._make_request = AsyncMock(return_value=mock_response)
        
        result = await client.create_user(
            username="newuser",
            email="newuser@example.com",
            display_name="New User",
            is_admin=False
        )
        
        assert result is True
        client._make_request.assert_called_once_with(
            "POST",
            "/api/v1/users",
            data={
                "username": "newuser",
                "email": "newuser@example.com",
                "display_name": "New User",
                "is_admin": False
            }
        )

    async def test_get_user_success(self, softserve_client):
        """Test successful user retrieval"""
        client = softserve_client
        
        # Mock successful response
        mock_response = AsyncMock()
        mock_response.status = 200
        mock_response.json.return_value = {
            "username": "testuser",
            "display_name": "Test User",
            "email": "test@example.com",
            "is_admin": False,
            "created_at": "2023-01-01T00:00:00Z",
            "last_active": "2023-01-02T00:00:00Z"
        }
        
        client._make_request = AsyncMock(return_value=mock_response)
        
        user = await client.get_user("testuser")
        
        assert user is not None
        assert user.username == "testuser"
        assert user.email == "test@example.com"
        assert user.is_admin is False
```

## Security Checklist for Soft-serve Integration

### API Authentication & Authorization
- [ ] Secure admin token management with rotation capabilities
- [ ] API token validation and expiration checking
- [ ] HTTPS-only communication with certificate validation
- [ ] Request signing for critical operations
- [ ] Authentication token secure storage and transmission
- [ ] API endpoint access control with permission validation
- [ ] Cross-origin request validation and restrictions
- [ ] Session management for long-running operations
- [ ] API token scope limitations for least privilege access
- [ ] Administrative operation logging and audit trails

### Network Security
- [ ] TLS 1.3 enforcement for all API communications
- [ ] Certificate pinning for Git server connections
- [ ] Network timeout configuration to prevent hanging connections
- [ ] Rate limiting to prevent API abuse (100 requests/minute)
- [ ] Request size limits to prevent DoS attacks
- [ ] Connection pooling with secure connection reuse
- [ ] Network error handling without information disclosure
- [ ] Protection against DNS hijacking attacks
- [ ] Secure proxy configuration if applicable
- [ ] Network monitoring and anomaly detection

### Input Validation & Sanitization
- [ ] Repository path validation to prevent injection attacks
- [ ] Username validation with character restrictions
- [ ] SSH key format validation and sanitization
- [ ] API parameter validation with type checking
- [ ] URL encoding for special characters in paths
- [ ] Request payload validation with size limits
- [ ] JSON schema validation for API requests
- [ ] Protection against malformed data attacks
- [ ] Input length limits to prevent buffer overflow
- [ ] Special character filtering in user inputs

### Error Handling & Information Disclosure
- [ ] Error message sanitization to prevent information leakage
- [ ] Secure error logging without sensitive data exposure
- [ ] Generic error responses for authentication failures
- [ ] Stack trace filtering in production environments
- [ ] API response sanitization for security details
- [ ] Debug information removal from production
- [ ] Consistent error response format
- [ ] Error correlation IDs for troubleshooting
- [ ] Rate limiting on error responses
- [ ] Security incident detection from error patterns

## Performance Requirements

### API Operations
- Repository creation < 3 seconds
- Repository deletion < 2 seconds
- Permission updates < 1 second
- SSH key operations < 500ms
- Health checks < 200ms
- User operations < 1 second

### Network Performance
- API response time < 500ms (95th percentile)
- Connection establishment < 1 second
- Request retry delays (exponential backoff)
- Connection pool efficiency > 80%
- SSL handshake optimization
- DNS resolution caching

### Error Handling & Recovery
- Retry mechanism < 3 attempts
- Circuit breaker recovery < 30 seconds
- Health check recovery < 1 minute
- Connection timeout < 30 seconds
- Rate limit recovery automatic
- Graceful degradation during outages

## Integration Testing

### End-to-End Soft-serve Testing
```python
async def test_softserve_complete_integration():
    """Test complete Soft-serve integration workflow"""
    client = SoftServeClient(...)
    
    async with client:
        # Test user creation
        user_created = await client.create_user(
            username="integration_test",
            email="test@example.com"
        )
        assert user_created is True
        
        # Test SSH key addition
        key_added = await client.add_user_ssh_key(
            username="integration_test",
            public_key="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAID",
            title="Integration Test Key"
        )
        assert key_added is True
        
        # Test repository creation
        repo_created = await client.create_repository(
            path="integration_test/test-repo",
            description="Integration test repository"
        )
        assert repo_created is True
        
        # Test repository permissions
        perms_set = await client.set_repository_permissions(
            repo_path="integration_test/test-repo",
            username="integration_test",
            permission="admin"
        )
        assert perms_set is True
        
        # Test repository retrieval
        repo = await client.get_repository("integration_test/test-repo")
        assert repo is not None
        assert repo.name == "test-repo"
        
        # Cleanup
        await client.delete_repository("integration_test/test-repo")
```

### Health Monitoring Integration
```python
async def test_health_monitoring_integration():
    """Test health monitoring and recovery"""
    client = SoftServeClient(...)
    
    # Test healthy server
    health = await client.health_check()
    assert health is True
    
    # Test recovery after failure
    client.health_status = False
    
    # Simulate recovery
    with patch.object(client.session, 'get') as mock_get:
        mock_response = AsyncMock()
        mock_response.status = 200
        mock_get.return_value.__aenter__.return_value = mock_response
        
        client.last_health_check = 0  # Force check
        health = await client.health_check()
        assert health is True
```

## Next Implementation Steps

1. **Complete Soft-serve client implementation** with all API endpoints
2. **Add webhook support** for repository events and notifications
3. **Implement repository mirroring** for external Git repositories
4. **Add branch protection rules** with advanced security policies
5. **Create repository templates** with automated initialization
6. **Add Git LFS support** for large file storage
7. **Implement repository statistics** and analytics integration

## Commit Guidelines

Soft-serve integration commits should include:
- **Comprehensive API coverage** for all required Git server operations
- **Security validation** with authentication and input sanitization
- **Error handling** with retry logic and graceful degradation
- **Test coverage** for all API operations and error scenarios (>85%)
- **Performance optimization** with connection pooling and caching
- **Documentation updates** with API usage examples and configuration guides