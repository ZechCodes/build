# Session 9: Git Integration with Soft-serve

## Objective
Implement comprehensive Git repository management through Soft-serve integration, providing users with secure, isolated Git repositories with SSH key management, access controls, and seamless integration with the development environment.

## Overview
This session creates a complete Git hosting solution by integrating with Charm's Soft-serve Git server. It implements repository lifecycle management, SSH key handling, access control integration with the authentication system, clone URL generation, repository quotas, and comprehensive Git operations logging. The system provides each user with isolated Git repositories accessible from their terminal sessions.

## Prerequisites
- Session 1 (Core Infrastructure) completed successfully
- Session 2 (Authentication) completed successfully
- Session 3 (VM Management) completed successfully
- Session 4 (PTY Layer) completed successfully
- Session 5 (WebSocket Layer) completed successfully
- Session 6 (Session Management) completed successfully
- Session 7 (VM Snapshot System) completed successfully
- Session 8 (Frontend Terminal) completed successfully
- Soft-serve Git server deployed and operational
- SSH key infrastructure available

## Components to Implement

### 1. Git Repository Manager
**Location**: `git-manager/core/`

#### Repository Management System
```python
# git-manager/core/repository_manager.py
import asyncio
import json
import subprocess
import tempfile
from typing import Dict, Any, Optional, List
from dataclasses import dataclass, asdict
from enum import Enum
from pathlib import Path
import aiofiles
import structlog

logger = structlog.get_logger()

class RepositoryState(Enum):
    CREATING = "creating"
    ACTIVE = "active"
    ARCHIVED = "archived"
    DELETING = "deleting"
    ERROR = "error"

class RepositoryVisibility(Enum):
    PRIVATE = "private"
    PUBLIC = "public"
    INTERNAL = "internal"

@dataclass
class RepositoryMetadata:
    id: str
    name: str
    user_id: str
    description: str
    visibility: RepositoryVisibility
    state: RepositoryState
    size_bytes: int
    clone_url_ssh: str
    clone_url_https: str
    default_branch: str
    created_at: float
    updated_at: float
    last_push_at: Optional[float]
    push_count: int
    clone_count: int
    collaborators: List[str]
    tags: List[str]
    is_template: bool
    template_source_id: Optional[str] = None

class GitRepositoryManager:
    def __init__(self, softserve_client, database, ssh_key_manager):
        self.softserve = softserve_client
        self.db = database
        self.ssh_keys = ssh_key_manager
        self.max_repos_per_user = 50
        self.max_repo_size = 10 * 1024 * 1024 * 1024  # 10GB
        self.max_total_size_per_user = 50 * 1024 * 1024 * 1024  # 50GB
        
    async def create_repository(self, user_id: str, name: str, description: str = "",
                              visibility: RepositoryVisibility = RepositoryVisibility.PRIVATE,
                              is_template: bool = False,
                              template_source_id: Optional[str] = None) -> str:
        """Create a new Git repository"""
        try:
            # Validate user quotas
            if not await self._check_user_quotas(user_id):
                raise ValueError("User repository quota exceeded")
            
            # Validate repository name
            if not self._validate_repo_name(name):
                raise ValueError("Invalid repository name")
            
            # Check for name conflicts
            if await self._repo_name_exists(user_id, name):
                raise ValueError("Repository name already exists")
            
            # Generate repository ID
            repo_id = await self._generate_repo_id(user_id, name)
            
            # Create repository metadata
            metadata = RepositoryMetadata(
                id=repo_id,
                name=name,
                user_id=user_id,
                description=description,
                visibility=visibility,
                state=RepositoryState.CREATING,
                size_bytes=0,
                clone_url_ssh="",
                clone_url_https="",
                default_branch="main",
                created_at=time.time(),
                updated_at=time.time(),
                last_push_at=None,
                push_count=0,
                clone_count=0,
                collaborators=[],
                tags=[],
                is_template=is_template,
                template_source_id=template_source_id
            )
            
            # Store initial metadata
            await self._store_metadata(metadata)
            
            # Start repository creation task
            asyncio.create_task(self._create_repository_task(metadata))
            
            logger.info("Repository creation initiated", repo_id=repo_id,
                       name=name, user_id=user_id)
            return repo_id
            
        except Exception as e:
            logger.error("Failed to initiate repository creation", 
                        name=name, user_id=user_id, error=str(e))
            raise
    
    async def _create_repository_task(self, metadata: RepositoryMetadata):
        """Background task to create repository"""
        try:
            # Create repository in Soft-serve
            repo_path = f"{metadata.user_id}/{metadata.name}"
            
            # Initialize repository
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
            
            # Initialize with template if specified
            if metadata.template_source_id:
                await self._initialize_from_template(metadata, metadata.template_source_id)
            else:
                await self._initialize_empty_repository(metadata, repo_path)
            
            # Update metadata
            metadata.state = RepositoryState.ACTIVE
            metadata.clone_url_ssh = ssh_url
            metadata.clone_url_https = https_url
            metadata.updated_at = time.time()
            
            await self._store_metadata(metadata)
            
            logger.info("Repository creation completed", repo_id=metadata.id,
                       name=metadata.name)
            
        except Exception as e:
            logger.error("Repository creation failed", repo_id=metadata.id,
                        error=str(e))
            
            # Update metadata to error state
            metadata.state = RepositoryState.ERROR
            metadata.updated_at = time.time()
            await self._store_metadata(metadata)
    
    async def clone_repository(self, repo_id: str, user_id: str, 
                             destination_path: str) -> bool:
        """Clone repository to VM file system"""
        try:
            # Get repository metadata
            metadata = await self._get_metadata(repo_id)
            if not metadata:
                raise ValueError("Repository not found")
            
            # Check access permissions
            if not await self._check_access_permission(repo_id, user_id, "read"):
                raise ValueError("Access denied")
            
            # Get user's SSH key for cloning
            ssh_key_path = await self.ssh_keys.get_user_private_key_path(user_id)
            if not ssh_key_path:
                raise ValueError("No SSH key configured for user")
            
            # Perform clone operation
            clone_success = await self._perform_clone(
                metadata.clone_url_ssh, destination_path, ssh_key_path
            )
            
            if clone_success:
                # Update clone count
                metadata.clone_count += 1
                metadata.updated_at = time.time()
                await self._store_metadata(metadata)
                
                logger.info("Repository cloned successfully", repo_id=repo_id,
                           user_id=user_id, destination=destination_path)
            
            return clone_success
            
        except Exception as e:
            logger.error("Repository clone failed", repo_id=repo_id,
                        user_id=user_id, error=str(e))
            return False
    
    async def _perform_clone(self, clone_url: str, destination: str, 
                           ssh_key_path: str) -> bool:
        """Perform the actual git clone operation"""
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
"""
            
            with tempfile.NamedTemporaryFile(mode='w', suffix='.ssh_config', delete=False) as f:
                f.write(ssh_config)
                ssh_config_path = f.name
            
            try:
                # Run git clone with custom SSH config
                env = {
                    **os.environ,
                    'GIT_SSH_COMMAND': f'ssh -F {ssh_config_path}'
                }
                
                process = await asyncio.create_subprocess_exec(
                    'git', 'clone', clone_url, destination,
                    env=env,
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE
                )
                
                stdout, stderr = await process.communicate()
                
                if process.returncode == 0:
                    return True
                else:
                    logger.error("Git clone failed", 
                               stdout=stdout.decode(), stderr=stderr.decode())
                    return False
                    
            finally:
                # Cleanup SSH config file
                Path(ssh_config_path).unlink(missing_ok=True)
                
        except Exception as e:
            logger.error("Clone operation failed", error=str(e))
            return False
    
    async def add_collaborator(self, repo_id: str, owner_user_id: str,
                             collaborator_user_id: str, permission: str) -> bool:
        """Add collaborator to repository"""
        try:
            # Verify repository ownership
            metadata = await self._get_metadata(repo_id)
            if not metadata or metadata.user_id != owner_user_id:
                return False
            
            # Validate permission level
            if permission not in ["read", "write", "admin"]:
                raise ValueError("Invalid permission level")
            
            # Add collaborator in Soft-serve
            repo_path = f"{metadata.user_id}/{metadata.name}"
            await self.softserve.set_repository_permissions(
                repo_path, collaborator_user_id, permission
            )
            
            # Update metadata
            if collaborator_user_id not in metadata.collaborators:
                metadata.collaborators.append(collaborator_user_id)
                metadata.updated_at = time.time()
                await self._store_metadata(metadata)
            
            logger.info("Collaborator added", repo_id=repo_id,
                       collaborator=collaborator_user_id, permission=permission)
            return True
            
        except Exception as e:
            logger.error("Failed to add collaborator", repo_id=repo_id,
                        collaborator=collaborator_user_id, error=str(e))
            return False
    
    async def remove_collaborator(self, repo_id: str, owner_user_id: str,
                                collaborator_user_id: str) -> bool:
        """Remove collaborator from repository"""
        try:
            # Verify repository ownership
            metadata = await self._get_metadata(repo_id)
            if not metadata or metadata.user_id != owner_user_id:
                return False
            
            # Remove collaborator from Soft-serve
            repo_path = f"{metadata.user_id}/{metadata.name}"
            await self.softserve.remove_repository_permissions(
                repo_path, collaborator_user_id
            )
            
            # Update metadata
            if collaborator_user_id in metadata.collaborators:
                metadata.collaborators.remove(collaborator_user_id)
                metadata.updated_at = time.time()
                await self._store_metadata(metadata)
            
            logger.info("Collaborator removed", repo_id=repo_id,
                       collaborator=collaborator_user_id)
            return True
            
        except Exception as e:
            logger.error("Failed to remove collaborator", repo_id=repo_id,
                        collaborator=collaborator_user_id, error=str(e))
            return False
    
    async def delete_repository(self, repo_id: str, user_id: str) -> bool:
        """Delete a repository"""
        try:
            # Get repository metadata
            metadata = await self._get_metadata(repo_id)
            if not metadata or metadata.user_id != user_id:
                return False
            
            # Update state to deleting
            metadata.state = RepositoryState.DELETING
            metadata.updated_at = time.time()
            await self._store_metadata(metadata)
            
            # Delete from Soft-serve
            repo_path = f"{metadata.user_id}/{metadata.name}"
            await self.softserve.delete_repository(repo_path)
            
            # Remove metadata
            await self._delete_metadata(repo_id)
            
            logger.info("Repository deleted", repo_id=repo_id, user_id=user_id)
            return True
            
        except Exception as e:
            logger.error("Failed to delete repository", repo_id=repo_id,
                        error=str(e))
            return False
    
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
```

