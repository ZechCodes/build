"""
Recovery Manager

Handles automatic session recovery after crashes/disconnections with
comprehensive validation, rate limiting, and security controls.
"""
import asyncio
import time
from typing import Dict, List, Optional, Set
from dataclasses import dataclass
import structlog
import logfire

logger = structlog.get_logger()


@dataclass
class RecoveryInfo:
    """Recovery context information"""
    session_id: str
    user_id: str
    connection_id: str
    last_seen_timestamp: float
    recovery_start_time: float
    recovery_attempts: int = 0
    is_recoverable: bool = True


class RecoveryManager:
    """Manages session recovery with security and rate limiting"""
    
    def __init__(self, session_manager, buffer_manager, websocket_bridge):
        self.session_manager = session_manager
        self.buffer_manager = buffer_manager
        self.websocket_bridge = websocket_bridge
        
        # Recovery configuration
        self.recovery_timeout = 300  # 5 minutes
        self.max_recovery_attempts = 3
        self.rate_limit_window = 3600  # 1 hour
        self.max_recoveries_per_window = 10
        
        # Active recovery tracking
        self.active_recoveries: Dict[str, RecoveryInfo] = {}
        self.recovery_attempts: Dict[str, List[float]] = {}  # user_id -> timestamps
        self.recovery_task: Optional[asyncio.Task] = None
        
    async def initialize(self):
        """Initialize recovery manager"""
        self.recovery_task = asyncio.create_task(self._recovery_loop())
        logger.info("Recovery manager initialized")
        logfire.info("Session recovery manager started")
    
    async def stop(self):
        """Stop recovery manager"""
        if self.recovery_task:
            self.recovery_task.cancel()
            try:
                await self.recovery_task
            except asyncio.CancelledError:
                pass
        
        # Cleanup active recoveries
        self.active_recoveries.clear()
        self.recovery_attempts.clear()
        
        logger.info("Recovery manager stopped")
    
    async def initiate_recovery(self, session_id: str, user_id: str, 
                              connection_id: str) -> bool:
        """Initiate session recovery process"""
        try:
            # Rate limiting check
            if not await self._check_rate_limits(user_id):
                logger.warning("Recovery rate limit exceeded", 
                             user_id=user_id, session_id=session_id)
                return False
            
            # Verify session exists and belongs to user
            session = await self.session_manager.get_session(session_id)
            if not session or session.user_id != user_id:
                logger.warning("Session recovery denied", session_id=session_id, 
                             user_id=user_id, reason="session_not_found_or_unauthorized")
                logfire.warning("Unauthorized session recovery attempt",
                              session_id=session_id, user_id=user_id)
                return False
            
            # Check if session is recoverable
            current_time = time.time()
            
            # Sessions are recoverable within the recovery timeout window
            if current_time - session.last_activity > self.recovery_timeout:
                logger.warning("Session recovery denied", session_id=session_id, 
                             reason="session_too_old",
                             last_activity=session.last_activity,
                             age_seconds=current_time - session.last_activity)
                return False
            
            # Check if recovery is already in progress
            if session_id in self.active_recoveries:
                existing_recovery = self.active_recoveries[session_id]
                if existing_recovery.user_id != user_id:
                    logger.warning("Recovery conflict - different user", 
                                 session_id=session_id, 
                                 requesting_user=user_id,
                                 active_user=existing_recovery.user_id)
                    return False
                
                # Update existing recovery with new connection
                existing_recovery.connection_id = connection_id
                existing_recovery.recovery_attempts += 1
                
                if existing_recovery.recovery_attempts > self.max_recovery_attempts:
                    logger.warning("Max recovery attempts exceeded", 
                                 session_id=session_id, user_id=user_id)
                    return False
            else:
                # Start new recovery process
                recovery_info = RecoveryInfo(
                    session_id=session_id,
                    user_id=user_id,
                    connection_id=connection_id,
                    last_seen_timestamp=session.last_activity,
                    recovery_start_time=current_time,
                    recovery_attempts=1
                )
                
                self.active_recoveries[session_id] = recovery_info
            
            # Update session state
            from core.state_manager import SessionState
            await self.session_manager.update_session_state(session_id, SessionState.ACTIVE)
            
            # Record recovery attempt for rate limiting
            await self._record_recovery_attempt(user_id)
            
            # Start recovery task
            recovery_info = self.active_recoveries[session_id]
            asyncio.create_task(self._perform_recovery(recovery_info))
            
            logger.info("Session recovery initiated", session_id=session_id, 
                       user_id=user_id, connection_id=connection_id)
            
            logfire.info("Session recovery initiated",
                        session_id=session_id,
                        user_id=user_id,
                        connection_id=connection_id,
                        recovery_attempts=recovery_info.recovery_attempts)
            
            return True
            
        except Exception as e:
            logger.error("Failed to initiate recovery", session_id=session_id, 
                        error=str(e))
            logfire.error("Recovery initiation failed",
                         session_id=session_id, error=str(e))
            return False
    
    async def _perform_recovery(self, recovery_info: RecoveryInfo):
        """Perform the actual session recovery"""
        try:
            session_id = recovery_info.session_id
            user_id = recovery_info.user_id
            connection_id = recovery_info.connection_id
            
            logger.info("Performing session recovery", 
                       session_id=session_id, user_id=user_id)
            
            # Get session buffer history
            buffer_data = await self.buffer_manager.retrieve_buffer(session_id, user_id)
            
            if not buffer_data:
                logger.warning("No buffer data found for recovery", 
                             session_id=session_id)
                await self._send_recovery_error(connection_id, "No session data available")
                return False
            
            # Validate data integrity
            if not await self._validate_recovery_data(buffer_data):
                logger.error("Recovery data integrity check failed", 
                           session_id=session_id)
                await self._send_recovery_error(connection_id, "Data integrity validation failed")
                return False
            
            # Prepare recovery message
            recovery_message = {
                "type": "session_recovery",
                "session_id": session_id,
                "recovery_data": {
                    "buffer": buffer_data.buffer_data.decode('utf-8', errors='replace'),
                    "cursor_position": buffer_data.cursor_position,
                    "scroll_position": buffer_data.scroll_position,
                    "last_activity": recovery_info.last_seen_timestamp,
                    "recovery_timestamp": recovery_info.recovery_start_time,
                    "buffer_metadata": {
                        "size_bytes": buffer_data.size_bytes,
                        "line_count": buffer_data.line_count,
                        "last_updated": buffer_data.last_updated
                    }
                }
            }
            
            # Send recovery data to client
            success = await self._send_recovery_message(connection_id, recovery_message)
            
            if success:
                # Update session state to active
                from core.state_manager import SessionState
                await self.session_manager.update_session_state(session_id, SessionState.ACTIVE)
                
                logger.info("Session recovery completed successfully", 
                           session_id=session_id, user_id=user_id)
                
                logfire.info("Session recovery completed",
                           session_id=session_id,
                           user_id=user_id,
                           buffer_size=buffer_data.size_bytes,
                           recovery_duration=time.time() - recovery_info.recovery_start_time)
            else:
                logger.error("Session recovery failed - could not send data", 
                           session_id=session_id)
                await self._send_recovery_error(connection_id, "Failed to transmit recovery data")
            
            # Cleanup recovery context
            self.active_recoveries.pop(session_id, None)
            
            return success
            
        except Exception as e:
            logger.error("Session recovery error", session_id=recovery_info.session_id, 
                        error=str(e))
            logfire.error("Session recovery failed",
                         session_id=recovery_info.session_id, error=str(e))
            
            # Cleanup on error
            self.active_recoveries.pop(recovery_info.session_id, None)
            await self._send_recovery_error(recovery_info.connection_id, 
                                          f"Recovery failed: {str(e)}")
            return False
    
    async def abandon_recovery(self, session_id: str, user_id: str) -> bool:
        """Abandon recovery process"""
        try:
            if session_id not in self.active_recoveries:
                return False
            
            recovery_info = self.active_recoveries[session_id]
            
            # Verify ownership
            if recovery_info.user_id != user_id:
                logger.warning("Unauthorized recovery abandonment attempt",
                             session_id=session_id, user_id=user_id)
                return False
            
            # Remove from active recoveries
            self.active_recoveries.pop(session_id, None)
            
            # Update session state
            from core.state_manager import SessionState
            await self.session_manager.update_session_state(session_id, SessionState.IDLE)
            
            logger.info("Session recovery abandoned", session_id=session_id, user_id=user_id)
            logfire.info("Session recovery abandoned", session_id=session_id, user_id=user_id)
            
            return True
            
        except Exception as e:
            logger.error("Failed to abandon recovery", session_id=session_id, error=str(e))
            return False
    
    async def _check_rate_limits(self, user_id: str) -> bool:
        """Check if user has exceeded recovery rate limits"""
        try:
            current_time = time.time()
            
            # Clean old attempts
            if user_id in self.recovery_attempts:
                self.recovery_attempts[user_id] = [
                    timestamp for timestamp in self.recovery_attempts[user_id]
                    if current_time - timestamp < self.rate_limit_window
                ]
            
            # Check rate limit
            attempts_count = len(self.recovery_attempts.get(user_id, []))
            
            if attempts_count >= self.max_recoveries_per_window:
                logger.warning("Recovery rate limit exceeded", 
                             user_id=user_id, 
                             attempts=attempts_count,
                             window_hours=self.rate_limit_window / 3600)
                return False
            
            return True
            
        except Exception as e:
            logger.error("Rate limit check failed", user_id=user_id, error=str(e))
            return False
    
    async def _record_recovery_attempt(self, user_id: str):
        """Record recovery attempt for rate limiting"""
        try:
            current_time = time.time()
            
            if user_id not in self.recovery_attempts:
                self.recovery_attempts[user_id] = []
            
            self.recovery_attempts[user_id].append(current_time)
            
        except Exception as e:
            logger.error("Failed to record recovery attempt", user_id=user_id, error=str(e))
    
    async def _validate_recovery_data(self, buffer_data) -> bool:
        """Validate recovery data integrity"""
        try:
            # Basic validation
            if not buffer_data or not buffer_data.buffer_data:
                return False
            
            # Check data consistency
            if buffer_data.size_bytes != len(buffer_data.buffer_data):
                logger.warning("Buffer size mismatch in recovery data")
                return False
            
            # Check timestamp validity
            if buffer_data.last_updated <= 0:
                logger.warning("Invalid timestamp in recovery data")
                return False
            
            return True
            
        except Exception as e:
            logger.error("Recovery data validation failed", error=str(e))
            return False
    
    async def _send_recovery_message(self, connection_id: str, message: Dict) -> bool:
        """Send recovery message to WebSocket connection"""
        try:
            if self.websocket_bridge:
                return await self.websocket_bridge.send_to_connection(connection_id, message)
            else:
                logger.warning("No WebSocket bridge available for recovery")
                return False
                
        except Exception as e:
            logger.error("Failed to send recovery message", 
                        connection_id=connection_id, error=str(e))
            return False
    
    async def _send_recovery_error(self, connection_id: str, error_message: str):
        """Send recovery error message"""
        try:
            error_response = {
                "type": "recovery_error",
                "message": error_message,
                "timestamp": time.time()
            }
            
            if self.websocket_bridge:
                await self.websocket_bridge.send_to_connection(connection_id, error_response)
                
        except Exception as e:
            logger.error("Failed to send recovery error", 
                        connection_id=connection_id, error=str(e))
    
    async def _recovery_loop(self):
        """Main recovery monitoring loop"""
        while True:
            try:
                await asyncio.sleep(60)  # Check every minute
                await self._monitor_active_recoveries()
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Recovery loop error", error=str(e))
    
    async def _monitor_active_recoveries(self):
        """Monitor active recoveries for timeouts"""
        try:
            current_time = time.time()
            timed_out_recoveries = []
            
            for session_id, recovery_info in self.active_recoveries.items():
                # Check for recovery timeout
                if current_time - recovery_info.recovery_start_time > self.recovery_timeout:
                    timed_out_recoveries.append(session_id)
            
            for session_id in timed_out_recoveries:
                recovery_info = self.active_recoveries[session_id]
                logger.warning("Recovery timed out", 
                             session_id=session_id, 
                             user_id=recovery_info.user_id)
                
                await self.abandon_recovery(session_id, recovery_info.user_id)
                await self._send_recovery_error(recovery_info.connection_id, 
                                              "Recovery timed out")
                
        except Exception as e:
            logger.error("Recovery monitoring error", error=str(e))
    
    def get_active_recoveries(self) -> Dict[str, RecoveryInfo]:
        """Get active recovery information (for monitoring)"""
        return self.active_recoveries.copy()
    
    def get_recovery_stats(self) -> Dict[str, int]:
        """Get recovery statistics"""
        try:
            current_time = time.time()
            total_attempts = 0
            recent_attempts = 0  # Last hour
            
            for user_attempts in self.recovery_attempts.values():
                total_attempts += len(user_attempts)
                recent_attempts += len([
                    t for t in user_attempts 
                    if current_time - t < 3600
                ])
            
            return {
                "active_recoveries": len(self.active_recoveries),
                "total_attempts": total_attempts,
                "recent_attempts": recent_attempts,
                "rate_limited_users": len([
                    user_id for user_id, attempts in self.recovery_attempts.items()
                    if len(attempts) >= self.max_recoveries_per_window
                ])
            }
            
        except Exception as e:
            logger.error("Failed to get recovery stats", error=str(e))
            return {}