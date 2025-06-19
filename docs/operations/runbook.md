# Build Platform - Operations Runbook

## Overview

This runbook provides operational procedures for managing the Build Platform infrastructure and services. It covers deployment, monitoring, troubleshooting, and emergency response procedures.

**Last Updated:** June 19, 2025  
**Version:** 1.0.0

## Quick Reference

### Emergency Contacts
- **On-Call Engineer:** Check Slack #build-platform-oncall
- **Platform Lead:** [Contact Info]
- **Security Team:** security@buildplatform.dev
- **Infrastructure Team:** infra@buildplatform.dev

### Critical Service URLs
- **Production API:** https://api.getbuild.ing
- **Staging API:** https://staging-api.getbuild.ing
- **Monitoring Dashboard:** https://monitoring.getbuild.ing
- **Status Page:** https://status.getbuild.ing

## Service Architecture

### Core Services
1. **API Service** (FastAPI) - Main application backend
2. **WebSocket Gateway** - Real-time communication
3. **PostgreSQL** - Primary database
4. **Redis** - Cache and session storage
5. **MinIO** - S3-compatible object storage
6. **Soft-serve** - Git repository hosting

### External Dependencies
- **Cloudflare** - CDN and DDoS protection
- **SendGrid** - Email delivery
- **Logfire** - Observability and monitoring

## Deployment Procedures

### Prerequisites
- Access to production Kubernetes cluster
- Valid kubectl context configured
- Required secrets and configurations in place
- Backup verification completed

### Standard Deployment

#### 1. Pre-deployment Checklist
```bash
# Verify cluster access
kubectl get nodes

# Check current service status
kubectl get pods -n build-platform

# Verify secrets are in place
kubectl get secrets -n build-platform

# Check monitoring alerts
curl -s https://monitoring.getbuild.ing/api/alerts | jq '.active_alerts'
```

#### 2. Deployment Steps
```bash
# 1. Update deployment manifests
git checkout main
git pull origin main

# 2. Apply database migrations (if any)
kubectl exec -n build-platform deployment/api -- python -m alembic upgrade head

# 3. Deploy API service
kubectl apply -f k8s/api/

# 4. Deploy WebSocket gateway
kubectl apply -f k8s/websocket/

# 5. Deploy frontend
kubectl apply -f k8s/frontend/

# 6. Verify deployment
kubectl rollout status deployment/api -n build-platform
kubectl rollout status deployment/websocket-gateway -n build-platform
kubectl rollout status deployment/frontend -n build-platform
```

#### 3. Post-deployment Verification
```bash
# Health checks
curl -f https://api.getbuild.ing/health
curl -f https://api.getbuild.ing/health/detailed

# API functionality test
curl -X POST https://api.getbuild.ing/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email":"test@example.com","password":"TestPassword123!"}'

# Monitor logs for errors
kubectl logs -f deployment/api -n build-platform | grep ERROR

# Check metrics
curl -s https://api.getbuild.ing/metrics | grep "http_requests_total"
```

### Rollback Procedure

#### Emergency Rollback
```bash
# 1. Immediate rollback to previous version
kubectl rollout undo deployment/api -n build-platform
kubectl rollout undo deployment/websocket-gateway -n build-platform
kubectl rollout undo deployment/frontend -n build-platform

# 2. Verify rollback status
kubectl rollout status deployment/api -n build-platform

# 3. Check service health
curl -f https://api.getbuild.ing/health

# 4. Notify team
echo "Emergency rollback completed at $(date)" | \
  slack-cli send -c "#build-platform-alerts"
```

#### Database Rollback (if needed)
```bash
# WARNING: Only use if database changes are incompatible
kubectl exec -n build-platform deployment/api -- \
  python -m alembic downgrade [previous_revision]
```

## Monitoring and Alerting

### Dashboard Access
- **Primary Dashboard:** https://monitoring.getbuild.ing/dashboard/main
- **Security Dashboard:** https://monitoring.getbuild.ing/dashboard/security
- **Operations Dashboard:** https://monitoring.getbuild.ing/dashboard/operations

### Critical Alerts

#### Database Connection Pool High (>80%)
**Severity:** HIGH  
**Symptoms:** Slow API responses, connection timeouts  
**Immediate Actions:**
1. Check current pool usage: `curl -s https://api.getbuild.ing/api/v1/health/system`
2. Look for connection leaks in logs: `kubectl logs deployment/api -n build-platform | grep "pool"`
3. Scale API pods if needed: `kubectl scale deployment/api --replicas=5 -n build-platform`
4. Consider increasing pool size in configuration