### 2. SSH Key Manager
**Location**: `git-manager/ssh/`

#### SSH Key Management System
```python
# git-manager/ssh/ssh_key_manager.py
import asyncio
import base64
import os
import tempfile
from typing import Dict, Any, Optional, List
from dataclasses import dataclass
from pathlib import Path
import cryptography.hazmat.primitives.asymmetric.rsa as rsa
import cryptography.hazmat.primitives.serialization as serialization
from cryptography.hazmat.primitives import hashes
import structlog

logger = structlog.get_logger()

@dataclass
class SSHKey:
    id: str
    user_id: str
    name: str
    public_key: str
    fingerprint: str
    key_type: str
    created_at: float
    last_used_at: Optional[float]
    is_active: bool

class SSHKeyManager:
    def __init__(self, database, softserve_client, key_storage_path: str):
        self.db = database
        self.softserve = softserve_client
        self.key_storage_path = Path(key_storage_path)
        self.key_storage_path.mkdir(exist_ok=True, mode=0o700)
        self.max_keys_per_user = 10
    
    async def generate_ssh_key_pair(self, user_id: str, name: str) -> str:
        """Generate a new SSH key pair for user"""
        try:
            # Check user quota
            user_keys = await self.list_user_keys(user_id)
            if len(user_keys) >= self.max_keys_per_user:
                raise ValueError("Maximum SSH keys per user exceeded")
            
            # Generate RSA key pair
            private_key = rsa.generate_private_key(
                public_exponent=65537,
                key_size=2048
            )
            
            # Serialize private key
            private_pem = private_key.private_bytes(
                encoding=serialization.Encoding.PEM,
                format=serialization.PrivateFormat.OpenSSH,
                encryption_algorithm=serialization.NoEncryption()
            )
            
            # Get public key
            public_key = private_key.public_key()
            public_ssh = public_key.public_bytes(
                encoding=serialization.Encoding.OpenSSH,
                format=serialization.PublicFormat.OpenSSH
            )
            
            # Calculate fingerprint
            fingerprint = self._calculate_fingerprint(public_ssh)
            
            # Generate key ID
            key_id = self._generate_key_id(user_id, fingerprint)
            
            # Store private key securely
            private_key_path = self.key_storage_path / f"{key_id}_private"
            private_key_path.write_bytes(private_pem)
            private_key_path.chmod(0o600)
            
            # Create SSH key metadata
            ssh_key = SSHKey(
                id=key_id,
                user_id=user_id,
                name=name,
                public_key=public_ssh.decode('utf-8'),
                fingerprint=fingerprint,
                key_type="rsa",
                created_at=time.time(),
                last_used_at=None,
                is_active=True
            )
            
            # Store metadata in database
            await self._store_ssh_key_metadata(ssh_key)
            
            # Register public key with Soft-serve
            await self.softserve.add_user_ssh_key(
                user_id, ssh_key.public_key, ssh_key.name
            )
            
            logger.info("SSH key generated", key_id=key_id, user_id=user_id, name=name)
            return key_id
            
        except Exception as e:
            logger.error("Failed to generate SSH key", user_id=user_id, 
                        name=name, error=str(e))
            raise
    
    async def add_existing_ssh_key(self, user_id: str, name: str, 
                                 public_key: str) -> str:
        """Add an existing SSH public key"""
        try:
            # Validate public key format
            if not self._validate_public_key(public_key):
                raise ValueError("Invalid SSH public key format")
            
            # Check for duplicate keys
            if await self._public_key_exists(public_key):
                raise ValueError("SSH key already exists")
            
            # Check user quota
            user_keys = await self.list_user_keys(user_id)
            if len(user_keys) >= self.max_keys_per_user:
                raise ValueError("Maximum SSH keys per user exceeded")
            
            # Calculate fingerprint
            fingerprint = self._calculate_fingerprint(public_key.encode())
            
            # Generate key ID
            key_id = self._generate_key_id(user_id, fingerprint)
            
            # Extract key type
            key_type = public_key.split()[0].replace('ssh-', '')
            
            # Create SSH key metadata
            ssh_key = SSHKey(
                id=key_id,
                user_id=user_id,
                name=name,
                public_key=public_key,
                fingerprint=fingerprint,
                key_type=key_type,
                created_at=time.time(),
                last_used_at=None,
                is_active=True
            )
            
            # Store metadata in database
            await self._store_ssh_key_metadata(ssh_key)
            
            # Register public key with Soft-serve
            await self.softserve.add_user_ssh_key(
                user_id, ssh_key.public_key, ssh_key.name
            )
            
            logger.info("SSH key added", key_id=key_id, user_id=user_id, name=name)
            return key_id
            
        except Exception as e:
            logger.error("Failed to add SSH key", user_id=user_id, 
                        name=name, error=str(e))
            raise
    
    async def delete_ssh_key(self, key_id: str, user_id: str) -> bool:
        """Delete an SSH key"""
        try:
            # Get key metadata
            ssh_key = await self._get_ssh_key_metadata(key_id)
            if not ssh_key or ssh_key.user_id != user_id:
                return False
            
            # Remove from Soft-serve
            await self.softserve.remove_user_ssh_key(user_id, ssh_key.fingerprint)
            
            # Delete private key file if it exists
            private_key_path = self.key_storage_path / f"{key_id}_private"
            private_key_path.unlink(missing_ok=True)
            
            # Delete metadata
            await self._delete_ssh_key_metadata(key_id)
            
            logger.info("SSH key deleted", key_id=key_id, user_id=user_id)
            return True
            
        except Exception as e:
            logger.error("Failed to delete SSH key", key_id=key_id, error=str(e))
            return False
    
    async def get_user_private_key_path(self, user_id: str) -> Optional[str]:
        """Get path to user's primary private key"""
        try:
            user_keys = await self.list_user_keys(user_id)
            active_keys = [key for key in user_keys if key.is_active]
            
            if not active_keys:
                return None
            
            # Return path to first active key (could implement priority system)
            primary_key = active_keys[0]
            private_key_path = self.key_storage_path / f"{primary_key.id}_private"
            
            if private_key_path.exists():
                return str(private_key_path)
            
            return None
            
        except Exception as e:
            logger.error("Failed to get private key path", user_id=user_id, error=str(e))
            return None
    
    def _calculate_fingerprint(self, public_key_bytes: bytes) -> str:
        """Calculate SSH key fingerprint"""
        digest = hashes.Hash(hashes.SHA256())
        digest.update(public_key_bytes)
        hash_bytes = digest.finalize()
        
        # Format as SHA256 fingerprint
        b64_hash = base64.b64encode(hash_bytes).decode('ascii').rstrip('=')
        return f"SHA256:{b64_hash}"
    
    def _validate_public_key(self, public_key: str) -> bool:
        """Validate SSH public key format"""
        try:
            parts = public_key.strip().split()
            if len(parts) < 2:
                return False
            
            # Check key type
            key_type = parts[0]
            if key_type not in ['ssh-rsa', 'ssh-ed25519', 'ecdsa-sha2-nistp256', 
                               'ecdsa-sha2-nistp384', 'ecdsa-sha2-nistp521']:
                return False
            
            # Validate base64 encoding of key data
            try:
                base64.b64decode(parts[1])
            except:
                return False
            
            return True
            
        except Exception:
            return False
```

