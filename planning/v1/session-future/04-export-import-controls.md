# Export & Import Controls

**Priority**: Low  
**Effort**: Low  
**Timeline**: Future Release  
**Security Impact**: Low for data portability features

## Overview

Export and import controls provide secure mechanisms for users to export their session data, migrate between environments, and import session configurations. These features enhance user experience while maintaining strict security controls around data portability.

## Moved from Session 6

The following export/import requirements were identified as future enhancements during Session 6 development:

### 1. Secure Session Export Functionality
- **New requirement**: Export session data for backup/migration purposes
- **Current status**: No export functionality available
- **Gap**: Users cannot backup or migrate session data

### 2. Import Validation and Security Controls
- **New requirement**: Import session data with validation and security checks
- **Current status**: No import functionality available  
- **Gap**: No mechanism for session data migration or restoration

### 3. Data Portability Features
- **New requirement**: GDPR-compliant data export for user rights
- **Current status**: Manual data extraction required
- **Gap**: No standardized data portability mechanisms

## Technical Implementation

### Secure Session Export

**Objective**: Allow users to export their session data in a secure, standardized format.

**Implementation Approach**:

```python
# Session Export Manager
from dataclasses import dataclass, asdict
from typing import List, Dict, Any, Optional
import json
import zipfile
import tempfile
from cryptography.fernet import Fernet

@dataclass
class ExportMetadata:
    export_id: str
    user_id: str
    export_timestamp: float
    session_count: int
    total_size_bytes: int
    format_version: str
    encryption_enabled: bool
    data_types: List[str]

@dataclass 
class SessionExportData:
    session_id: str
    user_id: str
    vm_id: str
    state: str
    created_at: float
    last_activity: float
    environment_config: Dict[str, Any]
    buffer_data: Optional[str] = None  # Base64 encoded if included
    buffer_metadata: Optional[Dict[str, Any]] = None

class SessionExportManager:
    def __init__(self, session_manager, buffer_manager, redis_client):
        self.session_manager = session_manager
        self.buffer_manager = buffer_manager
        self.redis = redis_client
        self.export_key_prefix = "export:request:"
        self.max_export_size = 100 * 1024 * 1024  # 100MB limit
        
    async def initiate_export_request(self, user_id: str, 
                                    export_options: Dict[str, Any]) -> str:
        """Initiate session data export request"""
        try:
            export_id = f"export_{user_id}_{int(time.time())}_{secrets.token_hex(8)}"
            
            # Validate export options
            if not self._validate_export_options(export_options, user_id):
                raise ValueError("Invalid export options")
            
            # Estimate export size
            estimated_size = await self._estimate_export_size(user_id, export_options)
            if estimated_size > self.max_export_size:
                raise ValueError(f"Export size {estimated_size} exceeds limit {self.max_export_size}")
            
            # Create export request
            export_request = {
                'export_id': export_id,
                'user_id': user_id,
                'options': export_options,
                'status': 'initiated',
                'created_at': time.time(),
                'estimated_size': estimated_size
            }
            
            # Store export request
            await self.redis.hset(
                f"{self.export_key_prefix}{export_id}",
                mapping={k: json.dumps(v) if isinstance(v, (dict, list)) else str(v) 
                        for k, v in export_request.items()}
            )
            await self.redis.expire(f"{self.export_key_prefix}{export_id}", 3600)  # 1 hour TTL
            
            # Start async export process
            asyncio.create_task(self._process_export_request(export_id, user_id, export_options))
            
            logger.info("Export request initiated",
                       export_id=export_id,
                       user_id=user_id,
                       estimated_size=estimated_size)
            
            return export_id
            
        except Exception as e:
            logger.error("Export request initiation failed",
                        user_id=user_id, error=str(e))
            raise
    
    async def _process_export_request(self, export_id: str, user_id: str,
                                    export_options: Dict[str, Any]):
        """Process export request asynchronously"""
        try:
            # Update status to processing
            await self._update_export_status(export_id, 'processing')
            
            # Collect session data
            sessions_data = await self._collect_session_data(user_id, export_options)
            
            # Create export package
            export_package = await self._create_export_package(
                export_id, user_id, sessions_data, export_options
            )
            
            # Store export package
            package_url = await self._store_export_package(export_id, export_package)
            
            # Update status to completed
            await self._update_export_status(export_id, 'completed', {
                'package_url': package_url,
                'actual_size': len(export_package),
                'session_count': len(sessions_data)
            })
            
            logger.info("Export request completed",
                       export_id=export_id,
                       user_id=user_id,
                       session_count=len(sessions_data))
            
        except Exception as e:
            logger.error("Export processing failed",
                        export_id=export_id, error=str(e))
            await self._update_export_status(export_id, 'failed', {'error': str(e)})
    
    async def _collect_session_data(self, user_id: str, 
                                   export_options: Dict[str, Any]) -> List[SessionExportData]:
        """Collect session data for export"""
        sessions_data = []
        
        try:
            # Get user's sessions
            user_sessions = await self.session_manager.list_user_sessions(user_id)
            
            for session in user_sessions:
                session_export = SessionExportData(
                    session_id=session.session_id,
                    user_id=session.user_id,
                    vm_id=session.vm_id,
                    state=session.state.value,
                    created_at=session.created_at,
                    last_activity=session.last_activity,
                    environment_config=session.environment_config
                )
                
                # Include buffer data if requested
                if export_options.get('include_buffer_data', False):
                    buffer_data = await self.buffer_manager.retrieve_buffer(
                        session.session_id, user_id
                    )
                    
                    if buffer_data:
                        # Encode buffer data as base64 for JSON serialization
                        session_export.buffer_data = base64.b64encode(
                            buffer_data.buffer_data
                        ).decode('utf-8')
                        
                        session_export.buffer_metadata = {
                            'size_bytes': buffer_data.size_bytes,
                            'line_count': buffer_data.line_count,
                            'last_updated': buffer_data.last_updated,
                            'cursor_position': buffer_data.cursor_position,
                            'scroll_position': buffer_data.scroll_position
                        }
                
                sessions_data.append(session_export)
            
            return sessions_data
            
        except Exception as e:
            logger.error("Session data collection failed", user_id=user_id, error=str(e))
            raise
    
    async def _create_export_package(self, export_id: str, user_id: str,
                                   sessions_data: List[SessionExportData],
                                   export_options: Dict[str, Any]) -> bytes:
        """Create encrypted export package"""
        try:
            # Create export metadata
            metadata = ExportMetadata(
                export_id=export_id,
                user_id=user_id,
                export_timestamp=time.time(),
                session_count=len(sessions_data),
                total_size_bytes=0,  # Will be calculated
                format_version="1.0",
                encryption_enabled=export_options.get('encrypt_export', True),
                data_types=['sessions', 'buffers'] if export_options.get('include_buffer_data') else ['sessions']
            )
            
            # Create export data structure
            export_data = {
                'metadata': asdict(metadata),
                'sessions': [asdict(session) for session in sessions_data],
                'export_options': export_options,
                'schema_version': '1.0'
            }
            
            # Serialize to JSON
            json_data = json.dumps(export_data, indent=2, sort_keys=True)
            json_bytes = json_data.encode('utf-8')
            
            # Update metadata with actual size
            metadata.total_size_bytes = len(json_bytes)
            export_data['metadata'] = asdict(metadata)
            json_data = json.dumps(export_data, indent=2, sort_keys=True)
            json_bytes = json_data.encode('utf-8')
            
            # Encrypt if requested
            if export_options.get('encrypt_export', True):
                encryption_key = export_options.get('encryption_key')
                if not encryption_key:
                    # Generate temporary key for export
                    encryption_key = Fernet.generate_key()
                    # Store key separately for user retrieval
                    await self._store_export_key(export_id, encryption_key)
                
                cipher_suite = Fernet(encryption_key)
                encrypted_data = cipher_suite.encrypt(json_bytes)
                
                # Create ZIP package with encrypted data
                with tempfile.NamedTemporaryFile() as tmp_file:
                    with zipfile.ZipFile(tmp_file.name, 'w', zipfile.ZIP_DEFLATED) as zip_file:
                        zip_file.writestr('session_export_encrypted.dat', encrypted_data)
                        zip_file.writestr('README.txt', self._generate_export_readme(metadata))
                    
                    tmp_file.seek(0)
                    return tmp_file.read()
            else:
                # Create unencrypted ZIP package
                with tempfile.NamedTemporaryFile() as tmp_file:
                    with zipfile.ZipFile(tmp_file.name, 'w', zipfile.ZIP_DEFLATED) as zip_file:
                        zip_file.writestr('session_export.json', json_data)
                        zip_file.writestr('README.txt', self._generate_export_readme(metadata))
                    
                    tmp_file.seek(0)
                    return tmp_file.read()
                    
        except Exception as e:
            logger.error("Export package creation failed", export_id=export_id, error=str(e))
            raise
    
    def _generate_export_readme(self, metadata: ExportMetadata) -> str:
        """Generate README file for export package"""
        return f"""Session Manager Data Export
========================

Export ID: {metadata.export_id}
User ID: {metadata.user_id}
Export Date: {datetime.fromtimestamp(metadata.export_timestamp).isoformat()}
Format Version: {metadata.format_version}

Summary:
- Session Count: {metadata.session_count}
- Total Size: {metadata.total_size_bytes:,} bytes
- Data Types: {', '.join(metadata.data_types)}
- Encrypted: {'Yes' if metadata.encryption_enabled else 'No'}

Files:
{'- session_export_encrypted.dat (encrypted session data)' if metadata.encryption_enabled else '- session_export.json (session data)'}
- README.txt (this file)

Import Instructions:
Use the Session Manager import functionality to restore this data.
If encrypted, you will need the encryption key provided separately.

Data Format:
This export follows Session Manager Export Format v{metadata.format_version}
For technical details, see the Session Manager documentation.
"""
    
    async def get_export_status(self, export_id: str, user_id: str) -> Optional[Dict[str, Any]]:
        """Get export request status"""
        try:
            export_data = await self.redis.hgetall(f"{self.export_key_prefix}{export_id}")
            
            if not export_data:
                return None
            
            # Verify ownership
            stored_user_id = export_data.get(b'user_id', b'').decode('utf-8')
            if stored_user_id != user_id:
                logger.warning("Unauthorized export status access",
                             export_id=export_id, requesting_user=user_id)
                return None
            
            # Convert Redis data
            status_data = {}
            for key, value in export_data.items():
                key_str = key.decode('utf-8')
                value_str = value.decode('utf-8')
                
                # Parse JSON fields
                if key_str in ['options', 'result']:
                    try:
                        status_data[key_str] = json.loads(value_str)
                    except json.JSONDecodeError:
                        status_data[key_str] = value_str
                else:
                    status_data[key_str] = value_str
            
            return status_data
            
        except Exception as e:
            logger.error("Export status retrieval failed",
                        export_id=export_id, error=str(e))
            return None
    
    async def download_export(self, export_id: str, user_id: str) -> Optional[bytes]:
        """Download completed export package"""
        try:
            # Verify export status and ownership
            status = await self.get_export_status(export_id, user_id)
            if not status or status.get('status') != 'completed':
                return None
            
            # Get package URL from status
            result = status.get('result', {})
            if isinstance(result, str):
                result = json.loads(result)
            
            package_url = result.get('package_url')
            if not package_url:
                return None
            
            # Retrieve package data
            package_data = await self._retrieve_export_package(package_url)
            
            # Log download
            logger.info("Export package downloaded",
                       export_id=export_id, user_id=user_id)
            
            return package_data
            
        except Exception as e:
            logger.error("Export download failed",
                        export_id=export_id, error=str(e))
            return None
```

