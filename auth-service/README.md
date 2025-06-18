# Auth Service

## Overview
Dedicated authentication and authorization service providing secure user management, JWT token handling, and permission enforcement across the Build platform.

## Structure
```
auth-service/
├── main.py                 # Service entry point
├── models/                 # User and auth models
├── schemas/                # Authentication schemas
├── security/               # Security utilities and JWT
├── middleware/             # Security middleware
├── authorization/          # Permission system
├── services/               # Authentication services
├── utils/                  # Utility functions
└── tests/                  # Testing suite
```

## Key Responsibilities
- User registration and authentication
- JWT token generation and validation
- Role-based access control (RBAC)
- Session management and security
- Rate limiting and brute force protection
- Password security and policies

## Features
- Secure password hashing with bcrypt
- JWT access and refresh token management
- Multi-factor authentication support
- Account lockout and security policies
- Password reset with email verification
- OAuth integration capabilities

## Security Features
- Brute force attack protection
- Account lockout mechanisms
- Rate limiting on auth endpoints
- Secure token storage and rotation
- Password complexity enforcement
- Session hijacking prevention

## Authorization System
- Role-based permissions
- Resource-level access control
- Permission inheritance
- Dynamic permission checking
- Administrative controls

## Integration
- WebSocket authentication for real-time connections
- API gateway integration
- Service-to-service authentication
- External OAuth provider support
- Audit logging for security events