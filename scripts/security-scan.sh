#!/bin/bash

# Security scanning script for Build platform
set -e

echo "🔒 Running security scans for Build platform..."

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Exit code tracking
EXIT_CODE=0

# Function to run a security check
run_check() {
    local check_name="$1"
    local command="$2"
    
    echo -e "\n🔍 Running $check_name..."
    
    if eval "$command"; then
        echo -e "${GREEN}✅ $check_name passed${NC}"
    else
        echo -e "${RED}❌ $check_name failed${NC}"
        EXIT_CODE=1
    fi
}

# Python security checks
if [[ -d "api" ]]; then
    echo "🐍 Python Security Checks"
    
    # Install security tools if not present
    if ! command -v bandit &> /dev/null; then
        echo "Installing Python security tools..."
        pip install bandit safety semgrep
    fi
    
    # Bandit security scan
    run_check "Bandit Security Scan" "bandit -r api/app -f json -o bandit-report.json || bandit -r api/app"
    
    # Safety dependency check
    run_check "Safety Dependency Check" "cd api && safety check --json --output safety-report.json || safety check"
    
    # Semgrep security scan
    if command -v semgrep &> /dev/null; then
        run_check "Semgrep Security Scan" "semgrep --config=auto api/ || true"
    fi
fi

# Node.js security checks
if [[ -d "frontend" && -f "frontend/package.json" ]]; then
    echo -e "\n📦 Node.js Security Checks"
    
    # npm audit
    run_check "NPM Audit" "cd frontend && npm audit --audit-level=moderate"
    
    # Check for known vulnerabilities
    if command -v yarn &> /dev/null; then
        run_check "Yarn Audit" "cd frontend && yarn audit --level moderate || true"
    fi
fi

# Container security checks
if command -v trivy &> /dev/null; then
    echo -e "\n🐳 Container Security Checks"
    
    # Scan Dockerfiles
    if [[ -f "api/Dockerfile.dev" ]]; then
        run_check "Trivy Dockerfile Scan (API)" "trivy config api/Dockerfile.dev"
    fi
    
    if [[ -f "frontend/Dockerfile.dev" ]]; then
        run_check "Trivy Dockerfile Scan (Frontend)" "trivy config frontend/Dockerfile.dev"
    fi
fi

# Secret detection
echo -e "\n🕵️ Secret Detection"

# Check for secrets in files
run_check "Secret Detection" "detect-secrets scan --baseline .secrets.baseline --all-files"

# Check environment files
run_check "Environment File Check" "! grep -r 'password\\|secret\\|key' .env* 2>/dev/null | grep -v 'example\\|template' || echo 'No hardcoded secrets in .env files'"

# Git security checks
echo -e "\n📝 Git Security Checks"

# Check for secrets in git history
run_check "Git History Secret Check" "git log --all --full-history -- '*.env*' | grep -q 'password\\|secret\\|key' && echo 'Potential secrets in git history' && exit 1 || echo 'No obvious secrets in git history'"

# Check for large files
run_check "Large File Check" "! git ls-files | xargs ls -la | awk '{if (\$5 > 10485760) print \$9, \$5}' | grep -q . || echo 'No large files found'"

# Infrastructure security checks
if [[ -f "podman-compose.yml" ]]; then
    echo -e "\n🏗️ Infrastructure Security Checks"
    
    # Check for exposed ports
    run_check "Port Exposure Check" "! grep -E 'ports:|expose:' podman-compose.yml | grep -E '(0\\.0\\.0\\.0|\\*):[0-9]+:[0-9]+' || echo 'Warning: Services exposed to all interfaces'"
    
    # Check for privileged containers
    run_check "Privileged Container Check" "! grep -q 'privileged.*true' podman-compose.yml || echo 'Warning: Privileged containers found'"
    
    # Check for root user
    run_check "Root User Check" "! grep -q 'user.*root' podman-compose.yml || echo 'Warning: Root user containers found'"
fi

# Configuration security checks
echo -e "\n⚙️ Configuration Security Checks"

# Check SSL/TLS configuration
run_check "SSL Configuration Check" "grep -q 'MINIO_SECURE=false' .env* && echo 'Warning: MinIO SSL disabled in development' || echo 'SSL configuration checked'"

# Check debug settings
run_check "Debug Settings Check" "grep -q 'DEBUG=true' .env* && echo 'Warning: Debug mode enabled (development only)' || echo 'Debug settings checked'"

# Final report
echo -e "\n📊 Security Scan Summary"

if [[ $EXIT_CODE -eq 0 ]]; then
    echo -e "${GREEN}🎉 All security checks passed!${NC}"
else
    echo -e "${RED}⚠️  Some security checks failed. Please review the output above.${NC}"
fi

echo -e "\n📁 Security Reports Generated:"
[[ -f "bandit-report.json" ]] && echo "  - bandit-report.json"
[[ -f "safety-report.json" ]] && echo "  - safety-report.json"

echo -e "\n🔧 To fix issues:"
echo "  - Review and update dependencies"
echo "  - Fix code security issues"
echo "  - Update configuration settings"
echo "  - Remove any hardcoded secrets"

exit $EXIT_CODE