### 3. Soft-serve Client Integration
**Location**: `git-manager/integrations/`

#### Soft-serve API Client
```python
# git-manager/integrations/softserve_client.py
import aiohttp
import asyncio
import json
from typing import Dict, Any, Optional, List
import structlog

logger = structlog.get_logger()

class SoftServeClient:
    def __init__(self, base_url: str, admin_token: str, ssh_hostname: str, ssh_port: int = 22):
        self.base_url = base_url.rstrip('/')
        self.admin_token = admin_token
        self.hostname = ssh_hostname
        self.ssh_port = ssh_port
        self.session: Optional[aiohttp.ClientSession] = None
    
    async def __aenter__(self):
        self.session = aiohttp.ClientSession(
            headers={'Authorization': f'Bearer {self.admin_token}'},
            timeout=aiohttp.ClientTimeout(total=30)
        )
        return self
    
    async def __aexit__(self, exc_type, exc_val, exc_tb):
        if self.session:
            await self.session.close()
    
    async def create_repository(self, path: str, description: str, 
                              visibility: str, default_branch: str = "main") -> bool:
        """Create a new repository in Soft-serve"""
        try:
            data = {
                "name": path,
                "description": description,
                "visibility": visibility,
                "default_branch": default_branch
            }
            
            async with self.session.post(f"{self.base_url}/api/v1/repos", json=data) as resp:
                if resp.status == 201:
                    logger.info("Repository created in Soft-serve", path=path)
                    return True
                else:
                    error = await resp.text()
                    logger.error("Failed to create repository", path=path, 
                               status=resp.status, error=error)
                    return False
                    
        except Exception as e:
            logger.error("Soft-serve API error during repository creation", 
                        path=path, error=str(e))
            return False
    
    async def delete_repository(self, path: str) -> bool:
        """Delete a repository from Soft-serve"""
        try:
            async with self.session.delete(f"{self.base_url}/api/v1/repos/{path}") as resp:
                if resp.status == 204:
                    logger.info("Repository deleted from Soft-serve", path=path)
                    return True
                else:
                    error = await resp.text()
                    logger.error("Failed to delete repository", path=path, 
                               status=resp.status, error=error)
                    return False
                    
        except Exception as e:
            logger.error("Soft-serve API error during repository deletion", 
                        path=path, error=str(e))
            return False
    
    async def set_repository_permissions(self, repo_path: str, user_id: str, 
                                       permission: str) -> bool:
        """Set user permissions for a repository"""
        try:
            data = {
                "user": user_id,
                "permission": permission
            }
            
            url = f"{self.base_url}/api/v1/repos/{repo_path}/collaborators"
            async with self.session.post(url, json=data) as resp:
                if resp.status in [200, 201]:
                    logger.info("Repository permissions set", repo_path=repo_path,
                               user_id=user_id, permission=permission)
                    return True
                else:
                    error = await resp.text()
                    logger.error("Failed to set repository permissions", 
                               repo_path=repo_path, user_id=user_id,
                               status=resp.status, error=error)
                    return False
                    
        except Exception as e:
            logger.error("Soft-serve API error setting permissions", 
                        repo_path=repo_path, user_id=user_id, error=str(e))
            return False
    
    async def add_user_ssh_key(self, user_id: str, public_key: str, name: str) -> bool:
        """Add SSH key for a user"""
        try:
            data = {
                "key": public_key,
                "title": name
            }
            
            url = f"{self.base_url}/api/v1/users/{user_id}/keys"
            async with self.session.post(url, json=data) as resp:
                if resp.status == 201:
                    logger.info("SSH key added for user", user_id=user_id, name=name)
                    return True
                else:
                    error = await resp.text()
                    logger.error("Failed to add SSH key", user_id=user_id,
                               status=resp.status, error=error)
                    return False
                    
        except Exception as e:
            logger.error("Soft-serve API error adding SSH key", 
                        user_id=user_id, error=str(e))
            return False
    
    async def get_ssh_clone_url(self, repo_path: str) -> str:
        """Generate SSH clone URL for repository"""
        return f"git@{self.hostname}:{repo_path}.git"
    
    async def get_https_clone_url(self, repo_path: str) -> str:
        """Generate HTTPS clone URL for repository"""
        return f"https://{self.hostname}/{repo_path}.git"
```