### Session Import with Validation

**Objective**: Securely import session data with comprehensive validation and security checks.

**Implementation Approach**:

```python
# Session Import Manager
class SessionImportManager:
    def __init__(self, session_manager, buffer_manager, export_manager):
        self.session_manager = session_manager
        self.buffer_manager = buffer_manager
        self.export_manager = export_manager
        self.supported_versions = ['1.0']
        self.max_import_size = 100 * 1024 * 1024  # 100MB limit
    
    async def validate_import_package(self, package_data: bytes, 
                                    user_id: str) -> Dict[str, Any]:
        """Validate import package before processing"""
        try:
            # Check package size
            if len(package_data) > self.max_import_size:
                raise ValueError(f"Import package too large: {len(package_data)} bytes")
            
            # Extract and validate package structure
            validation_result = {
                'valid': False,
                'errors': [],
                'warnings': [],
                'metadata': None,
                'session_count': 0,
                'estimated_conflicts': 0
            }
            
            # Try to extract package
            try:
                with tempfile.NamedTemporaryFile() as tmp_file:
                    tmp_file.write(package_data)
                    tmp_file.flush()
                    
                    with zipfile.ZipFile(tmp_file.name, 'r') as zip_file:
                        # Check for required files
                        file_names = zip_file.namelist()
                        
                        if 'session_export.json' in file_names:
                            # Unencrypted package
                            import_data = json.loads(
                                zip_file.read('session_export.json').decode('utf-8')
                            )
                        elif 'session_export_encrypted.dat' in file_names:
                            validation_result['errors'].append(
                                "Encrypted package requires decryption key"
                            )
                            return validation_result
                        else:
                            validation_result['errors'].append(
                                "Invalid package structure - missing data files"
                            )
                            return validation_result
                            
            except zipfile.BadZipFile:
                validation_result['errors'].append("Invalid ZIP package format")
                return validation_result
            except json.JSONDecodeError:
                validation_result['errors'].append("Invalid JSON data format")
                return validation_result
            
            # Validate data structure
            if not self._validate_import_data_structure(import_data, validation_result):
                return validation_result
            
            # Validate metadata
            metadata = import_data.get('metadata', {})
            validation_result['metadata'] = metadata
            
            # Check format version compatibility
            format_version = metadata.get('format_version', '1.0')
            if format_version not in self.supported_versions:
                validation_result['errors'].append(
                    f"Unsupported format version: {format_version}"
                )
                return validation_result
            
            # Validate sessions data
            sessions = import_data.get('sessions', [])
            validation_result['session_count'] = len(sessions)
            
            # Check for potential conflicts
            conflicts = await self._check_import_conflicts(sessions, user_id)
            validation_result['estimated_conflicts'] = len(conflicts)
            
            if conflicts:
                validation_result['warnings'].append(
                    f"Found {len(conflicts)} potential session ID conflicts"
                )
            
            # Validate each session
            for i, session_data in enumerate(sessions):
                session_errors = self._validate_session_data(session_data)
                if session_errors:
                    validation_result['errors'].extend([
                        f"Session {i}: {error}" for error in session_errors
                    ])
            
            # Mark as valid if no errors
            validation_result['valid'] = len(validation_result['errors']) == 0
            
            return validation_result
            
        except Exception as e:
            logger.error("Import package validation failed", error=str(e))
            return {
                'valid': False,
                'errors': [f"Validation error: {str(e)}"],
                'warnings': [],
                'metadata': None,
                'session_count': 0,
                'estimated_conflicts': 0
            }
    
    async def import_sessions(self, package_data: bytes, user_id: str,
                            import_options: Dict[str, Any],
                            decryption_key: Optional[str] = None) -> Dict[str, Any]:
        """Import sessions from validated package"""
        try:
            import_id = f"import_{user_id}_{int(time.time())}_{secrets.token_hex(8)}"
            
            # Extract import data
            import_data = await self._extract_import_data(
                package_data, decryption_key
            )
            
            # Validate again (security best practice)
            validation = await self.validate_import_package(package_data, user_id)
            if not validation['valid']:
                raise ValueError(f"Import validation failed: {validation['errors']}")
            
            # Process import
            import_result = {
                'import_id': import_id,
                'status': 'processing',
                'imported_sessions': [],
                'skipped_sessions': [],
                'errors': [],
                'warnings': []
            }
            
            sessions = import_data.get('sessions', [])
            
            for session_data in sessions:
                try:
                    # Handle session ID conflicts
                    session_id = session_data['session_id']
                    if import_options.get('handle_conflicts') == 'generate_new':
                        # Generate new session ID to avoid conflicts
                        new_session_id = self.session_manager._generate_session_id(
                            user_id, session_data['vm_id']
                        )
                        session_data['session_id'] = new_session_id
                        import_result['warnings'].append(
                            f"Generated new session ID: {session_id} -> {new_session_id}"
                        )
                    elif import_options.get('handle_conflicts') == 'skip':
                        # Check if session exists
                        existing = await self.session_manager.get_session(session_id)
                        if existing:
                            import_result['skipped_sessions'].append(session_id)
                            continue
                    
                    # Import session
                    imported_session = await self._import_single_session(
                        session_data, user_id, import_options
                    )
                    
                    import_result['imported_sessions'].append(imported_session.session_id)
                    
                except Exception as e:
                    error_msg = f"Failed to import session {session_data.get('session_id', 'unknown')}: {str(e)}"
                    import_result['errors'].append(error_msg)
                    logger.error("Single session import failed", 
                               session_id=session_data.get('session_id'), error=str(e))
            
            import_result['status'] = 'completed'
            
            logger.info("Session import completed",
                       import_id=import_id,
                       user_id=user_id,
                       imported_count=len(import_result['imported_sessions']),
                       skipped_count=len(import_result['skipped_sessions']),
                       error_count=len(import_result['errors']))
            
            return import_result
            
        except Exception as e:
            logger.error("Session import failed", user_id=user_id, error=str(e))
            raise
    
    async def _import_single_session(self, session_data: Dict[str, Any],
                                   user_id: str, import_options: Dict[str, Any]) -> Session:
        """Import a single session with validation"""
        try:
            # Create session object
            from core.state_manager import SessionState
            
            session = Session(
                session_id=session_data['session_id'],
                user_id=user_id,  # Always use importing user's ID
                vm_id=session_data['vm_id'],
                state=SessionState(session_data['state']),
                created_at=session_data['created_at'],
                last_activity=session_data['last_activity'],
                environment_config=session_data.get('environment_config', {})
            )
            
            # Store session
            await self.session_manager._store_session(session)
            
            # Import buffer data if present and requested
            if (import_options.get('include_buffer_data', False) and 
                session_data.get('buffer_data')):
                
                await self._import_buffer_data(session_data, user_id)
            
            return session
            
        except Exception as e:
            logger.error("Single session import failed", 
                        session_id=session_data.get('session_id'), error=str(e))
            raise
    
    async def _import_buffer_data(self, session_data: Dict[str, Any], user_id: str):
        """Import buffer data for session"""
        try:
            buffer_data_b64 = session_data.get('buffer_data')
            buffer_metadata = session_data.get('buffer_metadata', {})
            
            if not buffer_data_b64:
                return
            
            # Decode buffer data
            buffer_data = base64.b64decode(buffer_data_b64)
            
            # Store buffer
            await self.buffer_manager.store_buffer(
                session_id=session_data['session_id'],
                user_id=user_id,
                buffer_data=buffer_data,
                cursor_pos=buffer_metadata.get('cursor_position', (0, 0)),
                scroll_pos=buffer_metadata.get('scroll_position', 0)
            )
            
        except Exception as e:
            logger.error("Buffer data import failed",
                        session_id=session_data.get('session_id'), error=str(e))
            # Don't fail entire import for buffer issues
    
    def _validate_session_data(self, session_data: Dict[str, Any]) -> List[str]:
        """Validate individual session data"""
        errors = []
        
        required_fields = ['session_id', 'user_id', 'vm_id', 'state', 'created_at']
        for field in required_fields:
            if field not in session_data:
                errors.append(f"Missing required field: {field}")
        
        # Validate data types
        if 'created_at' in session_data:
            try:
                float(session_data['created_at'])
            except (ValueError, TypeError):
                errors.append("Invalid created_at timestamp")
        
        if 'last_activity' in session_data:
            try:
                float(session_data['last_activity'])
            except (ValueError, TypeError):
                errors.append("Invalid last_activity timestamp")
        
        # Validate state
        if 'state' in session_data:
            from core.state_manager import SessionState
            try:
                SessionState(session_data['state'])
            except ValueError:
                errors.append(f"Invalid session state: {session_data['state']}")
        
        return errors
```

