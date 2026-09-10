# Session 10: Terminal Recording System

## Objective
Implement a comprehensive terminal session recording and playback system using asciicast v2 format, providing users with the ability to record, store, replay, and share their terminal sessions while maintaining privacy and security controls.

## Overview
This session creates a complete terminal recording solution that captures terminal sessions in real-time, stores them efficiently, and provides playback capabilities. It implements the asciicast v2 format for compatibility, includes compression and storage optimization, provides privacy filtering, and enables session sharing and export functionality.

## Prerequisites
- Session 1-9 completed successfully
- Terminal session management operational
- WebSocket communication layer functional
- Storage backend available for recordings
- FFmpeg available for video export (optional)

## Components to Implement

### 1. Recording Engine
**Location**: `recording-manager/core/`

#### Terminal Recording System
```python
# recording-manager/core/recording_engine.py
import asyncio
import json
import time
import gzip
from typing import Dict, Any, Optional, List, AsyncGenerator
from dataclasses import dataclass, asdict
from enum import Enum
import structlog

logger = structlog.get_logger()

class RecordingState(Enum):
    IDLE = "idle"
    RECORDING = "recording"
    PAUSED = "paused"
    STOPPED = "stopped"
    PROCESSING = "processing"
    COMPLETED = "completed"
    ERROR = "error"

@dataclass
class RecordingMetadata:
    id: str
    session_id: str
    user_id: str
    title: str
    description: str
    state: RecordingState
    duration_seconds: float
    size_bytes: int
    compressed_size_bytes: int
    terminal_cols: int
    terminal_rows: int
    started_at: float
    ended_at: Optional[float]
    storage_path: str
    is_public: bool
    privacy_filtered: bool
    tags: List[str]
    view_count: int

class TerminalRecordingEngine:
    def __init__(self, storage_backend, privacy_filter):
        self.storage = storage_backend
        self.privacy_filter = privacy_filter
        self.active_recordings: Dict[str, 'RecordingSession'] = {}
        self.max_duration = 3600  # 1 hour max recording
        self.max_size = 100 * 1024 * 1024  # 100MB max
        
    async def start_recording(self, session_id: str, user_id: str,
                            title: str, description: str = "",
                            apply_privacy_filter: bool = True) -> str:
        """Start recording a terminal session"""
        try:
            recording_id = self._generate_recording_id(session_id, user_id)
            
            # Create recording metadata
            metadata = RecordingMetadata(
                id=recording_id,
                session_id=session_id,
                user_id=user_id,
                title=title,
                description=description,
                state=RecordingState.RECORDING,
                duration_seconds=0,
                size_bytes=0,
                compressed_size_bytes=0,
                terminal_cols=80,
                terminal_rows=24,
                started_at=time.time(),
                ended_at=None,
                storage_path="",
                is_public=False,
                privacy_filtered=apply_privacy_filter,
                tags=[],
                view_count=0
            )
            
            # Create recording session
            recording_session = RecordingSession(
                metadata, self.privacy_filter if apply_privacy_filter else None
            )
            
            self.active_recordings[recording_id] = recording_session
            
            logger.info("Recording started", recording_id=recording_id,
                       session_id=session_id, user_id=user_id)
            return recording_id
            
        except Exception as e:
            logger.error("Failed to start recording", session_id=session_id,
                        error=str(e))
            raise
    
    async def add_data(self, recording_id: str, data: bytes, timestamp: float = None):
        """Add terminal data to recording"""
        if recording_id not in self.active_recordings:
            return
        
        recording = self.active_recordings[recording_id]
        if recording.metadata.state != RecordingState.RECORDING:
            return
        
        # Check size limits
        if recording.metadata.size_bytes >= self.max_size:
            await self.stop_recording(recording_id)
            return
        
        # Check duration limits
        if timestamp is None:
            timestamp = time.time()
        
        duration = timestamp - recording.metadata.started_at
        if duration >= self.max_duration:
            await self.stop_recording(recording_id)
            return
        
        await recording.add_data(data, timestamp)
        
    async def stop_recording(self, recording_id: str) -> bool:
        """Stop and finalize recording"""
        try:
            if recording_id not in self.active_recordings:
                return False
            
            recording = self.active_recordings[recording_id]
            recording.metadata.state = RecordingState.PROCESSING
            recording.metadata.ended_at = time.time()
            recording.metadata.duration_seconds = (
                recording.metadata.ended_at - recording.metadata.started_at
            )
            
            # Finalize and store recording
            asyncio.create_task(self._finalize_recording(recording))
            
            logger.info("Recording stopped", recording_id=recording_id,
                       duration=recording.metadata.duration_seconds)
            return True
            
        except Exception as e:
            logger.error("Failed to stop recording", recording_id=recording_id,
                        error=str(e))
            return False
    
    async def _finalize_recording(self, recording: 'RecordingSession'):
        """Finalize and store recording"""
        try:
            # Generate asciicast content
            asciicast_content = await recording.generate_asciicast()
            
            # Compress content
            compressed_content = gzip.compress(asciicast_content.encode('utf-8'))
            
            # Store recording
            storage_path = await self.storage.store_recording(
                recording.metadata.id, compressed_content
            )
            
            # Update metadata
            recording.metadata.storage_path = storage_path
            recording.metadata.size_bytes = len(asciicast_content)
            recording.metadata.compressed_size_bytes = len(compressed_content)
            recording.metadata.state = RecordingState.COMPLETED
            
            # Store metadata
            await self._store_recording_metadata(recording.metadata)
            
            # Cleanup active recording
            self.active_recordings.pop(recording.metadata.id, None)
            
            logger.info("Recording finalized", recording_id=recording.metadata.id)
            
        except Exception as e:
            logger.error("Failed to finalize recording", 
                        recording_id=recording.metadata.id, error=str(e))
            recording.metadata.state = RecordingState.ERROR

class RecordingSession:
    def __init__(self, metadata: RecordingMetadata, privacy_filter=None):
        self.metadata = metadata
        self.privacy_filter = privacy_filter
        self.events: List[Dict[str, Any]] = []
        self.header = {
            "version": 2,
            "width": metadata.terminal_cols,
            "height": metadata.terminal_rows,
            "timestamp": metadata.started_at,
            "title": metadata.title,
            "env": {
                "SHELL": "/bin/bash",
                "TERM": "xterm-256color"
            }
        }
    
    async def add_data(self, data: bytes, timestamp: float):
        """Add terminal data to recording"""
        try:
            # Apply privacy filter if enabled
            if self.privacy_filter:
                filtered_data = await self.privacy_filter.filter_data(data)
            else:
                filtered_data = data
            
            # Calculate relative timestamp
            relative_time = timestamp - self.metadata.started_at
            
            # Create asciicast event
            event = [relative_time, "o", filtered_data.decode('utf-8', errors='replace')]
            self.events.append(event)
            
            # Update metadata
            self.metadata.size_bytes += len(data)
            
        except Exception as e:
            logger.error("Failed to add data to recording", error=str(e))
    
    async def generate_asciicast(self) -> str:
        """Generate asciicast v2 format content"""
        lines = [json.dumps(self.header)]
        lines.extend(json.dumps(event) for event in self.events)
        return '\n'.join(lines)
```

