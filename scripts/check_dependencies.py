#!/usr/bin/env python3
"""
Check if required dependencies are available for Firecracker integration
"""

import sys

required_packages = [
    'structlog',
    'logfire',
    'httpx',
    'aiofiles',
    'asyncio',  # Built-in
    'json',     # Built-in
    'subprocess', # Built-in
    'uuid',     # Built-in
    'time',     # Built-in
    'pathlib',  # Built-in
]

print("🔍 Checking Python dependencies...")

missing_packages = []
available_packages = []

for package in required_packages:
    try:
        __import__(package)
        available_packages.append(package)
        print(f"✅ {package}")
    except ImportError:
        missing_packages.append(package)
        print(f"❌ {package} - MISSING")

print(f"\n📊 Summary:")
print(f"✅ Available: {len(available_packages)}")
print(f"❌ Missing: {len(missing_packages)}")

if missing_packages:
    print(f"\n📦 Install missing packages with:")
    print(f"pip install {' '.join(missing_packages)}")
    sys.exit(1)
else:
    print(f"\n🎉 All required dependencies are available!")
    sys.exit(0)