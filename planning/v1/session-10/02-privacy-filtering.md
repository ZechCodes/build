# Session 10.2: Privacy Filtering & Data Protection

## Objective
Implement comprehensive privacy filtering system to protect sensitive data during terminal recording, ensuring user privacy and preventing accidental exposure of confidential information in recorded sessions.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for privacy filtering monitoring and security event tracking
- **Session 2**: Integrates with user preferences for privacy filter configuration
- **Session 10.1**: Provides real-time filtering for recording engine data processing
- **Session 5**: Uses database for privacy filter rules and audit logging

## Core Implementation

### Privacy Filter Engine
**Location**: `recording-manager/privacy/privacy_filter.py`

```python
# recording-manager/privacy/privacy_filter.py
import re
import asyncio
import time
import hashlib
from typing import List, Pattern, Dict, Any, Optional, Set, Callable
from dataclasses import dataclass
from enum import Enum
import structlog
import logfire
from concurrent.futures import ThreadPoolExecutor

logger = structlog.get_logger()

class FilterSensitivity(Enum):
    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"
    PARANOID = "paranoid"

class FilterCategory(Enum):
    CREDENTIALS = "credentials"
    FINANCIAL = "financial"
    PERSONAL = "personal"
    NETWORK = "network"
    SYSTEM = "system"
    CRYPTO = "crypto"
    CUSTOM = "custom"

@dataclass
class FilterRule:
    id: str
    name: str
    category: FilterCategory
    pattern: str
    replacement: str
    sensitivity: FilterSensitivity
    is_enabled: bool
    is_regex: bool
    case_sensitive: bool
    whole_word: bool
    priority: int
    created_at: float
    updated_at: float
    description: str
    examples: List[str]

@dataclass
class FilterMatch:
    rule_id: str
    rule_name: str
    category: FilterCategory
    original_text: str
    filtered_text: str
    position: int
    length: int
    timestamp: float
    confidence: float

@dataclass
class FilterStats:
    total_bytes_processed: int
    total_matches: int
    matches_by_category: Dict[str, int]
    processing_time_ms: float
    rules_applied: int
    sensitivity_level: FilterSensitivity
    session_id: str
    timestamp: float

class PrivacyFilter:
    def __init__(self, sensitivity: FilterSensitivity = FilterSensitivity.MEDIUM):
        self.sensitivity = sensitivity
        self.compiled_patterns: Dict[str, Pattern] = {}
        self.filter_rules: List[FilterRule] = []
        self.stats_cache: Dict[str, FilterStats] = {}
        self.thread_pool = ThreadPoolExecutor(max_workers=2)
        
        # Performance configuration
        self.max_processing_time_ms = 100  # Max time per filter operation
        self.pattern_cache_size = 1000
        self.stats_retention_hours = 24
        
        # Load default filter rules
        self.load_default_rules()
        self.compile_patterns()

    def load_default_rules(self) -> None:
        """Load default privacy filter rules"""
        default_rules = [
            # Credentials
            FilterRule(
                id="password_basic",
                name="Basic Password Pattern",
                category=FilterCategory.CREDENTIALS,
                pattern=r'(?i)(?:password|passwd|pwd)[\s=:]+\S+',
                replacement="[PASSWORD_FILTERED]",
                sensitivity=FilterSensitivity.LOW,
                is_enabled=True,
                is_regex=True,
                case_sensitive=False,
                whole_word=False,
                priority=10,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters basic password patterns in commands",
                examples=["password=secret123", "passwd: mypass"]
            ),
            FilterRule(
                id="api_key_generic",
                name="Generic API Key",
                category=FilterCategory.CREDENTIALS,
                pattern=r'(?i)(?:api[_-]?key|token|secret)[\s=:]+[a-zA-Z0-9+/=]{16,}',
                replacement="[API_KEY_FILTERED]",
                sensitivity=FilterSensitivity.MEDIUM,
                is_enabled=True,
                is_regex=True,
                case_sensitive=False,
                whole_word=False,
                priority=20,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters generic API keys and tokens",
                examples=["api_key=abc123xyz789", "token: Bearer xyz123"]
            ),
            FilterRule(
                id="ssh_private_key",
                name="SSH Private Key",
                category=FilterCategory.CREDENTIALS,
                pattern=r'-----BEGIN (?:RSA |OPENSSH |DSA |EC )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |OPENSSH |DSA |EC )?PRIVATE KEY-----',
                replacement="[SSH_PRIVATE_KEY_FILTERED]",
                sensitivity=FilterSensitivity.HIGH,
                is_enabled=True,
                is_regex=True,
                case_sensitive=True,
                whole_word=False,
                priority=30,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters SSH private keys",
                examples=["-----BEGIN RSA PRIVATE KEY-----"]
            ),
            
            # Financial
            FilterRule(
                id="credit_card",
                name="Credit Card Numbers",
                category=FilterCategory.FINANCIAL,
                pattern=r'\b(?:\d{4}[\s-]?){3}\d{4}\b',
                replacement="[CREDIT_CARD_FILTERED]",
                sensitivity=FilterSensitivity.HIGH,
                is_enabled=True,
                is_regex=True,
                case_sensitive=False,
                whole_word=True,
                priority=40,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters credit card numbers",
                examples=["4111-1111-1111-1111", "4111 1111 1111 1111"]
            ),
            FilterRule(
                id="bank_account",
                name="Bank Account Numbers",
                category=FilterCategory.FINANCIAL,
                pattern=r'\b\d{8,17}\b',
                replacement="[BANK_ACCOUNT_FILTERED]",
                sensitivity=FilterSensitivity.MEDIUM,
                is_enabled=False,  # Disabled by default due to false positives
                is_regex=True,
                case_sensitive=False,
                whole_word=True,
                priority=25,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters potential bank account numbers",
                examples=["12345678901234"]
            ),
            
            # Personal Information
            FilterRule(
                id="ssn_us",
                name="US Social Security Numbers",
                category=FilterCategory.PERSONAL,
                pattern=r'\b\d{3}-\d{2}-\d{4}\b',
                replacement="[SSN_FILTERED]",
                sensitivity=FilterSensitivity.HIGH,
                is_enabled=True,
                is_regex=True,
                case_sensitive=False,
                whole_word=True,
                priority=50,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters US Social Security Numbers",
                examples=["123-45-6789"]
            ),
            FilterRule(
                id="email_partial",
                name="Email Addresses (Partial)",
                category=FilterCategory.PERSONAL,
                pattern=r'\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b',
                replacement="[EMAIL_FILTERED]",
                sensitivity=FilterSensitivity.LOW,
                is_enabled=False,  # Disabled by default
                is_regex=True,
                case_sensitive=False,
                whole_word=False,
                priority=15,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters email addresses (may cause false positives)",
                examples=["user@example.com"]
            ),
            FilterRule(
                id="phone_us",
                name="US Phone Numbers",
                category=FilterCategory.PERSONAL,
                pattern=r'\b(?:\+?1[\s.-]?)?\(?\d{3}\)?[\s.-]?\d{3}[\s.-]?\d{4}\b',
                replacement="[PHONE_FILTERED]",
                sensitivity=FilterSensitivity.MEDIUM,
                is_enabled=True,
                is_regex=True,
                case_sensitive=False,
                whole_word=True,
                priority=35,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters US phone numbers",
                examples=["(555) 123-4567", "555-123-4567"]
            ),
            
            # Network Information
            FilterRule(
                id="ip_private",
                name="Private IP Addresses",
                category=FilterCategory.NETWORK,
                pattern=r'\b(?:10|172\.(?:1[6-9]|2[0-9]|3[01])|192\.168)\.\d{1,3}\.\d{1,3}\b',
                replacement="[PRIVATE_IP_FILTERED]",
                sensitivity=FilterSensitivity.LOW,
                is_enabled=False,  # Disabled by default
                is_regex=True,
                case_sensitive=False,
                whole_word=True,
                priority=5,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters private IP addresses",
                examples=["192.168.1.100", "10.0.0.1"]
            ),
            FilterRule(
                id="url_with_auth",
                name="URLs with Authentication",
                category=FilterCategory.NETWORK,
                pattern=r'https?://[^:]+:[^@]+@[^\s]+',
                replacement="[AUTH_URL_FILTERED]",
                sensitivity=FilterSensitivity.HIGH,
                is_enabled=True,
                is_regex=True,
                case_sensitive=False,
                whole_word=False,
                priority=45,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters URLs containing authentication credentials",
                examples=["https://user:pass@example.com"]
            ),
            
            # Cryptographic
            FilterRule(
                id="aws_access_key",
                name="AWS Access Keys",
                category=FilterCategory.CRYPTO,
                pattern=r'AKIA[0-9A-Z]{16}',
                replacement="[AWS_ACCESS_KEY_FILTERED]",
                sensitivity=FilterSensitivity.HIGH,
                is_enabled=True,
                is_regex=True,
                case_sensitive=True,
                whole_word=False,
                priority=55,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters AWS access keys",
                examples=["AKIAIOSFODNN7EXAMPLE"]
            ),
            FilterRule(
                id="jwt_token",
                name="JWT Tokens",
                category=FilterCategory.CRYPTO,
                pattern=r'eyJ[a-zA-Z0-9+/=]+\.[a-zA-Z0-9+/=]+\.[a-zA-Z0-9+/=]+',
                replacement="[JWT_TOKEN_FILTERED]",
                sensitivity=FilterSensitivity.HIGH,
                is_enabled=True,
                is_regex=True,
                case_sensitive=True,
                whole_word=False,
                priority=50,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters JWT tokens",
                examples=["eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"]
            ),
            FilterRule(
                id="base64_long",
                name="Long Base64 Strings",
                category=FilterCategory.CRYPTO,
                pattern=r'[A-Za-z0-9+/]{32,}={0,2}',
                replacement="[BASE64_DATA_FILTERED]",
                sensitivity=FilterSensitivity.LOW,
                is_enabled=False,  # Disabled due to false positives
                is_regex=True,
                case_sensitive=True,
                whole_word=False,
                priority=10,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters long base64 encoded strings",
                examples=["VGhpcyBpcyBhIGV4YW1wbGUgb2YgYmFzZTY0IGVuY29kaW5n"]
            ),
            
            # System Sensitive Information
            FilterRule(
                id="sudo_password",
                name="Sudo Password Prompts",
                category=FilterCategory.SYSTEM,
                pattern=r'\[sudo\] password for [^:]+:.*',
                replacement="[SUDO_PASSWORD_FILTERED]",
                sensitivity=FilterSensitivity.MEDIUM,
                is_enabled=True,
                is_regex=True,
                case_sensitive=False,
                whole_word=False,
                priority=25,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters sudo password prompts and responses",
                examples=["[sudo] password for user: secret123"]
            ),
            FilterRule(
                id="environment_secrets",
                name="Environment Variable Secrets",
                category=FilterCategory.SYSTEM,
                pattern=r'(?i)(?:export\s+)?(?:SECRET|PASSWORD|TOKEN|KEY)_[A-Z_]*=\S+',
                replacement="[ENV_SECRET_FILTERED]",
                sensitivity=FilterSensitivity.MEDIUM,
                is_enabled=True,
                is_regex=True,
                case_sensitive=False,
                whole_word=False,
                priority=30,
                created_at=time.time(),
                updated_at=time.time(),
                description="Filters environment variables containing secrets",
                examples=["export SECRET_KEY=abc123", "PASSWORD_HASH=xyz789"]
            )
        ]
        
        # Filter rules based on sensitivity level
        self.filter_rules = [
            rule for rule in default_rules
            if self._should_apply_rule(rule)
        ]

    def _should_apply_rule(self, rule: FilterRule) -> bool:
        """Determine if rule should be applied based on sensitivity settings"""
        if not rule.is_enabled:
            return False
        
        sensitivity_order = {
            FilterSensitivity.LOW: 1,
            FilterSensitivity.MEDIUM: 2,
            FilterSensitivity.HIGH: 3,
            FilterSensitivity.PARANOID: 4
        }
        
        return sensitivity_order[rule.sensitivity] <= sensitivity_order[self.sensitivity]

    def compile_patterns(self) -> None:
        """Compile regex patterns for performance"""
        self.compiled_patterns.clear()
        
        for rule in self.filter_rules:
            if rule.is_regex:
                try:
                    flags = 0 if rule.case_sensitive else re.IGNORECASE
                    if rule.whole_word:
                        pattern = rf'\b{rule.pattern}\b'
                    else:
                        pattern = rule.pattern
                    
                    self.compiled_patterns[rule.id] = re.compile(pattern, flags)
                    
                except re.error as e:
                    logger.error("Failed to compile regex pattern",
                               rule_id=rule.id,
                               pattern=rule.pattern,
                               error=str(e))

    async def filter_data(self, data: bytes, session_id: str = "") -> bytes:
        """Filter sensitive data from terminal output"""
        start_time = time.time()
        
        try:
            # Decode data
            text = data.decode('utf-8', errors='replace')
            if not text.strip():
                return data
            
            # Apply filters
            filtered_text, matches = await self._apply_filters_async(text, session_id)
            
            # Update statistics
            processing_time = (time.time() - start_time) * 1000
            await self._update_stats(session_id, len(data), matches, processing_time)
            
            # Log significant filtering events
            if matches:
                await self._log_filtering_event(session_id, matches)
            
            return filtered_text.encode('utf-8', errors='replace')
            
        except Exception as e:
            logger.error("Privacy filter error", 
                        session_id=session_id,
                        data_size=len(data),
                        error=str(e))
            return data

    async def _apply_filters_async(self, text: str, session_id: str) -> tuple[str, List[FilterMatch]]:
        """Apply filters asynchronously with timeout protection"""
        try:
            # Use thread pool for CPU-intensive regex operations
            future = asyncio.get_event_loop().run_in_executor(
                self.thread_pool, self._apply_filters_sync, text, session_id
            )
            
            # Apply timeout to prevent blocking
            filtered_text, matches = await asyncio.wait_for(
                future, timeout=self.max_processing_time_ms / 1000
            )
            
            return filtered_text, matches
            
        except asyncio.TimeoutError:
            logger.warning("Privacy filter timeout exceeded",
                         session_id=session_id,
                         text_length=len(text))
            return text, []

    def _apply_filters_sync(self, text: str, session_id: str) -> tuple[str, List[FilterMatch]]:
        """Apply filters synchronously"""
        filtered_text = text
        matches = []
        
        # Sort rules by priority (higher priority first)
        sorted_rules = sorted(self.filter_rules, key=lambda r: r.priority, reverse=True)
        
        for rule in sorted_rules:
            if rule.id in self.compiled_patterns:
                pattern = self.compiled_patterns[rule.id]
                
                # Find all matches
                for match in pattern.finditer(filtered_text):
                    filter_match = FilterMatch(
                        rule_id=rule.id,
                        rule_name=rule.name,
                        category=rule.category,
                        original_text=match.group(),
                        filtered_text=rule.replacement,
                        position=match.start(),
                        length=match.end() - match.start(),
                        timestamp=time.time(),
                        confidence=1.0  # Could be enhanced with ML confidence scoring
                    )
                    matches.append(filter_match)
                
                # Apply replacements
                filtered_text = pattern.sub(rule.replacement, filtered_text)
            
            elif not rule.is_regex:
                # Handle simple string replacement
                if rule.case_sensitive:
                    if rule.pattern in filtered_text:
                        matches.append(FilterMatch(
                            rule_id=rule.id,
                            rule_name=rule.name,
                            category=rule.category,
                            original_text=rule.pattern,
                            filtered_text=rule.replacement,
                            position=filtered_text.find(rule.pattern),
                            length=len(rule.pattern),
                            timestamp=time.time(),
                            confidence=1.0
                        ))
                        filtered_text = filtered_text.replace(rule.pattern, rule.replacement)
                else:
                    lower_text = filtered_text.lower()
                    lower_pattern = rule.pattern.lower()
                    if lower_pattern in lower_text:
                        # Find case-insensitive match
                        start_pos = lower_text.find(lower_pattern)
                        original_text = filtered_text[start_pos:start_pos + len(rule.pattern)]
                        
                        matches.append(FilterMatch(
                            rule_id=rule.id,
                            rule_name=rule.name,
                            category=rule.category,
                            original_text=original_text,
                            filtered_text=rule.replacement,
                            position=start_pos,
                            length=len(rule.pattern),
                            timestamp=time.time(),
                            confidence=1.0
                        ))
                        
                        # Replace case-insensitively
                        filtered_text = re.sub(re.escape(rule.pattern), rule.replacement, filtered_text, flags=re.IGNORECASE)
        
        return filtered_text, matches

    async def _update_stats(self, session_id: str, bytes_processed: int, 
                          matches: List[FilterMatch], processing_time_ms: float) -> None:
        """Update filtering statistics"""
        if session_id not in self.stats_cache:
            self.stats_cache[session_id] = FilterStats(
                total_bytes_processed=0,
                total_matches=0,
                matches_by_category={},
                processing_time_ms=0,
                rules_applied=len(self.filter_rules),
                sensitivity_level=self.sensitivity,
                session_id=session_id,
                timestamp=time.time()
            )
        
        stats = self.stats_cache[session_id]
        stats.total_bytes_processed += bytes_processed
        stats.total_matches += len(matches)
        stats.processing_time_ms += processing_time_ms
        stats.timestamp = time.time()
        
        # Update category counts
        for match in matches:
            category = match.category.value
            stats.matches_by_category[category] = stats.matches_by_category.get(category, 0) + 1

    async def _log_filtering_event(self, session_id: str, matches: List[FilterMatch]) -> None:
        """Log filtering events for monitoring"""
        high_sensitivity_matches = [
            match for match in matches 
            if any(rule.sensitivity == FilterSensitivity.HIGH 
                  for rule in self.filter_rules if rule.id == match.rule_id)
        ]
        
        if high_sensitivity_matches:
            logfire.warning("High sensitivity data filtered",
                          session_id=session_id,
                          matches_count=len(high_sensitivity_matches),
                          categories=[match.category.value for match in high_sensitivity_matches])
        
        # Log summary for all matches
        if matches:
            logfire.info("Privacy filtering applied",
                       session_id=session_id,
                       total_matches=len(matches),
                       categories=list(set(match.category.value for match in matches)))

    async def add_custom_rule(self, user_id: str, name: str, pattern: str, 
                            replacement: str, category: FilterCategory = FilterCategory.CUSTOM,
                            sensitivity: FilterSensitivity = FilterSensitivity.MEDIUM,
                            is_regex: bool = True) -> str:
        """Add custom filter rule for user"""
        try:
            # Validate pattern
            if is_regex:
                try:
                    re.compile(pattern)
                except re.error as e:
                    raise ValueError(f"Invalid regex pattern: {str(e)}")
            
            # Generate rule ID
            rule_id = f"custom_{user_id}_{int(time.time())}"
            
            # Create rule
            rule = FilterRule(
                id=rule_id,
                name=name,
                category=category,
                pattern=pattern,
                replacement=replacement,
                sensitivity=sensitivity,
                is_enabled=True,
                is_regex=is_regex,
                case_sensitive=True,
                whole_word=False,
                priority=100,  # Custom rules get high priority
                created_at=time.time(),
                updated_at=time.time(),
                description=f"Custom rule created by user {user_id}",
                examples=[]
            )
            
            # Add to active rules if it meets sensitivity requirements
            if self._should_apply_rule(rule):
                self.filter_rules.append(rule)
                
                # Compile pattern
                if rule.is_regex:
                    self.compiled_patterns[rule.id] = re.compile(pattern)
            
            # Store rule in database
            await self._store_custom_rule(user_id, rule)
            
            logfire.info("Custom privacy filter rule added",
                       user_id=user_id,
                       rule_id=rule_id,
                       category=category.value)
            
            return rule_id
            
        except Exception as e:
            logger.error("Failed to add custom filter rule",
                        user_id=user_id,
                        pattern=pattern,
                        error=str(e))
            raise

    async def get_filter_stats(self, session_id: str) -> Optional[FilterStats]:
        """Get filtering statistics for session"""
        return self.stats_cache.get(session_id)

    async def get_all_filter_stats(self) -> Dict[str, FilterStats]:
        """Get all filtering statistics"""
        # Clean up old stats
        current_time = time.time()
        cutoff_time = current_time - (self.stats_retention_hours * 3600)
        
        self.stats_cache = {
            session_id: stats for session_id, stats in self.stats_cache.items()
            if stats.timestamp > cutoff_time
        }
        
        return self.stats_cache.copy()

    async def update_sensitivity(self, new_sensitivity: FilterSensitivity) -> None:
        """Update filter sensitivity level"""
        self.sensitivity = new_sensitivity
        
        # Reload and recompile rules
        self.load_default_rules()
        self.compile_patterns()
        
        logfire.info("Privacy filter sensitivity updated",
                   new_sensitivity=new_sensitivity.value,
                   active_rules=len(self.filter_rules))

    async def test_filter_rule(self, pattern: str, test_text: str, 
                             is_regex: bool = True, case_sensitive: bool = True) -> Dict[str, Any]:
        """Test a filter rule against sample text"""
        try:
            if is_regex:
                flags = 0 if case_sensitive else re.IGNORECASE
                compiled_pattern = re.compile(pattern, flags)
                matches = list(compiled_pattern.finditer(test_text))
            else:
                if case_sensitive:
                    matches = [test_text.find(pattern)] if pattern in test_text else []
                else:
                    matches = [test_text.lower().find(pattern.lower())] if pattern.lower() in test_text.lower() else []
            
            return {
                "pattern": pattern,
                "test_text": test_text,
                "matches_found": len(matches),
                "match_positions": [match.span() if hasattr(match, 'span') else (match, match + len(pattern)) for match in matches],
                "is_valid": True,
                "error": None
            }
            
        except re.error as e:
            return {
                "pattern": pattern,
                "test_text": test_text,
                "matches_found": 0,
                "match_positions": [],
                "is_valid": False,
                "error": str(e)
            }

    # Database operations (implement based on your database choice)
    async def _store_custom_rule(self, user_id: str, rule: FilterRule) -> None:
        """Store custom filter rule in database"""
        # Implementation depends on database backend
        pass

    async def _load_user_custom_rules(self, user_id: str) -> List[FilterRule]:
        """Load user's custom filter rules from database"""
        # Implementation depends on database backend
        return []

    def cleanup(self) -> None:
        """Cleanup privacy filter resources"""
        self.compiled_patterns.clear()
        self.filter_rules.clear()
        self.stats_cache.clear()
        if self.thread_pool:
            self.thread_pool.shutdown(wait=True)
```

