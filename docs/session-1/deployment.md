# Deployment Guide

This document provides comprehensive deployment instructions for Session 1 infrastructure components of the Build Platform.

## Deployment Overview

The Build Platform uses containerized deployment with support for:

- **Local Development**: Podman Compose for isolated development environment
- **Staging Environment**: Container orchestration with proper security
- **Production Environment**: Scalable container deployment with monitoring

## Local Development Deployment

### Prerequisites

Ensure you have the following installed:

```bash
# Check prerequisites
python --version    # Python 3.11+
node --version     # Node.js 18+
podman --version   # Podman latest
git --version      # Git latest
```

### Environment Setup

1. **Clone Repository**
```bash
git clone <repository-url>
cd Build
```

2. **Create Environment Files**
```bash
# Create local environment file
cp .env.example .env.local

# Edit environment variables
nano .env.local
```

**Example `.env.local`:**
```bash
# Environment
ENVIRONMENT=development
DEBUG=true
LOG_LEVEL=DEBUG

# Database Configuration
DATABASE_URL=postgresql://postgres:dev_password@localhost:5432/build_dev
DATABASE_HOST=localhost
DATABASE_PORT=5432
DATABASE_NAME=build_dev
DATABASE_USER=postgres
DATABASE_PASSWORD=dev_password

# Redis Configuration
REDIS_URL=redis://:dev_password@localhost:6379/0
REDIS_HOST=localhost
REDIS_PORT=6379
REDIS_PASSWORD=dev_password

# MinIO S3 Storage
MINIO_ENDPOINT=localhost:9000
MINIO_ACCESS_KEY=minioadmin
MINIO_SECRET_KEY=minioadmin123
MINIO_BUCKET=build-dev
MINIO_SECURE=false

# Git Server (Soft-serve)
GIT_SERVER_HOST=localhost
GIT_SERVER_PORT=23231
GIT_SERVER_HTTP_PORT=23232

# JWT Configuration
JWT_SECRET=your-super-secret-jwt-key-for-development-only
JWT_ALGORITHM=HS256
JWT_ACCESS_TOKEN_EXPIRE_MINUTES=15
JWT_REFRESH_TOKEN_EXPIRE_DAYS=7

# CORS Settings for Development
ALLOWED_ORIGINS=["http://localhost:3000", "http://127.0.0.1:3000"]

# Logfire Configuration (optional)
LOGFIRE_TOKEN=your-logfire-token
LOGFIRE_ENABLED=true
```

### Podman Compose Configuration

**File**: `podman-compose.yml`

```yaml
version: '3.8'

services:
  postgres:
    image: postgres:16-alpine
    container_name: build_postgres_dev
    environment:
      POSTGRES_DB: build_dev
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: dev_password
      POSTGRES_INITDB_ARGS: "--encoding=UTF8 --locale=C"
    ports:
      - "127.0.0.1:5432:5432"
    volumes:
      - postgres_data:/var/lib/postgresql/data
      - ./scripts/init-db.sql:/docker-entrypoint-initdb.d/init-db.sql:ro
    networks:
      - build_dev_network
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres"]
      interval: 10s
      timeout: 5s
      retries: 5
    deploy:
      resources:
        limits:
          memory: 512M
          cpus: '0.5'
    security_opt:
      - no-new-privileges:true

  redis:
    image: redis:7-alpine
    container_name: build_redis_dev
    command: redis-server --appendonly yes --requirepass dev_password
    ports:
      - "127.0.0.1:6379:6379"
    volumes:
      - redis_data:/data
      - ./infrastructure/redis/redis.conf:/usr/local/etc/redis/redis.conf:ro
    networks:
      - build_dev_network
    healthcheck:
      test: ["CMD", "redis-cli", "--raw", "incr", "ping"]
      interval: 10s
      timeout: 3s
      retries: 5
    deploy:
      resources:
        limits:
          memory: 256M
          cpus: '0.25'
    security_opt:
      - no-new-privileges:true

  minio:
    image: minio/minio:latest
    container_name: build_minio_dev
    command: server /data --console-address ":9001"
    environment:
      MINIO_ROOT_USER: minioadmin
      MINIO_ROOT_PASSWORD: minioadmin123
    ports:
      - "127.0.0.1:9000:9000"
      - "127.0.0.1:9001:9001"
    volumes:
      - minio_data:/data
    networks:
      - build_dev_network
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:9000/minio/health/live"]
      interval: 30s
      timeout: 20s
      retries: 3
    deploy:
      resources:
        limits:
          memory: 256M
          cpus: '0.25'
    security_opt:
      - no-new-privileges:true

  softserve:
    image: charmcli/soft-serve:latest
    container_name: build_git_dev
    ports:
      - "127.0.0.1:23231:23231"  # SSH
      - "127.0.0.1:23232:23232"  # HTTP
    volumes:
      - softserve_data:/soft-serve
    networks:
      - build_dev_network
    deploy:
      resources:
        limits:
          memory: 128M
          cpus: '0.1'
    security_opt:
      - no-new-privileges:true

  api:
    build: 
      context: ./api
      dockerfile: Dockerfile.dev
    container_name: build_api_dev
    env_file: .env.local
    ports:
      - "127.0.0.1:8000:8000"
    volumes:
      - ./api:/app
      - api_logs:/app/logs
    networks:
      - build_dev_network
    depends_on:
      postgres:
        condition: service_healthy
      redis:
        condition: service_healthy
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8000/health"]
      interval: 30s
      timeout: 10s
      retries: 3
    deploy:
      resources:
        limits:
          memory: 512M
          cpus: '0.5'
    security_opt:
      - no-new-privileges:true
    read_only: true
    tmpfs:
      - /tmp:rw,size=100M

volumes:
  postgres_data:
    driver: local
  redis_data:
    driver: local
  minio_data:
    driver: local
  softserve_data:
    driver: local
  api_logs:
    driver: local

networks:
  build_dev_network:
    driver: bridge
    ipam:
      driver: default
      config:
        - subnet: 172.20.0.0/16
```

