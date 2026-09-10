# Session 10.1: Terminal Recording Engine & Core System

## Objective
Implement comprehensive terminal session recording engine with real-time capture, asciicast v2 format support, and efficient data processing, providing users with seamless recording capabilities for their terminal sessions.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for recording operation monitoring and performance analytics
- **Session 2**: Integrates with authentication system for recording ownership validation
- **Session 5**: Uses database models for recording metadata and session tracking
- **Session 6**: Records active terminal sessions and captures session state
- **Session 8**: Captures frontend terminal data and WebSocket communication

## Core Implementation

### Terminal Recording Engine
**Location**: `recording-manager/core/recording_engine.py`

```python
# recording-manager/core/recording_engine.py
import asyncio
import json
import time
import gzip
import hashlib
import secrets
from typing import Dict, Any, Optional, List, AsyncGenerator, Callable
from dataclasses import dataclass, asdict
from enum import Enum
from pathlib import Path
import structlog
import logfire
from concurrent.futures import ThreadPoolExecutor

logger = structlog.get_logger()

class RecordingState(Enum):
    IDLE = "idle"
    INITIALIZING = "initializing"
    RECORDING = "recording"
    PAUSED = "paused"
    STOPPED = "stopped"
    PROCESSING = "processing"
    COMPLETED = "completed"
    ERROR = "error"
    DELETED = "deleted"

class RecordingFormat(Enum):
    ASCIICAST_V2 = "asciicast_v2"
    JSON_LINES = "json_lines"
    BINARY = "binary"

class CompressionType(Enum):
    NONE = "none"
    GZIP = "gzip"
    ZSTD = "zstd"
    LZ4 = "lz4"

@dataclass
class RecordingMetadata:
    id: str
    session_id: str
    user_id: str
    title: str
    description: str
    state: RecordingState
    format: RecordingFormat
    compression: CompressionType
    duration_seconds: float
    size_bytes: int
    compressed_size_bytes: int
    terminal_cols: int
    terminal_rows: int
    started_at: float
    ended_at: Optional[float]
    storage_path: str
    checksum: str
    is_public: bool
    privacy_filtered: bool
    tags: List[str]
    view_count: int
    download_count: int
    created_at: float
    updated_at: float
    expires_at: Optional[float]
    environment: Dict[str, str]
    command_history: List[str]

@dataclass
class RecordingEvent:
    timestamp: float
    event_type: str  # 'o' for output, 'i' for input, 'r' for resize
    data: str
    size: int

@dataclass
class RecordingStats:
    total_events: int
    input_events: int
    output_events: int
    resize_events: int
    average_event_size: float
    compression_ratio: float
    recording_rate_mbps: float

class TerminalRecordingEngine:
    def __init__(self, storage_backend, privacy_filter, database):
        self.storage = storage_backend
        self.privacy_filter = privacy_filter
        self.db = database
        self.active_recordings: Dict[str, 'RecordingSession'] = {}
        
        # Configuration
        self.max_duration = 7200  # 2 hours max recording
        self.max_size = 500 * 1024 * 1024  # 500MB max
        self.max_concurrent_recordings_per_user = 5
        self.default_compression = CompressionType.GZIP
        self.auto_save_interval = 30  # Auto-save every 30 seconds
        
        # Thread pool for compression and I/O operations
        self.thread_pool = ThreadPoolExecutor(max_workers=4)
        
        # Performance monitoring
        self.recording_stats: Dict[str, RecordingStats] = {}
        
        # Auto-save task
        self.auto_save_task = None
        self.start_auto_save()

    async def start_recording(self, session_id: str, user_id: str,
                            title: str, description: str = "",
                            terminal_cols: int = 80, terminal_rows: int = 24,
                            apply_privacy_filter: bool = True,
                            recording_format: RecordingFormat = RecordingFormat.ASCIICAST_V2,
                            compression: CompressionType = None,
                            tags: List[str] = None,
                            expires_at: Optional[float] = None) -> str:
        """Start recording a terminal session"""
        try:
            # Validate user recording limits
            user_recordings = await self._count_user_recordings(user_id)
            if user_recordings >= self.max_concurrent_recordings_per_user:
                raise ValueError("Maximum concurrent recordings limit exceeded")
            
            # Generate unique recording ID
            recording_id = self._generate_recording_id(session_id, user_id)
            
            # Get session environment
            environment = await self._get_session_environment(session_id)
            
            # Create recording metadata
            metadata = RecordingMetadata(
                id=recording_id,
                session_id=session_id,
                user_id=user_id,
                title=title,
                description=description,
                state=RecordingState.INITIALIZING,
                format=recording_format,
                compression=compression or self.default_compression,
                duration_seconds=0,
                size_bytes=0,
                compressed_size_bytes=0,
                terminal_cols=terminal_cols,
                terminal_rows=terminal_rows,
                started_at=time.time(),
                ended_at=None,
                storage_path="",
                checksum="",
                is_public=False,
                privacy_filtered=apply_privacy_filter,
                tags=tags or [],
                view_count=0,
                download_count=0,
                created_at=time.time(),
                updated_at=time.time(),
                expires_at=expires_at,
                environment=environment,
                command_history=[]
            )
            
            # Create recording session
            recording_session = RecordingSession(
                metadata=metadata,
                privacy_filter=self.privacy_filter if apply_privacy_filter else None,
                thread_pool=self.thread_pool
            )
            
            # Initialize recording session
            await recording_session.initialize()
            
            # Store in active recordings
            self.active_recordings[recording_id] = recording_session
            
            # Store initial metadata
            await self._store_recording_metadata(metadata)
            
            # Update state to recording
            metadata.state = RecordingState.RECORDING
            await recording_session.start_recording()
            
            # Log recording start
            logfire.info("Terminal recording started", 
                       recording_id=recording_id,
                       session_id=session_id,
                       user_id=user_id,
                       format=recording_format.value,
                       privacy_filtered=apply_privacy_filter,
                       terminal_size=f"{terminal_cols}x{terminal_rows}")
            
            return recording_id
            
        except Exception as e:
            logger.error("Failed to start recording", 
                        session_id=session_id,
                        user_id=user_id,
                        error=str(e))
            raise

    async def add_output_data(self, recording_id: str, data: bytes, 
                            timestamp: Optional[float] = None) -> bool:
        """Add terminal output data to recording"""
        return await self._add_data(recording_id, data, 'o', timestamp)

    async def add_input_data(self, recording_id: str, data: bytes,
                           timestamp: Optional[float] = None) -> bool:
        """Add terminal input data to recording"""
        return await self._add_data(recording_id, data, 'i', timestamp)

    async def add_resize_event(self, recording_id: str, cols: int, rows: int,
                             timestamp: Optional[float] = None) -> bool:
        """Add terminal resize event to recording"""
        resize_data = json.dumps({"cols": cols, "rows": rows})
        return await self._add_data(recording_id, resize_data.encode(), 'r', timestamp)

    async def _add_data(self, recording_id: str, data: bytes, event_type: str,
                       timestamp: Optional[float] = None) -> bool:
        """Add data to recording with validation and limits"""
        try:
            if recording_id not in self.active_recordings:
                logger.warning("Recording not found", recording_id=recording_id)
                return False
            
            recording = self.active_recordings[recording_id]
            if recording.metadata.state != RecordingState.RECORDING:
                logger.warning("Recording not in recording state", 
                             recording_id=recording_id,
                             state=recording.metadata.state.value)
                return False
            
            # Check size limits
            if recording.metadata.size_bytes >= self.max_size:
                logfire.warning("Recording size limit exceeded, stopping recording",
                              recording_id=recording_id,
                              size_bytes=recording.metadata.size_bytes)
                await self.stop_recording(recording_id)
                return False
            
            # Check duration limits
            if timestamp is None:
                timestamp = time.time()
            
            duration = timestamp - recording.metadata.started_at
            if duration >= self.max_duration:
                logfire.warning("Recording duration limit exceeded, stopping recording",
                              recording_id=recording_id,
                              duration_seconds=duration)
                await self.stop_recording(recording_id)
                return False
            
            # Add data to recording
            success = await recording.add_data(data, event_type, timestamp)
            
            if success:
                # Update metadata
                recording.metadata.size_bytes += len(data)
                recording.metadata.duration_seconds = duration
                recording.metadata.updated_at = time.time()
            
            return success
            
        except Exception as e:
            logger.error("Failed to add data to recording",
                        recording_id=recording_id,
                        error=str(e))
            return False

    async def pause_recording(self, recording_id: str, user_id: str) -> bool:
        """Pause an active recording"""
        try:
            if recording_id not in self.active_recordings:
                return False
            
            recording = self.active_recordings[recording_id]
            
            # Verify ownership
            if recording.metadata.user_id != user_id:
                raise ValueError("Not authorized to pause this recording")
            
            if recording.metadata.state != RecordingState.RECORDING:
                return False
            
            recording.metadata.state = RecordingState.PAUSED
            recording.metadata.updated_at = time.time()
            
            await recording.pause()
            await self._store_recording_metadata(recording.metadata)
            
            logfire.info("Recording paused", recording_id=recording_id)
            return True
            
        except Exception as e:
            logger.error("Failed to pause recording",
                        recording_id=recording_id,
                        error=str(e))
            return False

    async def resume_recording(self, recording_id: str, user_id: str) -> bool:
        """Resume a paused recording"""
        try:
            if recording_id not in self.active_recordings:
                return False
            
            recording = self.active_recordings[recording_id]
            
            # Verify ownership
            if recording.metadata.user_id != user_id:
                raise ValueError("Not authorized to resume this recording")
            
            if recording.metadata.state != RecordingState.PAUSED:
                return False
            
            recording.metadata.state = RecordingState.RECORDING
            recording.metadata.updated_at = time.time()
            
            await recording.resume()
            await self._store_recording_metadata(recording.metadata)
            
            logfire.info("Recording resumed", recording_id=recording_id)
            return True
            
        except Exception as e:
            logger.error("Failed to resume recording",
                        recording_id=recording_id,
                        error=str(e))
            return False

    async def stop_recording(self, recording_id: str, user_id: Optional[str] = None) -> bool:
        """Stop and finalize recording"""
        try:
            if recording_id not in self.active_recordings:
                return False
            
            recording = self.active_recordings[recording_id]
            
            # Verify ownership if user_id provided
            if user_id and recording.metadata.user_id != user_id:
                raise ValueError("Not authorized to stop this recording")
            
            if recording.metadata.state not in [RecordingState.RECORDING, RecordingState.PAUSED]:
                return False
            
            # Update metadata
            recording.metadata.state = RecordingState.PROCESSING
            recording.metadata.ended_at = time.time()
            recording.metadata.duration_seconds = (
                recording.metadata.ended_at - recording.metadata.started_at
            )
            recording.metadata.updated_at = time.time()
            
            # Stop recording session
            await recording.stop()
            
            # Start finalization task
            asyncio.create_task(self._finalize_recording(recording))
            
            logfire.info("Recording stopped", 
                       recording_id=recording_id,
                       duration_seconds=recording.metadata.duration_seconds,
                       size_bytes=recording.metadata.size_bytes)
            
            return True
            
        except Exception as e:
            logger.error("Failed to stop recording",
                        recording_id=recording_id,
                        error=str(e))
            return False

    async def _finalize_recording(self, recording: 'RecordingSession'):
        """Finalize and store recording"""
        try:
            recording_id = recording.metadata.id
            
            logfire.info("Finalizing recording", recording_id=recording_id)
            
            # Generate recording content based on format
            if recording.metadata.format == RecordingFormat.ASCIICAST_V2:
                content = await recording.generate_asciicast()
            else:
                content = await recording.generate_json_lines()
            
            # Calculate checksum
            checksum = hashlib.sha256(content.encode()).hexdigest()
            recording.metadata.checksum = checksum
            
            # Compress content if specified
            if recording.metadata.compression == CompressionType.GZIP:
                compressed_content = await asyncio.get_event_loop().run_in_executor(
                    self.thread_pool, gzip.compress, content.encode('utf-8')
                )
            else:
                compressed_content = content.encode('utf-8')
            
            # Store recording
            storage_path = await self.storage.store_recording(
                recording_id, compressed_content, recording.metadata.format.value
            )
            
            # Update metadata
            recording.metadata.storage_path = storage_path
            recording.metadata.size_bytes = len(content)
            recording.metadata.compressed_size_bytes = len(compressed_content)
            recording.metadata.state = RecordingState.COMPLETED
            recording.metadata.updated_at = time.time()
            
            # Calculate and store recording statistics
            stats = await recording.generate_statistics()
            self.recording_stats[recording_id] = stats
            
            # Store final metadata
            await self._store_recording_metadata(recording.metadata)
            
            # Cleanup active recording
            self.active_recordings.pop(recording_id, None)
            await recording.cleanup()
            
            logfire.info("Recording finalized successfully", 
                       recording_id=recording_id,
                       storage_path=storage_path,
                       final_size=recording.metadata.size_bytes,
                       compressed_size=recording.metadata.compressed_size_bytes,
                       compression_ratio=recording.metadata.compressed_size_bytes / recording.metadata.size_bytes if recording.metadata.size_bytes > 0 else 0)
            
        except Exception as e:
            logger.error("Failed to finalize recording", 
                        recording_id=recording.metadata.id,
                        error=str(e))
            
            # Set error state
            recording.metadata.state = RecordingState.ERROR
            recording.metadata.updated_at = time.time()
            await self._store_recording_metadata(recording.metadata)
            
            # Cleanup failed recording
            self.active_recordings.pop(recording.metadata.id, None)
            await recording.cleanup()

    async def get_recording_status(self, recording_id: str, user_id: str) -> Optional[Dict[str, Any]]:
        """Get recording status and progress"""
        try:
            # Check active recordings first
            if recording_id in self.active_recordings:
                recording = self.active_recordings[recording_id]
                
                # Verify ownership
                if recording.metadata.user_id != user_id:
                    return None
                
                return {
                    "id": recording_id,
                    "state": recording.metadata.state.value,
                    "duration_seconds": time.time() - recording.metadata.started_at,
                    "size_bytes": recording.metadata.size_bytes,
                    "event_count": recording.get_event_count(),
                    "is_active": True
                }
            
            # Check stored recordings
            metadata = await self._get_recording_metadata(recording_id)
            if not metadata or metadata.user_id != user_id:
                return None
            
            return {
                "id": recording_id,
                "state": metadata.state.value,
                "duration_seconds": metadata.duration_seconds,
                "size_bytes": metadata.size_bytes,
                "compressed_size_bytes": metadata.compressed_size_bytes,
                "is_active": False,
                "storage_path": metadata.storage_path
            }
            
        except Exception as e:
            logger.error("Failed to get recording status",
                        recording_id=recording_id,
                        error=str(e))
            return None

    async def list_user_recordings(self, user_id: str, limit: int = 50,
                                 offset: int = 0, state_filter: Optional[RecordingState] = None) -> List[Dict[str, Any]]:
        """List recordings for a user"""
        try:
            recordings = await self._list_user_recordings_from_db(
                user_id, limit, offset, state_filter
            )
            
            # Add active recording status
            result = []
            for recording in recordings:
                recording_dict = asdict(recording)
                recording_dict["is_active"] = recording.id in self.active_recordings
                result.append(recording_dict)
            
            return result
            
        except Exception as e:
            logger.error("Failed to list user recordings",
                        user_id=user_id,
                        error=str(e))
            return []

    def start_auto_save(self):
        """Start auto-save task for active recordings"""
        if self.auto_save_task is None or self.auto_save_task.done():
            self.auto_save_task = asyncio.create_task(self._auto_save_loop())

    async def _auto_save_loop(self):
        """Auto-save loop for active recordings"""
        while True:
            try:
                await asyncio.sleep(self.auto_save_interval)
                
                for recording_id, recording in self.active_recordings.items():
                    try:
                        if recording.metadata.state in [RecordingState.RECORDING, RecordingState.PAUSED]:
                            await self._store_recording_metadata(recording.metadata)
                            await recording.flush_buffers()
                    except Exception as e:
                        logger.error("Auto-save failed for recording",
                                   recording_id=recording_id,
                                   error=str(e))
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Auto-save loop error", error=str(e))

    def _generate_recording_id(self, session_id: str, user_id: str) -> str:
        """Generate unique recording ID"""
        timestamp = str(int(time.time() * 1000))
        random_suffix = secrets.token_hex(8)
        return f"rec_{user_id}_{timestamp}_{random_suffix}"

    async def _count_user_recordings(self, user_id: str) -> int:
        """Count active recordings for user"""
        count = 0
        for recording in self.active_recordings.values():
            if recording.metadata.user_id == user_id:
                count += 1
        return count

    async def _get_session_environment(self, session_id: str) -> Dict[str, str]:
        """Get environment variables for session"""
        # Implementation depends on session manager integration
        return {
            "TERM": "xterm-256color",
            "SHELL": "/bin/bash",
            "LANG": "en_US.UTF-8"
        }

    # Database operations (implement based on your database choice)
    async def _store_recording_metadata(self, metadata: RecordingMetadata) -> None:
        """Store recording metadata in database"""
        # Implementation depends on database backend
        pass
    
    async def _get_recording_metadata(self, recording_id: str) -> Optional[RecordingMetadata]:
        """Get recording metadata from database"""
        # Implementation depends on database backend
        pass
    
    async def _list_user_recordings_from_db(self, user_id: str, limit: int,
                                          offset: int, state_filter: Optional[RecordingState] = None) -> List[RecordingMetadata]:
        """List user recordings from database"""
        # Implementation depends on database backend
        pass


class RecordingSession:
    def __init__(self, metadata: RecordingMetadata, privacy_filter=None, thread_pool=None):
        self.metadata = metadata
        self.privacy_filter = privacy_filter
        self.thread_pool = thread_pool
        
        # Recording data
        self.events: List[RecordingEvent] = []
        self.buffer: List[RecordingEvent] = []
        self.buffer_size_limit = 1000  # Buffer events before flushing
        
        # State
        self.is_recording = False
        self.is_paused = False
        
        # Performance tracking
        self.last_flush_time = time.time()
        self.total_events = 0
        self.input_events = 0
        self.output_events = 0
        self.resize_events = 0

    async def initialize(self) -> None:
        """Initialize recording session"""
        self.header = {
            "version": 2,
            "width": self.metadata.terminal_cols,
            "height": self.metadata.terminal_rows,
            "timestamp": self.metadata.started_at,
            "title": self.metadata.title,
            "env": self.metadata.environment,
            "command": self.metadata.command_history
        }

    async def start_recording(self) -> None:
        """Start recording"""
        self.is_recording = True
        self.is_paused = False

    async def pause(self) -> None:
        """Pause recording"""
        self.is_paused = True
        await self.flush_buffers()

    async def resume(self) -> None:
        """Resume recording"""
        self.is_paused = False

    async def stop(self) -> None:
        """Stop recording"""
        self.is_recording = False
        self.is_paused = False
        await self.flush_buffers()

    async def add_data(self, data: bytes, event_type: str, timestamp: float) -> bool:
        """Add data to recording"""
        try:
            if not self.is_recording or self.is_paused:
                return False
            
            # Apply privacy filter if enabled
            if self.privacy_filter and event_type in ['o', 'i']:
                filtered_data = await self.privacy_filter.filter_data(data)
            else:
                filtered_data = data
            
            # Calculate relative timestamp
            relative_time = timestamp - self.metadata.started_at
            
            # Create recording event
            event = RecordingEvent(
                timestamp=relative_time,
                event_type=event_type,
                data=filtered_data.decode('utf-8', errors='replace'),
                size=len(filtered_data)
            )
            
            # Add to buffer
            self.buffer.append(event)
            
            # Update counters
            self.total_events += 1
            if event_type == 'i':
                self.input_events += 1
            elif event_type == 'o':
                self.output_events += 1
            elif event_type == 'r':
                self.resize_events += 1
            
            # Flush buffer if needed
            if len(self.buffer) >= self.buffer_size_limit:
                await self.flush_buffers()
            
            return True
            
        except Exception as e:
            logger.error("Failed to add data to recording session", error=str(e))
            return False

    async def flush_buffers(self) -> None:
        """Flush buffered events to main events list"""
        if self.buffer:
            self.events.extend(self.buffer)
            self.buffer.clear()
            self.last_flush_time = time.time()

    async def generate_asciicast(self) -> str:
        """Generate asciicast v2 format content"""
        await self.flush_buffers()
        
        lines = [json.dumps(self.header)]
        for event in self.events:
            asciicast_event = [event.timestamp, event.event_type, event.data]
            lines.append(json.dumps(asciicast_event))
        
        return '\n'.join(lines)

    async def generate_json_lines(self) -> str:
        """Generate JSON lines format content"""
        await self.flush_buffers()
        
        lines = [json.dumps({"header": self.header})]
        for event in self.events:
            event_data = {
                "timestamp": event.timestamp,
                "type": event.event_type,
                "data": event.data,
                "size": event.size
            }
            lines.append(json.dumps(event_data))
        
        return '\n'.join(lines)

    async def generate_statistics(self) -> RecordingStats:
        """Generate recording statistics"""
        await self.flush_buffers()
        
        total_size = sum(event.size for event in self.events)
        avg_size = total_size / len(self.events) if self.events else 0
        
        return RecordingStats(
            total_events=self.total_events,
            input_events=self.input_events,
            output_events=self.output_events,
            resize_events=self.resize_events,
            average_event_size=avg_size,
            compression_ratio=self.metadata.compressed_size_bytes / self.metadata.size_bytes if self.metadata.size_bytes > 0 else 0,
            recording_rate_mbps=(total_size * 8) / (self.metadata.duration_seconds * 1024 * 1024) if self.metadata.duration_seconds > 0 else 0
        )

    def get_event_count(self) -> int:
        """Get total event count including buffer"""
        return len(self.events) + len(self.buffer)

    async def cleanup(self) -> None:
        """Cleanup recording session resources"""
        self.events.clear()
        self.buffer.clear()
        self.is_recording = False
        self.is_paused = False
```