#### Redis Memory High (>80%)
**Severity:** HIGH  
**Symptoms:** Cache misses, session issues  
**Immediate Actions:**
1. Check Redis memory: `kubectl exec redis-0 -n build-platform -- redis-cli info memory`
2. Flush expired keys: `kubectl exec redis-0 -n build-platform -- redis-cli expire`
3. Check for memory leaks: `kubectl exec redis-0 -n build-platform -- redis-cli --bigkeys`
4. Scale Redis if needed or increase memory limits

#### API Error Rate High (>1%)
**Severity:** MEDIUM  
**Symptoms:** User complaints, failed requests  
**Immediate Actions:**
1. Check error breakdown: `curl -s https://api.getbuild.ing/api/v1/metrics/logfire`
2. Review recent logs: `kubectl logs deployment/api -n build-platform --tail=100`
3. Check specific endpoints: `curl -s https://api.getbuild.ing/metrics | grep http_requests_total`
4. Scale API if traffic-related: `kubectl scale deployment/api --replicas=5 -n build-platform`

#### Authentication Attacks Detected
**Severity:** CRITICAL  
**Symptoms:** Multiple failed login attempts, potential brute force  
**Immediate Actions:**
1. Review security dashboard: https://monitoring.getbuild.ing/dashboard/security
2. Check failed login patterns: `kubectl logs deployment/api -n build-platform | grep "login_failed"`
3. Consider IP blocking: Add IPs to WAF blocklist
4. Notify security team immediately
5. Monitor for account lockouts: Check Redis for `auth_lockout:*` keys

#### Disk Space Low (<15% free)
**Severity:** HIGH  
**Symptoms:** Service instability, write failures  
**Immediate Actions:**
1. Check disk usage: `kubectl exec -it pod-name -- df -h`
2. Clean up logs: `kubectl exec -it pod-name -- find /var/log -name "*.log" -mtime +7 -delete`
3. Clean up temp files: `kubectl exec -it pod-name -- rm -rf /tmp/*`
4. Request disk expansion if needed

### Monitoring Commands

#### Service Health
```bash
# Overall health
curl -s https://api.getbuild.ing/health/detailed | jq .

# Individual service health
kubectl get pods -n build-platform
kubectl top pods -n build-platform

# Service logs
kubectl logs -f deployment/api -n build-platform
kubectl logs -f deployment/websocket-gateway -n build-platform
```

#### Performance Metrics
```bash
# Request rates and latency
curl -s https://api.getbuild.ing/metrics | grep -E "(http_requests_total|http_request_duration)"

# Database performance
curl -s https://api.getbuild.ing/api/v1/health/system | jq '.dependencies.database'

# Redis performance
curl -s https://api.getbuild.ing/api/v1/health/cache | jq .
```

#### Resource Usage
```bash
# CPU and Memory
kubectl top pods -n build-platform

# Network usage
kubectl exec -it deployment/api -n build-platform -- netstat -i

# Disk I/O
kubectl exec -it deployment/api -n build-platform -- iostat 1 3
```

## Troubleshooting Guide

### Common Issues

#### Service Won't Start
**Symptoms:** Pod in CrashLoopBackOff, startup errors  
**Investigation Steps:**
1. Check pod logs: `kubectl logs pod-name -n build-platform`
2. Check resource limits: `kubectl describe pod pod-name -n build-platform`
3. Verify configuration: `kubectl get configmap -n build-platform`
4. Check secrets: `kubectl get secrets -n build-platform`

**Common Causes:**
- Missing environment variables
- Database connection failure
- Insufficient resources
- Configuration errors

#### Database Connection Issues
**Symptoms:** Connection timeouts, pool exhaustion  
**Investigation Steps:**
1. Check database connectivity: `kubectl exec deployment/api -n build-platform -- pg_isready -h postgres`
2. Check connection pool: Look for pool metrics in health endpoint
3. Review database logs: `kubectl logs postgres-0 -n build-platform`
4. Check network policies: `kubectl get networkpolicies -n build-platform`

#### Authentication Problems
**Symptoms:** Login failures, token errors  
**Investigation Steps:**
1. Check JWT configuration: Verify JWT_SECRET is set correctly
2. Review auth logs: `kubectl logs deployment/api -n build-platform | grep auth`
3. Check Redis connectivity: `kubectl exec deployment/api -n build-platform -- redis-cli ping`
4. Verify user data: Check users table in database

