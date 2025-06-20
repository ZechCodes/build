# Session 14.1: GitHub Actions CI/CD Pipeline & Automated Testing

## Objective
Implement comprehensive GitHub Actions CI/CD pipeline with automated testing, security scanning, container building, and multi-environment deployment to enable rapid, secure, and reliable software delivery with zero-downtime deployments.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for deployment monitoring and pipeline observability
- **Session 11**: Integrates with metrics collection for deployment performance tracking
- **Session 12**: Ensures rate limiting rules are maintained during deployments
- **Session 13**: Leverages high availability infrastructure for zero-downtime deployments
- **All Sessions**: Provides automated deployment pipeline for entire platform stack

## Core Implementation

### GitHub Actions CI/CD Workflow
**Location**: `.github/workflows/ci-cd.yml`

```yaml
# .github/workflows/ci-cd.yml
name: CI/CD Pipeline

on:
  push:
    branches: [main, develop]
  pull_request:
    branches: [main]
  release:
    types: [published]

env:
  REGISTRY: ghcr.io
  IMAGE_NAME: ${{ github.repository }}

jobs:
  security-scan:
    name: Security Scan
    runs-on: ubuntu-latest
    steps:
      - name: Checkout code
        uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Run Trivy vulnerability scanner
        uses: aquasecurity/trivy-action@master
        with:
          scan-type: 'fs'
          format: 'sarif'
          output: 'trivy-results.sarif'

      - name: Upload Trivy scan results
        uses: github/codeql-action/upload-sarif@v3
        with:
          sarif_file: 'trivy-results.sarif'

      - name: Run Semgrep security analysis
        uses: returntocorp/semgrep-action@v1
        with:
          config: auto

      - name: Run dependency security audit
        run: |
          cd api
          pip install safety
          pip freeze | safety check --json --output safety-report.json || true

      - name: Upload security artifacts
        uses: actions/upload-artifact@v3
        with:
          name: security-reports
          path: |
            trivy-results.sarif
            safety-report.json

  test-backend:
    name: Backend Tests
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:16
        env:
          POSTGRES_DB: test_db
          POSTGRES_USER: postgres
          POSTGRES_PASSWORD: postgres
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5
        ports:
          - 5432:5432

      redis:
        image: redis:7
        options: >-
          --health-cmd "redis-cli ping"
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5
        ports:
          - 6379:6379

    steps:
      - name: Checkout code
        uses: actions/checkout@v4

      - name: Set up Python
        uses: actions/setup-python@v4
        with:
          python-version: '3.11'

      - name: Cache Python dependencies
        uses: actions/cache@v3
        with:
          path: ~/.cache/pip
          key: ${{ runner.os }}-pip-${{ hashFiles('**/requirements.txt') }}
          restore-keys: |
            ${{ runner.os }}-pip-

      - name: Install dependencies
        run: |
          cd api
          pip install -r requirements.txt
          pip install pytest pytest-cov pytest-asyncio pytest-mock

      - name: Run database migrations
        run: |
          cd api
          alembic upgrade head
        env:
          DATABASE_URL: postgresql://postgres:postgres@localhost:5432/test_db

      - name: Run unit tests
        run: |
          cd api
          pytest tests/unit/ -v --cov=app --cov-report=xml --cov-report=html
        env:
          DATABASE_URL: postgresql://postgres:postgres@localhost:5432/test_db
          REDIS_URL: redis://localhost:6379/0
          ENVIRONMENT: testing

      - name: Run integration tests
        run: |
          cd api
          pytest tests/integration/ -v --cov=app --cov-append --cov-report=xml
        env:
          DATABASE_URL: postgresql://postgres:postgres@localhost:5432/test_db
          REDIS_URL: redis://localhost:6379/0
          ENVIRONMENT: testing

      - name: Upload backend coverage
        uses: codecov/codecov-action@v3
        with:
          file: ./api/coverage.xml
          flags: backend
          name: backend-coverage

  test-frontend:
    name: Frontend Tests
    runs-on: ubuntu-latest
    steps:
      - name: Checkout code
        uses: actions/checkout@v4

      - name: Set up Node.js
        uses: actions/setup-node@v4
        with:
          node-version: '18'
          cache: 'npm'
          cache-dependency-path: frontend/package-lock.json

      - name: Install dependencies
        run: |
          cd frontend
          npm ci

      - name: Run ESLint
        run: |
          cd frontend
          npm run lint

      - name: Run TypeScript type checking
        run: |
          cd frontend
          npm run type-check

      - name: Run unit tests
        run: |
          cd frontend
          npm run test:unit -- --coverage

      - name: Run integration tests
        run: |
          cd frontend
          npm run test:integration

      - name: Upload frontend coverage
        uses: codecov/codecov-action@v3
        with:
          file: ./frontend/coverage/lcov.info
          flags: frontend
          name: frontend-coverage

  test-vm-manager:
    name: VM Manager Tests
    runs-on: ubuntu-latest
    steps:
      - name: Checkout code
        uses: actions/checkout@v4

      - name: Set up Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
          override: true
          components: rustfmt, clippy

      - name: Cache Rust dependencies
        uses: actions/cache@v3
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            vm-manager/target
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}

      - name: Run Rust formatting check
        uses: actions-rs/cargo@v1
        with:
          command: fmt
          args: --all --manifest-path vm-manager/Cargo.toml -- --check

      - name: Run Clippy
        uses: actions-rs/cargo@v1
        with:
          command: clippy
          args: --manifest-path vm-manager/Cargo.toml -- -D warnings

      - name: Run VM manager tests
        uses: actions-rs/cargo@v1
        with:
          command: test
          args: --manifest-path vm-manager/Cargo.toml --verbose

  build-and-push:
    name: Build and Push Container Images
    runs-on: ubuntu-latest
    needs: [security-scan, test-backend, test-frontend, test-vm-manager]
    if: github.event_name != 'pull_request'
    outputs:
      api-image: ${{ steps.meta-api.outputs.tags }}
      frontend-image: ${{ steps.meta-frontend.outputs.tags }}
      vm-manager-image: ${{ steps.meta-vm-manager.outputs.tags }}
      version: ${{ steps.meta-api.outputs.version }}
    
    steps:
      - name: Checkout code
        uses: actions/checkout@v4

      - name: Set up Docker Buildx
        uses: docker/setup-buildx-action@v3

      - name: Log in to Container Registry
        uses: docker/login-action@v3
        with:
          registry: ${{ env.REGISTRY }}
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Extract API metadata
        id: meta-api
        uses: docker/metadata-action@v5
        with:
          images: ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}/api
          tags: |
            type=ref,event=branch
            type=ref,event=pr
            type=semver,pattern={{version}}
            type=semver,pattern={{major}}.{{minor}}
            type=sha,prefix={{branch}}-

      - name: Build and push API image
        uses: docker/build-push-action@v5
        with:
          context: ./api
          file: ./api/Dockerfile.prod
          push: true
          tags: ${{ steps.meta-api.outputs.tags }}
          labels: ${{ steps.meta-api.outputs.labels }}
          cache-from: type=gha
          cache-to: type=gha,mode=max
          platforms: linux/amd64,linux/arm64

      - name: Extract Frontend metadata
        id: meta-frontend
        uses: docker/metadata-action@v5
        with:
          images: ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}/frontend
          tags: |
            type=ref,event=branch
            type=ref,event=pr
            type=semver,pattern={{version}}
            type=semver,pattern={{major}}.{{minor}}
            type=sha,prefix={{branch}}-

      - name: Build and push Frontend image
        uses: docker/build-push-action@v5
        with:
          context: ./frontend
          file: ./frontend/Dockerfile.prod
          push: true
          tags: ${{ steps.meta-frontend.outputs.tags }}
          labels: ${{ steps.meta-frontend.outputs.labels }}
          cache-from: type=gha
          cache-to: type=gha,mode=max
          platforms: linux/amd64,linux/arm64

      - name: Extract VM Manager metadata
        id: meta-vm-manager
        uses: docker/metadata-action@v5
        with:
          images: ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}/vm-manager
          tags: |
            type=ref,event=branch
            type=ref,event=pr
            type=semver,pattern={{version}}
            type=semver,pattern={{major}}.{{minor}}
            type=sha,prefix={{branch}}-

      - name: Build and push VM Manager image
        uses: docker/build-push-action@v5
        with:
          context: ./vm-manager
          file: ./vm-manager/Dockerfile.prod
          push: true
          tags: ${{ steps.meta-vm-manager.outputs.tags }}
          labels: ${{ steps.meta-vm-manager.outputs.labels }}
          cache-from: type=gha
          cache-to: type=gha,mode=max
          platforms: linux/amd64

      - name: Run container security scan
        uses: aquasecurity/trivy-action@master
        with:
          image-ref: ${{ steps.meta-api.outputs.tags }}
          format: 'sarif'
          output: 'container-scan-results.sarif'

      - name: Upload container scan results
        uses: github/codeql-action/upload-sarif@v3
        with:
          sarif_file: 'container-scan-results.sarif'

  deploy-staging:
    name: Deploy to Staging Environment
    runs-on: ubuntu-latest
    needs: build-and-push
    if: github.ref == 'refs/heads/develop'
    environment:
      name: staging
      url: https://staging.getbuild.ing
    
    steps:
      - name: Checkout code
        uses: actions/checkout@v4

      - name: Set up Helm
        uses: azure/setup-helm@v3
        with:
          version: '3.14.0'

      - name: Configure kubectl for staging
        uses: azure/k8s-set-context@v3
        with:
          method: kubeconfig
          kubeconfig: ${{ secrets.STAGING_KUBECONFIG }}

      - name: Deploy to staging with Helm
        run: |
          helm upgrade --install build-platform-staging ./k8s/helm/build-platform \
            --namespace staging \
            --create-namespace \
            --set image.api.tag=${{ needs.build-and-push.outputs.version }} \
            --set image.frontend.tag=${{ needs.build-and-push.outputs.version }} \
            --set image.vmManager.tag=${{ needs.build-and-push.outputs.version }} \
            --set environment=staging \
            --set ingress.host=staging.getbuild.ing \
            --values ./k8s/helm/build-platform/values-staging.yaml \
            --wait --timeout=10m

      - name: Wait for deployment readiness
        run: |
          kubectl wait --for=condition=ready pod \
            -l app.kubernetes.io/name=build-platform \
            -n staging --timeout=300s

      - name: Run staging smoke tests
        run: |
          curl -f https://staging.getbuild.ing/health || exit 1
          curl -f https://staging.getbuild.ing/health/ready || exit 1

  deploy-production:
    name: Deploy to Production Environment
    runs-on: ubuntu-latest
    needs: build-and-push
    if: github.event_name == 'release'
    environment:
      name: production
      url: https://getbuild.ing
    
    steps:
      - name: Checkout code
        uses: actions/checkout@v4

      - name: Set up Helm
        uses: azure/setup-helm@v3
        with:
          version: '3.14.0'

      - name: Configure kubectl for production
        uses: azure/k8s-set-context@v3
        with:
          method: kubeconfig
          kubeconfig: ${{ secrets.PRODUCTION_KUBECONFIG }}

      - name: Deploy to production with Helm
        run: |
          helm upgrade --install build-platform ./k8s/helm/build-platform \
            --namespace production \
            --create-namespace \
            --set image.api.tag=${{ needs.build-and-push.outputs.version }} \
            --set image.frontend.tag=${{ needs.build-and-push.outputs.version }} \
            --set image.vmManager.tag=${{ needs.build-and-push.outputs.version }} \
            --set environment=production \
            --set ingress.host=getbuild.ing \
            --values ./k8s/helm/build-platform/values-production.yaml \
            --wait --timeout=15m

      - name: Wait for production deployment
        run: |
          kubectl wait --for=condition=ready pod \
            -l app.kubernetes.io/name=build-platform \
            -n production --timeout=600s

      - name: Run production health checks
        run: |
          curl -f https://getbuild.ing/health || exit 1
          curl -f https://getbuild.ing/health/ready || exit 1
          curl -f https://getbuild.ing/api/health || exit 1

      - name: Notify deployment success
        uses: 8398a7/action-slack@v3
        with:
          status: success
          text: "🚀 Production deployment successful: ${{ github.event.release.tag_name }}"
        env:
          SLACK_WEBHOOK_URL: ${{ secrets.SLACK_WEBHOOK_URL }}

      - name: Log deployment to monitoring
        run: |
          curl -X POST "https://api.logfire.dev/v1/deployment" \
            -H "Authorization: Bearer ${{ secrets.LOGFIRE_API_KEY }}" \
            -H "Content-Type: application/json" \
            -d '{
              "environment": "production",
              "version": "${{ github.event.release.tag_name }}",
              "status": "success",
              "timestamp": "'$(date -u +%Y-%m-%dT%H:%M:%SZ)'"
            }'
```