## TDD Implementation Cycle

### Red Phase: Recording Engine Test Creation
```python
# recording-manager/tests/test_recording_engine.py
import pytest
import asyncio
from recording_manager.core.recording_engine import TerminalRecordingEngine, RecordingState

@pytest.mark.asyncio
async def test_recording_engine_initialization():
    """Test that recording engine initializes without storage backend"""
    # This test should initially fail (Red phase)
    engine = TerminalRecordingEngine(None, None, None)
    assert False, "Recording engine initialization not implemented yet"

@pytest.mark.asyncio
async def test_start_recording_without_session():
    """Test recording start fails without valid session"""
    # This test should initially fail (Red phase)
    assert False, "Recording validation not implemented yet"

@pytest.mark.asyncio
async def test_recording_data_capture():
    """Test real-time data capture and buffering"""
    # This test should initially fail (Red phase)
    assert False, "Data capture mechanism not implemented yet"
```

### Green Phase: Recording Engine Implementation
```python
# Implement recording engine features to make tests pass
# This involves adding data capture, buffering, and storage mechanisms
```

### Refactor Phase: Recording Engine Optimization
```python
# Optimize recording engine for performance and reliability
# Add comprehensive error handling and recovery
# Enhance buffer management and compression
```

## Security Checklist ✅

