# Session 10.3: Recording Playback System & Player Interface

## Objective
Implement comprehensive terminal recording playback system with interactive player interface, supporting multiple formats, advanced playback controls, and seamless integration with the frontend terminal component.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for playback performance monitoring and user analytics
- **Session 2**: Integrates with authentication system for recording access control
- **Session 7**: Uses storage backend for efficient recording retrieval
- **Session 8**: Integrates with frontend terminal for seamless playback rendering
- **Session 10.1**: Plays recordings created by the recording engine
- **Session 10.2**: Displays privacy-filtered content safely

## Core Implementation

### Playback Engine
**Location**: `recording-manager/playback/playback_engine.py`

```python
# recording-manager/playback/playback_engine.py
import asyncio
import json
import gzip
import time
import zlib
from typing import Dict, Any, Optional, List, AsyncGenerator, Callable
from dataclasses import dataclass, asdict
from enum import Enum
import structlog
import logfire
from concurrent.futures import ThreadPoolExecutor

logger = structlog.get_logger()

class PlaybackState(Enum):
    IDLE = "idle"
    LOADING = "loading"
    READY = "ready"
    PLAYING = "playing"
    PAUSED = "paused"
    SEEKING = "seeking"
    ENDED = "ended"
    ERROR = "error"

class PlaybackSpeed(Enum):
    SLOW_0_25X = 0.25
    SLOW_0_5X = 0.5
    NORMAL_1X = 1.0
    FAST_1_5X = 1.5
    FAST_2X = 2.0
    FAST_3X = 3.0
    FAST_5X = 5.0
    FAST_10X = 10.0

@dataclass
class PlaybackEvent:
    timestamp: float
    event_type: str
    data: str
    original_timestamp: float

@dataclass
class PlaybackSession:
    id: str
    recording_id: str
    user_id: str
    state: PlaybackState
    current_position: float
    duration: float
    speed: float
    created_at: float
    last_activity: float
    events_loaded: int
    terminal_cols: int
    terminal_rows: int
    auto_play: bool
    loop_enabled: bool

@dataclass
class PlaybackStats:
    session_id: str
    recording_id: str
    user_id: str
    total_events_played: int
    total_seeks: int
    total_pauses: int
    playback_duration_seconds: float
    average_speed: float
    completion_percentage: float
    started_at: float
    last_activity: float

class RecordingPlaybackEngine:
    def __init__(self, storage_backend, database):
        self.storage = storage_backend
        self.db = database
        self.active_sessions: Dict[str, 'PlaybackSession'] = {}
        self.loaded_recordings: Dict[str, 'LoadedRecording'] = {}
        
        # Configuration
        self.max_concurrent_sessions_per_user = 3
        self.recording_cache_size = 10
        self.playback_buffer_size = 1000  # Events to buffer ahead
        self.seek_precision_ms = 100  # Seek precision in milliseconds
        
        # Thread pool for I/O operations
        self.thread_pool = ThreadPoolExecutor(max_workers=4)
        
        # Performance monitoring
        self.playback_stats: Dict[str, PlaybackStats] = {}
        
        # Cleanup task
        self.cleanup_task = asyncio.create_task(self._cleanup_loop())

    async def create_playback_session(self, recording_id: str, user_id: str,
                                    auto_play: bool = False, 
                                    loop_enabled: bool = False) -> str:
        """Create a new playback session"""
        try:
            # Validate user access to recording
            if not await self._validate_recording_access(recording_id, user_id):
                raise ValueError("Access denied to recording")
            
            # Check concurrent session limits
            user_sessions = await self._count_user_sessions(user_id)
            if user_sessions >= self.max_concurrent_sessions_per_user:
                raise ValueError("Maximum concurrent playback sessions exceeded")
            
            # Load recording if not already loaded
            recording = await self._load_recording(recording_id)
            if not recording:
                raise ValueError("Recording not found or failed to load")
            
            # Generate session ID
            session_id = self._generate_session_id(recording_id, user_id)
            
            # Create playback session
            session = PlaybackSession(
                id=session_id,
                recording_id=recording_id,
                user_id=user_id,
                state=PlaybackState.READY,
                current_position=0.0,
                duration=recording.duration,
                speed=1.0,
                created_at=time.time(),
                last_activity=time.time(),
                events_loaded=len(recording.events),
                terminal_cols=recording.terminal_cols,
                terminal_rows=recording.terminal_rows,
                auto_play=auto_play,
                loop_enabled=loop_enabled
            )
            
            # Store session
            self.active_sessions[session_id] = session
            
            # Initialize playback stats
            self.playback_stats[session_id] = PlaybackStats(
                session_id=session_id,
                recording_id=recording_id,
                user_id=user_id,
                total_events_played=0,
                total_seeks=0,
                total_pauses=0,
                playback_duration_seconds=0,
                average_speed=1.0,
                completion_percentage=0,
                started_at=time.time(),
                last_activity=time.time()
            )
            
            # Auto-play if requested
            if auto_play:
                await self.play(session_id)
            
            logfire.info("Playback session created",
                       session_id=session_id,
                       recording_id=recording_id,
                       user_id=user_id,
                       duration=recording.duration)
            
            return session_id
            
        except Exception as e:
            logger.error("Failed to create playback session",
                        recording_id=recording_id,
                        user_id=user_id,
                        error=str(e))
            raise

    async def play(self, session_id: str) -> bool:
        """Start or resume playback"""
        try:
            if session_id not in self.active_sessions:
                return False
            
            session = self.active_sessions[session_id]
            recording = self.loaded_recordings.get(session.recording_id)
            
            if not recording or session.state not in [PlaybackState.READY, PlaybackState.PAUSED]:
                return False
            
            session.state = PlaybackState.PLAYING
            session.last_activity = time.time()
            
            # Start playback task
            asyncio.create_task(self._playback_loop(session_id))
            
            logfire.info("Playback started",
                       session_id=session_id,
                       position=session.current_position,
                       speed=session.speed)
            
            return True
            
        except Exception as e:
            logger.error("Failed to start playback",
                        session_id=session_id,
                        error=str(e))
            return False

    async def pause(self, session_id: str) -> bool:
        """Pause playback"""
        try:
            if session_id not in self.active_sessions:
                return False
            
            session = self.active_sessions[session_id]
            
            if session.state != PlaybackState.PLAYING:
                return False
            
            session.state = PlaybackState.PAUSED
            session.last_activity = time.time()
            
            # Update stats
            stats = self.playback_stats.get(session_id)
            if stats:
                stats.total_pauses += 1
                stats.last_activity = time.time()
            
            logfire.info("Playback paused",
                       session_id=session_id,
                       position=session.current_position)
            
            return True
            
        except Exception as e:
            logger.error("Failed to pause playback",
                        session_id=session_id,
                        error=str(e))
            return False

    async def seek(self, session_id: str, position: float) -> bool:
        """Seek to specific position in recording"""
        try:
            if session_id not in self.active_sessions:
                return False
            
            session = self.active_sessions[session_id]
            recording = self.loaded_recordings.get(session.recording_id)
            
            if not recording:
                return False
            
            # Validate position
            if position < 0:
                position = 0
            elif position > session.duration:
                position = session.duration
            
            # Update session state
            was_playing = session.state == PlaybackState.PLAYING
            session.state = PlaybackState.SEEKING
            session.current_position = position
            session.last_activity = time.time()
            
            # Find events at target position
            await self._prepare_playback_from_position(session_id, position)
            
            # Resume appropriate state
            if was_playing:
                session.state = PlaybackState.PLAYING
            else:
                session.state = PlaybackState.PAUSED
            
            # Update stats
            stats = self.playback_stats.get(session_id)
            if stats:
                stats.total_seeks += 1
                stats.completion_percentage = (position / session.duration) * 100 if session.duration > 0 else 0
                stats.last_activity = time.time()
            
            logfire.info("Playback seek completed",
                       session_id=session_id,
                       position=position,
                       duration=session.duration)
            
            return True
            
        except Exception as e:
            logger.error("Failed to seek playback",
                        session_id=session_id,
                        position=position,
                        error=str(e))
            return False

    async def set_speed(self, session_id: str, speed: float) -> bool:
        """Set playback speed"""
        try:
            if session_id not in self.active_sessions:
                return False
            
            session = self.active_sessions[session_id]
            
            # Validate speed
            valid_speeds = [s.value for s in PlaybackSpeed]
            if speed not in valid_speeds:
                return False
            
            session.speed = speed
            session.last_activity = time.time()
            
            # Update average speed in stats
            stats = self.playback_stats.get(session_id)
            if stats:
                # Calculate running average
                total_duration = time.time() - stats.started_at
                stats.average_speed = ((stats.average_speed * stats.playback_duration_seconds) + 
                                     (speed * total_duration)) / (stats.playback_duration_seconds + total_duration)
                stats.last_activity = time.time()
            
            logfire.info("Playback speed changed",
                       session_id=session_id,
                       speed=speed)
            
            return True
            
        except Exception as e:
            logger.error("Failed to set playback speed",
                        session_id=session_id,
                        speed=speed,
                        error=str(e))
            return False

    async def stop(self, session_id: str) -> bool:
        """Stop playback and cleanup session"""
        try:
            if session_id not in self.active_sessions:
                return False
            
            session = self.active_sessions[session_id]
            session.state = PlaybackState.ENDED
            session.last_activity = time.time()
            
            # Finalize stats
            stats = self.playback_stats.get(session_id)
            if stats:
                stats.playback_duration_seconds = time.time() - stats.started_at
                stats.completion_percentage = (session.current_position / session.duration) * 100 if session.duration > 0 else 0
                stats.last_activity = time.time()
                
                # Store final stats
                await self._store_playback_stats(stats)
            
            # Cleanup session
            self.active_sessions.pop(session_id, None)
            self.playback_stats.pop(session_id, None)
            
            logfire.info("Playback stopped and session cleaned up",
                       session_id=session_id)
            
            return True
            
        except Exception as e:
            logger.error("Failed to stop playback",
                        session_id=session_id,
                        error=str(e))
            return False

    async def get_session_info(self, session_id: str) -> Optional[Dict[str, Any]]:
        """Get playback session information"""
        if session_id not in self.active_sessions:
            return None
        
        session = self.active_sessions[session_id]
        stats = self.playback_stats.get(session_id)
        
        return {
            "session_id": session_id,
            "recording_id": session.recording_id,
            "state": session.state.value,
            "current_position": session.current_position,
            "duration": session.duration,
            "speed": session.speed,
            "completion_percentage": (session.current_position / session.duration) * 100 if session.duration > 0 else 0,
            "terminal_size": {
                "cols": session.terminal_cols,
                "rows": session.terminal_rows
            },
            "auto_play": session.auto_play,
            "loop_enabled": session.loop_enabled,
            "events_loaded": session.events_loaded,
            "stats": asdict(stats) if stats else None
        }

    async def _playback_loop(self, session_id: str):
        """Main playback loop"""
        try:
            session = self.active_sessions.get(session_id)
            recording = self.loaded_recordings.get(session.recording_id) if session else None
            
            if not session or not recording:
                return
            
            # Find starting event index
            event_index = await self._find_event_index_at_position(recording, session.current_position)
            last_event_time = time.time()
            
            while (session.state == PlaybackState.PLAYING and 
                   event_index < len(recording.events) and
                   session_id in self.active_sessions):
                
                event = recording.events[event_index]
                
                # Calculate delay until next event
                if event_index > 0:
                    time_diff = event.timestamp - recording.events[event_index - 1].timestamp
                    delay = time_diff / session.speed
                    
                    # Sleep for appropriate delay
                    if delay > 0:
                        await asyncio.sleep(delay)
                
                # Check if session still exists and is playing
                session = self.active_sessions.get(session_id)
                if not session or session.state != PlaybackState.PLAYING:
                    break
                
                # Update position
                session.current_position = event.timestamp
                session.last_activity = time.time()
                
                # Send event to client
                await self._send_playback_event(session_id, event)
                
                # Update stats
                stats = self.playback_stats.get(session_id)
                if stats:
                    stats.total_events_played += 1
                    stats.last_activity = time.time()
                
                event_index += 1
            
            # Handle playback completion
            if session and event_index >= len(recording.events):
                if session.loop_enabled:
                    # Restart from beginning
                    session.current_position = 0.0
                    asyncio.create_task(self._playback_loop(session_id))
                else:
                    # End playback
                    session.state = PlaybackState.ENDED
                    await self._send_playback_event(session_id, {"type": "playback_ended"})
            
        except Exception as e:
            logger.error("Playback loop error",
                        session_id=session_id,
                        error=str(e))
            
            # Set error state
            session = self.active_sessions.get(session_id)
            if session:
                session.state = PlaybackState.ERROR

    async def _load_recording(self, recording_id: str) -> Optional['LoadedRecording']:
        """Load recording from storage"""
        try:
            # Check cache first
            if recording_id in self.loaded_recordings:
                return self.loaded_recordings[recording_id]
            
            # Get recording metadata
            metadata = await self._get_recording_metadata(recording_id)
            if not metadata:
                return None
            
            # Load recording content
            content = await self.storage.load_recording(recording_id, metadata.storage_path)
            if not content:
                return None
            
            # Decompress if needed
            if metadata.compression.value == "gzip":
                content = await asyncio.get_event_loop().run_in_executor(
                    self.thread_pool, gzip.decompress, content
                )
                content = content.decode('utf-8')
            else:
                content = content.decode('utf-8')
            
            # Parse recording based on format
            if metadata.format.value == "asciicast_v2":
                recording = await self._parse_asciicast(content, metadata)
            else:
                recording = await self._parse_json_lines(content, metadata)
            
            # Cache recording
            self.loaded_recordings[recording_id] = recording
            
            # Manage cache size
            if len(self.loaded_recordings) > self.recording_cache_size:
                # Remove oldest recording
                oldest_id = min(self.loaded_recordings.keys(), 
                              key=lambda k: self.loaded_recordings[k].loaded_at)
                self.loaded_recordings.pop(oldest_id, None)
            
            logfire.info("Recording loaded successfully",
                       recording_id=recording_id,
                       events_count=len(recording.events),
                       duration=recording.duration)
            
            return recording
            
        except Exception as e:
            logger.error("Failed to load recording",
                        recording_id=recording_id,
                        error=str(e))
            return None

    async def _parse_asciicast(self, content: str, metadata) -> 'LoadedRecording':
        """Parse asciicast v2 format"""
        lines = content.strip().split('\n')
        
        # Parse header
        header = json.loads(lines[0])
        
        # Parse events
        events = []
        for line in lines[1:]:
            if line.strip():
                event_data = json.loads(line)
                event = PlaybackEvent(
                    timestamp=event_data[0],
                    event_type=event_data[1],
                    data=event_data[2],
                    original_timestamp=event_data[0]
                )
                events.append(event)
        
        # Calculate duration
        duration = events[-1].timestamp if events else 0
        
        return LoadedRecording(
            recording_id=metadata.id,
            events=events,
            header=header,
            duration=duration,
            terminal_cols=header.get('width', 80),
            terminal_rows=header.get('height', 24),
            loaded_at=time.time()
        )

    async def _send_playback_event(self, session_id: str, event):
        """Send playback event to client"""
        # This would integrate with WebSocket system
        # Implementation depends on WebSocket integration
        pass

    async def _cleanup_loop(self):
        """Cleanup inactive sessions"""
        while True:
            try:
                await asyncio.sleep(300)  # Run every 5 minutes
                
                current_time = time.time()
                inactive_threshold = 3600  # 1 hour
                
                inactive_sessions = []
                for session_id, session in self.active_sessions.items():
                    if current_time - session.last_activity > inactive_threshold:
                        inactive_sessions.append(session_id)
                
                for session_id in inactive_sessions:
                    await self.stop(session_id)
                    logger.info("Cleaned up inactive playback session",
                              session_id=session_id)
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Cleanup loop error", error=str(e))

    # Helper methods and database operations
    async def _validate_recording_access(self, recording_id: str, user_id: str) -> bool:
        """Validate user access to recording"""
        # Implementation depends on database backend
        pass
    
    async def _count_user_sessions(self, user_id: str) -> int:
        """Count active sessions for user"""
        count = 0
        for session in self.active_sessions.values():
            if session.user_id == user_id:
                count += 1
        return count
    
    def _generate_session_id(self, recording_id: str, user_id: str) -> str:
        """Generate unique session ID"""
        import secrets
        timestamp = str(int(time.time() * 1000))
        random_suffix = secrets.token_hex(8)
        return f"playback_{user_id}_{timestamp}_{random_suffix}"

    async def _get_recording_metadata(self, recording_id: str):
        """Get recording metadata from database"""
        # Implementation depends on database backend
        pass

    async def _store_playback_stats(self, stats: PlaybackStats) -> None:
        """Store playback statistics"""
        # Implementation depends on database backend
        pass


@dataclass
class LoadedRecording:
    recording_id: str
    events: List[PlaybackEvent]
    header: Dict[str, Any]
    duration: float
    terminal_cols: int
    terminal_rows: int
    loaded_at: float
```