### Pipeline Security Configuration
**Location**: `.github/workflows/security.yml`

```yaml
# .github/workflows/security.yml
name: Security Pipeline

on:
  schedule:
    - cron: '0 2 * * *'  # Daily at 2 AM
  workflow_dispatch:

jobs:
  dependency-audit:
    name: Dependency Security Audit
    runs-on: ubuntu-latest
    steps:
      - name: Checkout code
        uses: actions/checkout@v4

      - name: Python dependency audit
        run: |
          cd api
          pip install safety pip-audit
          pip freeze | safety check --json --output python-safety.json || true
          pip-audit --format=json --output=python-audit.json || true

      - name: Node.js dependency audit
        run: |
          cd frontend
          npm audit --audit-level=high --json > npm-audit.json || true

      - name: Rust dependency audit
        uses: actions-rs/audit@v1
        with:
          working-directory: vm-manager

      - name: Upload audit results
        uses: actions/upload-artifact@v3
        with:
          name: security-audit-results
          path: |
            api/python-safety.json
            api/python-audit.json
            frontend/npm-audit.json

  secret-scan:
    name: Secret Scanning
    runs-on: ubuntu-latest
    steps:
      - name: Checkout code
        uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Run TruffleHog
        uses: trufflesecurity/trufflehog@main
        with:
          path: ./
          base: main
          head: HEAD
          extra_args: --debug --only-verified

  license-check:
    name: License Compliance Check
    runs-on: ubuntu-latest
    steps:
      - name: Checkout code
        uses: actions/checkout@v4

      - name: Check Python licenses
        run: |
          cd api
          pip install pip-licenses
          pip-licenses --format=json --output-file=python-licenses.json

      - name: Check Node.js licenses
        run: |
          cd frontend
          npm install -g license-checker
          license-checker --json --out npm-licenses.json

      - name: Upload license reports
        uses: actions/upload-artifact@v3
        with:
          name: license-reports
          path: |
            api/python-licenses.json
            frontend/npm-licenses.json
```

