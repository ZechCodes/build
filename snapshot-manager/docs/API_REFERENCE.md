# VM Snapshot Manager API Reference

## Overview

The VM Snapshot Manager provides a comprehensive REST API for managing VM snapshots with advanced security, performance optimization, and monitoring capabilities.

**Base URL:** `https://api.snapshot-manager.com/api/v1`  
**Authentication:** JWT Bearer Token  
**Content-Type:** `application/json`

## Authentication

All API endpoints require authentication using JWT Bearer tokens.

```http
Authorization: Bearer <your-jwt-token>
```

### Obtain Token

```http
POST /auth/login
Content-Type: application/json

{
  "username": "user@example.com",
  "password": "secure_password"
}
```

**Response:**
```json
{
  "access_token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...",
  "token_type": "bearer",
  "expires_in": 900
}
```

## Core Snapshot Operations

### Create Snapshot

Create a new VM snapshot with optional optimization settings.

```http
POST /snapshots
Authorization: Bearer <token>
Content-Type: application/json

{
  "vm_id": "vm_12345",
  "name": "backup-before-update",
  "description": "Backup before system update",
  "snapshot_type": "manual",
  "tags": ["backup", "pre-update"],
  "encrypt": true,
  "priority": "normal",
  "optimization_strategy": "balanced"
}
```

**Parameters:**
- `vm_id` (string, required): VM identifier
- `name` (string, required): Human-readable snapshot name
- `description` (string, optional): Snapshot description
- `snapshot_type` (string, optional): Type of snapshot (`manual`, `automatic`, `scheduled`)
- `tags` (array, optional): Array of tags for categorization
- `encrypt` (boolean, optional): Enable encryption (default: true)
- `priority` (string, optional): Operation priority (`low`, `normal`, `high`)
- `optimization_strategy` (string, optional): Optimization strategy (`speed`, `compression`, `balanced`, `quality`)

**Response:**
```json
{
  "snapshot_id": "snap_vm_12345_user_1234567890",
  "status": "creating",
  "vm_id": "vm_12345",
  "name": "backup-before-update",
  "created_at": "2024-01-15T10:30:00Z",
  "estimated_completion": "2024-01-15T10:32:00Z",
  "trace_id": "trace_abc123def456"
}
```

### List Snapshots

Retrieve a list of snapshots with filtering and pagination.

```http
GET /snapshots?vm_id=vm_12345&status=available&limit=20&offset=0
Authorization: Bearer <token>
```

**Query Parameters:**
- `vm_id` (string, optional): Filter by VM ID
- `status` (string, optional): Filter by status (`creating`, `available`, `restoring`, `deleting`, `error`)
- `tags` (string, optional): Comma-separated tags to filter by
- `limit` (integer, optional): Number of results per page (default: 20, max: 100)
- `offset` (integer, optional): Number of results to skip (default: 0)
- `sort_by` (string, optional): Sort field (`created_at`, `name`, `size_bytes`)
- `sort_order` (string, optional): Sort order (`asc`, `desc`)

**Response:**
```json
{
  "snapshots": [
    {
      "snapshot_id": "snap_vm_12345_user_1234567890",
      "vm_id": "vm_12345",
      "name": "backup-before-update",
      "description": "Backup before system update",
      "status": "available",
      "snapshot_type": "manual",
      "tags": ["backup", "pre-update"],
      "created_at": "2024-01-15T10:30:00Z",
      "size_bytes": 1073741824,
      "compressed_size_bytes": 322122547,
      "compression_ratio": 0.3,
      "is_encrypted": true,
      "checksum": "sha256:abc123...",
      "expires_at": null
    }
  ],
  "pagination": {
    "total": 42,
    "limit": 20,
    "offset": 0,
    "has_next": true,
    "has_prev": false
  }
}
```

### Get Snapshot Details

Retrieve detailed information about a specific snapshot.

```http
GET /snapshots/{snapshot_id}
Authorization: Bearer <token>
```