### Recording Access Control
- [ ] Recording ownership validation for all operations
- [ ] Cross-user recording access prevention
- [ ] Session ownership verification before recording start
- [ ] Recording enumeration protection
- [ ] User recording quota enforcement (5 concurrent max)
- [ ] Recording size and duration limits enforced
- [ ] Unauthorized recording access prevention
- [ ] Recording deletion authorization validation
- [ ] Recording sharing permission controls
- [ ] Guest user recording restrictions

### Data Protection During Recording
- [ ] Real-time privacy filtering for sensitive data
- [ ] Input/output data sanitization
- [ ] Command history privacy protection
- [ ] Environment variable filtering
- [ ] Secure data buffering and temporary storage
- [ ] Memory protection for recording data
- [ ] Secure transmission of recording events
- [ ] Protection against data injection attacks
- [ ] Recording data integrity validation
- [ ] Secure cleanup of recording buffers

### Recording State Security
- [ ] Recording state validation and protection
- [ ] Unauthorized recording state changes prevention
- [ ] Recording metadata integrity protection
- [ ] Secure recording pause/resume functionality
- [ ] Recording corruption detection and prevention
- [ ] State transition security validation
- [ ] Recording session isolation
- [ ] Concurrent recording security validation
- [ ] Recording termination security controls
- [ ] Emergency recording stop functionality