### Playback API Endpoints
**Location**: `api/endpoints/playback.py`

```python
# api/endpoints/playback.py
from fastapi import APIRouter, HTTPException, Depends, status, WebSocket, WebSocketDisconnect
from typing import Optional
from pydantic import BaseModel
import structlog
import logfire
import json

from ..auth import get_current_user
from ..dependencies import get_playback_engine

logger = structlog.get_logger()
router = APIRouter(prefix="/api/v1/playback", tags=["playback"])

class CreatePlaybackSessionRequest(BaseModel):
    recording_id: str
    auto_play: bool = False
    loop_enabled: bool = False

class PlaybackControlRequest(BaseModel):
    action: str  # play, pause, stop, seek, speed
    position: Optional[float] = None
    speed: Optional[float] = None

@router.post("/sessions", status_code=status.HTTP_201_CREATED)
async def create_playback_session(
    request: CreatePlaybackSessionRequest,
    current_user = Depends(get_current_user),
    playback_engine = Depends(get_playback_engine)
):
    """Create a new playback session"""
    try:
        session_id = await playback_engine.create_playback_session(
            recording_id=request.recording_id,
            user_id=current_user.id,
            auto_play=request.auto_play,
            loop_enabled=request.loop_enabled
        )
        
        logfire.info("Playback session created via API",
                   session_id=session_id,
                   recording_id=request.recording_id,
                   user_id=current_user.id)
        
        return {
            "session_id": session_id,
            "message": "Playback session created successfully"
        }
        
    except Exception as e:
        logger.error("Failed to create playback session via API", error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to create playback session"
        )

@router.post("/sessions/{session_id}/control")
async def control_playback(
    session_id: str,
    request: PlaybackControlRequest,
    current_user = Depends(get_current_user),
    playback_engine = Depends(get_playback_engine)
):
    """Control playback session"""
    try:
        success = False
        
        if request.action == "play":
            success = await playback_engine.play(session_id)
        elif request.action == "pause":
            success = await playback_engine.pause(session_id)
        elif request.action == "stop":
            success = await playback_engine.stop(session_id)
        elif request.action == "seek" and request.position is not None:
            success = await playback_engine.seek(session_id, request.position)
        elif request.action == "speed" and request.speed is not None:
            success = await playback_engine.set_speed(session_id, request.speed)
        else:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Invalid action or missing parameters"
            )
        
        if not success:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Failed to execute playback action"
            )
        
        return {"message": f"Playback {request.action} executed successfully"}
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Failed to control playback via API", error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to control playback"
        )

@router.get("/sessions/{session_id}")
async def get_playback_session(
    session_id: str,
    current_user = Depends(get_current_user),
    playback_engine = Depends(get_playback_engine)
):
    """Get playback session information"""
    try:
        session_info = await playback_engine.get_session_info(session_id)
        
        if not session_info:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="Playback session not found"
            )
        
        return session_info
        
    except HTTPException:
        raise
    except Exception as e:
        logger.error("Failed to get playback session via API", error=str(e))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to get playback session"
        )

@router.websocket("/sessions/{session_id}/stream")
async def playback_stream(
    websocket: WebSocket,
    session_id: str,
    playback_engine = Depends(get_playback_engine)
):
    """WebSocket endpoint for real-time playback streaming"""
    await websocket.accept()
    
    try:
        # Validate session exists
        session_info = await playback_engine.get_session_info(session_id)
        if not session_info:
            await websocket.close(code=4004, reason="Session not found")
            return
        
        # Set up playback event streaming
        # This would integrate with the playback engine's event system
        while True:
            try:
                # Receive control messages from client
                message = await websocket.receive_text()
                control_data = json.loads(message)
                
                action = control_data.get("action")
                if action == "play":
                    await playback_engine.play(session_id)
                elif action == "pause":
                    await playback_engine.pause(session_id)
                elif action == "seek":
                    position = control_data.get("position", 0)
                    await playback_engine.seek(session_id, position)
                elif action == "speed":
                    speed = control_data.get("speed", 1.0)
                    await playback_engine.set_speed(session_id, speed)
                
                # Send acknowledgment
                await websocket.send_text(json.dumps({
                    "type": "control_ack",
                    "action": action,
                    "success": True
                }))
                
            except WebSocketDisconnect:
                break
            except Exception as e:
                logger.error("WebSocket playback error", error=str(e))
                await websocket.send_text(json.dumps({
                    "type": "error",
                    "message": "Playback error occurred"
                }))
    
    except Exception as e:
        logger.error("WebSocket playback connection error", error=str(e))
    finally:
        # Cleanup session if needed
        try:
            await playback_engine.stop(session_id)
        except:
            pass
```