### API Dockerfile for Development

**File**: `api/Dockerfile.dev`

```dockerfile
FROM python:3.11-slim

# Set working directory
WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y \
    curl \
    gcc \
    && rm -rf /var/lib/apt/lists/* \
    && apt-get clean

# Create non-root user
RUN groupadd -r appuser && useradd -r -g appuser appuser

# Copy requirements first for better caching
COPY requirements.txt .
COPY requirements-dev.txt .

# Install Python dependencies
RUN pip install --no-cache-dir -r requirements.txt
RUN pip install --no-cache-dir -r requirements-dev.txt

# Copy application code
COPY . .

# Create logs directory
RUN mkdir -p /app/logs && chown -R appuser:appuser /app/logs

# Switch to non-root user
USER appuser

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8000/health || exit 1

# Expose port
EXPOSE 8000

# Development command with hot reload
CMD ["uvicorn", "main:app", "--host", "0.0.0.0", "--port", "8000", "--reload"]
```

### Development Startup

1. **Start Core Services**
```bash
# Start all services
podman-compose up -d

# Check service status
podman-compose ps

# View logs
podman-compose logs -f
```

2. **Initialize Database**
```bash
# Run migrations
cd api
python -m alembic upgrade head

# Seed development data (optional)
python scripts/seed_dev_data.py
```

3. **Verify Services**
```bash
# Check API health
curl http://localhost:8000/health

# Check PostgreSQL
psql -h localhost -p 5432 -U postgres -d build_dev

# Check Redis
redis-cli -h localhost -p 6379 -a dev_password ping

# Check MinIO
curl http://localhost:9000/minio/health/live
```

### Development Scripts

**File**: `scripts/dev-helpers.sh`

