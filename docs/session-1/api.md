# API Reference

This document provides comprehensive API documentation for Session 1 endpoints and the foundation for future authentication and management APIs.

## API Overview

The Build Platform API is built with FastAPI and provides:

- **RESTful API Design** - Standard HTTP methods and status codes
- **OpenAPI Documentation** - Automatic interactive documentation
- **JSON-First** - All endpoints accept and return JSON
- **Comprehensive Error Handling** - Structured error responses
- **Security Headers** - Enhanced security middleware
- **Request/Response Logging** - Complete audit trail

## Base Configuration

### API Base URL

```
Development: http://localhost:8000
Production: https://api.getbuild.ing
```

### Content Type

All API requests and responses use JSON:

```
Content-Type: application/json
Accept: application/json
```

### API Versioning

Currently using URL versioning with `/v1/` prefix for future compatibility.

## Authentication (Foundation)

Session 1 establishes the authentication foundation. Full authentication will be implemented in Session 2.

### Authentication Headers

```http
Authorization: Bearer <jwt_token>
X-Request-ID: <unique_request_id>
```

### Authentication Flow (Planned)

```
1. POST /auth/login → JWT tokens
2. Include Bearer token in subsequent requests
3. Refresh tokens via POST /auth/refresh
4. Logout via POST /auth/logout
```

## Current Endpoints

### Health Check

Check API service health and status.

**Endpoint:** `GET /health`

**Response:**
```json
{
  "status": "healthy",
  "service": "build-api",
  "version": "1.0.0",
  "timestamp": "2024-01-01T00:00:00Z",
  "uptime": 3600,
  "database": {
    "status": "connected",
    "query_time_ms": 2.5
  },
  "redis": {
    "status": "connected",
    "memory_usage_mb": 45.2
  }
}
```

**Status Codes:**
- `200 OK` - Service healthy
- `503 Service Unavailable` - Service unhealthy

**Example Request:**
```bash
curl -X GET http://localhost:8000/health \
  -H "Accept: application/json"
```

### Detailed Health Check

Get comprehensive system health information.

**Endpoint:** `GET /health/detailed`

**Response:**
```json
{
  "status": "healthy",
  "service": "build-api",
  "version": "1.0.0",
  "timestamp": "2024-01-01T00:00:00Z",
  "uptime": 3600,
  "components": {
    "database": {
      "status": "healthy",
      "connectivity": "ok",
      "query_time_ms": 2.5,
      "pool_size": 20,
      "checked_out_connections": 3,
      "checked_in_connections": 17
    },
    "redis": {
      "status": "healthy",
      "connectivity": "ok",
      "memory_usage_mb": 45.2,
      "memory_usage_percent": 2.3,
      "keyspace_hits": 1250,
      "keyspace_misses": 30
    },
    "middleware": {
      "security_headers": "active",
      "rate_limiting": "active",
      "cors": "configured",
      "monitoring": "active"
    }
  },
  "metrics": {
    "requests_total": 1520,
    "requests_per_minute": 25.3,
    "average_response_time_ms": 45.2,
    "error_rate_percent": 0.1
  }
}
```

### OpenAPI Documentation

Access interactive API documentation.

**Endpoints:**
- `GET /docs` - Swagger UI
- `GET /redoc` - ReDoc UI  
- `GET /openapi.json` - OpenAPI specification

**Features:**
- Interactive API testing
- Complete endpoint documentation
- Request/response examples
- Authentication testing

## Planned Authentication Endpoints (Session 2)

### User Registration

Register a new user account.

**Endpoint:** `POST /auth/register`

**Request Body:**
```json
{
  "email": "user@example.com",
  "username": "username",
  "password": "SecurePassword123!",
  "full_name": "User Name"
}
```