## Critical Decisions

### Repository Naming Scheme
- **Decision**: User namespace isolation with format `{user_id}/{repo_name}`
- **Rationale**: Prevents naming conflicts and enforces access control boundaries
- **Validation**: Alphanumeric with limited special characters, 2-100 characters

### SSH Key Management
- **Decision**: Support both generated and user-provided SSH keys
- **Rationale**: Flexibility for users while maintaining security
- **Storage**: Encrypted private keys in secure storage, public keys in Soft-serve

### Access Control Model
- **Decision**: Owner-based repositories with collaborator permissions (read/write/admin)
- **Rationale**: Simple yet flexible model matching Git hosting standards
- **Integration**: Integrated with existing authentication and authorization system

### Storage Quotas
- **Decision**: 50 repositories per user, 50GB total storage per user, 10GB per repository
- **Rationale**: Prevent abuse while allowing substantial development projects
- **Enforcement**: Real-time quota checking and monitoring

## Security Checklist ✅

### Repository Access Control
- [ ] Repository ownership validation for all operations
- [ ] Cross-user repository access prevention
- [ ] Collaborator permission enforcement
- [ ] Repository enumeration prevention
- [ ] Branch protection rules implementation
- [ ] Force push restrictions for protected branches
- [ ] Repository visibility controls enforcement
- [ ] Access audit logging for all operations
- [ ] Rate limiting on repository operations
- [ ] Secure repository deletion with data wiping