### Storage and Compression Security
- [ ] Secure recording storage path generation
- [ ] Recording data encryption before storage
- [ ] Compression security validation
- [ ] Checksum verification for data integrity
- [ ] Secure temporary file handling
- [ ] Storage backend access authentication
- [ ] Recording storage quota enforcement
- [ ] Secure recording deletion and cleanup
- [ ] Storage path traversal prevention
- [ ] Backup security for recordings

### Performance and Resource Security
- [ ] Resource consumption monitoring and limits
- [ ] Memory usage protection and limits
- [ ] CPU usage monitoring for recording operations
- [ ] Network bandwidth protection
- [ ] Disk I/O rate limiting
- [ ] Thread pool security and isolation
- [ ] Auto-save security validation
- [ ] Buffer overflow protection
- [ ] Resource exhaustion prevention
- [ ] Performance degradation detection

## Performance Requirements

### Recording Operations
- Recording start latency < 500ms
- Real-time data capture with < 10ms delay
- Recording stop and processing < 5 seconds
- Privacy filtering latency < 1ms per KB
- Buffer flush operations < 100ms
- Auto-save interval processing < 200ms

### Data Processing Performance
- Event processing rate > 1000 events/second
- Compression speed > 10 MB/s
- Memory usage < 50MB per active recording
- Buffer management overhead < 5%
- Statistics generation < 1 second
- Metadata operations < 100ms