**Response:**
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "email": "user@example.com",
  "username": "username",
  "full_name": "User Name",
  "is_verified": false,
  "created_at": "2024-01-01T00:00:00Z"
}
```

### User Login

Authenticate user and receive JWT tokens.

**Endpoint:** `POST /auth/login`

**Request Body:**
```json
{
  "email": "user@example.com",
  "password": "SecurePassword123!"
}
```

**Response:**
```json
{
  "access_token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...",
  "refresh_token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...",
  "token_type": "bearer",
  "expires_in": 900,
  "user": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "email": "user@example.com",
    "username": "username",
    "full_name": "User Name"
  }
}
```

### Token Refresh

Refresh access token using refresh token.

**Endpoint:** `POST /auth/refresh`

**Request Body:**
```json
{
  "refresh_token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
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

### User Logout

Invalidate user session and tokens.

**Endpoint:** `POST /auth/logout`

**Headers:**
```
Authorization: Bearer <access_token>
```

**Response:**
```json
{
  "message": "Successfully logged out"
}
```

## Planned User Management Endpoints (Session 2)

### Get Current User

Get current authenticated user information.

**Endpoint:** `GET /users/me`

**Headers:**
```
Authorization: Bearer <access_token>
```

**Response:**
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "email": "user@example.com",
  "username": "username",
  "full_name": "User Name",
  "is_active": true,
  "is_verified": true,
  "created_at": "2024-01-01T00:00:00Z",
  "last_login": "2024-01-01T12:00:00Z"
}
```

### Update User Profile

Update current user profile information.

**Endpoint:** `PUT /users/me`

**Headers:**
```
Authorization: Bearer <access_token>
```

**Request Body:**
```json
{
  "full_name": "Updated Name",
  "email": "newemail@example.com"
}
```

**Response:**
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "email": "newemail@example.com",
  "username": "username",
  "full_name": "Updated Name",
  "is_active": true,
  "is_verified": true,
  "updated_at": "2024-01-01T12:30:00Z"
}
```

## Error Handling

### Error Response Format

All API errors follow a consistent format:

```json
{
  "error": {
    "code": "VALIDATION_ERROR",
    "message": "Request validation failed",
    "details": {
      "field": "email",
      "issue": "Invalid email format"
    },
    "request_id": "550e8400-e29b-41d4-a716-446655440000",
    "timestamp": "2024-01-01T00:00:00Z"
  }
}
```

### HTTP Status Codes

| Code | Description | Usage |
|------|-------------|-------|
| 200 | OK | Successful GET, PUT requests |
| 201 | Created | Successful POST requests |
| 204 | No Content | Successful DELETE requests |
| 400 | Bad Request | Invalid request format/data |
| 401 | Unauthorized | Authentication required |
| 403 | Forbidden | Insufficient permissions |
| 404 | Not Found | Resource not found |
| 409 | Conflict | Resource already exists |
| 422 | Unprocessable Entity | Validation errors |
| 429 | Too Many Requests | Rate limit exceeded |
| 500 | Internal Server Error | Server error |
| 503 | Service Unavailable | Service temporarily unavailable |

### Common Error Codes

| Error Code | Description |
|------------|-------------|
| `VALIDATION_ERROR` | Request validation failed |
| `AUTHENTICATION_REQUIRED` | Authentication token required |
| `INVALID_TOKEN` | JWT token is invalid or expired |
| `INSUFFICIENT_PERMISSIONS` | User lacks required permissions |
| `RESOURCE_NOT_FOUND` | Requested resource does not exist |
| `RESOURCE_CONFLICT` | Resource already exists |
| `RATE_LIMIT_EXCEEDED` | Too many requests |
| `INTERNAL_ERROR` | Internal server error |
| `SERVICE_UNAVAILABLE` | Service temporarily unavailable |

### Error Examples

**Validation Error (422):**
```json
{
  "error": {
    "code": "VALIDATION_ERROR",
    "message": "Password does not meet security requirements",
    "details": {
      "field": "password",
      "requirements": [
        "Minimum 12 characters",
        "At least one uppercase letter",
        "At least one lowercase letter",
        "At least one number",
        "At least one special character"
      ]
    },
    "request_id": "550e8400-e29b-41d4-a716-446655440000",
    "timestamp": "2024-01-01T00:00:00Z"
  }
}
```