### SSH Key Security
- [ ] SSH key format validation and sanitization
- [ ] Private key encryption at rest
- [ ] SSH key fingerprint verification
- [ ] Duplicate SSH key prevention
- [ ] SSH key access audit logging
- [ ] Secure SSH key generation using cryptographically secure methods
- [ ] SSH key rotation and expiration policies
- [ ] Protection against SSH key enumeration
- [ ] Secure SSH key storage with proper file permissions
- [ ] SSH key usage monitoring and alerting

### Git Operations Security
- [ ] Git hook validation and sandboxing
- [ ] Large file upload prevention (>100MB per file)
- [ ] Binary file scanning for malware
- [ ] Repository size monitoring and limits
- [ ] Git protocol security validation
- [ ] Clone operation rate limiting
- [ ] Push operation validation and filtering
- [ ] Commit signature verification support
- [ ] Branch creation and deletion controls
- [ ] Tag creation and deletion permissions

### Integration Security
- [ ] Soft-serve API authentication and authorization
- [ ] Secure communication with Git server
- [ ] API token rotation and management
- [ ] Network isolation between services
- [ ] Input validation for all API calls
- [ ] Error handling without information leakage
- [ ] Secure credential storage and transmission
- [ ] Protection against Git server compromise
- [ ] Backup verification and integrity checks
- [ ] Disaster recovery security procedures

