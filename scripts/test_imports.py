#!/usr/bin/env python3
"""
Test import paths for the Firecracker integration
"""

import sys
from pathlib import Path

# Add the project root to the Python path
project_root = Path(__file__).parent.parent
sys.path.insert(0, str(project_root))

# Add the hyphenated directories to the path directly
vm_manager_path = project_root / "vm-manager"
snapshot_manager_path = project_root / "snapshot-manager"
sys.path.insert(0, str(vm_manager_path))
sys.path.insert(0, str(snapshot_manager_path))

print(f"Project root: {project_root}")
print(f"VM Manager path: {vm_manager_path}")
print(f"Snapshot Manager path: {snapshot_manager_path}")

try:
    print("Testing firecracker module import...")
    import firecracker
    print(f"✅ firecracker module imported from: {firecracker.__file__}")
except ImportError as e:
    print(f"❌ firecracker module import failed: {e}")

try:
    print("Testing FirecrackerService import...")
    from firecracker.firecracker_service import FirecrackerService, FirecrackerError
    print(f"✅ FirecrackerService imported successfully")
except ImportError as e:
    print(f"❌ FirecrackerService import failed: {e}")

try:
    print("Testing FirecrackerVMManager import...")
    from firecracker.vm_manager_integration import FirecrackerVMManager
    print(f"✅ FirecrackerVMManager imported successfully")
except ImportError as e:
    print(f"❌ FirecrackerVMManager import failed: {e}")

try:
    print("Testing core module import...")
    import core
    print(f"✅ core module imported from: {core.__file__}")
except ImportError as e:
    print(f"❌ core module import failed: {e}")

try:
    print("Testing SnapshotManager import...")
    from core.snapshot_manager import SnapshotManager
    print(f"✅ SnapshotManager imported successfully")
except ImportError as e:
    print(f"❌ SnapshotManager import failed: {e}")

print("Import test completed!")