### 2. Privacy Filter
**Location**: `recording-manager/privacy/`

#### Privacy Protection System
```python
# recording-manager/privacy/privacy_filter.py
import re
from typing import List, Pattern
import structlog

logger = structlog.get_logger()

class PrivacyFilter:
    def __init__(self):
        self.patterns = self._load_privacy_patterns()
        self.replacement_text = "[FILTERED]"
    
    def _load_privacy_patterns(self) -> List[Pattern]:
        """Load privacy filter patterns"""
        patterns = [
            # Credit card numbers
            re.compile(r'\b(?:\d{4}[\s-]?){3}\d{4}\b'),
            # Social Security Numbers
            re.compile(r'\b\d{3}-\d{2}-\d{4}\b'),
            # Email addresses (partial filtering)
            re.compile(r'\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b'),
            # Phone numbers
            re.compile(r'\b(?:\+?1[\s.-]?)?\(?\d{3}\)?[\s.-]?\d{3}[\s.-]?\d{4}\b'),
            # IP addresses (private networks)
            re.compile(r'\b(?:10|172\.(?:1[6-9]|2[0-9]|3[01])|192\.168)\.\d{1,3}\.\d{1,3}\b'),
            # API keys and tokens (common patterns)
            re.compile(r'(?:api[_-]?key|token|secret)[\s=:]+[a-zA-Z0-9+/=]{16,}', re.IGNORECASE),
            # AWS keys
            re.compile(r'AKIA[0-9A-Z]{16}'),
            # JWT tokens
            re.compile(r'eyJ[a-zA-Z0-9+/=]+\.[a-zA-Z0-9+/=]+\.[a-zA-Z0-9+/=]+'),
            # Passwords in commands
            re.compile(r'(?:password|passwd|pwd)[\s=:]+\S+', re.IGNORECASE),
        ]
        return patterns
    
    async def filter_data(self, data: bytes) -> bytes:
        """Filter sensitive data from terminal output"""
        try:
            text = data.decode('utf-8', errors='replace')
            filtered_text = self._apply_filters(text)
            return filtered_text.encode('utf-8', errors='replace')
        except Exception as e:
            logger.error("Privacy filter error", error=str(e))
            return data
    
    def _apply_filters(self, text: str) -> str:
        """Apply privacy filters to text"""
        filtered_text = text
        
        for pattern in self.patterns:
            filtered_text = pattern.sub(self.replacement_text, filtered_text)
        
        return filtered_text
```