**Authentication Error (401):**
```json
{
  "error": {
    "code": "INVALID_TOKEN",
    "message": "JWT token has expired",
    "details": {
      "expired_at": "2024-01-01T00:15:00Z"
    },
    "request_id": "550e8400-e29b-41d4-a716-446655440000",
    "timestamp": "2024-01-01T00:16:00Z"
  }
}
```

**Rate Limit Error (429):**
```json
{
  "error": {
    "code": "RATE_LIMIT_EXCEEDED",
    "message": "Too many requests",
    "details": {
      "limit": 100,
      "window": "60 seconds",
      "retry_after": 45
    },
    "request_id": "550e8400-e29b-41d4-a716-446655440000",
    "timestamp": "2024-01-01T00:00:00Z"
  }
}
```

## Request/Response Patterns

### Request Headers

**Required Headers:**
```
Content-Type: application/json
Accept: application/json
```

**Optional Headers:**
```
Authorization: Bearer <token>
X-Request-ID: <unique_id>
User-Agent: <client_info>
```

### Response Headers

**Standard Headers:**
```
Content-Type: application/json
X-Request-ID: <request_id>
X-Response-Time: <time_ms>
X-Rate-Limit-Remaining: <count>
X-Rate-Limit-Reset: <timestamp>
```

**Security Headers:**
```
Strict-Transport-Security: max-age=31536000; includeSubDomains
X-Content-Type-Options: nosniff
X-Frame-Options: DENY
X-XSS-Protection: 1; mode=block
Content-Security-Policy: default-src 'self'
```

### Pagination

For endpoints returning multiple items:

**Request Parameters:**
```
?page=1&limit=20&sort=created_at&order=desc
```

**Response Format:**
```json
{
  "data": [...],
  "pagination": {
    "page": 1,
    "limit": 20,
    "total": 100,
    "pages": 5,
    "has_next": true,
    "has_prev": false
  }
}
```

### Filtering and Searching

**Query Parameters:**
```
?filter[status]=active&search=username&date_from=2024-01-01&date_to=2024-01-31
```

**Response includes filter info:**
```json
{
  "data": [...],
  "filters": {
    "status": "active",
    "search": "username",
    "date_from": "2024-01-01",
    "date_to": "2024-01-31"
  },
  "pagination": {...}
}
```

## Rate Limiting

### Rate Limit Rules

| Endpoint Category | Requests | Window | Burst |
|------------------|----------|---------|-------|
| Authentication   | 10       | 60s     | 5     |
| API Operations   | 100      | 60s     | 20    |
| Health Checks    | 1000     | 60s     | 100   |

### Rate Limit Headers

```
X-RateLimit-Limit: 100
X-RateLimit-Remaining: 95
X-RateLimit-Reset: 1640995200
X-RateLimit-Window: 60
```

### Rate Limit Exceeded Response

```json
{
  "error": {
    "code": "RATE_LIMIT_EXCEEDED",
    "message": "Too many requests",
    "details": {
      "limit": 100,
      "window": "60 seconds",
      "retry_after": 45
    }
  }
}
```

## Security Considerations

### Input Validation

- All inputs validated using Pydantic models
- SQL injection prevention via SQLAlchemy ORM
- XSS protection with input sanitization
- File upload validation and restrictions

### Authentication Security

- JWT tokens with short expiration times
- Refresh token rotation
- Account lockout after failed attempts
- Session invalidation on logout

### Data Protection

- Sensitive data excluded from logs
- Password hashing with bcrypt
- PII encryption for storage
- Audit logging for all operations

## Development and Testing

### Development Environment

```bash
# Start development server
cd api
uvicorn main:app --reload --host 0.0.0.0 --port 8000

# API available at:
# http://localhost:8000/docs (Swagger UI)
# http://localhost:8000/redoc (ReDoc)
```

