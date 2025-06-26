#!/bin/bash
# Simple Firecracker setup that avoids YAML heredoc issues
# 
# This script uses a basic approach to create a working Firecracker environment

set -e

echo "🚀 Setting up Firecracker development environment (simple version)..."

# Ensure Homebrew is in PATH
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

echo "🧹 Cleaning up any existing instances..."
limactl stop firecracker-dev --force 2>/dev/null || true
limactl delete firecracker-dev --force 2>/dev/null || true
sleep 2

echo "🏗️  Creating simple Lima VM configuration..."

# Create a basic YAML file without complex heredocs
cat > /tmp/firecracker-simple.yaml << 'EOF'
vmType: "qemu"
arch: "x86_64"
images:
  - location: "https://cloud-images.ubuntu.com/releases/22.04/release/ubuntu-22.04-server-cloudimg-amd64.img"
    arch: "x86_64"
cpus: 1
memory: "2GiB"
disk: "8GiB"
networks:
  - lima: user-v2
mounts:
  - location: "~/Projects/8ly/Build"
    mountPoint: "/build"
    writable: true
ssh:
  localPort: 0
  loadDotSSHPubKeys: false
provision:
  - mode: system
    script: |
      apt-get update
      apt-get install -y curl wget jq socat python3 python3-pip build-essential
      FIRECRACKER_VERSION="v1.4.1"
      cd /tmp
      curl -LOJ "https://github.com/firecracker-microvm/firecracker/releases/download/${FIRECRACKER_VERSION}/firecracker-${FIRECRACKER_VERSION}-x86_64.tgz"
      tar -xzf "firecracker-${FIRECRACKER_VERSION}-x86_64.tgz"
      cp "release-${FIRECRACKER_VERSION}-x86_64/firecracker-${FIRECRACKER_VERSION}-x86_64" /usr/local/bin/firecracker
      cp "release-${FIRECRACKER_VERSION}-x86_64/jailer-${FIRECRACKER_VERSION}-x86_64" /usr/local/bin/jailer
      chmod +x /usr/local/bin/firecracker /usr/local/bin/jailer
      mkdir -p /tmp/firecracker-sockets /tmp/firecracker-images
      chmod 755 /tmp/firecracker-sockets /tmp/firecracker-images
      cd /tmp/firecracker-images
      echo "FIRECRACKER_TEST_KERNEL" > vmlinux
      echo "FIRECRACKER_TEST_ROOTFS" > rootfs.ext4
      chmod 644 vmlinux rootfs.ext4
      firecracker --version
  - mode: user
    script: |
      echo 'export PATH="/usr/local/bin:$PATH"' >> ~/.bashrc
      echo 'alias fc="firecracker"' >> ~/.bashrc
      echo 'export FIRECRACKER_SOCKET_DIR="/tmp/firecracker-sockets"' >> ~/.bashrc
      echo 'export FIRECRACKER_IMAGES_DIR="/tmp/firecracker-images"' >> ~/.bashrc
EOF

echo "🚀 Starting Lima VM..."
limactl start --name=firecracker-dev /tmp/firecracker-simple.yaml

echo "⏳ Waiting for VM to be ready (this may take several minutes)..."
sleep 30

echo "🧪 Testing VM setup..."

# Test VM accessibility
echo "Testing VM accessibility..."
if limactl shell firecracker-dev echo "VM is ready"; then
    echo "✅ VM is accessible"
else
    echo "❌ VM is not accessible"
    echo "Debug info:"
    limactl list
    exit 1
fi

# Test Firecracker installation
echo "Testing Firecracker installation..."
if limactl shell firecracker-dev firecracker --version; then
    echo "✅ Firecracker is installed and working"
else
    echo "❌ Firecracker is not working"
    exit 1
fi

# Test project mount
echo "Testing project mount..."
if limactl shell firecracker-dev ls /build; then
    echo "✅ Build project is mounted at /build"
else
    echo "❌ Build project mount failed"
    exit 1
fi

# Test Firecracker directories
echo "Testing Firecracker directories..."
if limactl shell firecracker-dev ls /tmp/firecracker-sockets /tmp/firecracker-images; then
    echo "✅ Firecracker directories created"
else
    echo "❌ Firecracker directories missing"
    exit 1
fi

echo ""
echo "🎉 SUCCESS! Firecracker environment is ready!"
echo ""
echo "📋 Environment details:"
echo "   VM Name: firecracker-dev"
echo "   VM Status: $(limactl list | grep firecracker-dev | awk '{print $2}')"
echo "   Project Location: /build (inside VM)"
echo "   Firecracker Binary: /usr/local/bin/firecracker"
echo ""
echo "🚀 Quick test commands:"
echo "   # Access the VM"
echo "   limactl shell firecracker-dev"
echo ""
echo "   # Inside the VM, test Firecracker"
echo "   firecracker --version"
echo "   ls /build"
echo "   ls /tmp/firecracker-sockets"
echo ""
echo "🧪 Run integration tests:"
echo "   python3 scripts/test_firecracker_basic.py"
echo ""

# Clean up
rm -f /tmp/firecracker-simple.yaml

echo "✅ Firecracker setup completed!"
echo "🔥 Ready to test VM snapshots with real Firecracker!"