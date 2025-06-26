#!/usr/bin/env python3
"""
Direct import test without shell dependencies
"""

import sys
import os
from pathlib import Path

# Ensure we're working from the correct directory
project_root = Path("/Users/zech/Projects/8ly/Build")
os.chdir(str(project_root))
sys.path.insert(0, str(project_root))

print(f"Project root: {project_root}")
print(f"Current working directory: {os.getcwd()}")
print(f"Python path includes project root: {str(project_root) in sys.path}")
print("=" * 50)

# Test vm_manager import
try:
    print("Testing vm_manager import...")
    import vm_manager
    print(f"✅ vm_manager imported from: {vm_manager.__file__}")
except ImportError as e:
    print(f"❌ vm_manager import failed: {e}")
except Exception as e:
    print(f"❌ vm_manager import error: {e}")

print("-" * 30)

# Test vm_manager.firecracker import  
try:
    print("Testing vm_manager.firecracker import...")
    import vm_manager.firecracker
    print(f"✅ vm_manager.firecracker imported from: {vm_manager.firecracker.__file__}")
except ImportError as e:
    print(f"❌ vm_manager.firecracker import failed: {e}")
except Exception as e:
    print(f"❌ vm_manager.firecracker import error: {e}")

print("-" * 30)

# Test FirecrackerService import
try:
    print("Testing FirecrackerService import...")
    from vm_manager.firecracker.firecracker_service import FirecrackerService, FirecrackerError
    print(f"✅ FirecrackerService imported successfully")
except ImportError as e:
    print(f"❌ FirecrackerService import failed: {e}")
except Exception as e:
    print(f"❌ FirecrackerService import error: {e}")

print("-" * 30)

# Test FirecrackerVMManager import
try:
    print("Testing FirecrackerVMManager import...")
    from vm_manager.firecracker.vm_manager_integration import FirecrackerVMManager
    print(f"✅ FirecrackerVMManager imported successfully")
except ImportError as e:
    print(f"❌ FirecrackerVMManager import failed: {e}")
except Exception as e:
    print(f"❌ FirecrackerVMManager import error: {e}")

print("-" * 30)

# Test snapshot_manager import
try:
    print("Testing snapshot_manager import...")
    from snapshot_manager.core.snapshot_manager import SnapshotManager
    print(f"✅ SnapshotManager imported successfully")
except ImportError as e:
    print(f"❌ SnapshotManager import failed: {e}")
except Exception as e:
    print(f"❌ SnapshotManager import error: {e}")

print("=" * 50)
print("Import test completed!")

# Additional debugging - check what's in vm_manager directory
print("\nDebugging info:")
vm_manager_path = project_root / "vm-manager"
print(f"vm-manager directory exists: {vm_manager_path.exists()}")
if vm_manager_path.exists():
    print(f"Contents of vm-manager: {list(vm_manager_path.iterdir())}")
    
    firecracker_path = vm_manager_path / "firecracker"
    print(f"firecracker directory exists: {firecracker_path.exists()}")
    if firecracker_path.exists():
        print(f"Contents of firecracker: {list(firecracker_path.iterdir())}")