### Production Dockerfiles
**Location**: `api/Dockerfile.prod`

```dockerfile
# api/Dockerfile.prod
# Multi-stage build for production API container

# Build stage
FROM python:3.11-slim as builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    build-essential \
    libpq-dev \
    gcc \
    && rm -rf /var/lib/apt/lists/*

# Create build user
RUN groupadd -r build && useradd -r -g build build

# Set work directory
WORKDIR /build

# Copy requirements
COPY requirements.txt requirements-prod.txt ./

# Install Python dependencies
RUN pip install --no-cache-dir --user -r requirements-prod.txt

# Production stage
FROM python:3.11-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    libpq5 \
    curl \
    && rm -rf /var/lib/apt/lists/* \
    && apt-get clean

# Create application user
RUN groupadd -r app && useradd -r -g app app

# Copy Python packages from builder
COPY --from=builder /root/.local /home/app/.local

# Create app directory structure
RUN mkdir -p /app /app/logs /app/tmp \
    && chown -R app:app /app

# Set environment variables
ENV PATH=/home/app/.local/bin:$PATH
ENV PYTHONPATH=/app
ENV PYTHONUNBUFFERED=1
ENV PYTHONDONTWRITEBYTECODE=1

# Switch to app user
USER app
WORKDIR /app

# Copy application code
COPY --chown=app:app . .

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8000/health || exit 1

# Expose port
EXPOSE 8000

# Start application
CMD ["uvicorn", "main:app", "--host", "0.0.0.0", "--port", "8000", "--workers", "4"]
```