### Privacy Filter Configuration Manager
**Location**: `recording-manager/privacy/filter_config.py`

```python
# recording-manager/privacy/filter_config.py
from typing import Dict, Any, List, Optional
from dataclasses import dataclass, asdict
import structlog
import logfire
from .privacy_filter import PrivacyFilter, FilterSensitivity, FilterRule, FilterCategory

logger = structlog.get_logger()

@dataclass
class UserFilterPreferences:
    user_id: str
    sensitivity_level: FilterSensitivity
    enabled_categories: List[FilterCategory]
    disabled_rule_ids: List[str]
    custom_replacements: Dict[str, str]
    auto_filter_enabled: bool
    notification_on_filter: bool
    created_at: float
    updated_at: float

class PrivacyFilterManager:
    def __init__(self, database):
        self.db = database
        self.user_filters: Dict[str, PrivacyFilter] = {}
        self.user_preferences: Dict[str, UserFilterPreferences] = {}

    async def get_user_filter(self, user_id: str) -> PrivacyFilter:
        """Get or create privacy filter for user"""
        if user_id not in self.user_filters:
            # Load user preferences
            preferences = await self._load_user_preferences(user_id)
            
            # Create filter with user's sensitivity level
            privacy_filter = PrivacyFilter(preferences.sensitivity_level)
            
            # Apply user customizations
            await self._apply_user_customizations(privacy_filter, preferences)
            
            self.user_filters[user_id] = privacy_filter
            self.user_preferences[user_id] = preferences
        
        return self.user_filters[user_id]

    async def update_user_preferences(self, user_id: str, 
                                    sensitivity: Optional[FilterSensitivity] = None,
                                    enabled_categories: Optional[List[FilterCategory]] = None,
                                    disabled_rules: Optional[List[str]] = None) -> bool:
        """Update user's privacy filter preferences"""
        try:
            preferences = self.user_preferences.get(user_id) or await self._load_user_preferences(user_id)
            
            # Update preferences
            if sensitivity:
                preferences.sensitivity_level = sensitivity
            if enabled_categories is not None:
                preferences.enabled_categories = enabled_categories
            if disabled_rules is not None:
                preferences.disabled_rule_ids = disabled_rules
            
            preferences.updated_at = time.time()
            
            # Store updated preferences
            await self._store_user_preferences(preferences)
            
            # Update user's filter
            if user_id in self.user_filters:
                await self.user_filters[user_id].update_sensitivity(preferences.sensitivity_level)
            
            self.user_preferences[user_id] = preferences
            
            logfire.info("User privacy filter preferences updated",
                       user_id=user_id,
                       sensitivity=sensitivity.value if sensitivity else None)
            
            return True
            
        except Exception as e:
            logger.error("Failed to update user filter preferences",
                        user_id=user_id,
                        error=str(e))
            return False

    async def _load_user_preferences(self, user_id: str) -> UserFilterPreferences:
        """Load user preferences from database"""
        # Implementation depends on database backend
        # Return default preferences if none found
        return UserFilterPreferences(
            user_id=user_id,
            sensitivity_level=FilterSensitivity.MEDIUM,
            enabled_categories=list(FilterCategory),
            disabled_rule_ids=[],
            custom_replacements={},
            auto_filter_enabled=True,
            notification_on_filter=False,
            created_at=time.time(),
            updated_at=time.time()
        )

    async def _store_user_preferences(self, preferences: UserFilterPreferences) -> None:
        """Store user preferences in database"""
        # Implementation depends on database backend
        pass

    async def _apply_user_customizations(self, privacy_filter: PrivacyFilter, 
                                       preferences: UserFilterPreferences) -> None:
        """Apply user customizations to privacy filter"""
        # Disable specific rules
        for rule in privacy_filter.filter_rules:
            if rule.id in preferences.disabled_rule_ids:
                rule.is_enabled = False
        
        # Apply custom replacements
        for rule in privacy_filter.filter_rules:
            if rule.id in preferences.custom_replacements:
                rule.replacement = preferences.custom_replacements[rule.id]
        
        # Recompile patterns after customizations
        privacy_filter.compile_patterns()
```

