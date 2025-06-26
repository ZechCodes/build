#!/bin/bash
# Simple wrapper to run the Firecracker test with proper environment

set -e

echo "🚀 Running Firecracker environment test..."

# Set proper PATH
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

# Change to the Build directory
cd /Users/zech/Projects/8ly/Build

# Run the Python test script
python3 scripts/test_firecracker_basic.py

echo "🎉 Test completed!"