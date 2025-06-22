"""
Security Module Initialization

Ensures all security components are properly configured and initialized.
"""
import os
import sys

# Ensure security path is in Python path
current_dir = os.path.dirname(__file__)
if current_dir not in sys.path:
    sys.path.insert(0, current_dir)

# Import and initialize security components
from .input_sanitizer import input_sanitizer, privilege_validator

# Configure security settings
def initialize_security(config=None):
    """Initialize security components with configuration"""
    if config:
        # Configure admin users if provided
        if 'admin_users' in config:
            privilege_validator.admin_users.update(config['admin_users'])
    
    return {
        'input_sanitizer': input_sanitizer,
        'privilege_validator': privilege_validator
    }

__all__ = ['input_sanitizer', 'privilege_validator', 'initialize_security']