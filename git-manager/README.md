# Git Manager Service

## Overview
Integrates with Soft-serve Git server to provide repository management, access control, and Git operations for the Build platform.

## Structure
```
git-manager/
├── main.py                 # Service entry point
├── models/                 # Git repository models
├── services/               # Git management services
├── softserve/              # Soft-serve integration
├── access/                 # Access control management
├── operations/             # Git operation handlers
├── webhooks/               # Git webhook handling
├── utils/                  # Utility functions
└── tests/                  # Testing suite
```

## Key Responsibilities
- Git repository creation and deletion
- Access control management for repositories
- SSH key management and validation
- Clone URL generation and management
- Repository quota enforcement
- Git operations logging and auditing

## Features
- Automatic repository provisioning
- Fine-grained access control
- SSH key lifecycle management
- Repository templates and initialization
- Branch protection and policies
- Webhook integration for CI/CD

## Soft-serve Integration
- API integration for repository management
- SSH key distribution and validation
- User permission mapping
- Repository configuration management
- Backup and synchronization

## Security
- SSH key validation and security checks
- Repository isolation between users
- Access control enforcement
- Git hook restrictions for security
- Large file detection and prevention
- Malicious repository scanning

## Quota Management
- Repository size limits
- File count restrictions
- Bandwidth usage tracking
- Storage cleanup policies
- Usage reporting and alerts