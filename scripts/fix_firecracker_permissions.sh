#!/bin/bash
# Fix Firecracker permissions in Lima VM
set -e

echo "🔧 FIXING FIRECRACKER PERMISSIONS"
echo "================================="

limactl shell firecracker-dev <<'EOF'
echo "Current user: $(whoami)"
echo "Current permissions:"
ls -la /tmp/firecracker-sockets/
ls -la /tmp/firecracker-images/

echo ""
echo "Fixing permissions..."
sudo chown -R $(whoami):$(whoami) /tmp/firecracker-sockets /tmp/firecracker-images
sudo chmod -R 755 /tmp/firecracker-sockets /tmp/firecracker-images

echo ""
echo "New permissions:"
ls -la /tmp/firecracker-sockets/
ls -la /tmp/firecracker-images/

echo ""
echo "Testing socket creation as current user..."
touch /tmp/firecracker-sockets/test.socket
ls -la /tmp/firecracker-sockets/test.socket
rm -f /tmp/firecracker-sockets/test.socket
echo "✅ Socket creation now works!"

echo ""
echo "Testing Firecracker can create socket..."
timeout 3s firecracker --api-sock /tmp/firecracker-sockets/debug.socket &
sleep 1
if [ -S /tmp/firecracker-sockets/debug.socket ]; then
    echo "✅ Firecracker can create sockets!"
    rm -f /tmp/firecracker-sockets/debug.socket
else
    echo "❌ Still having issues..."
fi
pkill firecracker || true
EOF

echo "✅ Permissions fixed!"