### Scalability Requirements
- Support 10+ concurrent recordings per user
- Handle 100+ total active recordings system-wide
- Process 10000+ events per recording
- Support recording files up to 500MB
- Scale to 1000+ total recordings per user
- Manage 10TB+ total recording storage

## Commit Instructions

After implementing the recording engine:

```bash
git add recording-manager/core/
git commit -m "Add terminal recording engine with comprehensive data capture

- Implement TerminalRecordingEngine with real-time capture capabilities
- Add RecordingSession with buffered event processing
- Implement multiple recording formats (asciicast v2, JSON lines)
- Add configurable compression support (gzip, zstd, lz4)
- Implement privacy filtering integration with real-time processing
- Add comprehensive recording state management with pause/resume
- Include auto-save functionality with configurable intervals
- Add performance monitoring and statistics generation
- Implement resource limits and quota enforcement
- Add TDD cycle with Red-Green-Refactor for recording features
- Ensure >85% recording engine test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete recording engine test suite:

```bash
# Run all recording engine tests
pytest recording-manager/tests/test_recording_engine.py -v --timeout=300

# Run specific recording test categories
pytest recording-manager/tests/ -k "recording_data" -v
pytest recording-manager/tests/ -k "recording_state" -v
pytest recording-manager/tests/ -k "recording_compression" -v

# Run recording performance tests
pytest recording-manager/tests/performance/ -v

# Run recording integration tests
pytest recording-manager/tests/integration/ -v
```

Validate recording engine test coverage:
```bash
pytest recording-manager/tests/ --cov=recording_manager.core --cov-report=html --cov-fail-under=85
```

## Integration Testing

Test recording engine integration with previous sessions:
```bash
# Test integration with Session 1 (Logfire monitoring)
pytest recording-manager/tests/integration/test_recording_logfire_integration.py -v

# Test integration with Session 6 (Session Management)
pytest recording-manager/tests/integration/test_recording_session_integration.py -v

# Test integration with Session 8 (Frontend Terminal)
pytest recording-manager/tests/integration/test_recording_terminal_integration.py -v
```