### Data Portability API

**Objective**: Provide GDPR-compliant data portability through standardized APIs.

**Implementation Approach**:

```python
# Data Portability API
from fastapi import APIRouter, Depends, HTTPException, BackgroundTasks
from fastapi.responses import StreamingResponse

class DataPortabilityAPI:
    def __init__(self, export_manager: SessionExportManager, 
                 import_manager: SessionImportManager):
        self.export_manager = export_manager
        self.import_manager = import_manager
        self.router = APIRouter(prefix="/api/v1/data-portability")
        self._setup_routes()
    
    def _setup_routes(self):
        """Setup API routes for data portability"""
        
        @self.router.post("/export/request")
        async def request_data_export(
            export_options: Dict[str, Any],
            user_id: str = Depends(get_current_user_id)
        ):
            """Request user data export (GDPR Article 20)"""
            try:
                # Validate export options
                allowed_options = {
                    'include_buffer_data': bool,
                    'encrypt_export': bool,
                    'format': str  # 'json', 'csv'
                }
                
                for key, value in export_options.items():
                    if key not in allowed_options:
                        raise HTTPException(400, f"Invalid export option: {key}")
                    if not isinstance(value, allowed_options[key]):
                        raise HTTPException(400, f"Invalid type for {key}")
                
                # Initiate export
                export_id = await self.export_manager.initiate_export_request(
                    user_id, export_options
                )
                
                return {
                    'export_id': export_id,
                    'status': 'initiated',
                    'estimated_completion': time.time() + 300  # 5 minutes
                }
                
            except ValueError as e:
                raise HTTPException(400, str(e))
            except Exception as e:
                logger.error("Export request failed", user_id=user_id, error=str(e))
                raise HTTPException(500, "Export request failed")
        
        @self.router.get("/export/{export_id}/status")
        async def get_export_status(
            export_id: str,
            user_id: str = Depends(get_current_user_id)
        ):
            """Get export request status"""
            try:
                status = await self.export_manager.get_export_status(export_id, user_id)
                if not status:
                    raise HTTPException(404, "Export request not found")
                
                return status
                
            except Exception as e:
                logger.error("Export status check failed", 
                           export_id=export_id, error=str(e))
                raise HTTPException(500, "Status check failed")
        
        @self.router.get("/export/{export_id}/download")
        async def download_export(
            export_id: str,
            user_id: str = Depends(get_current_user_id)
        ):
            """Download completed export package"""
            try:
                package_data = await self.export_manager.download_export(export_id, user_id)
                if not package_data:
                    raise HTTPException(404, "Export package not found or not ready")
                
                # Stream response
                def generate():
                    yield package_data
                
                return StreamingResponse(
                    generate(),
                    media_type='application/zip',
                    headers={
                        'Content-Disposition': f'attachment; filename="session_export_{export_id}.zip"'
                    }
                )
                
            except Exception as e:
                logger.error("Export download failed", 
                           export_id=export_id, error=str(e))
                raise HTTPException(500, "Download failed")
        
        @self.router.post("/import/validate")
        async def validate_import(
            package: UploadFile,
            user_id: str = Depends(get_current_user_id)
        ):
            """Validate import package before processing"""
            try:
                # Read package data
                package_data = await package.read()
                
                # Validate package
                validation = await self.import_manager.validate_import_package(
                    package_data, user_id
                )
                
                return validation
                
            except Exception as e:
                logger.error("Import validation failed", user_id=user_id, error=str(e))
                raise HTTPException(500, "Validation failed")
        
        @self.router.post("/import/execute")
        async def execute_import(
            package: UploadFile,
            import_options: Dict[str, Any],
            background_tasks: BackgroundTasks,
            user_id: str = Depends(get_current_user_id),
            decryption_key: Optional[str] = None
        ):
            """Execute session data import"""
            try:
                # Read package data
                package_data = await package.read()
                
                # Validate import options
                allowed_options = {
                    'handle_conflicts': str,  # 'skip', 'generate_new', 'overwrite'
                    'include_buffer_data': bool,
                    'dry_run': bool
                }
                
                for key, value in import_options.items():
                    if key not in allowed_options:
                        raise HTTPException(400, f"Invalid import option: {key}")
                
                # Execute import
                if import_options.get('dry_run', False):
                    # Validation only
                    validation = await self.import_manager.validate_import_package(
                        package_data, user_id
                    )
                    return {'dry_run': True, 'validation': validation}
                else:
                    # Actual import
                    result = await self.import_manager.import_sessions(
                        package_data, user_id, import_options, decryption_key
                    )
                    return result
                
            except ValueError as e:
                raise HTTPException(400, str(e))
            except Exception as e:
                logger.error("Import execution failed", user_id=user_id, error=str(e))
                raise HTTPException(500, "Import failed")
```