**Response:**
```json
{
  "snapshot_id": "snap_vm_12345_user_1234567890",
  "vm_id": "vm_12345",
  "user_id": "user_12345",
  "name": "backup-before-update",
  "description": "Backup before system update",
  "status": "available",
  "snapshot_type": "manual",
  "tags": ["backup", "pre-update"],
  "created_at": "2024-01-15T10:30:00Z",
  "updated_at": "2024-01-15T10:32:15Z",
  "size_bytes": 1073741824,
  "compressed_size_bytes": 322122547,
  "compression_ratio": 0.3,
  "is_encrypted": true,
  "encryption_algorithm": "AES-256-GCM",
  "checksum": "sha256:abc123def456...",
  "storage_path": "s3://snapshots/encrypted/snap_vm_12345_user_1234567890",
  "expires_at": null,
  "metadata": {
    "vm_config": {
      "cpu_count": 2,
      "memory_mb": 2048,
      "disk_gb": 20
    },
    "creation_duration_seconds": 45.2,
    "optimization_strategy": "balanced"
  },
  "cryptographic_signature": {
    "algorithm": "HMAC-SHA256",
    "signature": "def789ghi012...",
    "created_at": "2024-01-15T10:32:15Z"
  }
}
```

### Restore Snapshot

Restore a VM from a snapshot.

```http
POST /snapshots/{snapshot_id}/restore
Authorization: Bearer <token>
Content-Type: application/json

{
  "target_vm_id": "vm_54321",
  "priority": "high",
  "validate_integrity": true
}
```

**Parameters:**
- `target_vm_id` (string, optional): Target VM ID (defaults to original VM)
- `priority` (string, optional): Operation priority (`low`, `normal`, `high`)
- `validate_integrity` (boolean, optional): Validate snapshot integrity before restore

**Response:**
```json
{
  "restore_id": "restore_abc123def456",
  "snapshot_id": "snap_vm_12345_user_1234567890",
  "target_vm_id": "vm_54321",
  "status": "restoring",
  "started_at": "2024-01-15T11:00:00Z",
  "estimated_completion": "2024-01-15T11:01:30Z",
  "trace_id": "trace_restore_123"
}
```

### Delete Snapshot

Delete a snapshot and its associated data.

```http
DELETE /snapshots/{snapshot_id}
Authorization: Bearer <token>
```

**Response:**
```json
{
  "snapshot_id": "snap_vm_12345_user_1234567890",
  "status": "deleting",
  "deleted_at": "2024-01-15T12:00:00Z",
  "trace_id": "trace_delete_456"
}
```

## Advanced Snapshot Operations

### Create Incremental Snapshot

Create an incremental snapshot with advanced deduplication and compression.

```http
POST /snapshots/incremental
Authorization: Bearer <token>
Content-Type: application/json

{
  "vm_id": "vm_12345",
  "name": "incremental-daily",
  "description": "Daily incremental backup",
  "force_full": false,
  "enable_deduplication": true,
  "compression_level": 6
}
```

**Response:**
```json
{
  "snapshot_id": "incr_vm_12345_user_1234567890",
  "snapshot_type": "incremental",
  "parent_snapshot_id": "snap_vm_12345_user_1234567789",
  "status": "creating",
  "estimated_size_reduction": 0.85,
  "trace_id": "trace_incremental_789"
}
```

### Get Snapshot Chain Information

Retrieve information about incremental snapshot chains.

```http
GET /snapshots/{snapshot_id}/chain
Authorization: Bearer <token>
```

**Response:**
```json
{
  "vm_id": "vm_12345",
  "chain_length": 5,
  "total_storage_bytes": 2147483648,
  "storage_efficiency": 0.82,
  "snapshots": [
    {
      "snapshot_id": "snap_vm_12345_full",
      "type": "full",
      "created_at": "2024-01-10T10:00:00Z",
      "size_bytes": 1073741824,
      "parent_id": null
    },
    {
      "snapshot_id": "incr_vm_12345_001",
      "type": "incremental",
      "created_at": "2024-01-11T10:00:00Z",
      "size_bytes": 104857600,
      "parent_id": "snap_vm_12345_full"
    }
  ]
}
```

