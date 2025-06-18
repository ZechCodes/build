# Build Platform API Documentation

The Build Platform API is built with FastAPI and provides RESTful endpoints for managing development environments, VMs, sessions, and user accounts.

## API Overview

### Base URL
- **Development**: http://localhost:8000
- **API Docs**: http://localhost:8000/docs
- **ReDoc**: http://localhost:8000/redoc

### Authentication
The API uses JWT (JSON Web Token) bearer authentication:

```bash
# Login to get tokens
curl -X POST http://localhost:8000/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email": "developer@build-platform.dev", "password": "dev123"}'

# Use access token in requests
curl -H "Authorization: Bearer <access_token>" \
  http://localhost:8000/api/v1/users/me
```

## API Endpoints

### Authentication (`/api/v1/auth`)

#### POST `/auth/login`
Authenticate user and receive access tokens.

**Request:**
```json
{
  "email": "user@example.com",
  "password": "password123"
}
```

**Response:**
```json
{
  "access_token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...",
  "refresh_token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...",
  "token_type": "bearer"
}
```

#### POST `/auth/register`
Register a new user account.

**Request:**
```json
{
  "email": "newuser@example.com",
  "password": "password123",
  "full_name": "New User"
}
```

#### POST `/auth/refresh`
Refresh access token using refresh token.

**Request:**
```json
{
  "refresh_token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
}
```

#### POST `/auth/logout`
Logout (client-side token removal).

### Users (`/api/v1/users`)

#### GET `/users/me`
Get current user information.

**Response:**
```json
{
  "id": 1,
  "email": "user@example.com",
  "full_name": "User Name",
  "is_active": true,
  "is_superuser": false,
  "created_at": "2024-01-01T00:00:00Z",
  "updated_at": "2024-01-01T00:00:00Z",
  "last_login": "2024-01-01T12:00:00Z"
}
```

#### PUT `/users/me`
Update current user information.

**Request:**
```json
{
  "full_name": "Updated Name",
  "email": "newemail@example.com"
}
```

#### GET `/users/` (Admin only)
List all users with pagination.

**Query Parameters:**
- `skip`: Number of users to skip (default: 0)
- `limit`: Maximum users to return (default: 100)

#### GET `/users/{user_id}` (Admin only)
Get user by ID.

#### PUT `/users/{user_id}` (Admin only)
Update user by ID.

### Health Check (`/api/v1/health`)

#### GET `/health/`
Basic health check.

**Response:**
```json
{
  "status": "healthy",
  "service": "api"
}
```

#### GET `/health/detailed`
Detailed health check with dependency status.

**Response:**
```json
{
  "status": "healthy",
  "service": "api",
  "components": {
    "database": "healthy"
  }
}
```

## Data Models

### User
```json
{
  "id": 1,
  "email": "user@example.com",
  "full_name": "User Name",
  "is_active": true,
  "is_superuser": false,
  "created_at": "2024-01-01T00:00:00Z",
  "updated_at": "2024-01-01T00:00:00Z",
  "last_login": "2024-01-01T12:00:00Z"
}
```

### VM (Future)
```json
{
  "id": 1,
  "name": "Development VM",
  "status": "running",
  "owner_id": 1,
  "cpu_count": 2,
  "memory_mb": 1024,
  "disk_gb": 10,
  "created_at": "2024-01-01T00:00:00Z"
}
```

### Session (Future)
```json
{
  "id": 1,
  "session_id": "sess_abc123",
  "status": "active",
  "user_id": 1,
  "vm_id": 1,
  "terminal_cols": 80,
  "terminal_rows": 24,
  "created_at": "2024-01-01T00:00:00Z"
}
```

## Error Handling

The API returns standard HTTP status codes with JSON error responses:

### 400 Bad Request
```json
{
  "detail": "Invalid input data"
}
```

### 401 Unauthorized
```json
{
  "detail": "Could not validate credentials"
}
```

### 403 Forbidden
```json
{
  "detail": "The user doesn't have enough privileges"
}
```

