#!/usr/bin/env python3
"""
Test script for Firecracker integration with the Build platform.

This script validates that Firecracker is working correctly with Lima
and can be used for real VM snapshot operations.
"""

import asyncio
import sys
import time
from pathlib import Path

# Add the project root to the Python path
project_root = Path(__file__).parent.parent
sys.path.insert(0, str(project_root))

# Add the hyphenated directories to the path directly
vm_manager_path = project_root / "vm-manager"
snapshot_manager_path = project_root / "snapshot-manager"
sys.path.insert(0, str(vm_manager_path))
sys.path.insert(0, str(snapshot_manager_path))

try:
    from firecracker.firecracker_service import FirecrackerService, FirecrackerError
    from firecracker.vm_manager_integration import FirecrackerVMManager
except ImportError as e:
    print(f"❌ Failed to import Firecracker modules: {e}")
    print("Please ensure the vm-manager package is properly installed")
    sys.exit(1)


async def test_firecracker_basic():
    """Test basic Firecracker service functionality."""
    print("🧪 Testing Firecracker basic functionality...")
    
    try:
        firecracker = FirecrackerService()
        await firecracker.initialize()
        print("✅ Firecracker service initialized successfully")
        return True
    except FirecrackerError as e:
        print(f"❌ Firecracker initialization failed: {e}")
        return False
    except Exception as e:
        print(f"❌ Unexpected error during initialization: {e}")
        return False


async def test_vm_creation():
    """Test VM creation and basic operations."""
    print("\n🧪 Testing VM creation...")
    
    try:
        vm_manager = FirecrackerVMManager()
        await vm_manager.initialize()
        
        # Test VM creation
        vm_id = "test_vm_001"
        user_id = "test_user"
        config = {
            "cpu_count": 1,
            "memory_mb": 256
        }
        
        print(f"Creating VM {vm_id}...")
        success = await vm_manager.create_vm(vm_id, user_id, config)
        
        if success:
            print("✅ VM created successfully")
            
            # Test VM status check
            is_running = await vm_manager.is_vm_running(vm_id)
            print(f"✅ VM running status: {is_running}")
            
            # Test VM pause/resume
            print("Testing VM pause...")
            pause_success = await vm_manager.pause_vm(vm_id)
            print(f"✅ VM pause: {'success' if pause_success else 'failed'}")
            
            print("Testing VM resume...")
            resume_success = await vm_manager.resume_vm(vm_id)
            print(f"✅ VM resume: {'success' if resume_success else 'failed'}")
            
            # Cleanup
            await vm_manager.stop_vm(vm_id)
            print("✅ VM stopped")
            
            return True
        else:
            print("❌ VM creation failed")
            return False
            
    except Exception as e:
        print(f"❌ VM creation test failed: {e}")
        return False


async def test_snapshot_operations():
    """Test snapshot creation and restoration."""
    print("\n🧪 Testing snapshot operations...")
    
    try:
        vm_manager = FirecrackerVMManager()
        await vm_manager.initialize()
        
        # Create a test VM
        vm_id = "test_vm_snapshot"
        user_id = "test_user"
        config = {
            "cpu_count": 1,
            "memory_mb": 256
        }
        
        print(f"Creating VM {vm_id} for snapshot testing...")
        success = await vm_manager.create_vm(vm_id, user_id, config)
        
        if not success:
            print("❌ Failed to create VM for snapshot testing")
            return False
        
        # Wait a bit for VM to fully start
        print("Waiting for VM to be ready...")
        await asyncio.sleep(5)
        
        # Test snapshot creation
        print("Creating snapshot...")
        try:
            snapshot_data = await vm_manager.create_firecracker_snapshot(vm_id)
            print(f"✅ Snapshot created successfully (size: {len(snapshot_data)} bytes)")
            
            # Test snapshot restoration
            print("Testing snapshot restoration...")
            restore_vm_id = "test_vm_restored"
            restore_success = await vm_manager.restore_vm_from_snapshot(
                restore_vm_id, snapshot_data, config
            )
            
            if restore_success:
                print("✅ Snapshot restoration successful")
                await vm_manager.stop_vm(restore_vm_id)
            else:
                print("❌ Snapshot restoration failed")
            
            # Cleanup
            await vm_manager.stop_vm(vm_id)
            
            return restore_success
            
        except Exception as e:
            print(f"❌ Snapshot operation failed: {e}")
            await vm_manager.stop_vm(vm_id)
            return False
            
    except Exception as e:
        print(f"❌ Snapshot test setup failed: {e}")
        return False


