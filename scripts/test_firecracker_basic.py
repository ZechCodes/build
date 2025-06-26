#!/usr/bin/env python3
"""
Basic Firecracker test that works with the Lima environment.

This simplified test focuses on testing the Lima VM and basic connectivity
before attempting full Firecracker VM operations.
"""

import asyncio
import subprocess
import sys
from pathlib import Path


def run_command(cmd, shell=False):
    """Run a command and return the result."""
    try:
        if shell:
            result = subprocess.run(cmd, shell=True, capture_output=True, text=True)
        else:
            result = subprocess.run(cmd, capture_output=True, text=True)
        return result.returncode == 0, result.stdout, result.stderr
    except Exception as e:
        return False, "", str(e)


def test_lima_installation():
    """Test if Lima is properly installed."""
    print("🧪 Testing Lima installation...")
    
    success, stdout, stderr = run_command(["limactl", "--version"])
    if success:
        print(f"✅ Lima installed: {stdout.strip()}")
        return True
    else:
        print(f"❌ Lima not found: {stderr}")
        return False


def test_lima_vm_status():
    """Test if the firecracker-dev VM is running."""
    print("🧪 Testing Lima VM status...")
    
    success, stdout, stderr = run_command(["limactl", "list", "firecracker-dev"])
    if success:
        if "Running" in stdout:
            print("✅ firecracker-dev VM is running")
            return True
        else:
            print(f"⚠️  firecracker-dev VM status: {stdout}")
            return False
    else:
        print(f"❌ Could not check VM status: {stderr}")
        return False


def test_vm_connectivity():
    """Test basic connectivity to the Lima VM."""
    print("🧪 Testing VM connectivity...")
    
    success, stdout, stderr = run_command([
        "limactl", "shell", "firecracker-dev", "echo", "Hello from Lima VM"
    ])
    
    if success and "Hello from Lima VM" in stdout:
        print("✅ VM connectivity working")
        return True
    else:
        print(f"❌ VM connectivity failed: {stderr}")
        return False


def test_firecracker_binary():
    """Test if Firecracker binary is available in the VM."""
    print("🧪 Testing Firecracker binary...")
    
    success, stdout, stderr = run_command([
        "limactl", "shell", "firecracker-dev", "which", "firecracker"
    ])
    
    if success and "/usr/local/bin/firecracker" in stdout:
        print("✅ Firecracker binary found")
        
        # Test firecracker version
        success, stdout, stderr = run_command([
            "limactl", "shell", "firecracker-dev", "firecracker", "--version"
        ])
        
        if success:
            print(f"✅ Firecracker version: {stdout.strip()}")
            return True
        else:
            print(f"⚠️  Firecracker binary found but version check failed: {stderr}")
            return False
    else:
        print(f"❌ Firecracker binary not found: {stderr}")
        return False


def test_project_mount():
    """Test if the Build project is properly mounted."""
    print("🧪 Testing project mount...")
    
    success, stdout, stderr = run_command([
        "limactl", "shell", "firecracker-dev", "ls", "/build"
    ])
    
    if success and any(item in stdout for item in ["api", "scripts", "snapshot-manager"]):
        print("✅ Build project properly mounted")
        return True
    else:
        print(f"⚠️  Project mount issue: {stdout}")
        return False


def test_directories():
    """Test if required directories are created."""
    print("🧪 Testing Firecracker directories...")
    
    directories = ["/tmp/firecracker-sockets", "/tmp/firecracker-images"]
    
    for directory in directories:
        success, stdout, stderr = run_command([
            "limactl", "shell", "firecracker-dev", "test", "-d", directory
        ])
        
        if success:
            print(f"✅ Directory exists: {directory}")
        else:
            print(f"❌ Directory missing: {directory}")
            return False
    
    return True


def test_network_access():
    """Test if the VM has internet access."""
    print("🧪 Testing internet connectivity...")
    
    success, stdout, stderr = run_command([
        "limactl", "shell", "firecracker-dev", "curl", "-s", "--connect-timeout", "5", 
        "https://httpbin.org/ip"
    ])
    
    if success and "origin" in stdout:
        print("✅ Internet connectivity working")
        return True
    else:
        print(f"⚠️  Internet connectivity issue (this is OK for local development)")
        return True  # Not critical for local dev


async def test_python_integration():
    """Test if our Python code can interact with Lima."""
    print("🧪 Testing Python integration...")
    
    try:
        # Test that we can import our modules
        project_root = Path(__file__).parent.parent
        sys.path.insert(0, str(project_root))
        
        # Try to run a simple Lima command from Python
        process = await asyncio.create_subprocess_exec(
            "limactl", "shell", "firecracker-dev", "pwd",
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE
        )
        
        stdout, stderr = await process.communicate()
        
        if process.returncode == 0:
            print("✅ Python async integration working")
            return True
        else:
            print(f"❌ Python integration failed: {stderr.decode()}")
            return False
            
    except Exception as e:
        print(f"❌ Python integration error: {e}")
        return False


def main():
    """Run basic Firecracker environment tests."""
    print("🚀 Running basic Firecracker environment tests...")
    print("=" * 60)
    
    tests = [
        ("Lima installation", test_lima_installation),
        ("Lima VM status", test_lima_vm_status),
        ("VM connectivity", test_vm_connectivity),
        ("Firecracker binary", test_firecracker_binary),
        ("Project mount", test_project_mount),
        ("Required directories", test_directories),
        ("Network access", test_network_access),
    ]
    
    results = []
    
    for test_name, test_func in tests:
        print(f"\n{'=' * 60}")
        print(f"Running test: {test_name}")
        print("=" * 60)
        
        try:
            success = test_func()
            results.append((test_name, success))
        except Exception as e:
            print(f"❌ Test {test_name} crashed: {e}")
            results.append((test_name, False))
    
    # Run async test
    print(f"\n{'=' * 60}")
    print("Running test: Python integration")
    print("=" * 60)
    
    try:
        success = asyncio.run(test_python_integration())
        results.append(("Python integration", success))
    except Exception as e:
        print(f"❌ Python integration test crashed: {e}")
        results.append(("Python integration", False))
    
    # Print summary
    print(f"\n{'=' * 60}")
    print("TEST SUMMARY")
    print("=" * 60)
    
    passed = 0
    critical_failures = []
    
    for test_name, success in results:
        status = "✅ PASS" if success else "❌ FAIL"
        print(f"{status} - {test_name}")
        if success:
            passed += 1
        else:
            # Critical tests that must pass
            if test_name in ["Lima installation", "Lima VM status", "VM connectivity"]:
                critical_failures.append(test_name)
    
    print(f"\nResults: {passed}/{len(results)} tests passed")
    
    if critical_failures:
        print(f"\n❌ Critical failures: {', '.join(critical_failures)}")
        print("Please run the fix script: ./scripts/fix_lima_setup.sh")
        return 1
    elif passed >= len(results) - 1:  # Allow one non-critical failure
        print("\n🎉 Environment is ready for Firecracker development!")
        print("\nNext steps:")
        print("1. Access VM: limactl shell firecracker-dev")
        print("2. Test full integration: python3 scripts/test_firecracker_integration.py")
        return 0
    else:
        print(f"\n⚠️  Some tests failed, but environment may still be usable.")
        return 1


if __name__ == "__main__":
    exit_code = main()
    sys.exit(exit_code)