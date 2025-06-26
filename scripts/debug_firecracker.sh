#!/bin/bash
# Debug Firecracker setup in Lima VM
set -e

echo "🔍 DEBUGGING FIRECRACKER SETUP"
echo "=============================="

echo "1. Checking Lima VM status..."
limactl list

echo ""
echo "2. Checking firecracker-dev VM..."
limactl shell firecracker-dev <<'EOF'
echo "User: $(whoami)"
echo "Groups: $(groups)"
echo ""
echo "Firecracker binary:"
which firecracker
firecracker --version
echo ""
echo "Socket directory:"
ls -la /tmp/firecracker-sockets/
echo ""
echo "Images directory:"
ls -la /tmp/firecracker-images/
echo ""
echo "Testing socket creation:"
touch /tmp/firecracker-sockets/test.socket
ls -la /tmp/firecracker-sockets/test.socket
rm -f /tmp/firecracker-sockets/test.socket
echo "✅ Socket creation works"
echo ""
echo "Testing Firecracker startup (quick test):"
timeout 5s firecracker --api-sock /tmp/firecracker-sockets/debug.socket || echo "Firecracker started (timeout expected)"
ls -la /tmp/firecracker-sockets/debug.socket 2>/dev/null && echo "✅ Socket created by Firecracker" || echo "❌ Socket NOT created"
rm -f /tmp/firecracker-sockets/debug.socket
EOF

echo ""
echo "🔍 Debug complete!"