### Optimize Storage

Trigger storage optimization for snapshot chains.

```http
POST /snapshots/optimize
Authorization: Bearer <token>
Content-Type: application/json

{
  "vm_id": "vm_12345",
  "optimization_type": "consolidate_chain",
  "max_chain_length": 10
}
```

**Response:**
```json
{
  "optimization_id": "opt_abc123def456",
  "vm_id": "vm_12345",
  "status": "optimizing",
  "estimated_storage_savings": 0.25,
  "started_at": "2024-01-15T13:00:00Z"
}
```

## Performance and Monitoring

### Get Performance Report

Retrieve comprehensive performance metrics and optimization data.

```http
GET /performance/report?time_range=24h&include_details=true
Authorization: Bearer <token>
```

**Query Parameters:**
- `time_range` (string, optional): Time range for metrics (`1h`, `24h`, `7d`, `30d`)
- `include_details` (boolean, optional): Include detailed operation metrics
- `operation_type` (string, optional): Filter by operation type

**Response:**
```json
{
  "timestamp": "2024-01-15T14:00:00Z",
  "time_range": "24h",
  "performance_summary": {
    "total_operations": 156,
    "successful_operations": 152,
    "failed_operations": 4,
    "success_rate": 0.974,
    "average_duration_seconds": 42.5,
    "average_throughput_mbps": 24.3
  },
  "operations_by_type": {
    "snapshot_creation": {
      "total_operations": 89,
      "successful_operations": 87,
      "success_rate": 0.978,
      "avg_duration_seconds": 45.2,
      "avg_throughput_mbps": 22.1
    },
    "snapshot_restore": {
      "total_operations": 34,
      "successful_operations": 33,
      "success_rate": 0.971,
      "avg_duration_seconds": 38.7,
      "avg_throughput_mbps": 28.5
    }
  },
  "resource_utilization": {
    "memory_usage_percent": 67.3,
    "memory_available_mb": 2048,
    "disk_usage_percent": 42.1,
    "disk_available_gb": 150,
    "active_operations": 2,
    "cache_entries": 1247
  },
  "optimization_insights": {
    "recommended_strategy": "balanced",
    "storage_efficiency": 0.78,
    "potential_savings_gb": 23.4
  }
}
```

### Get System Health

Check overall system health and status.

```http
GET /health
Authorization: Bearer <token>
```

**Response:**
```json
{
  "status": "healthy",
  "timestamp": "2024-01-15T14:00:00Z",
  "version": "1.0.0",
  "uptime_seconds": 86400,
  "components": {
    "snapshot_manager": {
      "status": "healthy",
      "active_operations": 2,
      "queue_length": 0
    },
    "resource_manager": {
      "status": "healthy",
      "memory_usage_percent": 67.3,
      "cpu_usage_percent": 23.1
    },
    "firecracker_service": {
      "status": "healthy",
      "active_vms": 12,
      "lima_vm_status": "running"
    },
    "storage_backend": {
      "status": "healthy",
      "connection_pool": "active",
      "replication_lag_ms": 45
    },
    "database": {
      "status": "healthy",
      "connection_pool": "active",
      "query_latency_ms": 12
    }
  },
  "metrics": {
    "total_snapshots": 1247,
    "total_storage_gb": 2345,
    "compression_efficiency": 0.73
  }
}
```

### Get Real-time Metrics

Retrieve real-time system metrics.

```http
GET /metrics
Authorization: Bearer <token>
```

**Response:**
```json
{
  "timestamp": "2024-01-15T14:00:00Z",
  "metrics": [
    {
      "name": "snapshot_operations_total",
      "value": 1567,
      "type": "counter",
      "labels": {
        "operation": "create",
        "status": "success"
      }
    },
    {
      "name": "system_memory_usage_percent",
      "value": 67.3,
      "type": "gauge",
      "labels": {
        "component": "system"
      }
    },
    {
      "name": "operation_duration_seconds",
      "value": 42.5,
      "type": "histogram",
      "labels": {
        "operation": "snapshot_create"
      }
    }
  ]
}
```