## TDD Implementation Cycle

### Red Phase: Playback System Test Creation
```python
# recording-manager/tests/test_playback_engine.py
import pytest
import asyncio
from recording_manager.playback.playback_engine import RecordingPlaybackEngine, PlaybackState

@pytest.mark.asyncio
async def test_playback_engine_initialization():
    """Test playback engine initializes correctly"""
    # This test should initially fail (Red phase)
    engine = RecordingPlaybackEngine(None, None)
    assert False, "Playback engine initialization not implemented yet"

@pytest.mark.asyncio
async def test_create_playback_session():
    """Test playback session creation"""
    # This test should initially fail (Red phase)
    assert False, "Playback session creation not implemented yet"

@pytest.mark.asyncio
async def test_playback_controls():
    """Test play, pause, seek, and speed controls"""
    # This test should initially fail (Red phase)
    assert False, "Playback controls not implemented yet"
```

### Green Phase: Playback System Implementation
```python
# Implement playback system features to make tests pass
# This involves adding session management, playback controls, and event streaming
```

### Refactor Phase: Playback System Optimization
```python
# Optimize playback system for performance and user experience
# Add comprehensive buffering and caching mechanisms
# Enhance playback controls and precision
```

## Security Checklist ✅

### Playback Session Security
- [ ] Playback session ownership and access validation
- [ ] Cross-user session access prevention
- [ ] Session enumeration protection
- [ ] Unauthorized playback prevention
- [ ] Session hijacking protection
- [ ] Playback rate limiting per user
- [ ] Session timeout and cleanup enforcement
- [ ] Concurrent session limits enforcement
- [ ] Session state manipulation prevention
- [ ] Playback authorization audit logging

