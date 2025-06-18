# Development Setup Guide

This guide will help you set up the Build platform for local development.

## Prerequisites

### Required Software
- **Python 3.11+**: Backend development
- **Node.js 18+**: Frontend development  
- **Podman**: Container orchestration (instead of Docker)
- **Git**: Version control with SSH keys configured

### System Requirements
- **OS**: Linux, macOS, or Windows with WSL2
- **RAM**: Minimum 8GB, recommended 16GB
- **Storage**: At least 100GB free space for VMs and data
- **Network**: Reliable internet connection for dependencies

## Quick Start

### 1. Clone the Repository
```bash
git clone <repository-url>
cd Build
```

### 2. Environment Setup
Copy the environment template:
```bash
cp .env.example .env.development
```

Edit `.env.development` if needed for your local setup.

### 3. Start Development Environment
Run the setup script:
```bash
./scripts/dev-setup.sh
```

This script will:
- Verify prerequisites
- Start infrastructure services (PostgreSQL, Redis, MinIO, Git server)
- Build and start application services
- Run database migrations
- Seed development data

### 4. Verify Installation
Test all services:
```bash
./scripts/test-services.sh
```

## Manual Setup (Alternative)

If the automated script doesn't work, follow these manual steps:

### 1. Install Dependencies

#### Python Dependencies
```bash
cd api
python -m venv venv
source venv/bin/activate  # On Windows: venv\Scripts\activate
pip install -r requirements.txt
```

#### Frontend Dependencies
```bash
cd frontend
npm install
```

### 2. Start Infrastructure Services
```bash
podman-compose up -d postgres redis minio softserve
```

Wait for services to be ready:
```bash
./scripts/wait-for-services.sh
```

### 3. Database Setup
```bash
cd api
source venv/bin/activate
alembic upgrade head
python ../scripts/seed-dev-data.py
```

### 4. Start Application Services
```bash
# Terminal 1: Start API
cd api
source venv/bin/activate
uvicorn app.main:app --reload --host 0.0.0.0 --port 8000

# Terminal 2: Start Frontend
cd frontend
npm run dev
```

## Service URLs

Once everything is running, you can access:

- **Frontend**: http://localhost:3000
- **API**: http://localhost:8000
- **API Documentation**: http://localhost:8000/docs
- **MinIO Console**: http://localhost:9001
- **PostgreSQL**: localhost:5434
- **Redis**: localhost:6381
- **Git Server SSH**: ssh://localhost:23231
- **Git Server HTTP**: http://localhost:23232

## Development Workflow

### Daily Development
```bash
# Start services
podman-compose up -d

# Check service status
podman-compose ps

# View logs
podman-compose logs -f [service-name]

# Stop services
podman-compose down
```

### Database Operations
```bash
# Create migration
cd api && alembic revision --autogenerate -m "Description"

# Apply migrations
cd api && alembic upgrade head

# Reset database (destructive)
podman-compose down -v
podman-compose up -d postgres
./scripts/wait-for-services.sh
cd api && alembic upgrade head
python ../scripts/seed-dev-data.py
```

### Code Quality
```bash
# Python formatting and linting
cd api
black .
ruff check . --fix
mypy .

# Frontend formatting and linting
cd frontend
npm run format
npm run lint:fix
npm run type-check
```

### Testing
```bash
# Python tests
cd api
pytest

# Frontend tests
cd frontend
npm test

# Integration tests
pytest tests/integration/

# Security scans
./scripts/security-scan.sh
```

## Development Credentials

The seed script creates these test accounts:

- **Admin**: admin@build-platform.dev / admin123
- **Developer**: developer@build-platform.dev / dev123  
- **Test**: test@build-platform.dev / test123

## Troubleshooting

### Port Conflicts
If you get port conflicts, check what's using the ports:
```bash
lsof -i :5434  # PostgreSQL
lsof -i :6381  # Redis
lsof -i :9000  # MinIO API
lsof -i :9001  # MinIO Console
```

### Container Issues
```bash
# Remove all containers and volumes
podman-compose down -v
podman system prune -a

# Restart from scratch
./scripts/dev-setup.sh
```

### Permission Issues
```bash
# Fix volume permissions (Linux)
podman unshare chown -R 999:999 ./postgres_data

# SELinux issues (Linux)
sudo setsebool -P container_manage_cgroup true
```

### Database Connection Issues
```bash
# Test database connection
podman exec build_postgres_dev psql -U postgres -d build_dev -c "SELECT 1;"

# Check container logs
podman logs build_postgres_dev
```

## IDE Configuration

### VS Code
Install recommended extensions:
- Python
- TypeScript and JavaScript
- Docker (for podman-compose files)
- GitLens
- Prettier
- ESLint

Configure settings in `.vscode/settings.json`:
```json
{
  "dev.containers.dockerPath": "podman",
  "dev.containers.dockerComposePath": "podman-compose",
  "python.defaultInterpreterPath": "./api/venv/bin/python",
  "python.formatting.provider": "black",
  "typescript.preferences.includePackageJsonAutoImports": "auto"
}
```

### PyCharm
- Configure Docker plugin to use Podman
- Set Python interpreter to use virtual environment
- Configure database connections to container ports

## Performance Tips

1. **Resource Allocation**: Increase container resource limits in `podman-compose.yml`
2. **Storage**: Use SSD storage for containers and volumes
3. **Memory**: Ensure sufficient RAM for all services
4. **Network**: Use wired connection for stability

## Security Notes

⚠️ **Development Environment Only**

This setup is optimized for development productivity, not production security:

- Simplified passwords and secrets
- Debug mode enabled
- All services accessible on localhost
- Self-signed certificates
- Permissive CORS settings

Never use development configuration in production!

## Getting Help

- **Documentation**: Check the `docs/` directory
- **Issues**: Review common issues in `docs/troubleshooting.md`
- **Logs**: Use `podman-compose logs` to debug issues
- **Community**: Create an issue in the project repository

## Next Steps

Once your development environment is running:

1. Read the [Architecture Overview](../architecture/overview.md)
2. Review the [API Documentation](../api/README.md)
3. Explore the [Security Guidelines](../security/guidelines.md)
4. Start contributing with the [Development Guidelines](./guidelines.md)