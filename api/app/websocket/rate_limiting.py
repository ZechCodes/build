"""WebSocket-specific rate limiting middleware."""

import asyncio
import time
from collections import defaultdict, deque
from typing import Dict, Any, Optional, List, Tuple
from enum import Enum
import structlog

from app.core.config import get_settings
from .redis_storage import get_redis_storage

logger = structlog.get_logger(__name__)


class RateLimitType(Enum):
    """Types of rate limits."""
    CONNECTION = "connection"       # New connections per IP/user
    MESSAGE = "message"            # Messages per connection
    BANDWIDTH = "bandwidth"        # Bytes per connection
    GLOBAL_USER = "global_user"    # Global limits per user
    GLOBAL_IP = "global_ip"        # Global limits per IP


class RateLimitRule:
    """Rate limiting rule configuration."""
    
    def __init__(self, 
                 limit_type: RateLimitType,
                 max_requests: int,
                 window_seconds: int,
                 message_types: Optional[List[str]] = None,
                 burst_multiplier: float = 1.5):
        self.limit_type = limit_type
        self.max_requests = max_requests
        self.window_seconds = window_seconds
        self.message_types = message_types or []  # Empty means all message types
        self.burst_multiplier = burst_multiplier  # Allow burst up to this multiplier
        self.burst_limit = int(max_requests * burst_multiplier)