```bash
#!/bin/bash

# Development helper functions

# Reset development environment
reset_dev_env() {
    echo "Resetting development environment..."
    podman-compose down -v
    podman system prune -f
    podman-compose up -d
    sleep 30
    cd api && python -m alembic upgrade head
    python scripts/seed_dev_data.py
    echo "Development environment reset complete!"
}

# Check service health
check_services() {
    echo "Checking service health..."
    
    # API
    if curl -f http://localhost:8000/health > /dev/null 2>&1; then
        echo "✅ API service: healthy"
    else
        echo "❌ API service: unhealthy"
    fi
    
    # PostgreSQL
    if podman exec build_postgres_dev pg_isready -U postgres > /dev/null 2>&1; then
        echo "✅ PostgreSQL: healthy"
    else
        echo "❌ PostgreSQL: unhealthy"
    fi
    
    # Redis
    if podman exec build_redis_dev redis-cli ping > /dev/null 2>&1; then
        echo "✅ Redis: healthy"
    else
        echo "❌ Redis: unhealthy"
    fi
    
    # MinIO
    if curl -f http://localhost:9000/minio/health/live > /dev/null 2>&1; then
        echo "✅ MinIO: healthy"
    else
        echo "❌ MinIO: unhealthy"
    fi
}

# View service logs
view_logs() {
    service=${1:-all}
    if [ "$service" = "all" ]; then
        podman-compose logs -f
    else
        podman-compose logs -f "$service"
    fi
}

# Backup development data
backup_dev_data() {
    echo "Backing up development data..."
    timestamp=$(date +%Y%m%d_%H%M%S)
    
    # Database backup
    podman exec build_postgres_dev pg_dump -U postgres build_dev > "backups/db_backup_${timestamp}.sql"
    
    # Redis backup
    podman exec build_redis_dev redis-cli --rdb - > "backups/redis_backup_${timestamp}.rdb"
    
    echo "Backup completed: backups/*_${timestamp}.*"
}

# Restore development data
restore_dev_data() {
    backup_file=$1
    if [ -z "$backup_file" ]; then
        echo "Usage: restore_dev_data <backup_file>"
        return 1
    fi
    
    echo "Restoring development data from $backup_file..."
    
    if [[ "$backup_file" == *.sql ]]; then
        # Database restore
        podman exec -i build_postgres_dev psql -U postgres build_dev < "$backup_file"
    elif [[ "$backup_file" == *.rdb ]]; then
        # Redis restore (requires restart)
        podman-compose stop redis
        cp "$backup_file" ./redis_data/dump.rdb
        podman-compose start redis
    else
        echo "Unknown backup file type: $backup_file"
        return 1
    fi
    
    echo "Restore completed!"
}

# Clean up development environment
cleanup_dev_env() {
    echo "Cleaning up development environment..."
    podman-compose down -v
    podman system prune -af
    podman volume prune -f
    echo "Cleanup completed!"
}

# Show development status
dev_status() {
    echo "=== Development Environment Status ==="
    echo
    echo "Services:"
    podman-compose ps
    echo
    echo "Resource Usage:"
    podman stats --no-stream
    echo
    echo "Volumes:"
    podman volume ls | grep build
    echo
    echo "Networks:"
    podman network ls | grep build
}

# Main script logic
case "$1" in
    reset)
        reset_dev_env
        ;;
    check)
        check_services
        ;;
    logs)
        view_logs "$2"
        ;;
    backup)
        backup_dev_data
        ;;
    restore)
        restore_dev_data "$2"
        ;;
    cleanup)
        cleanup_dev_env
        ;;
    status)
        dev_status
        ;;
    *)
        echo "Usage: $0 {reset|check|logs|backup|restore|cleanup|status}"
        echo
        echo "Commands:"
        echo "  reset    - Reset development environment"
        echo "  check    - Check service health"
        echo "  logs     - View service logs (optional: service name)"
        echo "  backup   - Backup development data"
        echo "  restore  - Restore development data"
        echo "  cleanup  - Clean up development environment"
        echo "  status   - Show development status"
        exit 1
        ;;
esac
```

## Staging Environment Deployment

### Infrastructure Setup

**File**: `infrastructure/staging/docker-compose.staging.yml`