## Testing Requirements

### Repository Management Testing
- [ ] Repository creation with various parameters
- [ ] Repository deletion and cleanup verification
- [ ] Repository access control validation
- [ ] Repository quota enforcement
- [ ] Repository naming validation
- [ ] Collaborator management functionality
- [ ] Repository visibility settings
- [ ] Template repository functionality

### SSH Key Management Testing
- [ ] SSH key generation and storage
- [ ] SSH key addition and validation
- [ ] SSH key deletion and cleanup
- [ ] SSH key format validation
- [ ] Duplicate key prevention
- [ ] SSH key quota enforcement
- [ ] Key fingerprint calculation
- [ ] Private key security validation

### Git Operations Testing
- [ ] Repository cloning functionality
- [ ] Push and pull operations
- [ ] Branch operations and permissions
- [ ] Tag operations and permissions
- [ ] Large repository handling
- [ ] Concurrent operations
- [ ] Network failure recovery
- [ ] Authentication failure handling

### Integration Testing
- [ ] Soft-serve API integration
- [ ] Authentication system integration
- [ ] Authorization system integration
- [ ] VM file system integration
- [ ] Monitoring system integration
- [ ] Error handling across services
- [ ] Performance under load
- [ ] Security boundary validation

## Performance Targets

### Repository Operations
- Repository creation < 5 seconds
- Repository deletion < 3 seconds
- Repository listing < 500ms
- Clone operation initiation < 2 seconds
- SSH key generation < 1 second
- Permission updates < 1 second

