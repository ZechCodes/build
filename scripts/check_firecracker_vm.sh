#!/bin/bash
# Check Firecracker VM status and continue setup
# 
# This script checks if the Lima VM is running and tests Firecracker

set -e

echo "🔍 Checking Firecracker VM status..."

# Ensure Homebrew is in PATH
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

echo "📋 Lima VM list:"
limactl list

echo ""
echo "🧪 Testing VM accessibility..."
if limactl shell firecracker-dev echo "VM is accessible"; then
    echo "✅ VM is accessible"
else
    echo "❌ VM is not accessible"
    exit 1
fi

echo ""
echo "🧪 Testing Firecracker installation..."
if limactl shell firecracker-dev firecracker --version; then
    echo "✅ Firecracker is working"
else
    echo "❌ Firecracker test failed"
    exit 1
fi

echo ""
echo "🧪 Testing project mount..."
if limactl shell firecracker-dev ls /build; then
    echo "✅ Project directory is mounted"
else
    echo "❌ Project mount failed"
    exit 1
fi

echo ""
echo "🧪 Testing Firecracker directories..."
if limactl shell firecracker-dev ls /tmp/firecracker-sockets /tmp/firecracker-images; then
    echo "✅ Firecracker directories are ready"
else
    echo "❌ Firecracker directories missing"
    exit 1
fi

echo ""
echo "🎉 Firecracker VM is ready for testing!"
echo ""
echo "📋 VM Status Summary:"
limactl shell firecracker-dev "echo 'Firecracker version:' && firecracker --version && echo 'Project files:' && ls -la /build | head -5 && echo 'Firecracker directories:' && ls -la /tmp/firecracker-*/"

echo ""
echo "🚀 Ready to run integration tests!"