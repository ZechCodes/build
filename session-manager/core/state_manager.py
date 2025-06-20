"""
Session State Manager

Manages terminal session lifecycle, state persistence, and recovery.
Provides Redis-backed session storage with automatic cleanup.
"""
import asyncio
import json
import time
import uuid
from typing import Dict, Any, Optional, List
from dataclasses import dataclass, asdict
from enum import Enum
import redis.asyncio as redis
import structlog
import logfire

logger = structlog.get_logger()


class SessionState(Enum):
    """Session state enumeration"""
    INITIALIZING = "initializing"
    ACTIVE = "active"
    IDLE = "idle"
    SUSPENDED = "suspended"
    TERMINATED = "terminated"
    ERROR = "error"


@dataclass
class SessionContext:
    """Session context data structure"""
    session_id: str
    user_id: str
    vm_id: str
    state: SessionState
    created_at: float
    last_activity: float
    terminal_size: tuple[int, int]
    environment_vars: Dict[str, str]
    working_directory: str
    active_processes: List[Dict[str, Any]]
    metadata: Dict[str, Any]

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary for serialization"""
        data = asdict(self)
        data["state"] = self.state.value
        data["terminal_size"] = json.dumps(self.terminal_size)
        data["environment_vars"] = json.dumps(self.environment_vars)
        data["active_processes"] = json.dumps(self.active_processes)
        data["metadata"] = json.dumps(self.metadata)
        return data

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "SessionContext":
        """Create from dictionary"""
        return cls(
            session_id=data["session_id"],
            user_id=data["user_id"],
            vm_id=data["vm_id"],
            state=SessionState(data["state"]),
            created_at=float(data["created_at"]),
            last_activity=float(data["last_activity"]),
            terminal_size=tuple(json.loads(data["terminal_size"])),
            environment_vars=json.loads(data["environment_vars"]),
            working_directory=data["working_directory"],
            active_processes=json.loads(data["active_processes"]),
            metadata=json.loads(data["metadata"])
        )


class SessionStateManager:
    """Manages session state with Redis persistence"""
    
    def __init__(self, redis_client: redis.Redis):
        self.redis = redis_client
        self.sessions: Dict[str, SessionContext] = {}
        self.state_key_prefix = "session:state:"
        self.activity_key_prefix = "session:activity:"
        self.user_index_prefix = "session:user:"
        self.cleanup_task: Optional[asyncio.Task] = None
        self.cleanup_interval = 300  # 5 minutes
        
    async def initialize(self):
        """Initialize the session state manager"""
        # Start cleanup task for expired sessions
        self.cleanup_task = asyncio.create_task(self._cleanup_loop())
        
        # Load active sessions from Redis
        await self._load_active_sessions()
        
        logger.info("Session state manager initialized", 
                   active_sessions=len(self.sessions))
        
        # Log to Logfire
        logfire.info("Session state manager started", 
                    session_count=len(self.sessions))
    
    async def stop(self):
        """Stop the session state manager"""
        if self.cleanup_task:
            self.cleanup_task.cancel()
            try:
                await self.cleanup_task
            except asyncio.CancelledError:
                pass
        
        logger.info("Session state manager stopped")
    
    def _generate_session_id(self, user_id: str, vm_id: str) -> str:
        """Generate a cryptographically secure session ID"""
        # Use UUID4 for cryptographic randomness
        base_id = str(uuid.uuid4())
        # Add timestamp and user context for uniqueness
        timestamp = str(int(time.time() * 1000))
        return f"sess_{base_id}_{timestamp}"
    
    async def create_session(self, user_id: str, vm_id: str, 
                           terminal_size: tuple[int, int] = (80, 24),
                           environment_vars: Dict[str, str] = None) -> str:
        """Create a new session"""
        try:
            if not user_id or not vm_id:
                raise ValueError("User ID and VM ID are required")
            
            session_id = self._generate_session_id(user_id, vm_id)
            current_time = time.time()
            
            context = SessionContext(
                session_id=session_id,
                user_id=user_id,
                vm_id=vm_id,
                state=SessionState.INITIALIZING,
                created_at=current_time,
                last_activity=current_time,
                terminal_size=terminal_size,
                environment_vars=environment_vars or {},
                working_directory="/home/user",
                active_processes=[],
                metadata={}
            )
            
            # Store in memory
            self.sessions[session_id] = context
            
            # Persist to Redis
            await self._persist_session(context)
            
            logger.info("Session created", session_id=session_id, 
                       user_id=user_id, vm_id=vm_id)
            
            # Log to Logfire with tracing
            logfire.info("Session created successfully",
                        session_id=session_id,
                        user_id=user_id,
                        vm_id=vm_id,
                        terminal_size=terminal_size)
            
            return session_id
            
        except Exception as e:
            logger.error("Failed to create session", user_id=user_id, 
                        vm_id=vm_id, error=str(e))
            logfire.error("Session creation failed", 
                         user_id=user_id, vm_id=vm_id, error=str(e))
            raise
    
    async def update_session_state(self, session_id: str, 
                                 new_state: SessionState) -> bool:
        """Update session state"""
        try:
            if session_id not in self.sessions:
                logger.warning("Session not found for state update", session_id=session_id)
                return False
            
            context = self.sessions[session_id]
            old_state = context.state
            context.state = new_state
            context.last_activity = time.time()
            
            # Persist to Redis
            await self._persist_session(context)
            
            logger.info("Session state updated", session_id=session_id,
                       old_state=old_state.value, new_state=new_state.value)
            
            # Log state transition to Logfire
            logfire.info("Session state transition",
                        session_id=session_id,
                        from_state=old_state.value,
                        to_state=new_state.value,
                        user_id=context.user_id)
            
            return True
            
        except Exception as e:
            logger.error("Failed to update session state", 
                        session_id=session_id, error=str(e))
            return False
    
    async def get_session(self, session_id: str) -> Optional[SessionContext]:
        """Get session by ID"""
        try:
            if session_id in self.sessions:
                return self.sessions[session_id]
            
            # Try to load from Redis
            context = await self._load_session_from_redis(session_id)
            if context:
                self.sessions[session_id] = context
                return context
            
            return None
            
        except Exception as e:
            logger.error("Failed to get session", session_id=session_id, error=str(e))
            return None
    
    async def delete_session(self, session_id: str) -> bool:
        """Delete a session"""
        try:
            context = self.sessions.get(session_id)
            if not context:
                logger.warning("Session not found for deletion", session_id=session_id)
                return False
            
            # Remove from memory
            self.sessions.pop(session_id, None)
            
            # Remove from Redis
            await self._delete_session_from_redis(session_id, context.user_id)
            
            logger.info("Session deleted", session_id=session_id, user_id=context.user_id)
            logfire.info("Session deleted", session_id=session_id, user_id=context.user_id)
            
            return True
            
        except Exception as e:
            logger.error("Failed to delete session", session_id=session_id, error=str(e))
            return False
    
    async def get_user_sessions(self, user_id: str) -> List[SessionContext]:
        """Get all sessions for a user"""
        try:
            user_sessions = []
            for session in self.sessions.values():
                if session.user_id == user_id:
                    user_sessions.append(session)
            
            return user_sessions
            
        except Exception as e:
            logger.error("Failed to get user sessions", user_id=user_id, error=str(e))
            return []
    
    async def _persist_session(self, context: SessionContext):
        """Persist session to Redis"""
        try:
            session_key = f"{self.state_key_prefix}{context.session_id}"
            user_index_key = f"{self.user_index_prefix}{context.user_id}"
            
            # Store session data
            session_data = context.to_dict()
            
            pipe = self.redis.pipeline()
            pipe.hset(session_key, mapping=session_data)
            pipe.expire(session_key, 3600)  # 1 hour expiration
            pipe.sadd(user_index_key, context.session_id)
            pipe.expire(user_index_key, 3600)
            await pipe.execute()
            
        except Exception as e:
            logger.error("Failed to persist session", session_id=context.session_id, error=str(e))
            raise
    
    async def _load_session_from_redis(self, session_id: str) -> Optional[SessionContext]:
        """Load session from Redis"""
        try:
            session_key = f"{self.state_key_prefix}{session_id}"
            data = await self.redis.hgetall(session_key)
            
            if not data:
                return None
            
            # Convert bytes to strings
            str_data = {k.decode() if isinstance(k, bytes) else k: 
                       v.decode() if isinstance(v, bytes) else v 
                       for k, v in data.items()}
            
            return SessionContext.from_dict(str_data)
            
        except Exception as e:
            logger.error("Failed to load session from Redis", session_id=session_id, error=str(e))
            return None
    
    async def _delete_session_from_redis(self, session_id: str, user_id: str):
        """Delete session from Redis"""
        try:
            session_key = f"{self.state_key_prefix}{session_id}"
            user_index_key = f"{self.user_index_prefix}{user_id}"
            
            pipe = self.redis.pipeline()
            pipe.delete(session_key)
            pipe.srem(user_index_key, session_id)
            await pipe.execute()
            
        except Exception as e:
            logger.error("Failed to delete session from Redis", 
                        session_id=session_id, error=str(e))
            raise
    
    async def _load_active_sessions(self):
        """Load active sessions from Redis on startup"""
        try:
            # This would scan Redis for active sessions
            # For now, start with empty session store
            logger.info("Active sessions loaded", count=0)
            
        except Exception as e:
            logger.error("Failed to load active sessions", error=str(e))
    
    async def _cleanup_loop(self):
        """Cleanup expired sessions"""
        while True:
            try:
                await asyncio.sleep(self.cleanup_interval)
                await self._cleanup_expired_sessions()
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Session cleanup loop error", error=str(e))
    
    async def _cleanup_expired_sessions(self):
        """Remove expired sessions"""
        try:
            current_time = time.time()
            expired_sessions = []
            
            for session_id, context in self.sessions.items():
                # Consider sessions expired after 1 hour of inactivity
                if current_time - context.last_activity > 3600:
                    expired_sessions.append(session_id)
            
            for session_id in expired_sessions:
                await self.delete_session(session_id)
                logger.info("Expired session cleaned up", session_id=session_id)
                
        except Exception as e:
            logger.error("Session cleanup error", error=str(e))