#!/bin/bash
# Install Lima additional guest agents and setup Firecracker
# 
# This script installs the required Lima guest agents and creates a working environment

set -e

echo "🔧 Installing Lima additional guest agents..."

# Ensure Homebrew is in PATH
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

# Install the additional guest agents package
echo "📦 Installing lima-additional-guestagents..."
brew install lima-additional-guestagents

echo "✅ Lima guest agents installed successfully!"

echo "🧹 Cleaning up any existing instances..."
limactl stop firecracker-dev --force 2>/dev/null || true
limactl delete firecracker-dev --force 2>/dev/null || true
sleep 2

echo "🏗️  Creating Lima VM with guest agents..."

# Create a working YAML configuration
cat > /tmp/firecracker-with-agents.yaml << 'EOF'
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
provision:
  - mode: system
    script: |
      apt-get update
      apt-get install -y curl wget jq socat python3 python3-pip build-essential
      cd /tmp
      curl -LOJ "https://github.com/firecracker-microvm/firecracker/releases/download/v1.4.1/firecracker-v1.4.1-x86_64.tgz"
      tar -xzf "firecracker-v1.4.1-x86_64.tgz"
      cp "release-v1.4.1-x86_64/firecracker-v1.4.1-x86_64" /usr/local/bin/firecracker
      cp "release-v1.4.1-x86_64/jailer-v1.4.1-x86_64" /usr/local/bin/jailer
      chmod +x /usr/local/bin/firecracker /usr/local/bin/jailer
      mkdir -p /tmp/firecracker-sockets /tmp/firecracker-images
      chmod 755 /tmp/firecracker-sockets /tmp/firecracker-images
      cd /tmp/firecracker-images
      echo "FIRECRACKER_TEST_KERNEL" > vmlinux
      echo "FIRECRACKER_TEST_ROOTFS" > rootfs.ext4
      chmod 644 vmlinux rootfs.ext4
      echo "Firecracker installation completed"
      firecracker --version
  - mode: user
    script: |
      echo 'export PATH="/usr/local/bin:$PATH"' >> ~/.bashrc
      echo 'alias fc="firecracker"' >> ~/.bashrc
      echo 'export FIRECRACKER_SOCKET_DIR="/tmp/firecracker-sockets"' >> ~/.bashrc
      echo 'export FIRECRACKER_IMAGES_DIR="/tmp/firecracker-images"' >> ~/.bashrc
      echo "User environment configured"
EOF

echo "🚀 Starting Lima VM with guest agents..."
limactl start --name=firecracker-dev /tmp/firecracker-with-agents.yaml

echo "⏳ Waiting for VM to be fully ready..."
sleep 30

echo "🧪 Testing the environment..."

# Test basic connectivity
echo "Testing VM connectivity..."
if limactl shell firecracker-dev echo "Hello from Firecracker VM"; then
    echo "✅ VM connectivity working"
else
    echo "❌ VM connectivity failed"
    exit 1
fi

# Test Firecracker
echo "Testing Firecracker installation..."
if limactl shell firecracker-dev firecracker --version; then
    echo "✅ Firecracker is working"
else
    echo "❌ Firecracker test failed"
    exit 1
fi

# Test project mount
echo "Testing project mount..."
if limactl shell firecracker-dev test -d /build; then
    echo "✅ Project directory mounted"
else
    echo "❌ Project mount failed"
    exit 1
fi

# Test directories
echo "Testing Firecracker directories..."
if limactl shell firecracker-dev test -d /tmp/firecracker-sockets; then
    echo "✅ Firecracker directories created"
else
    echo "❌ Directory creation failed"
    exit 1
fi

echo ""
echo "🎉 SUCCESS! Firecracker development environment is working!"
echo ""
echo "📋 Environment ready:"
echo "   ✅ Lima VM: firecracker-dev"
echo "   ✅ Guest agents: installed"
echo "   ✅ Firecracker: $(limactl shell firecracker-dev firecracker --version 2>/dev/null || echo 'installed')"
echo "   ✅ Project mount: /build"
echo "   ✅ Socket directory: /tmp/firecracker-sockets"
echo ""
echo "🚀 Try it out:"
echo "   # Access the VM"
echo "   limactl shell firecracker-dev"
echo ""
echo "   # Test Firecracker"
echo "   limactl shell firecracker-dev firecracker --version"
echo ""
echo "   # View your project"
echo "   limactl shell firecracker-dev ls /build"
echo ""
echo "🧪 Run the basic tests:"
echo "   python3 scripts/test_firecracker_basic.py"
echo ""

# Clean up
rm -f /tmp/firecracker-with-agents.yaml

echo "✅ Firecracker environment setup completed!"
echo "🔥 Ready for VM snapshot development!"