**Location**: `frontend/Dockerfile.prod`

```dockerfile
# frontend/Dockerfile.prod
# Multi-stage build for production frontend container

# Build stage
FROM node:18-alpine as builder

# Set work directory
WORKDIR /build

# Copy package files
COPY package*.json ./

# Install dependencies
RUN npm ci --only=production && npm cache clean --force

# Copy source code
COPY . .

# Build application
RUN npm run build

# Production stage
FROM nginx:alpine

# Copy nginx configuration
COPY nginx.prod.conf /etc/nginx/nginx.conf

# Copy built application
COPY --from=builder /build/dist /usr/share/nginx/html

# Create nginx user and set permissions
RUN chown -R nginx:nginx /usr/share/nginx/html \
    && chown -R nginx:nginx /var/cache/nginx \
    && chown -R nginx:nginx /var/log/nginx \
    && chown -R nginx:nginx /etc/nginx/conf.d

# Switch to nginx user
USER nginx

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:80/health || exit 1

# Expose port
EXPOSE 80

# Start nginx
CMD ["nginx", "-g", "daemon off;"]
```

## TDD Implementation Cycle

### Red Phase: CI/CD Pipeline Test Creation
```python
# .github/scripts/test_pipeline.py
import pytest
import requests
import subprocess
import time
from typing import Dict, List

class PipelineValidator:
    def __init__(self, repo_url: str, token: str):
        self.repo_url = repo_url
        self.token = token
        self.headers = {"Authorization": f"token {token}"}
    
    def test_pipeline_workflow_exists(self):
        """Test that CI/CD workflow file exists and is valid"""
        # This test should initially fail (Red phase)
        assert False, "CI/CD workflow not implemented yet"
    
    def test_security_scanning_works(self):
        """Test that security scanning catches vulnerabilities"""
        # This test should initially fail (Red phase)
        assert False, "Security scanning not implemented yet"
    
    def test_automated_testing_coverage(self):
        """Test that automated tests achieve required coverage"""
        # This test should initially fail (Red phase)
        assert False, "Automated testing not implemented yet"
    
    def test_container_building_process(self):
        """Test that container images build successfully"""
        # This test should initially fail (Red phase)
        assert False, "Container building not implemented yet"
    
    def test_deployment_automation(self):
        """Test that deployment automation works correctly"""
        # This test should initially fail (Red phase)
        assert False, "Deployment automation not implemented yet"

# CLI for testing pipeline
if __name__ == "__main__":
    import os
    
    repo_url = os.getenv('GITHUB_REPOSITORY')
    token = os.getenv('GITHUB_TOKEN')
    
    validator = PipelineValidator(repo_url, token)
    
    try:
        validator.test_pipeline_workflow_exists()
        validator.test_security_scanning_works()
        validator.test_automated_testing_coverage()
        validator.test_container_building_process()
        validator.test_deployment_automation()
        print("All pipeline tests passed!")
    except AssertionError as e:
        print(f"Pipeline test failed: {e}")
        exit(1)
```

