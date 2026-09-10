# Podman Setup for Build Platform Development

## Why Podman for Local Development

Podman provides several advantages for local development:
- **Rootless execution**: Enhanced security without requiring root privileges
- **Docker compatibility**: Drop-in replacement for most Docker commands
- **Better resource isolation**: Improved security boundaries
- **No daemon required**: Simpler architecture and better security

## Installation & Configuration

### Podman Installation
```bash
# macOS (via Homebrew)
brew install podman

# Ubuntu/Debian
sudo apt-get update
sudo apt-get install podman

# RHEL/CentOS/Fedora
sudo dnf install podman

# Verify installation
podman --version
podman info
```

### Podman Compose Setup
```bash
# Install podman-compose
pip install podman-compose

# Verify installation
podman-compose --version
```

### Rootless Configuration
```bash
# Initialize podman for rootless operation
podman system migrate

# Configure user namespaces (Linux only)
echo "$(id -un):100000:65536" | sudo tee /etc/subuid
echo "$(id -gn):100000:65536" | sudo tee /etc/subgid

# Start the user service
systemctl --user enable --now podman.socket
```

## Development Compose Configuration

### podman-compose.yml
```yaml
version: '3.8'

services:
  # PostgreSQL Database
  postgres:
    image: postgres:16-alpine
    container_name: build_postgres_dev
    environment:
      POSTGRES_DB: build_dev
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: dev_password
      PGDATA: /var/lib/postgresql/data/pgdata
    volumes:
      - postgres_data:/var/lib/postgresql/data
      - ./scripts/init-db.sql:/docker-entrypoint-initdb.d/init-db.sql
    ports:
      - "5432:5432"
    networks:
      - build_network
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres"]
      interval: 10s
      timeout: 5s
      retries: 5

  # Redis Cache
  redis:
    image: redis:7-alpine
    container_name: build_redis_dev
    command: redis-server --appendonly yes --requirepass dev_password
    volumes:
      - redis_data:/data
    ports:
      - "6379:6379"
    networks:
      - build_network
    healthcheck:
      test: ["CMD", "redis-cli", "--raw", "incr", "ping"]
      interval: 10s
      timeout: 3s
      retries: 5

  # MinIO S3 Storage
  minio:
    image: minio/minio:latest
    container_name: build_minio_dev
    command: server /data --console-address ":9001"
    environment:
      MINIO_ROOT_USER: minioadmin
      MINIO_ROOT_PASSWORD: minioadmin123
    volumes:
      - minio_data:/data
    ports:
      - "9000:9000"
      - "9001:9001"
    networks:
      - build_network
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:9000/minio/health/live"]
      interval: 30s
      timeout: 20s
      retries: 3

  # Soft-serve Git Server
  softserve:
    image: charmcli/soft-serve:latest
    container_name: build_git_dev
    environment:
      SOFT_SERVE_INITIAL_ADMIN_KEYS: "ssh-rsa AAAA... your-key-here"
    volumes:
      - softserve_data:/soft-serve
    ports:
      - "23231:23231"  # SSH
      - "23232:23232"  # HTTP
    networks:
      - build_network

volumes:
  postgres_data:
    driver: local
  redis_data:
    driver: local
  minio_data:
    driver: local
  softserve_data:
    driver: local

networks:
  build_network:
    driver: bridge
    ipam:
      config:
        - subnet: 172.20.0.0/16
```

## Podman-Specific Considerations

### Volume Mounts
```bash
# Podman uses different volume mount semantics
# Host volume mounts require explicit SELinux labels on Linux
podman run -v ./data:/app/data:Z myimage

# Named volumes work similarly to Docker
podman volume create postgres_data
```

### Network Configuration
```bash
# Create custom network for development
podman network create build_network --subnet 172.20.0.0/16

# List networks
podman network ls

# Inspect network
podman network inspect build_network
```

