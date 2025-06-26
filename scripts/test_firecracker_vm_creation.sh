#!/bin/bash
# Test Firecracker VM creation manually to debug issues
set -e

echo "🧪 TESTING FIRECRACKER VM CREATION MANUALLY"
echo "==========================================="

limactl shell firecracker-dev <<'EOF'
echo "Setting up test VM creation..."

VM_ID="manual_test_vm"
SOCKET_PATH="/tmp/firecracker-sockets/${VM_ID}.socket"

# Clean up any existing socket
rm -f "$SOCKET_PATH"

echo ""
echo "1. Creating Firecracker config..."
cat > /tmp/test_vm_config.json << 'FCCONFIG'
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
FCCONFIG

echo "Config created:"
cat /tmp/test_vm_config.json

echo ""
echo "2. Starting Firecracker process..."
firecracker --api-sock "$SOCKET_PATH" > "/tmp/${VM_ID}.log" 2>&1 &
FC_PID=$!
echo "Firecracker PID: $FC_PID"

sleep 3

echo ""
echo "3. Checking if socket was created..."
if [ -S "$SOCKET_PATH" ]; then
    echo "✅ Socket created: $SOCKET_PATH"
    ls -la "$SOCKET_PATH"
else
    echo "❌ Socket NOT created"
    echo "Firecracker log:"
    cat "/tmp/${VM_ID}.log" || echo "No log file"
    exit 1
fi

echo ""
echo "4. Testing API connection..."
if curl -s --unix-socket "$SOCKET_PATH" http://localhost/ > /dev/null; then
    echo "✅ API connection successful"
else
    echo "❌ API connection failed"
    echo "Firecracker log:"
    cat "/tmp/${VM_ID}.log"
    exit 1
fi

echo ""
echo "5. Configuring VM via API..."
curl -X PUT --unix-socket "$SOCKET_PATH" \
     -H "Content-Type: application/json" \
     -d @/tmp/test_vm_config.json \
     http://localhost/machine-config

echo ""
curl -X PUT --unix-socket "$SOCKET_PATH" \
     -H "Content-Type: application/json" \
     -d '{"kernel_image_path": "/tmp/firecracker-images/vmlinux", "boot_args": "console=ttyS0 reboot=k panic=1 pci=off"}' \
     http://localhost/boot-source

echo ""
curl -X PUT --unix-socket "$SOCKET_PATH" \
     -H "Content-Type: application/json" \
     -d '{"drive_id": "rootfs", "path_on_host": "/tmp/firecracker-images/rootfs.ext4", "is_root_device": true, "is_read_only": false}' \
     http://localhost/drives/rootfs

echo ""
echo "6. Starting VM..."
START_RESPONSE=$(curl -X PUT --unix-socket "$SOCKET_PATH" \
     -H "Content-Type: application/json" \
     -d '{"action_type": "InstanceStart"}' \
     http://localhost/actions 2>&1)

echo "Start response: $START_RESPONSE"

echo ""
echo "7. Checking VM status..."
VM_INFO=$(curl -s --unix-socket "$SOCKET_PATH" http://localhost/ 2>&1)
echo "VM info: $VM_INFO"

echo ""
echo "8. Cleanup..."
kill $FC_PID 2>/dev/null || true
rm -f "$SOCKET_PATH" /tmp/test_vm_config.json "/tmp/${VM_ID}.log"

echo ""
echo "✅ Manual VM creation test complete!"
EOF

echo "Manual test finished!"