### Green Phase: CI/CD Pipeline Implementation
```python
# Implement CI/CD pipeline features to make tests pass
# This involves creating GitHub Actions workflows, Dockerfiles, and deployment scripts
```

### Refactor Phase: CI/CD Pipeline Optimization
```python
# Optimize CI/CD pipeline for performance and reliability
# Add advanced deployment strategies and enhanced monitoring
# Improve error handling and recovery procedures
```

## Security Checklist ✅

### CI/CD Pipeline Security
- [ ] Secure CI/CD pipeline with least privilege access
- [ ] Vulnerability scanning in all pipeline stages
- [ ] Secret scanning and prevention of credential leaks
- [ ] Code signing and image attestation
- [ ] Dependency vulnerability scanning and updates
- [ ] Static application security testing (SAST)
- [ ] Dynamic application security testing (DAST)
- [ ] Infrastructure as Code security scanning
- [ ] Secure artifact storage and access controls
- [ ] Audit logging for all CI/CD activities

### Container Security
- [ ] Container image vulnerability scanning
- [ ] Distroless or minimal base images
- [ ] Non-root container execution
- [ ] Read-only root filesystems
- [ ] Resource limits and security contexts
- [ ] Container runtime security policies
- [ ] Image signing and verification
- [ ] Registry access controls and scanning
- [ ] Container network security
- [ ] Runtime threat detection