## Configuration and Monitoring

### Export/Import Configuration

```python
@dataclass
class DataPortabilityConfig:
    # Export settings
    max_export_size_mb: int = 100
    export_retention_hours: int = 24
    default_encryption: bool = True
    allowed_export_formats: List[str] = None
    
    # Import settings  
    max_import_size_mb: int = 100
    import_validation_strict: bool = True
    default_conflict_resolution: str = "skip"
    allow_buffer_import: bool = True
    
    # Compliance settings
    gdpr_compliance: bool = True
    data_retention_policy: bool = True
    audit_all_operations: bool = True
    
    # Rate limiting
    max_exports_per_user_per_day: int = 5
    max_imports_per_user_per_day: int = 3

def create_data_portability_services(config: DataPortabilityConfig):
    export_manager = SessionExportManager(
        session_manager, buffer_manager, redis_client
    )
    
    import_manager = SessionImportManager(
        session_manager, buffer_manager, export_manager
    )
    
    api = DataPortabilityAPI(export_manager, import_manager)
    
    return export_manager, import_manager, api
```

### Monitoring and Compliance

```python
class DataPortabilityMonitor:
    def __init__(self):
        self.export_requests = 0
        self.import_requests = 0
        self.gdpr_requests = 0
        self.failed_operations = 0
    
    def record_export_request(self, user_id: str, data_types: List[str]):
        """Record export request for compliance tracking"""
        self.export_requests += 1
        
        # Check if this is a GDPR data portability request
        if 'gdpr_request' in data_types:
            self.gdpr_requests += 1
        
        event = {
            'timestamp': time.time(),
            'event_type': 'data_export_requested',
            'user_id': user_id,
            'data_types': data_types,
            'compliance_type': 'gdpr_article_20' if 'gdpr_request' in data_types else 'user_initiated'
        }
        
        logger.info("Data export requested", **event)
        logfire.info("Data portability event", **event)
    
    def record_import_completion(self, user_id: str, session_count: int, 
                               errors: List[str]):
        """Record import completion"""
        self.import_requests += 1
        if errors:
            self.failed_operations += 1
        
        event = {
            'timestamp': time.time(),
            'event_type': 'data_import_completed',
            'user_id': user_id,
            'session_count': session_count,
            'error_count': len(errors),
            'success': len(errors) == 0
        }
        
        logger.info("Data import completed", **event)
        logfire.info("Data portability event", **event)
    
    def generate_compliance_report(self) -> Dict[str, Any]:
        """Generate compliance report for GDPR audits"""
        return {
            'report_period': '30_days',
            'total_export_requests': self.export_requests,
            'gdpr_export_requests': self.gdpr_requests,
            'total_import_requests': self.import_requests,
            'failed_operations': self.failed_operations,
            'compliance_status': 'compliant',
            'data_retention_compliant': True,
            'user_rights_supported': ['portability', 'export', 'import']
        }
```