### Testing Endpoints

```bash
# Health check
curl -X GET http://localhost:8000/health

# Detailed health check
curl -X GET http://localhost:8000/health/detailed

# OpenAPI specification
curl -X GET http://localhost:8000/openapi.json
```

### Test Data

Development environment includes:
- Test user accounts
- Sample data for testing
- Mock external services
- Seed data scripts

## Client Integration

### JavaScript/TypeScript Example

```typescript
interface ApiClient {
  baseURL: string;
  authToken?: string;
}

class BuildApiClient implements ApiClient {
  baseURL = 'http://localhost:8000';
  authToken?: string;

  async healthCheck(): Promise<HealthResponse> {
    const response = await fetch(`${this.baseURL}/health`);
    return response.json();
  }

  async login(email: string, password: string): Promise<LoginResponse> {
    const response = await fetch(`${this.baseURL}/auth/login`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json'
      },
      body: JSON.stringify({ email, password })
    });
    
    if (!response.ok) {
      throw new Error('Login failed');
    }
    
    const data = await response.json();
    this.authToken = data.access_token;
    return data;
  }

  private async authenticatedRequest(
    endpoint: string, 
    options: RequestInit = {}
  ): Promise<Response> {
    return fetch(`${this.baseURL}${endpoint}`, {
      ...options,
      headers: {
        'Content-Type': 'application/json',
        'Authorization': `Bearer ${this.authToken}`,
        ...options.headers
      }
    });
  }
}
```

### Python Client Example

```python
import httpx
from typing import Optional, Dict, Any

class BuildApiClient:
    def __init__(self, base_url: str = "http://localhost:8000"):
        self.base_url = base_url
        self.auth_token: Optional[str] = None
        self.client = httpx.AsyncClient()

    async def health_check(self) -> Dict[str, Any]:
        """Check API health."""
        response = await self.client.get(f"{self.base_url}/health")
        response.raise_for_status()
        return response.json()

    async def login(self, email: str, password: str) -> Dict[str, Any]:
        """Authenticate user."""
        response = await self.client.post(
            f"{self.base_url}/auth/login",
            json={"email": email, "password": password}
        )
        response.raise_for_status()
        
        data = response.json()
        self.auth_token = data["access_token"]
        return data

    async def authenticated_request(
        self, 
        method: str, 
        endpoint: str, 
        **kwargs
    ) -> httpx.Response:
        """Make authenticated request."""
        headers = kwargs.get("headers", {})
        if self.auth_token:
            headers["Authorization"] = f"Bearer {self.auth_token}"
        
        kwargs["headers"] = headers
        
        response = await self.client.request(
            method, 
            f"{self.base_url}{endpoint}", 
            **kwargs
        )
        return response
```

## Monitoring and Observability

### Request Tracking

Every API request is tracked with:
- Unique request ID
- User identification
- Request timing
- Response status
- Error details

### Metrics Collection

- Request count by endpoint
- Response time percentiles
- Error rate by endpoint
- Authentication success/failure rates
- Rate limit violations

### Structured Logging

```json
{
  "timestamp": "2024-01-01T00:00:00Z",
  "level": "INFO",
  "message": "API request completed",
  "request_id": "550e8400-e29b-41d4-a716-446655440000",
  "method": "POST",
  "path": "/auth/login",
  "status_code": 200,
  "response_time_ms": 45.2,
  "user_id": "550e8400-e29b-41d4-a716-446655440000",
  "ip_address": "192.168.1.100",
  "user_agent": "Mozilla/5.0..."
}
```

## Future API Extensions

Session 1 provides the foundation for future API expansions:

- **Session 2**: Complete authentication and user management
- **Session 3**: VM instance management endpoints
- **Session 4**: Terminal session management
- **Session 5**: WebSocket API for real-time communication
- **Session 6**: Session persistence and recovery
- **Session 7**: Snapshot management API

Each session will add new endpoints while maintaining backward compatibility and consistent API design patterns.