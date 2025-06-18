#!/bin/bash

# Build Platform Development Setup Script
set -e

echo "🚀 Starting Build platform development environment..."

# Check prerequisites
echo "📋 Checking prerequisites..."

if ! command -v podman &> /dev/null; then
    echo "❌ Podman is not installed. Please install Podman first."
    exit 1
fi

if ! command -v python3 &> /dev/null; then
    echo "❌ Python 3 is not installed. Please install Python 3.11+ first."
    exit 1
fi

if ! command -v node &> /dev/null; then
    echo "❌ Node.js is not installed. Please install Node.js 18+ first."
    exit 1
fi

# Check Python version
PYTHON_VERSION=$(python3 -c "import sys; print(f'{sys.version_info.major}.{sys.version_info.minor}')")
if [[ "$(echo "$PYTHON_VERSION < 3.11" | bc -l 2>/dev/null || echo "1")" == "1" ]]; then
    echo "❌ Python 3.11+ is required. Current version: $PYTHON_VERSION"
    exit 1
fi

# Check Node.js version
NODE_VERSION=$(node -v | sed 's/v//' | cut -d. -f1)
if [[ "$NODE_VERSION" -lt 18 ]]; then
    echo "❌ Node.js 18+ is required. Current version: $(node -v)"
    exit 1
fi

echo "✅ Prerequisites check passed"

# Create development environment file if it doesn't exist
if [[ ! -f .env.development ]]; then
    echo "📝 Creating development environment file..."
    cp .env.example .env.development
    echo "✅ Created .env.development from template"
fi

# Start infrastructure services first
echo "🏗️  Starting infrastructure services..."
podman-compose up -d postgres redis minio softserve

# Wait for services to be ready
echo "⏳ Waiting for services to be ready..."
./scripts/wait-for-services.sh

# Build and start application services
echo "🔨 Building and starting application services..."
podman-compose up -d --build api websocket-gateway frontend

echo "⏳ Waiting for application services to be ready..."
sleep 15

# Run database migrations
echo "🗄️  Running database migrations..."
if podman exec build_api_dev alembic upgrade head; then
    echo "✅ Database migrations completed"
else
    echo "⚠️  Database migrations skipped (alembic not ready yet)"
fi

# Seed development data
echo "🌱 Seeding development data..."
if [[ -f scripts/seed-dev-data.py ]]; then
    if podman exec build_api_dev python scripts/seed-dev-data.py; then
        echo "✅ Development data seeded"
    else
        echo "⚠️  Development data seeding skipped"
    fi
fi

echo ""
echo "🎉 Build platform development environment is ready!"
echo ""
echo "📍 Service URLs:"
echo "   Frontend:      http://localhost:3000"
echo "   API:           http://localhost:8000"
echo "   API Docs:      http://localhost:8000/docs"
echo "   MinIO Console: http://localhost:9001"
echo "   Git Server:    ssh://localhost:23231"
echo ""
echo "🔧 Useful commands:"
echo "   View logs:     podman-compose logs -f [service]"
echo "   Stop services: podman-compose down"
echo "   Reset data:    podman-compose down -v && ./scripts/dev-setup.sh"
echo ""
echo "📚 Documentation: http://localhost:3000/docs"