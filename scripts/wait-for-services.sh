#!/bin/bash

# Wait for services to be ready
set -e

echo "⏳ Waiting for infrastructure services to be ready..."

# Wait for PostgreSQL
echo "🔄 Waiting for PostgreSQL..."
timeout=30
while ! podman exec build_postgres_dev pg_isready -U postgres >/dev/null 2>&1; do
    sleep 1
    timeout=$((timeout - 1))
    if [[ $timeout -eq 0 ]]; then
        echo "❌ PostgreSQL failed to start within 30 seconds"
        exit 1
    fi
done
echo "✅ PostgreSQL is ready"

# Wait for Redis
echo "🔄 Waiting for Redis..."
timeout=30
while ! podman exec build_redis_dev redis-cli -a dev_password ping >/dev/null 2>&1; do
    sleep 1
    timeout=$((timeout - 1))
    if [[ $timeout -eq 0 ]]; then
        echo "❌ Redis failed to start within 30 seconds"
        exit 1
    fi
done
echo "✅ Redis is ready"

# Wait for MinIO
echo "🔄 Waiting for MinIO..."
timeout=30
while ! curl -f http://localhost:9000/minio/health/live >/dev/null 2>&1; do
    sleep 1
    timeout=$((timeout - 1))
    if [[ $timeout -eq 0 ]]; then
        echo "❌ MinIO failed to start within 30 seconds"
        exit 1
    fi
done
echo "✅ MinIO is ready"

echo "🎉 All infrastructure services are ready!"