#!/bin/bash

# Test script to verify all services are working correctly
set -e

echo "🧪 Testing Build platform services..."

# Color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

EXIT_CODE=0

# Function to test a service
test_service() {
    local service_name="$1"
    local test_command="$2"
    
    echo -n "Testing $service_name... "
    
    if eval "$test_command" >/dev/null 2>&1; then
        echo -e "${GREEN}✅ PASS${NC}"
    else
        echo -e "${RED}❌ FAIL${NC}"
        EXIT_CODE=1
    fi
}

# Test PostgreSQL
echo "🐘 Testing PostgreSQL..."
test_service "PostgreSQL Connection" "podman exec build_postgres_dev psql -U postgres -d build_dev -c 'SELECT 1;'"

# Test Redis
echo -e "\n🔴 Testing Redis..."
test_service "Redis Connection" "podman exec build_redis_dev redis-cli -a dev_password ping"

# Test MinIO
echo -e "\n📦 Testing MinIO..."
test_service "MinIO Health Check" "curl -f http://localhost:9000/minio/health/live"
test_service "MinIO Console" "curl -f http://localhost:9001/"

# Test Soft-serve Git Server
echo -e "\n🐙 Testing Git Server..."
test_service "Git Server SSH" "nc -z localhost 23231"
test_service "Git Server HTTP" "nc -z localhost 23232"

# Summary
echo -e "\n📊 Service Test Summary"
if [[ $EXIT_CODE -eq 0 ]]; then
    echo -e "${GREEN}🎉 All services are running correctly!${NC}"
    echo -e "\n📍 Service URLs:"
    echo "  PostgreSQL:    localhost:5434"
    echo "  Redis:         localhost:6381"
    echo "  MinIO API:     http://localhost:9000"
    echo "  MinIO Console: http://localhost:9001"
    echo "  Git SSH:       ssh://localhost:23231"
    echo "  Git HTTP:      http://localhost:23232"
else
    echo -e "${RED}⚠️  Some services are not working correctly.${NC}"
    echo "Check the service logs with: podman-compose logs [service-name]"
fi

exit $EXIT_CODE