### Port Binding Differences
```bash
# Podman binds to all interfaces by default in rootless mode
# Be explicit about localhost binding for security
podman run -p 127.0.0.1:5432:5432 postgres

# Or use podman-compose port configuration
ports:
  - "127.0.0.1:5432:5432"
```

## Security Considerations

### Rootless Execution
```bash
# Verify running rootless
podman unshare cat /proc/self/uid_map

# Check user namespace mapping
podman info | grep -A5 -B5 "runRoot"
```

### Container Security
```yaml
# Example secure service configuration
services:
  api:
    image: build_api:dev
    security_opt:
      - no-new-privileges:true
    read_only: true
    tmpfs:
      - /tmp:rw,noexec,nosuid,size=100m
    cap_drop:
      - ALL
    cap_add:
      - NET_BIND_SERVICE
```

### Secret Management
```bash
# Use podman secrets for sensitive data
echo "supersecret" | podman secret create db_password -

# Reference in compose
services:
  postgres:
    secrets:
      - db_password
    environment:
      POSTGRES_PASSWORD_FILE: /run/secrets/db_password
```

## Development Workflow

### Starting Services
```bash
# Start all services
podman-compose up -d

# Start specific service
podman-compose up -d postgres

# View logs
podman-compose logs -f api

# Scale services (if supported)
podman-compose up -d --scale api=3
```

### Service Management
```bash
# List running containers
podman ps

# Execute commands in containers
podman exec -it build_postgres_dev psql -U postgres

# Inspect container
podman inspect build_postgres_dev

# Container resource usage
podman stats
```

### Cleanup
```bash
# Stop and remove all containers
podman-compose down

# Remove volumes too
podman-compose down -v

# Clean system
podman system prune -a
```

## Troubleshooting Common Issues

### Permission Issues
```bash
# Fix volume permissions
podman unshare chown -R 999:999 ./postgres_data

# SELinux context issues (Linux)
sudo setsebool -P container_manage_cgroup true
```

### Network Connectivity
```bash
# Test network connectivity between containers
podman exec build_api_dev ping build_postgres_dev

# Check port binding
ss -tulpn | grep :5432
```

### Performance Issues
```bash
# Increase resource limits
echo 'net.core.rmem_max = 134217728' | sudo tee -a /etc/sysctl.conf

# Configure storage driver
podman info --format "{{.Store.GraphDriverName}}"
```

## Development Scripts

### Helper Scripts
```bash
#!/bin/bash
# scripts/dev-setup.sh

# Start development environment
echo "Starting Build platform development environment..."
podman-compose up -d

# Wait for services to be ready
echo "Waiting for services to be ready..."
sleep 30

# Run database migrations
echo "Running database migrations..."
podman exec build_api_dev alembic upgrade head

# Install development data
echo "Installing development data..."
podman exec build_api_dev python scripts/seed_dev_data.py

echo "Development environment ready!"
echo "API: http://localhost:8000"
echo "Frontend: http://localhost:3000"
echo "MinIO Console: http://localhost:9001"
```

## Integration with IDE

### VS Code Configuration
```json
// .vscode/settings.json
{
  "dev.containers.dockerPath": "podman",
  "dev.containers.dockerComposePath": "podman-compose"
}
```

### PyCharm Configuration
- Configure Docker plugin to use Podman socket
- Set interpreter to use containerized Python
- Configure database connections to use container ports

## Production Differences

### Key Differences from Production
1. **Single-node deployment**: All services on one machine
2. **Simplified networking**: Bridge network instead of overlay
3. **Development certificates**: Self-signed SSL certificates
4. **Relaxed security**: Development-friendly configurations
5. **Debug features**: Enhanced logging and debugging tools

### Migration to Production
- Replace podman-compose with Kubernetes/Docker Swarm
- Implement proper secret management
- Add production monitoring and logging
- Configure production-grade databases
- Implement proper backup strategies

---

**Note**: This configuration provides a secure, isolated development environment using Podman that closely mimics the production architecture while maintaining development productivity.