```yaml
version: '3.8'

services:
  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_DB: ${DATABASE_NAME}
      POSTGRES_USER: ${DATABASE_USER}
      POSTGRES_PASSWORD_FILE: /run/secrets/postgres_password
    volumes:
      - postgres_data:/var/lib/postgresql/data
    networks:
      - build_staging_network
    secrets:
      - postgres_password
    deploy:
      resources:
        limits:
          memory: 1G
          cpus: '1.0'
        reservations:
          memory: 512M
          cpus: '0.5'
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U ${DATABASE_USER}"]
      interval: 10s
      timeout: 5s
      retries: 5
    logging:
      driver: "json-file"
      options:
        max-size: "10m"
        max-file: "3"

  redis:
    image: redis:7-alpine
    command: redis-server --requirepass-file /run/secrets/redis_password --appendonly yes
    volumes:
      - redis_data:/data
    networks:
      - build_staging_network
    secrets:
      - redis_password
    deploy:
      resources:
        limits:
          memory: 512M
          cpus: '0.5'
        reservations:
          memory: 256M
          cpus: '0.25'
    healthcheck:
      test: ["CMD", "redis-cli", "--raw", "incr", "ping"]
      interval: 10s
      timeout: 3s
      retries: 5

  api:
    image: build-api:staging
    environment:
      ENVIRONMENT: staging
      DATABASE_URL: postgresql://${DATABASE_USER}:${DATABASE_PASSWORD}@postgres:5432/${DATABASE_NAME}
      REDIS_URL: redis://:${REDIS_PASSWORD}@redis:6379/0
    networks:
      - build_staging_network
    depends_on:
      postgres:
        condition: service_healthy
      redis:
        condition: service_healthy
    deploy:
      replicas: 2
      resources:
        limits:
          memory: 1G
          cpus: '1.0'
        reservations:
          memory: 512M
          cpus: '0.5'
      restart_policy:
        condition: on-failure
        delay: 5s
        max_attempts: 3
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8000/health"]
      interval: 30s
      timeout: 10s
      retries: 3
    logging:
      driver: "json-file"
      options:
        max-size: "10m"
        max-file: "5"

  nginx:
    image: nginx:alpine
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./nginx/nginx.conf:/etc/nginx/nginx.conf:ro
      - ./nginx/ssl:/etc/nginx/ssl:ro
      - nginx_logs:/var/log/nginx
    networks:
      - build_staging_network
    depends_on:
      - api
    deploy:
      resources:
        limits:
          memory: 256M
          cpus: '0.25'

secrets:
  postgres_password:
    file: ./secrets/postgres_password.txt
  redis_password:
    file: ./secrets/redis_password.txt

volumes:
  postgres_data:
    driver: local
  redis_data:
    driver: local
  nginx_logs:
    driver: local

networks:
  build_staging_network:
    driver: bridge
```

### Nginx Configuration

**File**: `infrastructure/staging/nginx/nginx.conf`

```nginx
events {
    worker_connections 1024;
}

http {
    upstream api_backend {
        server api:8000;
    }
    
    # Rate limiting
    limit_req_zone $binary_remote_addr zone=api:10m rate=10r/s;
    limit_req_zone $binary_remote_addr zone=auth:10m rate=5r/s;
    
    # Security headers
    add_header X-Frame-Options DENY;
    add_header X-Content-Type-Options nosniff;
    add_header X-XSS-Protection "1; mode=block";
    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains";
    
    server {
        listen 80;
        server_name staging.getbuild.ing;
        return 301 https://$server_name$request_uri;
    }
    
    server {
        listen 443 ssl http2;
        server_name staging.getbuild.ing;
        
        ssl_certificate /etc/nginx/ssl/cert.pem;
        ssl_certificate_key /etc/nginx/ssl/key.pem;
        ssl_protocols TLSv1.2 TLSv1.3;
        ssl_ciphers ECDHE-RSA-AES128-GCM-SHA256:ECDHE-RSA-AES256-GCM-SHA384;
        ssl_prefer_server_ciphers off;
        
        # API endpoints
        location /api/ {
            limit_req zone=api burst=20 nodelay;
            
            proxy_pass http://api_backend/;
            proxy_set_header Host $host;
            proxy_set_header X-Real-IP $remote_addr;
            proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
            proxy_set_header X-Forwarded-Proto $scheme;
            
            # Timeouts
            proxy_connect_timeout 5s;
            proxy_send_timeout 60s;
            proxy_read_timeout 60s;
        }
        
        # Auth endpoints (stricter rate limiting)
        location /api/auth/ {
            limit_req zone=auth burst=10 nodelay;
            
            proxy_pass http://api_backend/auth/;
            proxy_set_header Host $host;
            proxy_set_header X-Real-IP $remote_addr;
            proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
            proxy_set_header X-Forwarded-Proto $scheme;
        }
        
        # Health check
        location /health {
            proxy_pass http://api_backend/health;
            access_log off;
        }
        
        # Static files (future frontend)
        location / {
            root /var/www/html;
            try_files $uri $uri/ /index.html;
        }
    }
}
```

### Staging Deployment Script

**File**: `scripts/deploy-staging.sh`

