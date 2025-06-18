# Build Platform Scripts

This directory contains utility and deployment scripts for the Build platform.

## Scripts Overview

- `dev-setup.sh` - Initialize development environment
- `seed-dev-data.py` - Populate development database with test data
- `wait-for-services.sh` - Wait for services to be ready
- `backup-dev-data.sh` - Backup development data
- `security-scan.sh` - Run security checks

## Usage

Scripts should be run from the project root directory:

```bash
# Start development environment
./scripts/dev-setup.sh

# Seed development data
./scripts/seed-dev-data.py
```

## Development Scripts

Scripts in this directory are for development and deployment automation. Production deployment uses infrastructure configuration in the `infrastructure/` directory.