## Critical Decisions

### Recording Format
- **Decision**: Use asciicast v2 format for maximum compatibility
- **Rationale**: Industry standard, supported by multiple players
- **Storage**: JSON-based format with gzip compression

### Privacy Controls
- **Decision**: Configurable privacy filtering with common sensitive data patterns
- **Rationale**: Protect users from accidentally sharing sensitive information
- **Implementation**: Real-time filtering during recording

### Storage Strategy
- **Decision**: Compressed storage with metadata separation
- **Rationale**: Optimize storage costs while maintaining fast metadata access
- **Format**: Gzipped asciicast files with database metadata

### Sharing Controls
- **Decision**: Private by default with explicit sharing controls
- **Rationale**: Privacy-first approach with user control
- **Features**: Public/private visibility, expiration dates, view tracking

## Security Checklist ✅

### Recording Security
- [ ] Recording ownership validation for all operations
- [ ] Cross-user recording access prevention
- [ ] Session ownership verification before recording
- [ ] Recording enumeration prevention
- [ ] Rate limiting on recording operations (5 concurrent per user)
- [ ] Recording size and duration limits enforced
- [ ] Audit logging for all recording operations
- [ ] Secure recording deletion with data wiping
- [ ] Privacy filter effectiveness validation
- [ ] Recording sharing permission controls

### Data Protection
- [ ] Sensitive data filtering in real-time
- [ ] Encryption of stored recordings
- [ ] Secure transmission of recording data
- [ ] Privacy filter pattern regular updates
- [ ] Recording access logging and monitoring
- [ ] Data retention policy enforcement
- [ ] Secure export functionality
- [ ] Protection against data exfiltration
- [ ] User consent for recording and sharing
- [ ] GDPR compliance for recording data

### Storage Security
- [ ] Recordings encrypted at rest
- [ ] Storage access controls and authentication
- [ ] Recording integrity verification
- [ ] Secure storage path generation
- [ ] Backup encryption and access controls
- [ ] Storage quota enforcement per user
- [ ] Secure cleanup of temporary files
- [ ] Protection against storage enumeration
- [ ] Storage backend access monitoring
- [ ] Disaster recovery security procedures

### Playback Security
- [ ] Recording access validation before playback
- [ ] XSS prevention in playback interface
- [ ] Content Security Policy for playback
- [ ] Rate limiting on playback requests
- [ ] Secure handling of playback sessions
- [ ] Protection against playback abuse
- [ ] View count integrity protection
- [ ] Secure sharing URL generation
- [ ] Playback session timeout enforcement
- [ ] Security headers for playback responses