```bash
#!/bin/bash

set -e

echo "=== Build Platform Staging Deployment ==="

# Configuration
COMPOSE_FILE="infrastructure/staging/docker-compose.staging.yml"
API_IMAGE="build-api:staging"
BACKUP_DIR="backups/staging"

# Create backup directory
mkdir -p "$BACKUP_DIR"

# Build API image
echo "Building API image..."
cd api
docker build -t "$API_IMAGE" -f Dockerfile.prod .
cd ..

# Backup current database
echo "Creating database backup..."
timestamp=$(date +%Y%m%d_%H%M%S)
docker-compose -f "$COMPOSE_FILE" exec -T postgres pg_dump -U postgres build_staging > "$BACKUP_DIR/backup_${timestamp}.sql"

# Deploy services
echo "Deploying services..."
docker-compose -f "$COMPOSE_FILE" pull
docker-compose -f "$COMPOSE_FILE" up -d --remove-orphans

# Run database migrations
echo "Running database migrations..."
docker-compose -f "$COMPOSE_FILE" exec api python -m alembic upgrade head

# Health check
echo "Performing health checks..."
sleep 30

for i in {1..5}; do
    if curl -f https://staging.getbuild.ing/health > /dev/null 2>&1; then
        echo "✅ Staging deployment successful!"
        exit 0
    fi
    echo "⏳ Waiting for services to be ready... ($i/5)"
    sleep 10
done

echo "❌ Staging deployment failed - health check timeout"
exit 1
```

## Production Environment Deployment

### Infrastructure as Code

**File**: `infrastructure/production/terraform/main.tf`

```hcl
# Example Terraform configuration for production
provider "aws" {
  region = var.aws_region
}

# VPC and networking
module "vpc" {
  source = "terraform-aws-modules/vpc/aws"
  
  name = "build-platform-vpc"
  cidr = "10.0.0.0/16"
  
  azs             = ["${var.aws_region}a", "${var.aws_region}b", "${var.aws_region}c"]
  private_subnets = ["10.0.1.0/24", "10.0.2.0/24", "10.0.3.0/24"]
  public_subnets  = ["10.0.101.0/24", "10.0.102.0/24", "10.0.103.0/24"]
  
  enable_nat_gateway = true
  enable_vpn_gateway = true
  
  tags = {
    Environment = "production"
    Project     = "build-platform"
  }
}

# RDS PostgreSQL
resource "aws_db_instance" "postgres" {
  identifier = "build-platform-postgres"
  
  engine         = "postgres"
  engine_version = "16.1"
  instance_class = "db.t3.medium"
  
  allocated_storage     = 100
  max_allocated_storage = 1000
  storage_encrypted     = true
  
  db_name  = "build_prod"
  username = "postgres"
  password = var.database_password
  
  vpc_security_group_ids = [aws_security_group.rds.id]
  db_subnet_group_name   = aws_db_subnet_group.postgres.name
  
  backup_retention_period = 30
  backup_window          = "03:00-04:00"
  maintenance_window     = "sun:04:00-sun:05:00"
  
  skip_final_snapshot = false
  final_snapshot_identifier = "build-platform-final-snapshot"
  
  tags = {
    Environment = "production"
    Project     = "build-platform"
  }
}

# ElastiCache Redis
resource "aws_elasticache_subnet_group" "redis" {
  name       = "build-platform-redis-subnet"
  subnet_ids = module.vpc.private_subnets
}

resource "aws_elasticache_replication_group" "redis" {
  replication_group_id         = "build-platform-redis"
  description                  = "Redis cluster for Build Platform"
  
  node_type                    = "cache.t3.micro"
  port                         = 6379
  parameter_group_name         = "default.redis7"
  
  num_cache_clusters           = 2
  automatic_failover_enabled   = true
  multi_az_enabled            = true
  
  subnet_group_name           = aws_elasticache_subnet_group.redis.name
  security_group_ids          = [aws_security_group.redis.id]
  
  at_rest_encryption_enabled  = true
  transit_encryption_enabled  = true
  auth_token                  = var.redis_password
  
  tags = {
    Environment = "production"
    Project     = "build-platform"
  }
}

# ECS Cluster
resource "aws_ecs_cluster" "main" {
  name = "build-platform"
  
  capacity_providers = ["FARGATE"]
  
  default_capacity_provider_strategy {
    capacity_provider = "FARGATE"
    weight           = 1
  }
  
  setting {
    name  = "containerInsights"
    value = "enabled"
  }
  
  tags = {
    Environment = "production"
    Project     = "build-platform"
  }
}
```

