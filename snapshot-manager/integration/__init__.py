"""
External system integration components.

This module handles integration with external systems like Firecracker VMs,
storage backends, and other infrastructure components.
"""

from .firecracker_adapter import get_vm_manager, FirecrackerVMManager

__all__ = ["get_vm_manager", "FirecrackerVMManager"]