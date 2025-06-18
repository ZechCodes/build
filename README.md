# Build Platform

A distributed platform providing isolated development environments through Firecracker VMs, accessible via web-based terminals.

## Architecture Overview

The Build platform is a microservices-based system that provides:
- Isolated development environments using Firecracker VMs
- Web-based terminal access with real-time collaboration
- Git repository management with soft-serve integration
- Snapshot and session management
- Authentication and authorization
- Comprehensive monitoring and observability

## Quick Start

### Prerequisites

- Python 3.11+
- Node.js 18+
- Podman (for containerization)
- Git with SSH keys configured

### Development Setup

1. **Clone the repository**:
   ```bash
   git clone <repository-url>
   cd Build
   ```

2. **Start development services**:
   ```bash
   ./scripts/dev-setup.sh
   ```

3. **Access the application**:
   - Frontend: http://localhost:3000
   - API: http://localhost:8000
   - API Docs: http://localhost:8000/docs
   - MinIO Console: http://localhost:9001

## Project Structure

```
Build/
├── api/                    # FastAPI backend service
├── frontend/               # React frontend application
├── vm-manager/             # Firecracker VM management
├── session-manager/        # Terminal session management
├── snapshot-manager/       # VM snapshot handling
├── git-manager/           # Git repository management
├── websocket-gateway/     # WebSocket connection handling
├── auth-service/          # Authentication service
├── monitoring/            # Observability setup
├── infrastructure/        # Infrastructure as Code
├── scripts/               # Utility scripts
├── docs/                  # Documentation
├── tests/                 # Integration tests
├── planning/              # Project planning docs
└── .github/               # CI/CD workflows
```

## Services

### Core Services
- **API**: Central REST API built with FastAPI
- **Frontend**: React-based web interface
- **WebSocket Gateway**: Real-time terminal communication
- **Auth Service**: JWT-based authentication

### Management Services
- **VM Manager**: Firecracker VM lifecycle management
- **Session Manager**: Terminal session handling
- **Snapshot Manager**: VM state snapshots
- **Git Manager**: Repository operations

### Infrastructure Services
- **PostgreSQL**: Primary database
- **Redis**: Caching and pub/sub messaging
- **MinIO**: S3-compatible object storage
- **Soft-serve**: Git server

## Development

### Local Development with Podman

The platform uses Podman for local development instead of Docker for enhanced security:

```bash
# Start all services
podman-compose up -d

# View logs
podman-compose logs -f

# Stop services
podman-compose down
```

### Running Tests

```bash
# Unit tests
cd api && python -m pytest
cd frontend && npm test

# Integration tests
pytest tests/integration/

# E2E tests
pytest tests/e2e/
```

### Code Quality

```bash
# Python formatting
black api/ vm-manager/ session-manager/

# TypeScript formatting
cd frontend && npm run format

# Type checking
cd api && mypy .
cd frontend && npm run type-check

# Linting
cd api && ruff check .
cd frontend && npm run lint
```

## Security

The platform implements defense-in-depth security:
- Rootless container execution with Podman
- VM isolation with Firecracker
- JWT-based authentication
- Network segmentation
- Comprehensive audit logging
- Regular security scanning

See [Security Documentation](docs/security/README.md) for details.

## Deployment

### Development
- Local development with podman-compose
- All services on single machine
- Self-signed certificates

### Production
- Kubernetes orchestration
- Multi-zone deployment
- Proper secret management
- SSL certificates from CA
- Comprehensive monitoring

See [Deployment Guide](docs/deployment/README.md) for details.

## Contributing

1. Read the [Development Guide](docs/development/setup.md)
2. Review the [Architecture Documentation](docs/architecture/overview.md)
3. Check the [API Documentation](docs/api/README.md)
4. Follow the [Security Guidelines](docs/security/guidelines.md)

## Monitoring & Observability

- **Logs**: Structured logging with correlation IDs
- **Metrics**: Performance and business metrics
- **Tracing**: Distributed request tracing
- **Health Checks**: Service health monitoring
- **Alerts**: Automated alerting for issues

## License

This project is proprietary software. All rights reserved.

## Support

For development issues:
- Check the [Troubleshooting Guide](docs/troubleshooting.md)
- Review [Common Issues](docs/common-issues.md)
- Create an issue in the project repository