#### High Memory Usage
**Symptoms:** Pods being OOMKilled, slow performance  
**Investigation Steps:**
1. Check memory usage: `kubectl top pods -n build-platform`
2. Review memory limits: `kubectl describe pod pod-name -n build-platform`
3. Look for memory leaks: Check application metrics and logs
4. Profile memory usage: Use development tools if needed

#### Network Connectivity Issues
**Symptoms:** Service-to-service communication failures  
**Investigation Steps:**
1. Check service endpoints: `kubectl get endpoints -n build-platform`
2. Test connectivity: `kubectl exec pod-name -n build-platform -- nc -zv service-name port`
3. Review network policies: `kubectl get networkpolicies -n build-platform`
4. Check DNS resolution: `kubectl exec pod-name -n build-platform -- nslookup service-name`

### Debugging Commands

#### Pod Investigation
```bash
# Get pod details
kubectl describe pod pod-name -n build-platform

# Execute into pod
kubectl exec -it pod-name -n build-platform -- /bin/bash

# Check resource usage
kubectl top pod pod-name -n build-platform

# Get pod logs
kubectl logs pod-name -n build-platform --previous
```

#### Network Debugging
```bash
# Test service connectivity
kubectl exec -it pod-name -n build-platform -- curl -v http://service-name:port/health

# Check DNS resolution
kubectl exec -it pod-name -n build-platform -- nslookup service-name

# Port forwarding for local testing
kubectl port-forward service/api 8080:8000 -n build-platform
```

#### Database Debugging
```bash
# Connect to database
kubectl exec -it postgres-0 -n build-platform -- psql -U postgres -d build_prod

# Check database connections
kubectl exec -it postgres-0 -n build-platform -- psql -U postgres -c "SELECT * FROM pg_stat_activity;"

# Check database size
kubectl exec -it postgres-0 -n build-platform -- psql -U postgres -c "SELECT pg_size_pretty(pg_database_size('build_prod'));"
```

## Security Procedures

### Incident Response

#### Security Incident Classification
- **Level 1 (Critical):** Data breach, unauthorized access to production
- **Level 2 (High):** Successful attack, service compromise
- **Level 3 (Medium):** Attempted attack, suspicious activity
- **Level 4 (Low):** Policy violation, minor security issue

#### Immediate Response Steps
1. **Assess and Contain**
   - Identify scope of incident
   - Isolate affected systems
   - Preserve evidence

2. **Notify Stakeholders**
   - Security team (immediate)
   - Platform lead (within 15 minutes)
   - Management (within 1 hour)

3. **Document Everything**
   - Timeline of events
   - Actions taken
   - Evidence collected

#### Security Monitoring

##### Failed Authentication Monitoring
```bash
# Check failed login attempts
kubectl logs deployment/api -n build-platform | grep "login_failed" | tail -50

# Check account lockouts
kubectl exec redis-0 -n build-platform -- redis-cli keys "auth_lockout:*"

# Review security events
curl -s https://api.getbuild.ing/api/v1/security/events | jq '.events[] | select(.severity=="high")'
```

##### Suspicious Activity Detection
```bash
# Unusual API usage patterns
curl -s https://api.getbuild.ing/metrics | grep -E "http_requests_total.*[45][0-9][0-9]"

# Check for privilege escalation attempts
kubectl logs deployment/api -n build-platform | grep -E "(admin|sudo|root)"

# Monitor large data exports
kubectl logs deployment/api -n build-platform | grep "large_export"
```

### Access Management

#### Emergency Access
```bash
# Enable emergency access (break-glass)
kubectl create clusterrolebinding emergency-access \
  --clusterrole=cluster-admin \
  --user=emergency@buildplatform.dev

# Disable after incident
kubectl delete clusterrolebinding emergency-access
```

#### Rotate Secrets
```bash
# Generate new JWT secret
kubectl create secret generic jwt-secret \
  --from-literal=JWT_SECRET=$(openssl rand -base64 32) \
  --dry-run=client -o yaml | kubectl apply -f -

# Restart services to pick up new secret
kubectl rollout restart deployment/api -n build-platform
```

## Backup and Recovery

### Backup Procedures

#### Database Backup
```bash
# Create database backup
kubectl exec postgres-0 -n build-platform -- \
  pg_dump -U postgres build_prod | gzip > backup-$(date +%Y%m%d-%H%M%S).sql.gz

# Upload to S3
aws s3 cp backup-*.sql.gz s3://build-platform-backups/database/
```