### Recording Access Control
- [ ] Recording access permission validation for playback
- [ ] Cross-user recording access prevention
- [ ] Private recording protection
- [ ] Recording sharing permission enforcement
- [ ] Recording visibility controls validation
- [ ] Expired recording access prevention
- [ ] Deleted recording playback prevention
- [ ] Recording metadata security validation
- [ ] Access audit logging for playback events
- [ ] Recording enumeration prevention through playback

### Playback Data Security
- [ ] Secure recording content transmission
- [ ] Playback data encryption in transit
- [ ] WebSocket connection security validation
- [ ] Event streaming data integrity protection
- [ ] Client-side data handling security
- [ ] Playback buffer security and cleanup
- [ ] Temporary data security during playback
- [ ] Memory protection for playback content
- [ ] Secure event serialization and transmission
- [ ] Protection against data injection in playback events

### Performance and Resource Security
- [ ] Playback resource consumption monitoring and limits
- [ ] Memory usage protection during playback
- [ ] CPU usage monitoring for playback operations
- [ ] Network bandwidth protection for streaming
- [ ] Disk I/O monitoring during recording loading
- [ ] Thread pool security and isolation
- [ ] Playback loop resource management
- [ ] Cache security and memory protection
- [ ] Resource exhaustion prevention
- [ ] Performance degradation detection and alerting

