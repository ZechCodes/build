"""Terminal output sanitization for XSS prevention."""

import re
import html
from typing import Union, Dict, Any
import structlog

logger = structlog.get_logger(__name__)


class TerminalOutputSanitizer:
    """Sanitize terminal output to prevent XSS and malicious sequences."""
    
    def __init__(self):
        # ANSI escape sequence pattern (for color, cursor movement, etc.)
        self.ansi_escape_pattern = re.compile(r'\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])')
        
        # Control characters that should be filtered (except common ones like \n, \r, \t)
        self.dangerous_control_chars = re.compile(r'[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]')
        
        # Terminal escape sequences that could be dangerous
        self.dangerous_sequences = [
            r'\x1B\].*;.*\x07',  # OSC sequences (can execute commands)
            r'\x1B\[\d*;\d*[Hf]',  # Cursor positioning (potential for UI manipulation)
            r'\x1Bc',  # Full reset (can clear screen inappropriately)
            r'\x1B\[2J',  # Clear screen
            r'\x1B\[K',  # Clear line
        ]
        
        # Compile dangerous sequence patterns
        self.dangerous_patterns = [re.compile(pattern) for pattern in self.dangerous_sequences]
        
        # HTML tags and entities that should be escaped
        self.html_escape_needed = re.compile(r'[<>&"\']')
        
        # Maximum line length to prevent DoS
        self.max_line_length = 10000
        
        # Maximum total output size per message
        self.max_output_size = 64 * 1024  # 64KB
    
    def sanitize_terminal_output(self, data: Union[str, bytes]) -> str:
        """Sanitize terminal output for safe transmission to browser."""
        try:
            # Convert bytes to string if needed
            if isinstance(data, bytes):
                try:
                    text = data.decode('utf-8', errors='replace')
                except UnicodeDecodeError:
                    # If UTF-8 fails, try latin-1 as fallback
                    text = data.decode('latin-1', errors='replace')
            else:
                text = data
            
            # 1. Check output size
            if len(text) > self.max_output_size:
                logger.warning("Terminal output too large, truncating", 
                             size=len(text), max_size=self.max_output_size)
                text = text[:self.max_output_size] + "\n[OUTPUT TRUNCATED - SIZE LIMIT EXCEEDED]\n"
            
            # 2. Split into lines and check line length
            lines = text.split('\n')
            sanitized_lines = []
            
            for line in lines:
                if len(line) > self.max_line_length:
                    logger.warning("Terminal line too long, truncating", 
                                 length=len(line), max_length=self.max_line_length)
                    line = line[:self.max_line_length] + "[TRUNCATED]"
                
                sanitized_line = self._sanitize_line(line)
                sanitized_lines.append(sanitized_line)
            
            sanitized_text = '\n'.join(sanitized_lines)
            
            # 3. Final HTML escaping for any remaining dangerous characters
            sanitized_text = self._escape_html_entities(sanitized_text)
            
            return sanitized_text
            
        except Exception as e:
            logger.error("Error sanitizing terminal output", error=str(e))
            # Return safe fallback
            return "[ERROR: Unable to sanitize terminal output safely]"
    
    def _sanitize_line(self, line: str) -> str:
        """Sanitize a single line of terminal output."""
        # 1. Remove dangerous control characters
        line = self.dangerous_control_chars.sub('', line)
        
        # 2. Check for dangerous escape sequences
        for pattern in self.dangerous_patterns:
            if pattern.search(line):
                logger.warning("Dangerous terminal sequence detected and removed")
                line = pattern.sub('', line)
        
        # 3. Sanitize ANSI escape sequences (keep safe ones, remove dangerous ones)
        line = self._sanitize_ansi_sequences(line)
        
        return line
    
    def _sanitize_ansi_sequences(self, text: str) -> str:
        """Sanitize ANSI escape sequences, keeping safe color/formatting ones."""
        # Define safe ANSI sequences (colors, basic formatting)
        safe_ansi_patterns = [
            r'\x1B\[([0-9]{1,2}(;[0-9]{1,2})*)?m',  # Color and formatting
            r'\x1B\[([0-9]+)?[ABCD]',  # Cursor movement (limited)
            r'\x1B\[([0-9]+)?[GH]',   # Cursor positioning (limited)
        ]
        
        # First, extract all ANSI sequences
        all_sequences = self.ansi_escape_pattern.findall(text)
        
        # Remove all ANSI sequences first
        cleaned_text = self.ansi_escape_pattern.sub('', text)
        
        # Add back only safe sequences
        for sequence in all_sequences:
            is_safe = False
            for safe_pattern in safe_ansi_patterns:
                if re.match(safe_pattern, sequence):
                    is_safe = True
                    break
            
            if is_safe:
                # Add back the safe sequence at the beginning of the text
                cleaned_text = sequence + cleaned_text
                break  # Only add one sequence to avoid accumulation
        
        return cleaned_text
    
    def _escape_html_entities(self, text: str) -> str:
        """Escape HTML entities to prevent XSS."""
        # Use html.escape for basic HTML escaping
        return html.escape(text, quote=True)
    
    def sanitize_terminal_input(self, data: Union[str, bytes]) -> str:
        """Sanitize terminal input to prevent command injection."""
        try:
            # Convert bytes to string if needed
            if isinstance(data, bytes):
                text = data.decode('utf-8', errors='replace')
            else:
                text = data
            
            # Check input size
            max_input_size = 1024  # 1KB max for single input
            if len(text) > max_input_size:
                logger.warning("Terminal input too large, truncating", 
                             size=len(text), max_size=max_input_size)
                text = text[:max_input_size]
            
            # Detect and prevent command injection attempts
            if self._detect_command_injection(text):
                logger.warning("Command injection attempt detected and blocked")
                return ""  # Block the entire input
            
            # Remove dangerous control characters but keep common ones
            # Keep: \n (10), \r (13), \t (9), \b (8), \x7f (DEL)
            text = re.sub(r'[\x00-\x07\x0B\x0C\x0E-\x1F]', '', text)
            
            # Remove potentially dangerous escape sequences that could affect terminal
            dangerous_input_sequences = [
                r'\x1B\].*',  # OSC sequences
                r'\x1B\[.*[~]',  # Function key sequences that might be crafted
            ]
            
            for pattern in dangerous_input_sequences:
                text = re.sub(pattern, '', text)
            
            return text
            
        except Exception as e:
            logger.error("Error sanitizing terminal input", error=str(e))
            return ""  # Return empty string for safety
    
    def _detect_command_injection(self, text: str) -> bool:
        """Detect potential command injection attempts."""
        # Common command injection patterns
        injection_patterns = [
            # Command chaining
            r'[;&|`]',  # Basic command separators
            r'\$\(',    # Command substitution
            r'`[^`]*`', # Backtick command execution
            
            # Dangerous commands
            r'\b(rm|del|format|fdisk|dd|mkfs)\b',  # File/disk destruction
            r'\b(sudo|su|passwd|chsh|chown)\b',    # Privilege escalation
            r'\b(wget|curl|nc|netcat|telnet|ssh)\b', # Network access
            r'\b(eval|exec|source|\.|\\)\b',       # Code execution
            r'\b(chmod|chattr|setfacl)\b',         # Permission changes
            
            # File access patterns
            r'\/etc\/',      # System configuration
            r'\/proc\/',     # Process information
            r'\/sys\/',      # System information
            r'\/dev\/',      # Device files
            r'\/root\/',     # Root directory
            r'\.\.\/+',      # Directory traversal
            
            # Shell expansion
            r'\*',           # Wildcard expansion
            r'\$\{[^}]*\}',  # Variable expansion
            r'\$[A-Za-z_]',  # Variable access
            
            # Redirection and pipes that could be dangerous
            r'>\s*\/\w+',    # Output redirection to system paths
            r'<\s*\/\w+',    # Input redirection from system paths
            
            # Programming language execution
            r'\b(python|perl|ruby|php|node|bash|sh|zsh|fish)\b\s+', # Script execution
        ]
        
        # Check for dangerous patterns
        for pattern in injection_patterns:
            if re.search(pattern, text, re.IGNORECASE):
                logger.warning("Command injection pattern detected", 
                             pattern=pattern, 
                             text_snippet=text[:50])
                return True
        
        # Check for suspicious character combinations
        suspicious_chars = text.count(';') + text.count('|') + text.count('&')
        if suspicious_chars > 2:  # Allow some normal usage
            logger.warning("High number of command separators detected", count=suspicious_chars)
            return True
        
        # Check for long sequences of special characters (potential obfuscation)
        special_char_sequences = re.findall(r'[!@#$%^&*()+={}[\]|\\:";\'<>?/~`]+', text)
        for sequence in special_char_sequences:
            if len(sequence) > 5:  # Long sequences are suspicious
                logger.warning("Long special character sequence detected", sequence=sequence)
                return True
        
        return False
    
    def detect_malicious_patterns(self, text: str) -> Dict[str, Any]:
        """Detect potentially malicious patterns in terminal data."""
        threats = {
            "detected": False,
            "threats": [],
            "severity": "low"
        }
        
        # Patterns that could indicate attacks
        malicious_patterns = {
            "script_injection": r'<script[^>]*>.*?</script>',
            "html_injection": r'<[^>]+>',
            "url_schemes": r'(javascript|data|vbscript)://',
            "command_execution": r'(\$\(|\`|eval\()',
            "file_access": r'(\.\.\/|\/etc\/|\/proc\/)',
            "escape_sequences": r'\x1B\][^;]*;[^;]*\x07',  # OSC sequences
        }
        
        for threat_type, pattern in malicious_patterns.items():
            if re.search(pattern, text, re.IGNORECASE):
                threats["detected"] = True
                threats["threats"].append(threat_type)
                
                # Determine severity
                if threat_type in ["script_injection", "command_execution", "escape_sequences"]:
                    threats["severity"] = "high"
                elif threat_type in ["html_injection", "url_schemes"]:
                    threats["severity"] = "medium"
        
        if threats["detected"]:
            logger.warning("Malicious patterns detected", 
                         threats=threats["threats"], 
                         severity=threats["severity"])
        
        return threats


# Global sanitizer instance
_terminal_sanitizer = None


def get_terminal_sanitizer() -> TerminalOutputSanitizer:
    """Get global terminal sanitizer instance."""
    global _terminal_sanitizer
    if _terminal_sanitizer is None:
        _terminal_sanitizer = TerminalOutputSanitizer()
    return _terminal_sanitizer