## TDD Implementation Cycle

### Red Phase: Privacy Filter Test Creation
```python
# recording-manager/tests/test_privacy_filter.py
import pytest
from recording_manager.privacy.privacy_filter import PrivacyFilter, FilterSensitivity

@pytest.mark.asyncio
async def test_privacy_filter_initialization():
    """Test privacy filter initializes with default rules"""
    # This test should initially fail (Red phase)
    filter = PrivacyFilter()
    assert False, "Privacy filter initialization not implemented yet"

@pytest.mark.asyncio
async def test_password_filtering():
    """Test basic password filtering"""
    # This test should initially fail (Red phase)
    assert False, "Password filtering not implemented yet"

@pytest.mark.asyncio
async def test_custom_rule_addition():
    """Test adding custom filter rules"""
    # This test should initially fail (Red phase)
    assert False, "Custom rule functionality not implemented yet"
```

### Green Phase: Privacy Filter Implementation
```python
# Implement privacy filter features to make tests pass
# This involves adding pattern matching, filtering logic, and rule management
```

### Refactor Phase: Privacy Filter Optimization
```python
# Optimize privacy filter for performance and accuracy
# Add comprehensive pattern library and smart filtering
# Enhance customization and user control features
```

## Security Checklist ✅

### Filter Rule Security
- [ ] Filter rule pattern validation and sanitization
- [ ] Regex pattern security validation (ReDoS prevention)
- [ ] Custom rule authorization and ownership validation
- [ ] Filter rule enumeration protection
- [ ] Malicious pattern injection prevention
- [ ] Filter rule priority and precedence validation
- [ ] Rule compilation security and error handling
- [ ] Pattern cache security and memory protection
- [ ] Filter rule audit logging and monitoring
- [ ] Rule modification authorization controls