#### Configuration Backup
```bash
# Backup Kubernetes configurations
kubectl get all -n build-platform -o yaml > k8s-backup-$(date +%Y%m%d).yaml

# Backup secrets (encrypted)
kubectl get secrets -n build-platform -o yaml | \
  gpg --cipher-algo AES256 --compress-algo 1 --symmetric > secrets-backup-$(date +%Y%m%d).yaml.gpg
```

### Recovery Procedures

#### Database Recovery
```bash
# Stop API services
kubectl scale deployment/api --replicas=0 -n build-platform

# Restore database
gunzip -c backup-20250619-120000.sql.gz | \
  kubectl exec -i postgres-0 -n build-platform -- psql -U postgres build_prod

# Restart services
kubectl scale deployment/api --replicas=3 -n build-platform
```

#### Service Recovery
```bash
# Restore from backup configuration
kubectl apply -f k8s-backup-20250619.yaml

# Verify services
kubectl get pods -n build-platform
kubectl get services -n build-platform
```

## Maintenance Procedures

### Scheduled Maintenance

#### Database Maintenance
```bash
# Analyze and vacuum database (during low traffic)
kubectl exec postgres-0 -n build-platform -- \
  psql -U postgres -d build_prod -c "VACUUM ANALYZE;"

# Reindex database (if needed)
kubectl exec postgres-0 -n build-platform -- \
  psql -U postgres -d build_prod -c "REINDEX DATABASE build_prod;"
```

#### Cache Maintenance
```bash
# Clear expired Redis keys
kubectl exec redis-0 -n build-platform -- redis-cli FLUSHEXPIRED

# Check Redis memory usage
kubectl exec redis-0 -n build-platform -- redis-cli INFO memory
```

#### Log Rotation
```bash
# Clean old logs (automated via logrotate)
kubectl exec deployment/api -n build-platform -- \
  find /var/log -name "*.log" -mtime +30 -delete

# Compress old logs
kubectl exec deployment/api -n build-platform -- \
  find /var/log -name "*.log" -mtime +7 -exec gzip {} \;
```

### System Updates

#### Operating System Updates
```bash
# Update node images (rolling update)
kubectl get nodes
# Use cluster autoscaler to replace nodes with updated images

# Verify no disruption
kubectl get pods -n build-platform
```

#### Application Updates
```bash
# Update application dependencies
# Update Dockerfile with new base image
# Build and push new image
# Update deployment manifests
# Apply rolling update
```

## Performance Optimization

### Database Optimization

#### Query Performance
```bash
# Check slow queries
kubectl exec postgres-0 -n build-platform -- \
  psql -U postgres -d build_prod -c "SELECT * FROM pg_stat_statements ORDER BY total_time DESC LIMIT 10;"

# Check index usage
kubectl exec postgres-0 -n build-platform -- \
  psql -U postgres -d build_prod -c "SELECT * FROM pg_stat_user_indexes WHERE idx_scan = 0;"
```

#### Connection Pool Tuning
```bash
# Monitor connection pool usage
curl -s https://api.getbuild.ing/api/v1/health/system | jq '.dependencies.database.pool_usage'

# Adjust pool size in configuration if needed
# Update deployment with new pool settings
```

### Cache Optimization

#### Redis Performance
```bash
# Monitor cache hit ratio
kubectl exec redis-0 -n build-platform -- redis-cli INFO stats | grep hit_rate

# Check memory usage patterns
kubectl exec redis-0 -n build-platform -- redis-cli --bigkeys

# Optimize memory usage
kubectl exec redis-0 -n build-platform -- redis-cli CONFIG SET maxmemory-policy allkeys-lru
```

### Application Performance

#### API Performance
```bash
# Monitor response times
curl -s https://api.getbuild.ing/metrics | grep http_request_duration

# Check for N+1 queries in logs
kubectl logs deployment/api -n build-platform | grep "query" | grep -c "SELECT"

# Profile memory usage
kubectl top pods -n build-platform
```

## Contact Information

### Team Contacts
- **Platform Team:** #build-platform-team
- **On-Call Engineer:** #build-platform-oncall
- **Security Team:** #security-incidents
- **Infrastructure Team:** #infrastructure-alerts

### Escalation Path
1. **Level 1:** On-call engineer
2. **Level 2:** Platform lead
3. **Level 3:** Engineering manager
4. **Level 4:** CTO

### External Contacts
- **Cloud Provider Support:** [Support Portal]
- **Security Vendor:** [Contact Info]
- **Monitoring Vendor:** [Contact Info]

---

**Document Maintainer:** Platform Team  
**Review Frequency:** Monthly  
**Last Review:** June 19, 2025