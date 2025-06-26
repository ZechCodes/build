#!/bin/bash
# Clean up existing Lima instance and create a fresh Firecracker environment
# 
# This script completely removes the old instance and creates a new working one

set -e

echo "🧹 Cleaning up existing Lima instance..."

# Force stop and delete the existing instance
limactl stop firecracker-dev --force || true
limactl delete firecracker-dev --force || true

# Wait a moment for cleanup
sleep 2

echo "🏗️  Creating fresh Lima VM with simplified networking..."

cat > /tmp/firecracker-clean.yaml << 'EOF'
# Clean Lima VM configuration for Firecracker development
vmType: "qemu"
arch: "x86_64"

# Use Ubuntu 22.04 LTS
images:
  - location: "https://cloud-images.ubuntu.com/releases/22.04/release/ubuntu-22.04-server-cloudimg-amd64.img"
    arch: "x86_64"

# VM resources - conservative for compatibility
cpus: 1
memory: "2GiB"
disk: "8GiB"

# Use user-v2 networking (no socket_vmnet required)
networks:
  - lima: user-v2

# Mount Build project directory
mounts:
  - location: "~/Projects/8ly/Build"
    mountPoint: "/build"
    writable: true

# SSH settings
ssh:
  localPort: 0
  loadDotSSHPubKeys: false

# Provision script to install Firecracker
provision:
  - mode: system
    script: |
      #!/bin/bash
      set -e
      
      echo "🔧 Installing system dependencies..."
      
      # Update package list
      apt-get update
      
      # Install required packages
      apt-get install -y \
        curl \
        wget \
        jq \
        socat \
        python3 \
        python3-pip \
        build-essential \
        unzip
      
      echo "📦 Installing Firecracker..."
      
      # Download and install Firecracker
      FIRECRACKER_VERSION="v1.4.1"
      cd /tmp
      
      echo "Downloading Firecracker ${FIRECRACKER_VERSION}..."
      curl -LOJ "https://github.com/firecracker-microvm/firecracker/releases/download/${FIRECRACKER_VERSION}/firecracker-${FIRECRACKER_VERSION}-x86_64.tgz"
      
      echo "Extracting Firecracker..."
      tar -xzf "firecracker-${FIRECRACKER_VERSION}-x86_64.tgz"
      
      # Install Firecracker binaries
      echo "Installing Firecracker binaries..."
      cp "release-${FIRECRACKER_VERSION}-x86_64/firecracker-${FIRECRACKER_VERSION}-x86_64" /usr/local/bin/firecracker
      cp "release-${FIRECRACKER_VERSION}-x86_64/jailer-${FIRECRACKER_VERSION}-x86_64" /usr/local/bin/jailer
      chmod +x /usr/local/bin/firecracker /usr/local/bin/jailer
      
      # Create directories for Firecracker
      echo "Setting up Firecracker directories..."
      mkdir -p /tmp/firecracker-sockets
      mkdir -p /tmp/firecracker-images
      mkdir -p /opt/firecracker
      chmod 755 /tmp/firecracker-sockets /tmp/firecracker-images /opt/firecracker
      
      # Create simple test files for development
      echo "📦 Creating test kernel and rootfs for development..."
      cd /tmp/firecracker-images
      
      # Create minimal test kernel (placeholder)
      echo "FIRECRACKER_TEST_KERNEL_v1.4.1" > vmlinux
      echo "FIRECRACKER_TEST_ROOTFS_v1.4.1" > rootfs.ext4
      chmod 644 vmlinux rootfs.ext4
      
      # Create basic Firecracker configuration template
      cat > /opt/firecracker/vm-config-template.json << 'TEMPLATE'
{
  "boot-source": {
    "kernel_image_path": "/tmp/firecracker-images/vmlinux",
    "boot_args": "console=ttyS0 reboot=k panic=1 pci=off"
  },
  "drives": [
    {
      "drive_id": "rootfs",
      "path_on_host": "/tmp/firecracker-images/rootfs.ext4",
      "is_root_device": true,
      "is_read_only": false
    }
  ],
  "machine-config": {
    "vcpu_count": 1,
    "mem_size_mib": 256
  }
}
TEMPLATE
      
      echo "✅ Firecracker installation completed!"
      firecracker --version
      
  - mode: user
    script: |
      #!/bin/bash
      echo "🔧 Setting up user environment..."
      
      # Add helpful aliases and environment variables
      cat >> ~/.bashrc << 'BASHRC'
# Firecracker development aliases
alias fc="firecracker"
alias fclist="ps aux | grep firecracker"
alias fctest="firecracker --version"

# Firecracker environment variables
export FIRECRACKER_SOCKET_DIR="/tmp/firecracker-sockets"
export FIRECRACKER_IMAGES_DIR="/tmp/firecracker-images"
export FIRECRACKER_CONFIG_DIR="/opt/firecracker"
export PATH="/usr/local/bin:$PATH"

# Helper functions
fcclean() {
  echo "Cleaning up Firecracker sockets..."
  sudo rm -f /tmp/firecracker-sockets/*.socket
  echo "Done."
}

fcstatus() {
  echo "Firecracker processes:"
  ps aux | grep firecracker | grep -v grep || echo "No Firecracker processes running"
  echo ""
  echo "Socket files:"
  ls -la /tmp/firecracker-sockets/ 2>/dev/null || echo "No socket files"
}
BASHRC
      
      echo "✅ User environment setup completed!"
      echo "✅ Firecracker development environment ready!"
EOF

echo "🚀 Starting fresh Lima VM..."
limactl start --name=firecracker-dev /tmp/firecracker-clean.yaml

echo "⏳ Waiting for VM to be fully ready..."
sleep 20

# Test the installation
echo "🧪 Testing Lima VM accessibility..."
if limactl shell firecracker-dev echo "Hello from firecracker-dev VM"; then
    echo "✅ Lima VM is accessible"
else
    echo "❌ Lima VM is not accessible"
    exit 1
fi

echo "🧪 Testing Firecracker installation..."
if limactl shell firecracker-dev firecracker --version; then
    echo "✅ Firecracker is installed and working"
else
    echo "❌ Firecracker installation failed"
    exit 1
fi

echo "🧪 Testing project mount..."
if limactl shell firecracker-dev ls /build; then
    echo "✅ Build project is mounted"
else
    echo "❌ Build project mount failed"
    exit 1
fi

echo "🧪 Testing Firecracker directories..."
if limactl shell firecracker-dev ls /tmp/firecracker-sockets /tmp/firecracker-images; then
    echo "✅ Firecracker directories created"
else
    echo "❌ Firecracker directories missing"
    exit 1
fi

echo ""
echo "🎉 Firecracker development environment is ready!"
echo ""
echo "📋 Quick start:"
echo "   1. Access VM: limactl shell firecracker-dev"
echo "   2. Test environment: python3 /build/scripts/test_firecracker_basic.py"
echo "   3. Check Firecracker: firecracker --version"
echo "   4. View project: ls /build"
echo ""
echo "🔧 Useful commands:"
echo "   - VM status: limactl list"
echo "   - VM info: limactl info firecracker-dev"
echo "   - Stop VM: limactl stop firecracker-dev"
echo "   - Start VM: limactl start firecracker-dev"
echo ""
echo "🧪 Inside VM commands:"
echo "   - fcstatus: Check Firecracker processes and sockets"
echo "   - fcclean: Clean up Firecracker sockets"
echo "   - fctest: Test Firecracker binary"
echo ""

# Clean up temporary file
rm -f /tmp/firecracker-clean.yaml

echo "✅ Setup completed successfully!"
echo "🚀 Ready to test Firecracker integration!"