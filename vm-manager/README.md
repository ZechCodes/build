# VM Manager Service

## Overview
Dedicated service for managing Firecracker virtual machine lifecycle, including creation, configuration, monitoring, and cleanup. Provides isolation and security for VM operations.

## Structure
```
vm-manager/
├── main.py                 # Service entry point
├── models/                 # VM data models
├── services/               # VM management services
├── firecracker/            # Firecracker integration
├── networking/             # Network configuration
├── storage/                # Storage management
├── monitoring/             # VM health monitoring
├── config/                 # Configuration management
├── utils/                  # Utility functions
└── tests/                  # Testing suite
```

## Key Responsibilities
- Firecracker VM lifecycle management
- Network allocation and TAP device management
- Resource monitoring and limits enforcement
- VM health checks and recovery
- Root filesystem management
- Security policy enforcement

## Features
- Automatic VM provisioning
- Resource quota enforcement
- Network isolation
- Security container integration
- Performance monitoring
- Automatic cleanup procedures

## Dependencies
- Firecracker binary
- Python asyncio for concurrent operations
- Network configuration tools
- Storage management utilities
- System monitoring libraries

## Security
- SELinux/AppArmor profile enforcement
- Resource limit enforcement via cgroups
- Network isolation between VMs
- Secure VM configuration templates

## Development
Includes comprehensive testing with mock Firecracker instances for development environments.