### ECS Task Definition

**File**: `infrastructure/production/ecs/api-task-definition.json`

```json
{
  "family": "build-api",
  "networkMode": "awsvpc",
  "requiresCompatibilities": ["FARGATE"],
  "cpu": "512",
  "memory": "1024",
  "executionRoleArn": "arn:aws:iam::ACCOUNT:role/ecsTaskExecutionRole",
  "taskRoleArn": "arn:aws:iam::ACCOUNT:role/ecsTaskRole",
  "containerDefinitions": [
    {
      "name": "build-api",
      "image": "your-registry/build-api:latest",
      "portMappings": [
        {
          "containerPort": 8000,
          "protocol": "tcp"
        }
      ],
      "environment": [
        {
          "name": "ENVIRONMENT",
          "value": "production"
        }
      ],
      "secrets": [
        {
          "name": "DATABASE_URL",
          "valueFrom": "arn:aws:secretsmanager:region:account:secret:build/database-url"
        },
        {
          "name": "REDIS_URL",
          "valueFrom": "arn:aws:secretsmanager:region:account:secret:build/redis-url"
        },
        {
          "name": "JWT_SECRET",
          "valueFrom": "arn:aws:secretsmanager:region:account:secret:build/jwt-secret"
        }
      ],
      "logConfiguration": {
        "logDriver": "awslogs",
        "options": {
          "awslogs-group": "/ecs/build-api",
          "awslogs-region": "us-west-2",
          "awslogs-stream-prefix": "ecs"
        }
      },
      "healthCheck": {
        "command": ["CMD-SHELL", "curl -f http://localhost:8000/health || exit 1"],
        "interval": 30,
        "timeout": 5,
        "retries": 3,
        "startPeriod": 60
      }
    }
  ]
}
```

### Production Deployment Pipeline

**File**: `.github/workflows/deploy-production.yml`

```yaml
name: Deploy to Production

on:
  push:
    tags:
      - 'v*'

env:
  AWS_REGION: us-west-2
  ECS_CLUSTER: build-platform
  ECS_SERVICE: build-api
  ECR_REPOSITORY: build-api

jobs:
  deploy:
    runs-on: ubuntu-latest
    environment: production
    
    steps:
    - name: Checkout code
      uses: actions/checkout@v4
    
    - name: Configure AWS credentials
      uses: aws-actions/configure-aws-credentials@v4
      with:
        aws-access-key-id: ${{ secrets.AWS_ACCESS_KEY_ID }}
        aws-secret-access-key: ${{ secrets.AWS_SECRET_ACCESS_KEY }}
        aws-region: ${{ env.AWS_REGION }}
    
    - name: Login to Amazon ECR
      id: login-ecr
      uses: aws-actions/amazon-ecr-login@v2
    
    - name: Build and push Docker image
      env:
        ECR_REGISTRY: ${{ steps.login-ecr.outputs.registry }}
        IMAGE_TAG: ${{ github.sha }}
      run: |
        cd api
        docker build -t $ECR_REGISTRY/$ECR_REPOSITORY:$IMAGE_TAG -f Dockerfile.prod .
        docker push $ECR_REGISTRY/$ECR_REPOSITORY:$IMAGE_TAG
        docker tag $ECR_REGISTRY/$ECR_REPOSITORY:$IMAGE_TAG $ECR_REGISTRY/$ECR_REPOSITORY:latest
        docker push $ECR_REGISTRY/$ECR_REPOSITORY:latest
    
    - name: Download task definition
      run: |
        aws ecs describe-task-definition \
          --task-definition build-api \
          --query taskDefinition > task-definition.json
    
    - name: Update ECS task definition
      id: task-def
      uses: aws-actions/amazon-ecs-render-task-definition@v1
      with:
        task-definition: task-definition.json
        container-name: build-api
        image: ${{ steps.login-ecr.outputs.registry }}/${{ env.ECR_REPOSITORY }}:${{ github.sha }}
    
    - name: Deploy to ECS
      uses: aws-actions/amazon-ecs-deploy-task-definition@v1
      with:
        task-definition: ${{ steps.task-def.outputs.task-definition }}
        service: ${{ env.ECS_SERVICE }}
        cluster: ${{ env.ECS_CLUSTER }}
        wait-for-service-stability: true
    
    - name: Verify deployment
      run: |
        # Wait for deployment to stabilize
        sleep 60
        
        # Health check
        HEALTH_URL="https://api.getbuild.ing/health"
        for i in {1..10}; do
          if curl -f "$HEALTH_URL" > /dev/null 2>&1; then
            echo "✅ Production deployment successful!"
            exit 0
          fi
          echo "⏳ Waiting for health check... ($i/10)"
          sleep 30
        done
        
        echo "❌ Production deployment failed"
        exit 1
```