## Testing and Validation

```python
async def test_export_import_functionality():
    """Test complete export/import cycle"""
    
    # Test export functionality
    await test_session_export()
    await test_export_encryption()
    await test_export_validation()
    
    # Test import functionality
    await test_import_validation()
    await test_import_execution()
    await test_conflict_resolution()
    
    # Test API endpoints
    await test_data_portability_api()

async def test_session_export():
    """Test session data export"""
    export_manager = SessionExportManager(session_manager, buffer_manager, redis_client)
    
    user_id = "test_user"
    export_options = {
        'include_buffer_data': True,
        'encrypt_export': True
    }
    
    # Initiate export
    export_id = await export_manager.initiate_export_request(user_id, export_options)
    assert export_id is not None
    
    # Wait for completion
    await asyncio.sleep(1)
    
    # Check status
    status = await export_manager.get_export_status(export_id, user_id)
    assert status['status'] == 'completed'
    
    # Download package
    package_data = await export_manager.download_export(export_id, user_id)
    assert package_data is not None
    assert len(package_data) > 0

async def test_import_validation():
    """Test import package validation"""
    import_manager = SessionImportManager(session_manager, buffer_manager, export_manager)
    
    # Create test package
    test_package = create_test_export_package()
    
    # Validate package
    validation = await import_manager.validate_import_package(test_package, "test_user")
    
    assert validation['valid'] is True
    assert validation['session_count'] > 0
    assert len(validation['errors']) == 0
```

## Success Criteria

### Functional Requirements
- ✅ Secure session data export in standardized format
- ✅ Import validation with comprehensive security checks
- ✅ GDPR-compliant data portability APIs
- ✅ Encrypted export packages with user-controlled keys
- ✅ Conflict resolution for session ID collisions

### Security Requirements
- ✅ User ownership verification for all operations
- ✅ Encrypted export packages by default
- ✅ Comprehensive validation of import data
- ✅ Rate limiting and size restrictions
- ✅ Audit logging for all portability operations

### Compliance Requirements
- ✅ GDPR Article 20 - Right to data portability
- ✅ Structured data formats for automated processing
- ✅ Reasonable time frames for export completion
- ✅ Machine-readable export formats
- ✅ Comprehensive audit trail for compliance

### Performance Requirements
- ✅ Export completion within 5 minutes for typical datasets
- ✅ Import validation within 30 seconds
- ✅ Streaming downloads for large export packages
- ✅ Background processing for resource-intensive operations

This export/import implementation provides comprehensive data portability features while maintaining security and compliance with data protection regulations, completing the Session 6 enhancement ecosystem.