### WebSocket Security
- [ ] WebSocket authentication and authorization
- [ ] WebSocket connection rate limiting
- [ ] Message validation for playback controls
- [ ] WebSocket message injection prevention
- [ ] Connection state security validation
- [ ] WebSocket protocol security compliance
- [ ] Cross-origin request security for WebSocket
- [ ] WebSocket message size limits
- [ ] Connection timeout enforcement
- [ ] Secure WebSocket cleanup and resource management

## Performance Requirements

### Playback Performance
- Playback session creation < 2 seconds
- Playback start latency < 1 second
- Seek operation response time < 500ms
- Speed change response time < 200ms
- Event streaming latency < 50ms
- Recording loading time < 5 seconds for 1-hour recordings

### Streaming Performance
- WebSocket message throughput > 1000 messages/second
- Event buffering latency < 10ms
- Playback synchronization accuracy ± 50ms
- Memory usage < 100MB per active session
- Network bandwidth efficiency > 90%
- Frame rate consistency > 95% for all playback speeds

### Scalability Requirements
- Support 50+ concurrent playback sessions per server
- Handle 500+ WebSocket connections simultaneously
- Support recordings up to 2 hours duration
- Cache 10+ recordings simultaneously (up to 1GB total)
- Scale to 1000+ users with personal playback sessions
- Support playback of 100GB+ total recording storage