### Build Security
- [ ] Secure build environment and isolation
- [ ] Build process validation and verification
- [ ] Secure dependency management and updates
- [ ] Build artifact integrity and signing
- [ ] Secure build cache management
- [ ] Build environment access controls
- [ ] Build process monitoring and logging
- [ ] Secure build tool configuration
- [ ] Build reproducibility and verification
- [ ] Emergency build procedures and access

### Deployment Security
- [ ] Secure deployment pipelines with approval gates
- [ ] Environment isolation and access controls
- [ ] Production deployment restrictions
- [ ] Rollback procedures and emergency access
- [ ] Change management and approval processes
- [ ] Deployment verification and health checks
- [ ] Monitoring and alerting for deployments
- [ ] Incident response procedures
- [ ] Compliance validation and reporting
- [ ] Security testing in deployment pipeline

## Performance Requirements

### Build Performance
- Build time < 10 minutes for complete pipeline
- Test execution < 15 minutes for full test suite
- Container image build < 5 minutes per service
- Security scanning < 3 minutes per scan
- Parallel job execution for maximum efficiency
- Build cache hit rate > 80%

### Deployment Performance
- Staging deployment time < 5 minutes
- Production deployment time < 5 minutes
- Rollback time < 2 minutes
- Zero-downtime deployment capability
- Health check response < 100ms
- Deployment success rate > 99%

### Pipeline Performance
- Pipeline trigger latency < 30 seconds
- Artifact upload/download speed > 10 MB/s
- Test result processing < 1 minute
- Notification delivery < 10 seconds
- Resource utilization efficiency > 80%
- Pipeline failure recovery < 5 minutes

## Commit Instructions

After implementing the CI/CD pipeline system:

```bash
git add .github/ api/Dockerfile.prod frontend/Dockerfile.prod vm-manager/Dockerfile.prod
git commit -m "Add comprehensive GitHub Actions CI/CD pipeline

- Implement multi-stage CI/CD workflow with security scanning
- Add automated testing pipeline for backend, frontend, and VM manager
- Include container image building with multi-platform support
- Add staging and production deployment automation with Helm
- Implement security scanning with Trivy and Semgrep
- Add dependency auditing and license compliance checking
- Include comprehensive test coverage reporting
- Add deployment monitoring and notification integration
- Implement zero-downtime deployment with health checks
- Add TDD cycle with Red-Green-Refactor for CI/CD pipeline
- Ensure >95% pipeline test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete CI/CD pipeline test suite:

```bash
# Test GitHub Actions workflow validation
.github/scripts/validate-workflows.sh

# Test Docker build process
docker build -f api/Dockerfile.prod -t test-api ./api
docker build -f frontend/Dockerfile.prod -t test-frontend ./frontend
docker build -f vm-manager/Dockerfile.prod -t test-vm-manager ./vm-manager

# Test security scanning
trivy fs --format table .
semgrep --config=auto .

# Test deployment automation
helm template ./k8s/helm/build-platform --values ./k8s/helm/build-platform/values-staging.yaml
```

Validate CI/CD pipeline test coverage:
```bash
python .github/scripts/test_pipeline.py
pytest .github/tests/ --cov=pipeline --cov-report=html --cov-fail-under=95
```

## Integration Testing

Test CI/CD pipeline integration with platform components:
```bash
# Test integration with Session 1 (Logfire)
pytest .github/tests/integration/test_pipeline_logfire_integration.py -v

# Test integration with monitoring systems
pytest .github/tests/integration/test_pipeline_monitoring_integration.py -v

# Test complete deployment pipeline
pytest .github/tests/integration/test_complete_deployment_pipeline.py -v
```