### Git Operations
- Small repository clone (< 10MB) < 30 seconds
- Large repository clone (< 1GB) < 10 minutes
- Push operations < 5 seconds + transfer time
- Pull operations < 3 seconds + transfer time
- Branch/tag operations < 2 seconds

### System Performance
- API response times < 200ms
- Concurrent repository operations (20 per node)
- SSH key operations < 100ms
- Database queries < 50ms
- Soft-serve API calls < 2 seconds

## Monitoring & Alerting

### Repository Metrics
- Repository creation/deletion rates
- Repository size distribution
- Clone operation frequencies
- Push/pull operation metrics
- Collaborator activity patterns
- Quota utilization per user

### SSH Key Metrics
- SSH key creation/deletion rates
- SSH key usage frequencies
- Key type distribution
- Authentication success/failure rates
- Key rotation compliance
- Security incident frequencies

### Performance Metrics
- Git operation response times
- Repository access latencies
- Soft-serve API performance
- Database query performance
- Storage utilization rates
- Network bandwidth usage

### Alert Conditions
- Repository creation failure rate > 5%
- SSH authentication failure rate > 10%
- User quota utilization > 90%
- Git operation timeout rate > 2%
- Soft-serve API unavailability
- Repository corruption detected

## Documentation Deliverables

### Technical Documentation
- [ ] Git repository API specification
- [ ] SSH key management guide
- [ ] Soft-serve integration documentation
- [ ] Security architecture documentation
- [ ] Performance optimization guide
- [ ] Troubleshooting guide

### User Documentation
- [ ] Git repository management guide
- [ ] SSH key setup instructions
- [ ] Git workflow documentation
- [ ] Collaboration features guide
- [ ] Quota and limits documentation
- [ ] Best practices guide

## Next Steps

Upon successful completion of Session 9:
1. Git repository management system fully operational
2. SSH key management providing secure access
3. Soft-serve integration enabling Git hosting
4. Repository quotas and access controls enforced
5. Security measures fully implemented and tested
6. Integration with development environment working
7. Performance targets met under load testing
8. Proceed to Session 10: Terminal Recording System

## Risk Mitigation

### Technical Risks
1. **Git server failures**: Backup strategies, health monitoring
2. **SSH key compromise**: Key rotation, monitoring, revocation
3. **Repository corruption**: Backup verification, integrity checks
4. **Storage exhaustion**: Quota enforcement, cleanup procedures
5. **Performance degradation**: Caching, optimization, scaling

### Security Risks
1. **Unauthorized access**: Strong access controls, audit logging
2. **SSH key abuse**: Monitoring, rate limiting, validation
3. **Repository tampering**: Integrity checks, backup verification
4. **Data exfiltration**: Access controls, monitoring alerts
5. **Git server compromise**: Network isolation, security updates

---

**Session 9 Success Criteria:**
- Git repository management system fully operational with create/delete/clone
- SSH key management providing secure authentication
- Soft-serve integration enabling complete Git hosting functionality
- Repository access controls and collaborator management working
- Security checklist 100% complete with comprehensive protection
- Performance targets achieved for all Git operations
- User quotas and limits preventing abuse and ensuring fair usage
- Integration with Sessions 1-8 validated and working seamlessly
- All tests passing with >80% coverage including security tests
- Documentation complete with user guides and technical references
- Ready for Session 10 terminal recording implementation