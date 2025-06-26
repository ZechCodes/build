#!/bin/bash
# Setup Firecracker locally using Lima on macOS
# 
# This script sets up a complete Firecracker development environment
# that integrates with the Build platform snapshot system.

set -e

echo "🚀 Setting up Firecracker development environment with Lima..."

# Ensure Homebrew is in PATH
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

# Check if Homebrew is available
if ! command -v brew &> /dev/null; then
    echo "❌ Homebrew not found. Please install Homebrew first:"
    echo "   /bin/bash -c \"\$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\""
    exit 1
fi

echo "✅ Homebrew found"

# Install Lima if not already installed
if ! command -v lima &> /dev/null; then
    echo "📦 Installing Lima..."
    brew install lima
else
    echo "✅ Lima already installed"
fi

# Check if Firecracker VM already exists
if limactl list | grep -q "firecracker-dev"; then
    echo "🔄 Firecracker VM already exists. Stopping and recreating..."
    limactl stop firecracker-dev || true
    limactl delete firecracker-dev || true
fi

echo "🏗️  Creating Firecracker-enabled Lima VM..."

# Create Lima VM configuration
cat > /tmp/firecracker-vm.yaml << 'EOF'
# Lima VM configuration for Firecracker development
vmType: "qemu"
arch: "x86_64"
images:
  - location: "https://cloud-images.ubuntu.com/releases/22.04/release/ubuntu-22.04-server-cloudimg-amd64.img"
    arch: "x86_64"

cpus: 4
memory: "8GiB"
disk: "20GiB"

# Enable nested virtualization for Firecracker
qemu:
  args:
    - "-enable-kvm"
    - "-cpu"
    - "host"

# Network configuration
networks:
  - lima: shared

# Mount Build project directory
mounts:
  - location: "~/Projects/8ly/Build"
    mountPoint: "/build"
    writable: true

# Provision script to install Firecracker
provision:
  - mode: system
    script: |
      #!/bin/bash
      set -e
      
      echo "🔧 Installing Firecracker and dependencies..."
      
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
        qemu-kvm \
        libvirt-daemon-system \
        libvirt-clients \
        bridge-utils
      
      # Download and install Firecracker
      FIRECRACKER_VERSION="v1.4.1"
      curl -LOJ "https://github.com/firecracker-microvm/firecracker/releases/download/${FIRECRACKER_VERSION}/firecracker-${FIRECRACKER_VERSION}-x86_64.tgz"
      tar -xzf "firecracker-${FIRECRACKER_VERSION}-x86_64.tgz"
      
      # Install Firecracker binaries
      cp "release-${FIRECRACKER_VERSION}-x86_64/firecracker-${FIRECRACKER_VERSION}-x86_64" /usr/local/bin/firecracker
      cp "release-${FIRECRACKER_VERSION}-x86_64/jailer-${FIRECRACKER_VERSION}-x86_64" /usr/local/bin/jailer
      chmod +x /usr/local/bin/firecracker /usr/local/bin/jailer
      
      # Create directories for Firecracker
      mkdir -p /tmp/firecracker-sockets
      mkdir -p /tmp/firecracker-images
      chmod 755 /tmp/firecracker-sockets /tmp/firecracker-images
      
      # Download a minimal kernel and rootfs for testing
      echo "📦 Downloading kernel and rootfs for Firecracker VMs..."
      cd /tmp/firecracker-images
      
      # Download kernel
      wget -O vmlinux "https://s3.amazonaws.com/spec.ccfc.min/img/quickstart_guide/x86_64/kernels/vmlinux.bin"
      
      # Download minimal rootfs
      wget -O rootfs.ext4 "https://s3.amazonaws.com/spec.ccfc.min/img/hello/fsfiles/hello-rootfs.ext4"
      
      # Set proper permissions
      chmod 644 vmlinux rootfs.ext4
      
      echo "✅ Firecracker installation completed!"
      firecracker --version
      
  - mode: user
    script: |
      #!/bin/bash
      echo "🔧 Setting up user environment..."
      
      # Add helpful aliases
      echo 'alias fc="firecracker"' >> ~/.bashrc
      echo 'alias fclist="ps aux | grep firecracker"' >> ~/.bashrc
      echo 'export FIRECRACKER_SOCKET_DIR="/tmp/firecracker-sockets"' >> ~/.bashrc
      echo 'export FIRECRACKER_IMAGES_DIR="/tmp/firecracker-images"' >> ~/.bashrc
      
      echo "✅ User environment setup completed!"

# Port forwarding for development
portForwards:
  - guestPort: 8000
    hostPort: 8000
  - guestPort: 8080
    hostPort: 8080
  - guestPort: 5432
    hostPort: 5432
EOF

# Start Lima VM with Firecracker
echo "🚀 Starting Lima VM (this may take a few minutes)..."
limactl start --name=firecracker-dev /tmp/firecracker-vm.yaml

echo "⏳ Waiting for VM to be ready..."
sleep 10

# Test Firecracker installation
echo "🧪 Testing Firecracker installation..."
limactl shell firecracker-dev firecracker --version

echo ""
echo "🎉 Firecracker development environment is ready!"
echo ""
echo "📋 Next steps:"
echo "   1. Access the VM: limactl shell firecracker-dev"
echo "   2. Your Build project is mounted at: /build"
echo "   3. Firecracker is available at: /usr/local/bin/firecracker"
echo "   4. Test images are in: /tmp/firecracker-images/"
echo ""
echo "🔧 Useful commands:"
echo "   - List VMs: limactl list"
echo "   - Stop VM: limactl stop firecracker-dev"
echo "   - SSH to VM: limactl shell firecracker-dev"
echo "   - Copy files: limactl copy"
echo ""

# Clean up temporary file
rm -f /tmp/firecracker-vm.yaml

echo "✅ Setup completed successfully!"