## Security Hardening

### Container Security

**File**: `api/Dockerfile.prod`

```dockerfile
# Multi-stage build for production
FROM python:3.11-slim as builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    gcc \
    && rm -rf /var/lib/apt/lists/* \
    && apt-get clean

# Copy and install Python dependencies
COPY requirements.txt .
RUN pip install --user --no-cache-dir -r requirements.txt

# Production stage
FROM python:3.11-slim

# Create non-root user
RUN groupadd -r appuser && useradd -r -g appuser appuser

# Install runtime dependencies only
RUN apt-get update && apt-get install -y \
    curl \
    && rm -rf /var/lib/apt/lists/* \
    && apt-get clean

# Copy Python packages from builder
COPY --from=builder /root/.local /home/appuser/.local

# Set working directory
WORKDIR /app

# Copy application code
COPY --chown=appuser:appuser . .

# Create required directories
RUN mkdir -p /app/logs && chown -R appuser:appuser /app/logs

# Switch to non-root user
USER appuser

# Set PATH to include user packages
ENV PATH=/home/appuser/.local/bin:$PATH

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8000/health || exit 1

# Expose port
EXPOSE 8000

# Production command
CMD ["gunicorn", "main:app", "-w", "4", "-k", "uvicorn.workers.UvicornWorker", "--bind", "0.0.0.0:8000"]
```

### Secret Management

**File**: `scripts/setup-secrets.sh`

```bash
#!/bin/bash

# Setup secrets for different environments

setup_development_secrets() {
    echo "Setting up development secrets..."
    
    # Create secrets directory
    mkdir -p secrets
    
    # Generate development secrets
    echo "dev_password" > secrets/postgres_password.txt
    echo "dev_password" > secrets/redis_password.txt
    openssl rand -base64 32 > secrets/jwt_secret.txt
    
    chmod 600 secrets/*
    echo "Development secrets created"
}

setup_staging_secrets() {
    echo "Setting up staging secrets..."
    
    # Generate stronger secrets for staging
    openssl rand -base64 16 > secrets/postgres_password.txt
    openssl rand -base64 16 > secrets/redis_password.txt
    openssl rand -base64 32 > secrets/jwt_secret.txt
    
    chmod 600 secrets/*
    echo "Staging secrets created"
}

setup_production_secrets() {
    echo "Setting up production secrets..."
    echo "Use AWS Secrets Manager for production!"
    echo "This script only shows the commands:"
    
    echo "aws secretsmanager create-secret --name build/database-url --secret-string 'postgresql://...'"
    echo "aws secretsmanager create-secret --name build/redis-url --secret-string 'redis://...'"
    echo "aws secretsmanager create-secret --name build/jwt-secret --secret-string '$(openssl rand -base64 32)'"
}

case "$1" in
    dev|development)
        setup_development_secrets
        ;;
    staging)
        setup_staging_secrets
        ;;
    prod|production)
        setup_production_secrets
        ;;
    *)
        echo "Usage: $0 {development|staging|production}"
        exit 1
        ;;
esac
```

## Monitoring and Logging

### Centralized Logging

**File**: `infrastructure/logging/fluent-bit.conf`

```ini
[SERVICE]
    Flush         1
    Log_Level     info
    Daemon        off
    Parsers_File  parsers.conf

[INPUT]
    Name              forward
    Listen            0.0.0.0
    Port              24224

[FILTER]
    Name                kubernetes
    Match               kube.*
    Merge_Log           On
    Keep_Log            Off
    K8S-Logging.Parser  On
    K8S-Logging.Exclude On

[OUTPUT]
    Name        cloudwatch_logs
    Match       *
    region      us-west-2
    log_group_name /build-platform/logs
    log_stream_prefix application-
    auto_create_group On
```