## Commit Instructions

After implementing the playback system:

```bash
git add recording-manager/playback/
git add api/endpoints/playback.py
git commit -m "Add comprehensive recording playback system with interactive controls

- Implement RecordingPlaybackEngine with full session management
- Add playback controls (play, pause, seek, speed, loop)
- Implement LoadedRecording with efficient event processing
- Add WebSocket streaming for real-time playback events
- Implement recording caching and memory management
- Add comprehensive playback statistics and monitoring
- Include playback API endpoints with RESTful interface
- Add session cleanup and resource management
- Implement multiple playback speeds and precision seeking
- Add TDD cycle with Red-Green-Refactor for playback features
- Ensure >85% playback system test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete playback system test suite:

```bash
# Run all playback tests
pytest recording-manager/tests/test_playback_engine.py -v --timeout=300

# Run specific playback test categories
pytest recording-manager/tests/playback/ -k "session_management" -v
pytest recording-manager/tests/playback/ -k "playback_controls" -v
pytest recording-manager/tests/playback/ -k "streaming" -v

# Run playback API tests
pytest api/tests/test_playback_endpoints.py -v

# Run playback performance tests
pytest recording-manager/tests/playback/performance/ -v
```

Validate playback system test coverage:
```bash
pytest recording-manager/tests/playback/ --cov=recording_manager.playback --cov-report=html --cov-fail-under=85
```

## Integration Testing

Test playback system integration with other components:
```bash
# Test integration with Session 8 (Frontend Terminal)
pytest recording-manager/tests/integration/test_playback_terminal_integration.py -v

# Test integration with Session 10.1 (Recording Engine)
pytest recording-manager/tests/integration/test_playback_recording_integration.py -v

# Test WebSocket streaming integration
pytest recording-manager/tests/integration/test_playback_websocket_integration.py -v
```