"""Main VM Manager service orchestrating all VM operations."""

import asyncio
from typing import Dict, Optional
import structlog

from config.settings import settings, validate_all

logger = structlog.get_logger(__name__)


class VMManagerService:
    """Main service class for managing VM operations."""
    
    def __init__(self):
        self.initialized = False
        self.active_vms: Dict[str, Dict] = {}
        
    async def initialize(self):
        """Initialize the VM Manager service."""
        logger.info("Initializing VM Manager Service")
        
        try:
            # Run configuration validation
            validate_all()
            
            # Initialize components
            await self._initialize_components()
            
            self.initialized = True
            logger.info("VM Manager Service initialized successfully")
            
        except Exception as e:
            logger.error("Failed to initialize VM Manager Service", error=str(e))
            raise
    
    async def _initialize_components(self):
        """Initialize all service components."""
        # This will be expanded as we implement each component
        logger.info("Initializing service components", mock_mode=settings.use_mock_firecracker)
        
    async def cleanup(self):
        """Cleanup resources on shutdown."""
        logger.info("Cleaning up VM Manager Service")
        
        try:
            # Stop all active VMs gracefully
            for vm_id in list(self.active_vms.keys()):
                logger.info("Stopping VM during cleanup", vm_id=vm_id)
                # This will be implemented when we have the full VM management
                
            self.active_vms.clear()
            logger.info("VM Manager Service cleanup complete")
            
        except Exception as e:
            logger.error("Error during VM Manager cleanup", error=str(e))