### Data Processing Security
- [ ] Sensitive data handling during filtering process
- [ ] Memory protection for filtered content
- [ ] Secure temporary data storage during processing
- [ ] Input validation for filter data
- [ ] Output sanitization after filtering
- [ ] Processing timeout protection (DoS prevention)
- [ ] Thread pool security and isolation
- [ ] Error handling without data leakage
- [ ] Filtering bypass prevention
- [ ] Performance monitoring and alerting

### Privacy Protection Validation
- [ ] Filter effectiveness testing and validation
- [ ] False positive and false negative monitoring
- [ ] Comprehensive pattern coverage validation
- [ ] Privacy filter accuracy measurement
- [ ] User consent and preference enforcement
- [ ] Data retention and cleanup validation
- [ ] Privacy policy compliance checking
- [ ] Cross-user filter isolation
- [ ] Filter configuration security
- [ ] Privacy breach prevention and detection

### Performance and Resource Security
- [ ] CPU usage monitoring and limits for filtering
- [ ] Memory usage protection during pattern matching
- [ ] Processing timeout enforcement
- [ ] Resource exhaustion prevention
- [ ] Filter operation rate limiting
- [ ] Pattern compilation resource limits
- [ ] Statistics collection security
- [ ] Cache security and cleanup
- [ ] Thread pool resource management
- [ ] Performance degradation detection