### Monitoring Configuration

**File**: `infrastructure/monitoring/prometheus.yml`

```yaml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

rule_files:
  - "alert_rules.yml"

scrape_configs:
  - job_name: 'build-api'
    static_configs:
      - targets: ['api:8000']
    metrics_path: '/metrics'
    scrape_interval: 5s
    
  - job_name: 'postgres-exporter'
    static_configs:
      - targets: ['postgres-exporter:9187']
      
  - job_name: 'redis-exporter'
    static_configs:
      - targets: ['redis-exporter:9121']

alerting:
  alertmanagers:
    - static_configs:
        - targets:
          - alertmanager:9093
```

## Backup and Recovery

### Automated Backup Script

**File**: `scripts/backup.sh`

```bash
#!/bin/bash

set -e

ENVIRONMENT=${1:-development}
BACKUP_DIR="backups/${ENVIRONMENT}"
S3_BUCKET="build-platform-backups"
RETENTION_DAYS=30

# Create backup directory
mkdir -p "$BACKUP_DIR"

# Database backup
echo "Creating database backup..."
timestamp=$(date +%Y%m%d_%H%M%S)

if [ "$ENVIRONMENT" = "production" ]; then
    # Production: Use RDS snapshot
    aws rds create-db-snapshot \
        --db-instance-identifier build-platform-postgres \
        --db-snapshot-identifier build-platform-backup-$timestamp
else
    # Development/Staging: Use pg_dump
    docker-compose exec -T postgres pg_dump -U postgres build_${ENVIRONMENT} > "${BACKUP_DIR}/db_backup_${timestamp}.sql"
    
    # Compress backup
    gzip "${BACKUP_DIR}/db_backup_${timestamp}.sql"
    
    # Upload to S3 (if configured)
    if [ -n "$S3_BUCKET" ]; then
        aws s3 cp "${BACKUP_DIR}/db_backup_${timestamp}.sql.gz" \
            "s3://${S3_BUCKET}/${ENVIRONMENT}/database/"
    fi
fi

# Redis backup
echo "Creating Redis backup..."
if [ "$ENVIRONMENT" = "production" ]; then
    # Production: ElastiCache backup
    aws elasticache create-snapshot \
        --replication-group-id build-platform-redis \
        --snapshot-name build-platform-redis-backup-$timestamp
else
    # Development/Staging: Manual backup
    docker-compose exec -T redis redis-cli --rdb - > "${BACKUP_DIR}/redis_backup_${timestamp}.rdb"
    
    # Upload to S3
    if [ -n "$S3_BUCKET" ]; then
        aws s3 cp "${BACKUP_DIR}/redis_backup_${timestamp}.rdb" \
            "s3://${S3_BUCKET}/${ENVIRONMENT}/redis/"
    fi
fi

# Cleanup old local backups
echo "Cleaning up old backups..."
find "$BACKUP_DIR" -name "*.sql.gz" -mtime +$RETENTION_DAYS -delete
find "$BACKUP_DIR" -name "*.rdb" -mtime +$RETENTION_DAYS -delete

echo "Backup completed successfully!"
```

## Troubleshooting

### Common Issues

1. **Service Won't Start**
```bash
# Check logs
podman-compose logs service_name

# Check resource usage
podman stats

# Check network connectivity
podman exec service_name ping other_service
```

2. **Database Connection Issues**
```bash
# Test connection
psql -h localhost -p 5432 -U postgres -d build_dev

# Check PostgreSQL logs
podman logs build_postgres_dev

# Verify environment variables
podman exec build_api_dev env | grep DATABASE
```

3. **Redis Connection Issues**
```bash
# Test Redis connection
redis-cli -h localhost -p 6379 -a dev_password ping

# Check Redis logs
podman logs build_redis_dev

# Monitor Redis
redis-cli -h localhost -p 6379 -a dev_password monitor
```

4. **Performance Issues**
```bash
# Check resource usage
podman stats

# Analyze slow queries
# (Connect to database and check pg_stat_statements)

# Monitor API performance
curl -w "@curl-format.txt" http://localhost:8000/health
```

This deployment guide provides comprehensive instructions for deploying the Build Platform Session 1 infrastructure across all environments with proper security, monitoring, and operational procedures.