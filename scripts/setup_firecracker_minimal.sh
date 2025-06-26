#!/bin/bash
# Minimal Firecracker setup for testing
set -e

echo "🚀 Setting up minimal Firecracker environment for testing..."

# Ensure Homebrew is in PATH
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

echo "🏗️ Creating minimal Lima VM configuration..."

cat > /tmp/firecracker-minimal.yaml << 'EOF'
vmType: "qemu"
arch: "x86_64"

images:
  - location: "https://cloud-images.ubuntu.com/releases/22.04/release/ubuntu-22.04-server-cloudimg-amd64.img"
    arch: "x86_64"

cpus: 1
memory: "1GiB"
disk: "4GiB"

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
      
      echo "📦 Installing Firecracker..."
      apt-get update
      apt-get install -y curl wget
      
      FIRECRACKER_VERSION="v1.4.1"
      cd /tmp
      
      curl -LOJ "https://github.com/firecracker-microvm/firecracker/releases/download/${FIRECRACKER_VERSION}/firecracker-${FIRECRACKER_VERSION}-x86_64.tgz"
      tar -xzf "firecracker-${FIRECRACKER_VERSION}-x86_64.tgz"
      
      cp "release-${FIRECRACKER_VERSION}-x86_64/firecracker-${FIRECRACKER_VERSION}-x86_64" /usr/local/bin/firecracker
      chmod +x /usr/local/bin/firecracker
      
      mkdir -p /tmp/firecracker-sockets /tmp/firecracker-images
      chmod 755 /tmp/firecracker-sockets /tmp/firecracker-images
      
      echo "✅ Firecracker installation completed!"
      firecracker --version
EOF

echo "🚀 Starting minimal Lima VM..."
limactl start --name=firecracker-dev /tmp/firecracker-minimal.yaml

echo "⏳ Waiting for VM to be ready..."
sleep 15

echo "🧪 Testing setup..."
if limactl shell firecracker-dev firecracker --version; then
    echo "✅ Firecracker is working in Lima VM"
else
    echo "❌ Firecracker test failed"
    exit 1
fi

rm -f /tmp/firecracker-minimal.yaml

echo ""
echo "🎉 SUCCESS! Minimal Firecracker environment ready!"
echo ""
echo "🔧 Next steps:"
echo "   1. Run: python -m pytest tests/test_real_firecracker_integration.py -v -s"
echo "   2. Test: limactl shell firecracker-dev"
echo ""