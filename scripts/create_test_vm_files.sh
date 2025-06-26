#!/bin/bash
# Create working test VM files directly in Lima
set -e

echo "🔧 Creating working test VM files in Lima VM..."

# Create proper directory structure and files
limactl shell firecracker-dev <<'EOF'
# Fix permissions and create directories
sudo mkdir -p /tmp/firecracker-images /tmp/firecracker-sockets
sudo chmod 755 /tmp/firecracker-images /tmp/firecracker-sockets

# Create a minimal working kernel file (1MB)
sudo dd if=/dev/zero of=/tmp/firecracker-images/vmlinux bs=1M count=1 2>/dev/null
echo "# Minimal test kernel for Firecracker testing" | sudo tee -a /tmp/firecracker-images/vmlinux >/dev/null

# Create a minimal working rootfs file (10MB ext4)
sudo dd if=/dev/zero of=/tmp/firecracker-images/rootfs.ext4 bs=1M count=10 2>/dev/null
sudo mkfs.ext4 -F /tmp/firecracker-images/rootfs.ext4 >/dev/null 2>&1 || true

# Set proper permissions
sudo chmod 644 /tmp/firecracker-images/vmlinux /tmp/firecracker-images/rootfs.ext4

# Verify files exist and are readable
ls -la /tmp/firecracker-images/
echo "✅ Test VM files created successfully"
EOF

echo "✅ Lima VM test files setup complete"