### User Control and Transparency
- [ ] User filter preference security validation
- [ ] Filter notification system security
- [ ] User rule testing and validation security
- [ ] Filter statistics privacy protection
- [ ] User consent management security
- [ ] Filter configuration backup and recovery
- [ ] User data deletion and cleanup
- [ ] Filter transparency and explainability
- [ ] User control authorization validation
- [ ] Privacy setting inheritance security

## Performance Requirements

### Filtering Performance
- Filter processing latency < 1ms per KB of data
- Regex pattern compilation < 10ms per pattern
- Custom rule addition < 500ms
- Filter sensitivity update < 1 second
- Statistics generation < 100ms
- Memory usage < 10MB per active filter

### Pattern Matching Performance
- Pattern matching throughput > 100 MB/s
- Concurrent filtering operations (up to 50)
- Pattern cache hit ratio > 95%
- Rule evaluation speed > 1000 rules/second
- False positive rate < 1%
- Filter accuracy > 99% for known patterns

### Scalability Requirements
- Support 1000+ custom rules per user
- Handle 100+ concurrent filter operations
- Support 10000+ filter rule evaluations per second
- Scale to 1000+ users with personalized filters
- Manage 100TB+ of filtered data processing
- Support 24/7 continuous filtering operations

## Commit Instructions

