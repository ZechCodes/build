#!/bin/bash
# Final Firecracker setup script with clean YAML
# 
# This script creates a working Firecracker environment with proper YAML formatting

set -e

echo "🚀 Setting up Firecracker development environment (final version)..."

# Ensure Homebrew is in PATH
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

echo "🧹 Cleaning up any existing instances..."
limactl stop firecracker-dev --force || true
limactl delete firecracker-dev --force || true
sleep 2

echo "🏗️  Creating Lima VM configuration..."

# Create the YAML configuration file
cat > /tmp/firecracker-final.yaml << 'EOF'
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
      #!/bin/bash
      set -e
      
      echo "🔧 Installing system dependencies..."
      apt-get update
      apt-get install -y curl wget jq socat python3 python3-pip build-essential unzip
      
      echo "📦 Installing Firecracker..."
      FIRECRACKER_VERSION="v1.4.1"
      cd /tmp
      
      curl -LOJ "https://github.com/firecracker-microvm/firecracker/releases/download/${FIRECRACKER_VERSION}/firecracker-${FIRECRACKER_VERSION}-x86_64.tgz"
      tar -xzf "firecracker-${FIRECRACKER_VERSION}-x86_64.tgz"
      
      cp "release-${FIRECRACKER_VERSION}-x86_64/firecracker-${FIRECRACKER_VERSION}-x86_64" /usr/local/bin/firecracker
      cp "release-${FIRECRACKER_VERSION}-x86_64/jailer-${FIRECRACKER_VERSION}-x86_64" /usr/local/bin/jailer
      chmod +x /usr/local/bin/firecracker /usr/local/bin/jailer
      
      mkdir -p /tmp/firecracker-sockets /tmp/firecracker-images /opt/firecracker
      chmod 755 /tmp/firecracker-sockets /tmp/firecracker-images /opt/firecracker
      
      cd /tmp/firecracker-images
      echo "FIRECRACKER_TEST_KERNEL" > vmlinux
      echo "FIRECRACKER_TEST_ROOTFS" > rootfs.ext4
      chmod 644 vmlinux rootfs.ext4
      
      echo "✅ Firecracker installation completed!"
      firecracker --version
      
  - mode: user
    script: |
      #!/bin/bash
      echo "🔧 Setting up user environment..."
      
      cat >> ~/.bashrc << 'BASHRC_END'
      alias fc="firecracker"
      alias fclist="ps aux | grep firecracker"
      alias fctest="firecracker --version"
      export FIRECRACKER_SOCKET_DIR="/tmp/firecracker-sockets"
      export FIRECRACKER_IMAGES_DIR="/tmp/firecracker-images"
      export PATH="/usr/local/bin:$PATH"
      BASHRC_END
      
      echo "✅ User environment setup completed!"
EOF

echo "🚀 Starting Lima VM..."
limactl start --name=firecracker-dev /tmp/firecracker-final.yaml

echo "⏳ Waiting for VM to be ready..."
sleep 25

echo "🧪 Testing VM setup..."

# Test VM accessibility
echo "Testing VM accessibility..."
if limactl shell firecracker-dev echo "VM accessible"; then
    echo "✅ VM is accessible"
else
    echo "❌ VM accessibility test failed"
    exit 1
fi

# Test Firecracker installation
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
    echo "✅ Project directory is mounted"
else
    echo "❌ Project mount test failed"
    exit 1
fi

# Test directories
echo "Testing Firecracker directories..."
if limactl shell firecracker-dev test -d /tmp/firecracker-sockets && limactl shell firecracker-dev test -d /tmp/firecracker-images; then
    echo "✅ Firecracker directories are ready"
else
    echo "❌ Firecracker directories test failed"
    exit 1
fi

echo ""
echo "🎉 SUCCESS! Firecracker development environment is ready!"
echo ""
echo "📋 What's available:"
echo "   ✅ Lima VM: firecracker-dev"
echo "   ✅ Firecracker binary: /usr/local/bin/firecracker"
echo "   ✅ Project mount: /build"
echo "   ✅ Socket directory: /tmp/firecracker-sockets"
echo "   ✅ Images directory: /tmp/firecracker-images"
echo ""
echo "🚀 Next steps:"
echo "   1. Test the environment: python3 scripts/test_firecracker_basic.py"
echo "   2. Access the VM: limactl shell firecracker-dev"
echo "   3. Test Firecracker integration: python3 scripts/test_firecracker_integration.py"
echo ""
echo "🔧 Quick commands:"
echo "   - VM status: limactl list"
echo "   - Access VM: limactl shell firecracker-dev"
echo "   - Stop VM: limactl stop firecracker-dev"
echo "   - Start VM: limactl start firecracker-dev"
echo ""

# Clean up
rm -f /tmp/firecracker-final.yaml

echo "✅ Firecracker setup completed successfully!"
echo "🔥 Ready for snapshot development with real Firecracker VMs!"