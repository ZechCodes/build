# WebSocket API Documentation

## Overview

The Build Platform WebSocket API provides real-time terminal session management with enterprise-grade security, monitoring, and reliability features. This document covers all WebSocket endpoints, message types, security protocols, and integration guidelines.

## Table of Contents

1. [Connection Establishment](#connection-establishment)
2. [Authentication Flow](#authentication-flow)
3. [Message Protocol](#message-protocol)
4. [Terminal Session Management](#terminal-session-management)
5. [Security Features](#security-features)
6. [Error Handling](#error-handling)
7. [Rate Limiting](#rate-limiting)
8. [Monitoring and Metrics](#monitoring-and-metrics)
9. [Client Libraries](#client-libraries)
10. [Troubleshooting](#troubleshooting)

## Connection Establishment

### WebSocket Endpoint

```
wss://api.build.8ly.com/ws/terminal
```

### Connection Headers

```http
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Version: 13
Sec-WebSocket-Key: <base64-encoded-key>
Origin: https://app.8ly.com
User-Agent: <client-user-agent>
Authorization: Bearer <jwt-token>
```

### Protocol Negotiation

Upon connection, clients must negotiate the protocol version:

```json
{
  "type": "protocol_negotiate",
  "data": {
    "supported_versions": ["1.2.0", "1.1.0", "1.0.0"],
    "capabilities": {
      "compression": true,
      "acknowledgments": true,
      "binary_data": true,
      "certificate_pinning": true
    },
    "client_id": "webapp-v2.1.0"
  }
}
```

### Server Response

```json
{
  "type": "protocol_accepted",
  "data": {
    "negotiated_version": "1.2.0",
    "server_capabilities": {
      "compression": true,
      "acknowledgments": true,
      "binary_data": true,
      "enhanced_security": true,
      "adaptive_compression": true
    },
    "protocol_features": {
      "compression_algorithms": ["gzip", "deflate", "brotli"],
      "messagepack_support": true,
      "enhanced_security": true
    }
  }
}
```

## Authentication Flow

### 1. Initial Authentication

After protocol negotiation, authenticate the connection:

```json
{
  "type": "auth_request",
  "data": {
    "token": "<jwt-access-token>",
    "refresh_token": "<jwt-refresh-token>",
    "client_info": {
      "platform": "web",
      "version": "2.1.0",
      "features": ["terminal", "file_transfer"]
    }
  }
}
```

### 2. Authentication Success

```json
{
  "type": "auth_response",
  "data": {
    "status": "authenticated",
    "user_id": "user-123",
    "permissions": ["terminal:read", "terminal:write", "session:create"],
    "session_limits": {
      "max_sessions": 5,
      "max_connections_per_session": 3
    }
  }
}
```

### 3. Authentication Failure

```json
{
  "type": "error",
  "data": {
    "code": "AUTH_FAILED",
    "message": "Invalid or expired token",
    "details": {
      "reason": "token_expired",
      "expires_at": "2025-06-27T20:00:00Z"
    }
  }
}
```

## Message Protocol

### Message Structure

All messages follow this structure:

```json
{
  "type": "<message_type>",
  "timestamp": 1751068507.281035,
  "protocol_version": "1.2.0",
  "sequence": 12345,
  "data": { /* message-specific data */ },
  "session_id": "session-uuid",
  "message_id": "msg-uuid",
  "requires_ack": true,
  "binary": false,
  "integrity": "hmac-sha256-hash"
}
```

### Message Types

#### Terminal Data
```json
{
  "type": "terminal_data",
  "data": "ls -la\n",
  "session_id": "session-123",
  "binary": false
}
```

#### Terminal Resize
```json
{
  "type": "terminal_resize", 
  "data": {
    "rows": 24,
    "cols": 80
  },
  "session_id": "session-123"
}
```

#### Session Management
```json
{
  "type": "session_create",
  "data": {
    "vm_id": "vm-456",
    "environment": {
      "shell": "/bin/bash",
      "working_directory": "/home/user"
    }
  }
}
```

#### Heartbeat
```json
{
  "type": "heartbeat",
  "timestamp": 1751068507.281035
}
```

#### Acknowledgment
```json
{
  "type": "ack",
  "message_id": "msg-uuid",
  "success": true,
  "data": {
    "processing_time_ms": 15.2
  }
}
```

## Terminal Session Management

### Creating a Session

1. **Request Session Creation**
```json
{
  "type": "session_create",
  "data": {
    "vm_id": "vm-123",
    "environment": {
      "shell": "/bin/bash",
      "working_directory": "/workspace",
      "environment_variables": {
        "TERM": "xterm-256color",
        "PATH": "/usr/local/bin:/usr/bin:/bin"
      }
    }
  }
}
```

2. **Session Created Response**
```json
{
  "type": "session_created",
  "data": {
    "session_id": "session-abc-123",
    "vm_id": "vm-123",
    "initial_size": {
      "rows": 24,
      "cols": 80
    },
    "capabilities": ["resize", "file_transfer", "environment_variables"]
  }
}
```

### Joining an Existing Session

```json
{
  "type": "session_join",
  "session_id": "session-abc-123"
}
```

### Restoring a Session

```json
{
  "type": "session_restore",
  "session_id": "session-abc-123"
}
```

Response with history:
```json
{
  "type": "session_recovered",
  "data": {
    "session_id": "session-abc-123",
    "recovery_data": {
      "history": [
        "user@vm:~$ ls -la",
        "total 12",
        "drwxr-xr-x 3 user user 4096 Jun 27 20:00 .",
        "drwxr-xr-x 5 root root 4096 Jun 27 19:00 .."
      ],
      "recovery_timestamp": 1751068507.281035
    }
  }
}
```

## Security Features

### Message Encryption

Sensitive messages are automatically encrypted:

```json
{
  "type": "encrypted_message",
  "data": {
    "encrypted_data": "base64-encoded-encrypted-payload",
    "algorithm": "AES-256-GCM",
    "nonce": "base64-encoded-nonce",
    "auth_tag": "base64-encoded-auth-tag"
  },
  "encryption_metadata": {
    "key_id": "key-123",
    "timestamp": 1751068507.281035
  }
}
```

### Pattern Detection

The system automatically detects malicious patterns:

- **Command Injection**: Shell metacharacters, command chaining
- **XSS Attacks**: Script tags, event handlers
- **Path Traversal**: Directory traversal attempts
- **Data Exfiltration**: Large base64 data, credential harvesting
- **Malware Signatures**: Known malware tool signatures

### Certificate Pinning

Clients should implement certificate pinning:

```javascript
const expectedFingerprints = [
  "sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
  "sha256:BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB="
];

// Validate certificate during connection
function validateCertificate(certificate) {
  const fingerprint = calculateSHA256Fingerprint(certificate);
  return expectedFingerprints.includes(fingerprint);
}
```

## Error Handling

### Error Response Format

```json
{
  "type": "error",
  "data": {
    "code": "ERROR_CODE",
    "message": "Human-readable error message",
    "details": {
      "field": "specific_field",
      "reason": "validation_failed",
      "suggestions": ["Try this", "Or this"]
    },
    "retry_after": 30,
    "severity": "high"
  }
}
```

### Common Error Codes

| Code | Description | Action |
|------|-------------|--------|
| `AUTH_FAILED` | Authentication failure | Re-authenticate |
| `RATE_LIMITED` | Rate limit exceeded | Wait and retry |
| `SESSION_NOT_FOUND` | Session doesn't exist | Create new session |
| `INVALID_MESSAGE` | Malformed message | Fix message format |
| `SECURITY_VIOLATION` | Security threat detected | Review content |
| `PROTOCOL_ERROR` | Protocol violation | Check protocol version |
| `CRITICAL_THREAT_DETECTED` | Critical security threat | Connection terminated |

## Rate Limiting

### Limits by Connection Type

| Connection Type | Messages/sec | Bytes/sec | Burst |
|----------------|--------------|-----------|-------|
| Authenticated User | 100 | 1MB | 50 |
| Anonymous | 10 | 100KB | 5 |
| Premium User | 200 | 5MB | 100 |

### Rate Limit Headers

```json
{
  "type": "rate_limit_info",
  "data": {
    "limit": 100,
    "remaining": 87,
    "reset_time": 1751068567,
    "retry_after": 0
  }
}
```

### Rate Limit Exceeded

```json
{
  "type": "error",
  "data": {
    "code": "RATE_LIMITED",
    "message": "Rate limit exceeded",
    "details": {
      "limit": 100,
      "window": 60,
      "retry_after": 30
    }
  }
}
```

## Monitoring and Metrics

### Connection Metrics

Real-time metrics are tracked for each connection:

- **Performance**: Response times, throughput
- **Resources**: Memory usage, CPU usage, bandwidth
- **Security**: Threat detection, violations
- **Reliability**: Message delivery, acknowledgments

### Health Check Endpoint

```json
GET /ws/health
{
  "status": "healthy",
  "metrics": {
    "active_connections": 1250,
    "total_sessions": 890,
    "avg_response_time_ms": 12.5,
    "threat_detection_rate": 0.02
  }
}
```

## Client Libraries

### JavaScript/TypeScript

```typescript
import { BuildWebSocketClient } from '@8ly/websocket-client';

const client = new BuildWebSocketClient({
  url: 'wss://api.build.8ly.com/ws/terminal',
  token: 'your-jwt-token',
  options: {
    enableCompression: true,
    enableAcknowledgments: true,
    reconnectAttempts: 5,
    heartbeatInterval: 30000
  }
});

// Connect and authenticate
await client.connect();

// Create a session
const session = await client.createSession({
  vmId: 'vm-123',
  environment: { shell: '/bin/bash' }
});

// Send terminal data
await session.sendInput('ls -la\n');

// Listen for output
session.on('output', (data) => {
  console.log('Terminal output:', data);
});
```

### Python

```python
from build_websocket import BuildWebSocketClient

client = BuildWebSocketClient(
    url='wss://api.build.8ly.com/ws/terminal',
    token='your-jwt-token',
    enable_compression=True,
    enable_acknowledgments=True
)

async with client:
    # Create session
    session_id = await client.create_session(vm_id='vm-123')
    
    # Send commands
    await client.send_terminal_data(session_id, 'ls -la\n')
    
    # Listen for output
    async for output in client.listen_output(session_id):
        print(f"Output: {output}")
```

### Go

```go
package main

import (
    "github.com/8ly/websocket-client-go"
)

func main() {
    client := websocket.NewClient(&websocket.Config{
        URL:   "wss://api.build.8ly.com/ws/terminal",
        Token: "your-jwt-token",
        Options: websocket.Options{
            EnableCompression:     true,
            EnableAcknowledgments: true,
            ReconnectAttempts:     5,
        },
    })

    // Connect
    err := client.Connect()
    if err != nil {
        log.Fatal(err)
    }
    defer client.Close()

    // Create session
    sessionID, err := client.CreateSession("vm-123")
    if err != nil {
        log.Fatal(err)
    }

    // Send terminal data
    err = client.SendTerminalData(sessionID, "ls -la\n")
    if err != nil {
        log.Fatal(err)
    }
}
```

## Troubleshooting

### Connection Issues

**Problem**: Connection fails immediately
```
Solution: Check authentication token, origin header, and network connectivity
```

**Problem**: Protocol negotiation fails
```
Solution: Ensure client supports required protocol version (min 1.0.0)
```

### Performance Issues

**Problem**: High latency
```
Solution: Enable compression, check network conditions, verify server health
```

**Problem**: Messages not acknowledged
```
Solution: Check acknowledgment timeout settings, verify message format
```

### Security Issues

**Problem**: Security violations detected
```
Solution: Review message content, check for malicious patterns, sanitize input
```

**Problem**: Rate limiting triggered
```
Solution: Implement exponential backoff, reduce message frequency, check limits
```

### Debugging Tools

1. **Enable Debug Logging**
```javascript
const client = new BuildWebSocketClient({
  debug: true,
  logLevel: 'debug'
});
```

2. **Monitor Connection Health**
```javascript
client.on('health', (metrics) => {
  console.log('Connection health:', metrics);
});
```

3. **Track Message Flow**
```javascript
client.on('message_sent', (msg) => console.log('Sent:', msg));
client.on('message_received', (msg) => console.log('Received:', msg));
```

## Best Practices

### Connection Management
- Implement proper reconnection logic with exponential backoff
- Use heartbeat messages to detect connection issues
- Handle protocol upgrades gracefully
- Implement certificate pinning for production

### Message Handling
- Always validate message format before sending
- Implement acknowledgment handling for critical messages
- Use compression for large messages
- Sanitize all user input before sending to terminal

### Security
- Validate all incoming messages
- Implement proper authentication token refresh
- Monitor for unusual patterns or behavior
- Use secure origins in production
- Implement proper error handling without exposing sensitive information

### Performance
- Enable compression for large payloads
- Batch small messages when possible
- Implement client-side rate limiting
- Monitor connection metrics and adjust behavior accordingly

## API Versioning

The WebSocket API uses semantic versioning:

- **Major Version**: Breaking changes to protocol
- **Minor Version**: New features, backward compatible
- **Patch Version**: Bug fixes, no API changes

Current version: **1.2.0**

Supported versions:
- 1.2.0 (current)
- 1.1.0 (compression support)
- 1.0.0 (legacy, minimum supported)

## Support

For technical support and questions:

- **Documentation**: https://docs.8ly.com/websocket-api
- **GitHub Issues**: https://github.com/8ly/build-platform/issues
- **Community**: https://community.8ly.com
- **Enterprise Support**: support@8ly.com

---

*This documentation is for WebSocket API version 1.2.0. Last updated: June 27, 2025*