After implementing the privacy filtering system:

```bash
git add recording-manager/privacy/
git commit -m "Add comprehensive privacy filtering system with customizable rules

- Implement PrivacyFilter with extensive pattern library
- Add FilterRule system with configurable sensitivity levels
- Implement real-time filtering with performance optimization
- Add custom rule creation and management capabilities
- Include comprehensive filter categories (credentials, financial, personal, etc.)
- Add user preference management and filter customization
- Implement filter statistics and monitoring
- Add pattern testing and validation functionality
- Include thread pool optimization for CPU-intensive operations
- Add TDD cycle with Red-Green-Refactor for privacy features
- Ensure >90% privacy filter test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete privacy filtering test suite:

```bash
# Run all privacy filter tests
pytest recording-manager/tests/test_privacy_filter.py -v --timeout=300

# Run specific filter test categories
pytest recording-manager/tests/privacy/ -k "filter_rules" -v
pytest recording-manager/tests/privacy/ -k "user_preferences" -v
pytest recording-manager/tests/privacy/ -k "pattern_matching" -v

# Run privacy filter performance tests
pytest recording-manager/tests/privacy/performance/ -v

# Run privacy filter security tests
pytest recording-manager/tests/privacy/security/ -v
```

Validate privacy filter test coverage:
```bash
pytest recording-manager/tests/privacy/ --cov=recording_manager.privacy --cov-report=html --cov-fail-under=90
```

## Integration Testing

Test privacy filtering integration with recording engine:
```bash
# Test integration with Session 10.1 (Recording Engine)
pytest recording-manager/tests/integration/test_privacy_recording_integration.py -v

# Test real-time filtering performance
pytest recording-manager/tests/integration/test_privacy_realtime_integration.py -v

# Test user preference integration
pytest recording-manager/tests/integration/test_privacy_user_integration.py -v
```