## Security and Validation

### Get Security Status

Retrieve comprehensive security validation status.

```http
GET /security/validation
Authorization: Bearer <token>
```

**Response:**
```json
{
  "overall_score": 98.8,
  "max_score": 100.0,
  "compliance_percentage": 98.8,
  "last_validation": "2024-01-15T13:45:00Z",
  "security_categories": {
    "authentication_authorization": {
      "score": 100.0,
      "max_score": 100.0,
      "status": "compliant",
      "checks_passed": 8,
      "checks_total": 8
    },
    "data_protection": {
      "score": 97.5,
      "max_score": 100.0,
      "status": "mostly_compliant",
      "checks_passed": 7,
      "checks_total": 8,
      "failed_checks": ["backup_encryption_verification"]
    },
    "input_validation": {
      "score": 100.0,
      "max_score": 100.0,
      "status": "compliant",
      "checks_passed": 6,
      "checks_total": 6
    }
  },
  "recommendations": [
    {
      "category": "data_protection",
      "issue": "backup_encryption_verification",
      "severity": "low",
      "description": "Implement automated verification of backup encryption",
      "remediation": "Add periodic verification of encrypted backup integrity"
    }
  ]
}
```

### Security Events

Retrieve security-related events and audit logs.

```http
GET /security/events?limit=50&severity=warning
Authorization: Bearer <token>
```

**Response:**
```json
{
  "events": [
    {
      "event_id": "sec_event_123",
      "timestamp": "2024-01-15T13:30:00Z",
      "event_type": "rate_limit_exceeded",
      "severity": "warning",
      "user_id": "user_789",
      "vm_id": "vm_12345",
      "details": {
        "attempts": 6,
        "limit": 5,
        "window": "1h"
      },
      "resolved": true,
      "resolution_time": "2024-01-15T13:35:00Z"
    }
  ]
}
```

## Administrative Operations

### Manual Cleanup

Trigger manual system cleanup operations.

```http
POST /admin/cleanup
Authorization: Bearer <admin-token>
Content-Type: application/json

{
  "cleanup_type": "all",
  "force": false,
  "dry_run": false
}
```

**Parameters:**
- `cleanup_type` (string): Type of cleanup (`temp_files`, `memory`, `storage`, `all`)
- `force` (boolean): Force cleanup even if operations are active
- `dry_run` (boolean): Preview cleanup without executing

**Response:**
```json
{
  "cleanup_id": "cleanup_abc123",
  "status": "running",
  "started_at": "2024-01-15T14:00:00Z",
  "operations": [
    {
      "type": "temp_files",
      "status": "completed",
      "files_cleaned": 45,
      "bytes_freed": 1073741824
    },
    {
      "type": "memory",
      "status": "completed",
      "memory_freed_mb": 256
    }
  ]
}
```

### System Configuration

Get or update system configuration.

```http
GET /admin/config
Authorization: Bearer <admin-token>
```

```http
PUT /admin/config
Authorization: Bearer <admin-token>
Content-Type: application/json

{
  "max_snapshots_per_user": 100,
  "max_storage_per_user_gb": 500,
  "default_compression_level": 6,
  "security_validation_enabled": true
}
```

## Error Handling

All API endpoints return standardized error responses:

### Error Response Format

```json
{
  "error": {
    "code": "VALIDATION_ERROR",
    "message": "Invalid snapshot name format",
    "details": {
      "field": "name",
      "constraint": "must be 3-50 characters alphanumeric"
    },
    "trace_id": "trace_error_123",
    "timestamp": "2024-01-15T14:00:00Z"
  }
}
```

### HTTP Status Codes