class WebSocketRateLimiter:
    """Advanced rate limiter for WebSocket connections."""
    
    def __init__(self):
        self.settings = get_settings()
        self.redis_storage = None
        
        # In-memory tracking (for fast checks)
        self.connection_counters: Dict[str, Dict[str, deque]] = defaultdict(lambda: defaultdict(deque))
        self.user_counters: Dict[str, Dict[str, deque]] = defaultdict(lambda: defaultdict(deque))
        self.ip_counters: Dict[str, Dict[str, deque]] = defaultdict(lambda: defaultdict(deque))
        
        # Rate limiting rules
        self.rules = self._initialize_rules()
        
        # Track violations for adaptive limiting
        self.violation_history: Dict[str, List[float]] = defaultdict(list)
        
    def _initialize_rules(self) -> List[RateLimitRule]:
        """Initialize rate limiting rules."""
        return [
            # Connection-level rules
            RateLimitRule(
                RateLimitType.MESSAGE,
                max_requests=100,  # 100 messages per minute
                window_seconds=60,
                message_types=["terminal_data"]
            ),
            RateLimitRule(
                RateLimitType.MESSAGE,
                max_requests=10,   # 10 resize events per minute
                window_seconds=60,
                message_types=["terminal_resize"]
            ),
            RateLimitRule(
                RateLimitType.MESSAGE,
                max_requests=30,   # 30 session operations per minute
                window_seconds=60,
                message_types=["session_create", "session_join", "session_restore"]
            ),
            
            # Global user rules
            RateLimitRule(
                RateLimitType.GLOBAL_USER,
                max_requests=1000,  # 1000 messages per user per hour
                window_seconds=3600
            ),
            RateLimitRule(
                RateLimitType.BANDWIDTH,
                max_requests=10 * 1024 * 1024,  # 10MB per user per hour
                window_seconds=3600
            ),
            
            # Global IP rules (DDoS protection)
            RateLimitRule(
                RateLimitType.GLOBAL_IP,
                max_requests=5000,  # 5000 messages per IP per hour
                window_seconds=3600
            ),
            RateLimitRule(
                RateLimitType.CONNECTION,
                max_requests=50,    # 50 new connections per IP per hour
                window_seconds=3600
            ),
        ]
    
    async def initialize(self):
        """Initialize rate limiter with Redis storage."""
        try:
            self.redis_storage = await get_redis_storage()
            logger.info("WebSocket rate limiter initialized with Redis")
        except Exception as e:
            logger.warning("WebSocket rate limiter initialized without Redis", error=str(e))
    
    async def check_rate_limit(self, 
                              connection_id: str,
                              user_id: str,
                              client_ip: str,
                              message_type: str,
                              message_size: int = 0) -> Tuple[bool, Optional[str]]:
        """
        Check if request is within rate limits.
        
        Returns:
            (allowed: bool, violation_reason: str | None)
        """
        current_time = time.time()
        
        # Check each applicable rule
        for rule in self.rules:
            if not self._rule_applies(rule, message_type):
                continue
            
            # Get the appropriate counter key and counter dict
            counter_key, counter_dict = self._get_counter(
                rule.limit_type, connection_id, user_id, client_ip
            )
            
            if not counter_key:
                continue
            
            # Check this specific rule
            allowed = await self._check_rule_limit(
                rule, counter_key, counter_dict, current_time, message_size
            )
            
            if not allowed:
                violation_reason = f"{rule.limit_type.value}:{rule.max_requests}/{rule.window_seconds}s"
                await self._record_violation(counter_key, current_time, violation_reason)
                return False, violation_reason
        
        return True, None
    
    def _rule_applies(self, rule: RateLimitRule, message_type: str) -> bool:
        """Check if a rule applies to the given message type."""
        if not rule.message_types:  # Empty list means applies to all
            return True
        return message_type in rule.message_types
    
    def _get_counter(self, 
                     limit_type: RateLimitType, 
                     connection_id: str, 
                     user_id: str, 
                     client_ip: str) -> Tuple[Optional[str], Optional[Dict]]:
        """Get the appropriate counter key and dictionary for the limit type."""
        if limit_type == RateLimitType.MESSAGE:
            return connection_id, self.connection_counters[connection_id]
        elif limit_type in [RateLimitType.GLOBAL_USER, RateLimitType.BANDWIDTH]:
            return user_id, self.user_counters[user_id]
        elif limit_type in [RateLimitType.GLOBAL_IP, RateLimitType.CONNECTION]:
            return client_ip, self.ip_counters[client_ip]
        else:
            return None, None
    
    async def _check_rule_limit(self,
                               rule: RateLimitRule,
                               key: str,
                               counter_dict: Dict[str, deque],
                               current_time: float,
                               message_size: int) -> bool:
        """Check if a specific rule's limit is exceeded."""
        # Get the counter for this rule type
        counter_key = f"{rule.limit_type.value}:{rule.window_seconds}"
        counter = counter_dict[counter_key]
        
        # Clean old entries
        while counter and counter[0][0] <= current_time - rule.window_seconds:
            counter.popleft()
        
        # Calculate current usage
        if rule.limit_type == RateLimitType.BANDWIDTH:
            current_usage = sum(entry[1] for entry in counter)
            current_usage += message_size
            limit = rule.max_requests
        else:
            current_usage = len(counter) + 1
            limit = rule.max_requests
        
        # Check against limit (with burst allowance)
        burst_limit = rule.burst_limit
        
        # Apply adaptive limiting based on violation history
        adjusted_limit = self._get_adjusted_limit(key, rule, current_time)
        
        if current_usage > adjusted_limit:
            return False
        
        # Add current request to counter
        counter.append((current_time, message_size))
        
        # Store in Redis for persistence (async, don't wait)
        if self.redis_storage:
            asyncio.create_task(self._store_counter_in_redis(key, counter_key, counter))
        
        return True
    
    def _get_adjusted_limit(self, key: str, rule: RateLimitRule, current_time: float) -> int:
        """Get adjusted limit based on violation history (adaptive rate limiting)."""
        violations = self.violation_history.get(key, [])
        
        # Remove old violations (older than 1 hour)
        violations = [v for v in violations if current_time - v < 3600]
        self.violation_history[key] = violations
        
        # If there are recent violations, reduce the limit
        if len(violations) > 3:  # More than 3 violations in the last hour
            reduction_factor = min(0.5, 1.0 - (len(violations) * 0.1))
            adjusted_limit = int(rule.max_requests * reduction_factor)
            logger.info("Applied adaptive rate limiting", 
                       key=key, 
                       original_limit=rule.max_requests,
                       adjusted_limit=adjusted_limit,
                       violations=len(violations))
            return adjusted_limit
        
        return rule.max_requests
    
    async def _record_violation(self, key: str, current_time: float, reason: str):
        """Record a rate limit violation."""
        self.violation_history[key].append(current_time)
        
        # Store violation in Redis for persistence
        if self.redis_storage:
            try:
                violation_key = f"ws:violations:{key}"
                violation_data = {
                    "timestamp": current_time,
                    "reason": reason,
                    "key": key
                }
                
                # Store with 24 hour TTL
                await self.redis_storage.redis_client.setex(
                    violation_key, 
                    86400, 
                    self.redis_storage.encryption.encrypt_connection_state(violation_data)
                )
            except Exception as e:
                logger.error("Failed to store violation in Redis", error=str(e))
    
    async def _store_counter_in_redis(self, key: str, counter_key: str, counter: deque):
        """Store counter data in Redis for persistence."""
        try:
            if not self.redis_storage or not self.redis_storage.redis_client:
                return
            
            # Convert deque to list for serialization
            counter_data = {
                "key": key,
                "counter_key": counter_key,
                "entries": list(counter),
                "updated_at": time.time()
            }
            
            redis_key = f"ws:rate_limit:{key}:{counter_key}"
            encrypted_data = self.redis_storage.encryption.encrypt_connection_state(counter_data)
            
            # Store with appropriate TTL
            ttl = 7200  # 2 hours
            await self.redis_storage.redis_client.setex(redis_key, ttl, encrypted_data)
            
        except Exception as e:
            logger.error("Failed to store rate limit counter in Redis", error=str(e))
    
    async def get_rate_limit_status(self, 
                                   connection_id: str,
                                   user_id: str,
                                   client_ip: str) -> Dict[str, Any]:
        """Get current rate limit status for debugging/monitoring."""
        current_time = time.time()
        status = {
            "connection_id": connection_id,
            "user_id": user_id,
            "client_ip": client_ip,
            "timestamp": current_time,
            "limits": {}
        }
        
        # Check status for each rule
        for rule in self.rules:
            counter_key, counter_dict = self._get_counter(
                rule.limit_type, connection_id, user_id, client_ip
            )
            
            if not counter_key or not counter_dict:
                continue
            
            rule_key = f"{rule.limit_type.value}:{rule.window_seconds}"
            counter = counter_dict.get(rule_key, deque())
            
            # Clean old entries
            while counter and counter[0][0] <= current_time - rule.window_seconds:
                counter.popleft()
            
            # Calculate current usage
            if rule.limit_type == RateLimitType.BANDWIDTH:
                current_usage = sum(entry[1] for entry in counter)
            else:
                current_usage = len(counter)
            
            adjusted_limit = self._get_adjusted_limit(counter_key, rule, current_time)
            
            status["limits"][rule_key] = {
                "current_usage": current_usage,
                "limit": rule.max_requests,
                "adjusted_limit": adjusted_limit,
                "window_seconds": rule.window_seconds,
                "usage_percentage": (current_usage / adjusted_limit) * 100 if adjusted_limit > 0 else 0,
                "message_types": rule.message_types
            }
        
        return status
    
    async def cleanup_expired_data(self):
        """Clean up expired rate limiting data."""
        current_time = time.time()
        
        # Clean in-memory counters
        for connection_counters in self.connection_counters.values():
            for counter in connection_counters.values():
                while counter and counter[0][0] <= current_time - 3600:  # 1 hour
                    counter.popleft()
        
        for user_counters in self.user_counters.values():
            for counter in user_counters.values():
                while counter and counter[0][0] <= current_time - 3600:
                    counter.popleft()
        
        for ip_counters in self.ip_counters.values():
            for counter in ip_counters.values():
                while counter and counter[0][0] <= current_time - 3600:
                    counter.popleft()
        
        # Clean violation history
        for key in list(self.violation_history.keys()):
            violations = [v for v in self.violation_history[key] if current_time - v < 3600]
            if violations:
                self.violation_history[key] = violations
            else:
                del self.violation_history[key]
    
    async def get_global_rate_limit_stats(self) -> Dict[str, Any]:
        """Get global rate limiting statistics."""
        current_time = time.time()
        
        stats = {
            "timestamp": current_time,
            "total_connections_tracked": len(self.connection_counters),
            "total_users_tracked": len(self.user_counters),
            "total_ips_tracked": len(self.ip_counters),
            "total_violations": sum(len(violations) for violations in self.violation_history.values()),
            "active_violations_last_hour": 0,
            "rules_count": len(self.rules)
        }
        
        # Count recent violations
        for violations in self.violation_history.values():
            recent_violations = [v for v in violations if current_time - v < 3600]
            stats["active_violations_last_hour"] += len(recent_violations)
        
        return stats


# Global instance
_websocket_rate_limiter = None


async def get_websocket_rate_limiter() -> WebSocketRateLimiter:
    """Get global WebSocket rate limiter instance."""
    global _websocket_rate_limiter
    if _websocket_rate_limiter is None:
        _websocket_rate_limiter = WebSocketRateLimiter()
        await _websocket_rate_limiter.initialize()
    return _websocket_rate_limiter