async def test_integration_with_snapshot_manager():
    """Test integration with the Build platform's snapshot manager."""
    print("\n🧪 Testing integration with snapshot manager...")
    
    try:
        # Import the actual snapshot manager
        from core.snapshot_manager import (
            SnapshotManager, SnapshotType, SnapshotState
        )
        
        # Create mock dependencies
        class MockStorage:
            async def store_snapshot(self, snapshot_id, data):
                return f"mock://storage/{snapshot_id}"
            
            async def retrieve_snapshot(self, snapshot_id):
                return b"mock_snapshot_data"
            
            async def delete_snapshot(self, snapshot_id):
                return True
        
        class MockEncryption:
            async def encrypt_data(self, data, key):
                return data + b"_encrypted"
            
            async def decrypt_data(self, data, key):
                return data.replace(b"_encrypted", b"")
        
        class MockDatabase:
            async def save_snapshot_metadata(self, metadata):
                pass
            
            async def count_user_snapshots(self, user_id):
                return 5
            
            async def get_user_storage_usage(self, user_id):
                return 1024 * 1024 * 1024  # 1GB
        
        # Create VM manager and snapshot manager
        vm_manager = FirecrackerVMManager()
        await vm_manager.initialize()
        
        storage = MockStorage()
        encryption = MockEncryption()
        database = MockDatabase()
        
        snapshot_manager = SnapshotManager(vm_manager, storage, encryption, database)
        await snapshot_manager.initialize()
        
        # Create a test VM first
        vm_id = "integration_test_vm"
        user_id = "integration_test_user"
        config = {"cpu_count": 1, "memory_mb": 256}
        
        await vm_manager.create_vm(vm_id, user_id, config)
        
        # Test snapshot creation through snapshot manager
        print("Creating snapshot through snapshot manager...")
        snapshot_id = await snapshot_manager.create_snapshot(
            vm_id=vm_id,
            user_id=user_id,
            name="integration_test_snapshot",
            description="Test snapshot for Firecracker integration"
        )
        
        print(f"✅ Snapshot created: {snapshot_id}")
        
        # Wait for background snapshot creation to complete
        print("Waiting for snapshot creation to complete...")
        for i in range(30):  # Wait up to 30 seconds
            metadata = snapshot_manager.snapshots.get(snapshot_id)
            if metadata and metadata.state == SnapshotState.AVAILABLE:
                print("✅ Snapshot creation completed")
                break
            elif metadata and metadata.state == SnapshotState.ERROR:
                print("❌ Snapshot creation failed")
                break
            await asyncio.sleep(1)
        else:
            print("⚠️  Snapshot creation timeout")
        
        # Cleanup
        await vm_manager.stop_vm(vm_id)
        
        return True
        
    except Exception as e:
        print(f"❌ Integration test failed: {e}")
        import traceback
        traceback.print_exc()
        return False


async def main():
    """Run all Firecracker integration tests."""
    print("🚀 Starting Firecracker integration tests...")
    print("=" * 60)
    
    tests = [
        ("Basic Firecracker functionality", test_firecracker_basic),
        ("VM creation and operations", test_vm_creation),
        ("Snapshot operations", test_snapshot_operations),
        ("Snapshot manager integration", test_integration_with_snapshot_manager),
    ]
    
    results = []
    
    for test_name, test_func in tests:
        print(f"\n{'=' * 60}")
        print(f"Running test: {test_name}")
        print("=" * 60)
        
        try:
            success = await test_func()
            results.append((test_name, success))
        except Exception as e:
            print(f"❌ Test {test_name} crashed: {e}")
            results.append((test_name, False))
    
    # Print summary
    print(f"\n{'=' * 60}")
    print("TEST SUMMARY")
    print("=" * 60)
    
    passed = 0
    for test_name, success in results:
        status = "✅ PASS" if success else "❌ FAIL"
        print(f"{status} - {test_name}")
        if success:
            passed += 1
    
    print(f"\nResults: {passed}/{len(results)} tests passed")
    
    if passed == len(results):
        print("\n🎉 All tests passed! Firecracker integration is working correctly.")
        return 0
    else:
        print(f"\n⚠️  {len(results) - passed} tests failed. Check the output above for details.")
        return 1


if __name__ == "__main__":
    exit_code = asyncio.run(main())
    sys.exit(exit_code)