- `200 OK` - Success
- `201 Created` - Resource created successfully
- `202 Accepted` - Request accepted for processing
- `400 Bad Request` - Invalid request parameters
- `401 Unauthorized` - Authentication required
- `403 Forbidden` - Insufficient permissions
- `404 Not Found` - Resource not found
- `409 Conflict` - Resource conflict (e.g., duplicate name)
- `422 Unprocessable Entity` - Validation error
- `429 Too Many Requests` - Rate limit exceeded
- `500 Internal Server Error` - Server error
- `503 Service Unavailable` - Service temporarily unavailable

### Common Error Codes

- `AUTHENTICATION_REQUIRED` - Valid JWT token required
- `AUTHORIZATION_FAILED` - Insufficient permissions
- `VALIDATION_ERROR` - Request validation failed
- `RESOURCE_NOT_FOUND` - Requested resource doesn't exist
- `RATE_LIMIT_EXCEEDED` - Too many requests
- `QUOTA_EXCEEDED` - User quota limit reached
- `OPERATION_IN_PROGRESS` - Conflicting operation in progress
- `SYSTEM_OVERLOADED` - System resource limits exceeded
- `STORAGE_ERROR` - Storage backend error
- `VM_NOT_ACCESSIBLE` - VM not found or accessible
- `ENCRYPTION_ERROR` - Encryption/decryption failed
- `INTEGRITY_VIOLATION` - Data integrity check failed

## Rate Limiting

API endpoints are rate-limited to ensure fair usage:

- **Standard Operations**: 100 requests per minute per user
- **Snapshot Creation**: 5 requests per hour per user
- **Admin Operations**: 50 requests per minute per admin user

Rate limit headers are included in responses:

```http
X-RateLimit-Limit: 100
X-RateLimit-Remaining: 95
X-RateLimit-Reset: 1642261200
```

## Pagination

List endpoints support pagination with the following parameters:

- `limit`: Number of results per page (default: 20, max: 100)
- `offset`: Number of results to skip (default: 0)

Pagination information is included in responses:

```json
{
  "pagination": {
    "total": 256,
    "limit": 20,
    "offset": 40,
    "has_next": true,
    "has_prev": true,
    "next_url": "/api/v1/snapshots?limit=20&offset=60",
    "prev_url": "/api/v1/snapshots?limit=20&offset=20"
  }
}
```

## WebSocket API

Real-time updates are available via WebSocket connections:

### Connection

```javascript
const ws = new WebSocket('wss://api.snapshot-manager.com/ws');
ws.onopen = function() {
  // Send authentication
  ws.send(JSON.stringify({
    type: 'auth',
    token: 'your-jwt-token'
  }));
  
  // Subscribe to events
  ws.send(JSON.stringify({
    type: 'subscribe',
    events: ['snapshot.created', 'snapshot.status_changed']
  }));
};
```

### Event Types

- `snapshot.created` - New snapshot created
- `snapshot.status_changed` - Snapshot status updated
- `snapshot.completed` - Snapshot operation completed
- `system.alert` - System alert generated
- `performance.threshold` - Performance threshold exceeded

## SDK and Libraries

Official SDKs are available for popular programming languages:

- **Python**: `pip install snapshot-manager-sdk`
- **JavaScript/Node.js**: `npm install @snapshot-manager/sdk`
- **Go**: `go get github.com/snapshot-manager/go-sdk`
- **Java**: Maven/Gradle coordinates available

### Python SDK Example

```python
from snapshot_manager import SnapshotManager

client = SnapshotManager(
    base_url="https://api.snapshot-manager.com",
    token="your-jwt-token"
)

# Create snapshot
snapshot = client.snapshots.create(
    vm_id="vm_12345",
    name="backup-before-update",
    priority="high"
)

# Wait for completion
snapshot.wait_for_completion()

# Restore snapshot
restore = snapshot.restore(target_vm_id="vm_54321")
```

This API reference provides comprehensive documentation for all available endpoints and features of the VM Snapshot Manager system.