### 404 Not Found
```json
{
  "detail": "User not found"
}
```

### 422 Unprocessable Entity
```json
{
  "detail": [
    {
      "loc": ["body", "email"],
      "msg": "field required",
      "type": "value_error.missing"
    }
  ]
}
```

### 500 Internal Server Error
```json
{
  "detail": "Internal server error"
}
```

## Rate Limiting

The API implements rate limiting to prevent abuse:

- **Per minute**: 60 requests per IP
- **Per hour**: 1000 requests per IP

Rate limit headers are included in responses:
```
X-RateLimit-Limit: 60
X-RateLimit-Remaining: 59
X-RateLimit-Reset: 1609459200
```

## Security

### Authentication Flow
1. User provides email/password to `/auth/login`
2. API validates credentials and returns JWT tokens
3. Client stores tokens securely
4. Client includes access token in `Authorization` header
5. API validates token on each request
6. Client refreshes token when expired using refresh token

### Security Headers
The API includes security headers:
- `X-Content-Type-Options: nosniff`
- `X-Frame-Options: DENY`
- `X-XSS-Protection: 1; mode=block`
- `Strict-Transport-Security: max-age=31536000`

### CORS
Cross-Origin Resource Sharing is configured for development:
- Allowed origins: `localhost:3000`, `127.0.0.1:3000`
- Allowed methods: All
- Allowed headers: All
- Credentials: Allowed

## Development

### Running Locally
```bash
cd api
source venv/bin/activate
uvicorn app.main:app --reload --host 0.0.0.0 --port 8000
```

### Testing APIs
Use the interactive documentation at http://localhost:8000/docs or:

```bash
# Get access token
TOKEN=$(curl -s -X POST http://localhost:8000/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email": "developer@build-platform.dev", "password": "dev123"}' \
  | jq -r '.access_token')

# Use token
curl -H "Authorization: Bearer $TOKEN" \
  http://localhost:8000/api/v1/users/me
```

### Adding New Endpoints

1. **Create schema** in `app/schemas/`
2. **Add model** in `app/models/` (if needed)
3. **Create service** in `app/services/`
4. **Add endpoint** in `app/api/v1/endpoints/`
5. **Register router** in `app/api/v1/api.py`
6. **Add tests** in `tests/`

### Database Migrations
```bash
# Create migration
alembic revision --autogenerate -m "Add new table"

# Apply migration
alembic upgrade head
```

## Future Endpoints (Planned)

### VMs (`/api/v1/vms`)
- `GET /vms/` - List user VMs
- `POST /vms/` - Create new VM
- `GET /vms/{vm_id}` - Get VM details
- `PUT /vms/{vm_id}` - Update VM
- `DELETE /vms/{vm_id}` - Delete VM
- `POST /vms/{vm_id}/start` - Start VM
- `POST /vms/{vm_id}/stop` - Stop VM
- `POST /vms/{vm_id}/suspend` - Suspend VM

### Sessions (`/api/v1/sessions`)
- `GET /sessions/` - List user sessions
- `POST /sessions/` - Create new session
- `GET /sessions/{session_id}` - Get session details
- `DELETE /sessions/{session_id}` - Terminate session

### Snapshots (`/api/v1/snapshots`)
- `GET /snapshots/` - List VM snapshots
- `POST /snapshots/` - Create snapshot
- `DELETE /snapshots/{snapshot_id}` - Delete snapshot
- `POST /snapshots/{snapshot_id}/restore` - Restore snapshot

### Git Repositories (`/api/v1/git`)
- `GET /git/repos/` - List repositories
- `POST /git/repos/` - Create repository
- `GET /git/repos/{repo_id}` - Get repository details

## Performance

### Caching
- Redis caching for frequently accessed data
- Database query optimization
- Connection pooling

### Monitoring
- Prometheus metrics endpoint: `/metrics`
- Health checks for dependencies
- Structured logging with correlation IDs

## Support

- **Documentation**: http://localhost:8000/docs
- **Issues**: Create issue in repository
- **Logs**: Check application logs for debugging