## Testing Requirements

### Recording Functionality
- [ ] Recording start/stop operations
- [ ] Real-time data capture and storage
- [ ] Recording size and duration limits
- [ ] Concurrent recording handling
- [ ] Recording failure recovery
- [ ] Privacy filter effectiveness
- [ ] Asciicast format validation
- [ ] Compression and storage efficiency

### Playback Functionality
- [ ] Recording retrieval and playback
- [ ] Playback speed controls
- [ ] Seek functionality
- [ ] Full-screen playback
- [ ] Mobile device compatibility
- [ ] Cross-browser playback support
- [ ] Large recording performance
- [ ] Playback error handling

### Privacy and Security
- [ ] Sensitive data filtering validation
- [ ] Cross-user access prevention
- [ ] Recording permission enforcement
- [ ] Sharing controls validation
- [ ] Data encryption verification
- [ ] Audit trail completeness
- [ ] Rate limiting effectiveness
- [ ] Storage security validation

### Integration Testing
- [ ] Terminal session integration
- [ ] WebSocket data capture
- [ ] Storage backend integration
- [ ] Authentication system integration
- [ ] Frontend playback integration
- [ ] API endpoint functionality
- [ ] Error handling across services
- [ ] Performance under load

## Performance Targets

### Recording Performance
- Recording start latency < 500ms
- Real-time data capture with < 10ms delay
- Recording stop and finalization < 5 seconds
- Privacy filtering latency < 1ms per KB
- Compression efficiency > 70% for typical sessions
- Concurrent recordings (10 per user max)

### Playback Performance
- Recording load time < 2 seconds
- Playback start latency < 1 second
- Smooth playback at all speeds (0.5x to 3x)
- Seek operation response < 500ms
- Memory usage < 50MB for 1-hour recording
- Browser compatibility across modern browsers

### Storage Performance
- Recording storage write speed > 10 MB/s
- Recording retrieval speed > 50 MB/s
- Metadata queries < 100ms
- Search operations < 500ms
- Backup operations < 1 hour for 100GB
- Storage space efficiency > 80%

## Documentation Deliverables

### Technical Documentation
- [ ] Recording API specification
- [ ] Asciicast format documentation
- [ ] Privacy filter configuration guide
- [ ] Playback integration guide
- [ ] Storage backend setup
- [ ] Performance optimization guide

### User Documentation
- [ ] Recording usage guide
- [ ] Privacy and sharing controls
- [ ] Playback interface guide
- [ ] Export and download options
- [ ] Troubleshooting common issues
- [ ] Best practices for recording

## Next Steps

Upon successful completion of Session 10:
1. Terminal recording system fully operational
2. Privacy filtering protecting sensitive data
3. Asciicast v2 format providing compatibility
4. Secure storage and playback functionality
5. Integration with terminal sessions working
6. Performance targets met for recording/playback
7. Security measures fully implemented
8. Proceed to Session 11: Monitoring & Observability

## Risk Mitigation

### Technical Risks
1. **Recording failures**: Robust error handling, recovery procedures
2. **Storage corruption**: Checksums, backup verification
3. **Performance issues**: Streaming, compression, optimization
4. **Privacy leaks**: Comprehensive filtering, regular updates
5. **Playback issues**: Format validation, fallback mechanisms

### Security Risks
1. **Data exposure**: Strong privacy filters, access controls
2. **Unauthorized access**: Authentication, authorization validation
3. **Storage compromise**: Encryption, access monitoring
4. **Sharing abuse**: Rate limiting, permission validation
5. **Privacy violations**: Consent management, data protection

---

**Session 10 Success Criteria:**
- Terminal recording system fully functional with real-time capture
- Privacy filtering protecting sensitive data from exposure
- Asciicast v2 format ensuring compatibility and portability
- Secure storage system with encryption and access controls
- Playback system providing excellent user experience
- Security checklist 100% complete with comprehensive protection
- Performance targets achieved for all recording operations
- Integration with Sessions 1-9 working seamlessly
- All tests passing with >80% coverage including security tests
- Documentation complete with user and technical guides
- Ready for Session 11 monitoring and observability implementation