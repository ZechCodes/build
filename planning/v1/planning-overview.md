# Claude Code Platform - Architecture Overview & Dependencies

## Executive Summary

The Claude Code Platform is a distributed system providing isolated development environments through Firecracker VMs, accessible via web-based terminals. The architecture emphasizes security, scalability, and reliability through careful component isolation and well-defined interfaces.

## System Architecture

### High-Level Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                                   Internet                                   │
└─────────────────────┬───────────────────────────┬───────────────────────────┘
                      │                           │
                      ▼                           ▼
              ┌───────────────┐           ┌───────────────┐
              │   Cloudflare  │           │   Route 53   │
              │   (CDN/WAF)   │           │    (DNS)     │
              └───────┬───────┘           └───────────────┘
                      │
                      ▼
         ┌────────────────────────────┐
         │   Load Balancer (ALB)      │
         │   - SSL Termination        │
         │   - Health Checks          │
         └────────────┬───────────────┘
                      │
     ┌────────────────┴────────────────┬─────────────────┐
     ▼                                 ▼                 ▼
┌─────────────┐               ┌─────────────┐   ┌─────────────┐
│  Web Tier   │               │   API Tier  │   │  WS Tier    │
│  (React)    │               │  (FastAPI)  │   │ (WebSocket) │
│             │               │             │   │             │
│ - Terminal  │               │ - REST API  │   │ - Terminal  │
│ - Git UI    │               │ - Auth      │   │   Streams   │
│ - Admin     │               │ - VM Mgmt   │   │ - Live Data │
└─────────────┘               └──────┬──────┘   └──────┬──────┘
                                     │                  │
                    ┌────────────────┴──────────────────┴─────────┐
                    │                                             │
                    ▼                                             ▼
         ┌─────────────────────┐                      ┌─────────────────────┐
         │   Service Layer     │                      │   Message Queue     │
         │                     │                      │   (Redis Pub/Sub)   │
         ├─────────────────────┤                      └─────────────────────┘
         │ - VM Manager        │                                │
         │ - Session Manager   │                                │
         │ - Git Manager       │                                │
         │ - Snapshot Manager  │                                │
         │ - Recording Manager │                                │
         └──────────┬──────────┘                                │
                    │                                           │
     ┌──────────────┴──────────────┬────────────────┬──────────┴─────────┐
     ▼                             ▼                ▼                    ▼
┌─────────────┐           ┌─────────────┐  ┌─────────────┐    ┌─────────────┐
│ PostgreSQL  │           │    Redis    │  │    MinIO    │    │ Soft-serve  │
│             │           │             │  │     (S3)    │    │    (Git)    │
│ - Users     │           │ - Sessions  │  │             │    │             │
│ - VMs       │           │ - Cache     │  │ - Snapshots │    │ - Repos     │
│ - Snapshots │           │ - Buffers   │  │ - Recordings│    │ - SSH Keys  │
│ - Sessions  │           │ - Rate Limit│  │ - Exports   │    │             │
└─────────────┘           └─────────────┘  └─────────────┘    └─────────────┘
                                                    │
                          ┌─────────────────────────┴─────────────────────┐
                          │                                               │
                          ▼                                               ▼
              ┌───────────────────────┐                       ┌───────────────────────┐
              │   Firecracker Hosts   │                       │   Firecracker Hosts   │
              │   (Compute Fleet)     │                       │   (Compute Fleet)     │
              │                       │                       │                       │
              │ ┌─────┐ ┌─────┐      │                       │ ┌─────┐ ┌─────┐      │
              │ │ VM1 │ │ VM2 │ ...  │                       │ │ VM5 │ │ VM6 │ ...  │
              │ └─────┘ └─────┘      │                       │ └─────┘ └─────┘      │
              └───────────────────────┘                       └───────────────────────┘
```

## Component Dependencies

### External Dependencies

| Component | Version | Purpose | Critical? |
|-----------|---------|---------|-----------|
| **Python** | 3.11+ | Backend runtime | Yes |
| **Node.js** | 18+ | Frontend build | Yes |
| **PostgreSQL** | 16+ | Primary database | Yes |
| **Redis** | 7+ | Cache & sessions | Yes |
| **MinIO** | Latest | S3-compatible storage | Yes |
| **Firecracker** | 1.5+ | VM isolation | Yes |
| **Soft-serve** | Latest | Git hosting | Yes |
| **Socat** | 1.7+ | PTY bridging | Yes |
| **Nginx** | 1.24+ | Reverse proxy | No* |
| **Docker** | 24+ | Development only | No |

*Can use ALB instead

### Python Dependencies Tree

```
claude-code-platform
├── Core Framework
│   ├── fastapi (0.109.0) - Web framework
│   ├── uvicorn[standard] (0.27.0) - ASGI server
│   └── pydantic (2.5.0) - Data validation
│
├── Database & Storage
│   ├── sqlalchemy[asyncio] (2.0.25) - ORM
│   ├── asyncpg (0.29.0) - PostgreSQL driver
│   ├── alembic (1.13.1) - Migrations
│   ├── redis[hiredis] (5.0.1) - Cache client
│   └── aioboto3 (12.1.0) - S3 client
│
├── Security
│   ├── python-jose[cryptography] (3.3.0) - JWT
│   ├── passlib[bcrypt] (1.7.4) - Password hashing
│   └── cryptography (latest) - Encryption
│
├── Observability
│   ├── pydantic-logfire (0.1.0) - Logging & metrics
│   └── structlog (24.1.0) - Structured logging
│
├── Utilities
│   ├── httpx (0.26.0) - HTTP client
│   ├── websockets (12.0) - WebSocket server
│   ├── msgpack (1.0.7) - Binary serialization
│   ├── GitPython (3.1.41) - Git operations
│   ├── psutil (5.9.8) - System monitoring
│   └── tenacity (8.2.3) - Retry logic
│
└── Development
    ├── pytest (7.4.4) - Testing
    ├── black (23.12.1) - Code formatting
    └── mypy (1.8.0) - Type checking
```

### Frontend Dependencies Tree

```
claude-code-frontend
├── Core
│   ├── react (18.2.0) - UI framework
│   ├── react-router-dom (6.21.0) - Routing
│   └── typescript (5.2.2) - Type safety
│
├── State Management
│   ├── @tanstack/react-query (5.17.0) - Server state
│   └── zustand (4.4.7) - Client state
│
├── UI Components
│   ├── @radix-ui/react-* - Headless components
│   ├── tailwindcss (3.4.0) - Styling
│   ├── framer-motion (10.18.0) - Animations
│   └── lucide-react (0.312.0) - Icons
│
├── Terminal
│   ├── xterm (5.3.0) - Terminal emulator
│   ├── xterm-addon-fit - Auto-sizing
│   ├── xterm-addon-search - Search functionality
│   └── xterm-addon-web-links - Link detection
│
├── Code & Diff
│   ├── @monaco-editor/react (4.6.0) - Code editor
│   └── diff2html (3.4.45) - Diff visualization
│
└── Utilities
    ├── axios (1.6.5) - HTTP client
    ├── date-fns (3.2.0) - Date utilities
    └── react-hot-toast (2.4.1) - Notifications
```

## Service Dependencies Graph

```
┌─────────────────┐
│   Web Browser   │
└────────┬────────┘
         │ depends on
         ▼
┌─────────────────┐     ┌──────────────────┐
│  Frontend (React)│────►│ API Gateway      │
└─────────────────┘     └────────┬─────────┘
                                 │ depends on
                    ┌────────────┴────────────┬───────────────┐
                    ▼                         ▼               ▼
         ┌──────────────────┐      ┌──────────────────┐  ┌──────────────┐
         │ Auth Service     │      │ WebSocket Service│  │ REST API     │
         └────────┬─────────┘      └────────┬─────────┘  └──────┬───────┘
                  │                         │                    │
                  └─────────────┬───────────┴────────────────────┘
                                │ all depend on
                                ▼
                    ┌───────────────────────┐
                    │   Service Layer       │
                    │  ┌─────────────────┐  │
                    │  │  VM Manager     │  │
                    │  ├─────────────────┤  │
                    │  │Session Manager  │  │
                    │  ├─────────────────┤  │
                    │  │Snapshot Manager │  │
                    │  ├─────────────────┤  │
                    │  │  Git Manager    │  │
                    │  └─────────────────┘  │
                    └───────────┬───────────┘
                                │ depends on
                ┌───────────────┴─────────────────┬────────────────┐
                ▼                                 ▼                ▼
    ┌─────────────────────┐         ┌─────────────────────┐  ┌──────────────┐
    │   Data Layer        │         │  Message Layer      │  │Infrastructure│
    │ - PostgreSQL        │         │ - Redis Pub/Sub     │  │- Firecracker │
    │ - Redis Cache       │         │ - Task Queue        │  │- Network     │
    │ - MinIO Storage     │         └─────────────────────┘  │- Storage     │
    └─────────────────────┘                                   └──────────────┘
```

## Session Implementation Dependencies

### Dependency Matrix

| Session | Depends On Sessions | Provides For Sessions |
|---------|--------------------|-----------------------|
| **1. Core Infrastructure** | None | All others |
| **2. Authentication** | 1 | 3,4,5,6,7,8,9,10,11,12,13,14,15 |
| **3. VM Management** | 1,2 | 4,5,6,7,13,14 |
| **4. PTY Layer** | 1,3 | 5,6,10 |
| **5. WebSocket Layer** | 1,2,4 | 6,8,10 |
| **6. Session Management** | 1,2,4,5 | 8,10,13 |
| **7. Snapshot System** | 1,2,3 | 13,14 |
| **8. Frontend Terminal** | 2,5,6 | None |
| **9. Git Integration** | 1,2 | 8 |
| **10. Recording System** | 1,4,5,6 | 8 |
| **11. Monitoring** | 1 | All (cross-cutting) |
| **12. Rate Limiting** | 1,2 | All (cross-cutting) |
| **13. High Availability** | 1,3,6,7 | 14,15 |
| **14. Performance Opt** | All | 15 |
| **15. Deployment** | All | None |

### Implementation Phases

```
Phase 1: Foundation (Weeks 1-2)
├── Session 1: Core Infrastructure
├── Session 2: Authentication
└── Session 3: VM Management (basic)

Phase 2: Core Features (Weeks 3-5)
├── Session 4: PTY Layer
├── Session 5: WebSocket Layer
├── Session 6: Session Management
└── Session 8: Frontend Terminal (basic)

Phase 3: Advanced Features (Weeks 6-8)
├── Session 7: Snapshot System
├── Session 9: Git Integration
├── Session 10: Recording System
└── Session 8: Frontend Terminal (complete)

Phase 4: Production Ready (Weeks 9-10)
├── Session 11: Monitoring
├── Session 12: Rate Limiting
├── Session 13: High Availability
├── Session 14: Performance
└── Session 15: Deployment
```

## Key Architectural Patterns

### 1. **Microservices with Shared Libraries**
- Services communicate via APIs, not shared database
- Common functionality in shared Python packages
- Each service has its own deployment lifecycle

### 2. **Event-Driven Architecture**
- Redis Pub/Sub for real-time events
- Async task processing for heavy operations
- Event sourcing for audit trails

### 3. **Repository Pattern**
- Abstract data access behind repositories
- Enables testing with mocks
- Supports multiple data sources

### 4. **Circuit Breaker Pattern**
- Prevent cascade failures
- Automatic recovery
- Graceful degradation

### 5. **CQRS (Command Query Responsibility Segregation)**
- Separate read and write models
- Optimized queries for different use cases
- Event-driven synchronization

## Data Flow Scenarios

### Scenario 1: VM Creation with Snapshot
```
User Request → API Gateway → Auth Check → VM Manager
                                              ↓
                                    Check User Quotas
                                              ↓
                                    Allocate Resources
                                              ↓
                                    Download Snapshot ← MinIO
                                              ↓
                                    Create Firecracker VM
                                              ↓
                                    Update Database
                                              ↓
                                    Notify via Redis → WebSocket → User
```

### Scenario 2: Terminal Connection
```
Browser → WebSocket Upgrade → Auth Validation
                                    ↓
                            Session Creation/Recovery
                                    ↓
                            PTY Connection to VM
                                    ↓
              ┌────────────────────┴────────────────────┐
              ↓                                         ↓
        Input Stream:                            Output Stream:
        Browser → WS → PTY → VM                  VM → PTY → WS → Browser
              ↓                                         ↓
        Rate Limiting                            Buffer Management
              ↓                                         ↓
        Security Filter                          Security Filter
```

### Scenario 3: Snapshot Creation
```
User Request → Validate VM Ownership → Pause VM
                                           ↓
                                    Create Snapshot
                                           ↓
                            Upload to MinIO (Parallel)
                            ├── VM State
                            ├── Memory State
                            └── Root Filesystem
                                           ↓
                                    Update Metadata
                                           ↓
                                    Resume VM
```

## Infrastructure Requirements

### Compute Requirements

| Component | CPU | Memory | Storage | Network | Special |
|-----------|-----|--------|---------|---------|---------|
| **API Servers** | 4 cores | 8GB | 50GB | 1Gbps | - |
| **WebSocket Servers** | 4 cores | 16GB | 50GB | 10Gbps | High conn limit |
| **PostgreSQL Primary** | 8 cores | 32GB | 500GB SSD | 10Gbps | NVMe preferred |
| **PostgreSQL Replica** | 8 cores | 32GB | 500GB SSD | 10Gbps | NVMe preferred |
| **Redis Primary** | 4 cores | 16GB | 100GB SSD | 10Gbps | - |
| **Redis Replica** | 4 cores | 16GB | 100GB SSD | 10Gbps | - |
| **MinIO Nodes** | 4 cores | 8GB | 2TB HDD | 10Gbps | x4 minimum |
| **Firecracker Hosts** | 32 cores | 128GB | 1TB NVMe | 10Gbps | KVM enabled |

### Network Architecture

```
                          Internet
                              │
                    ┌─────────┴─────────┐
                    │   Public Subnet   │
                    │  ┌─────────────┐  │
                    │  │Load Balancer│  │
                    │  └──────┬──────┘  │
                    └─────────┼─────────┘
                              │
                    ┌─────────┴─────────┐
                    │  Private Subnet 1 │
                    │  (API/WebSocket)  │
                    │  10.0.1.0/24      │
                    └─────────┬─────────┘
                              │
                    ┌─────────┴─────────┐
                    │  Private Subnet 2 │
                    │  (Databases)      │
                    │  10.0.2.0/24      │
                    └─────────┬─────────┘
                              │
                    ┌─────────┴─────────┐
                    │  Private Subnet 3 │
                    │  (Firecracker VMs)│
                    │  10.0.100.0/20    │ ← 4094 IPs for VMs
                    └───────────────────┘
```

## Security Zones

### Zone Architecture
```
┌─────────────────────────────────────────────────────┐
│                    DMZ Zone                         │
│  - Load Balancers                                   │
│  - WAF                                              │
└─────────────────────┬───────────────────────────────┘
                      │ Firewall Rules
┌─────────────────────┴───────────────────────────────┐
│                Application Zone                      │
│  - API Servers                                       │
│  - WebSocket Servers                                 │
│  - Background Workers                                │
└─────────────────────┬───────────────────────────────┘
                      │ Firewall Rules
┌─────────────────────┴───────────────────────────────┐
│                  Data Zone                           │
│  - PostgreSQL                                        │
│  - Redis                                             │
│  - MinIO                                             │
└─────────────────────┬───────────────────────────────┘
                      │ Firewall Rules
┌─────────────────────┴───────────────────────────────┐
│                Compute Zone                          │
│  - Firecracker Hosts                                 │
│  - Isolated VM Networks                              │
└─────────────────────────────────────────────────────┘
```

## Scalability Considerations

### Horizontal Scaling Points
1. **API Servers**: Stateless, scale based on CPU
2. **WebSocket Servers**: Scale based on connection count
3. **Firecracker Hosts**: Scale based on VM demand
4. **MinIO**: Scale storage nodes as needed
5. **PostgreSQL**: Read replicas for queries

### Bottlenecks to Monitor
1. **PostgreSQL Write Throughput**: Consider sharding
2. **Redis Memory**: Implement eviction policies
3. **Network Bandwidth**: Especially for snapshots
4. **Storage IOPS**: For VM operations
5. **WebSocket Connections**: OS limits per server

## Disaster Recovery

### Backup Strategy
```
Component          | Frequency | Retention | Location
-------------------|-----------|-----------|----------
PostgreSQL         | 1 hour    | 30 days   | S3 + Glacier
Redis              | 1 hour    | 7 days    | S3
MinIO              | Real-time | Infinite  | Cross-region
Git Repositories   | 1 hour    | 90 days   | S3
VM Snapshots       | On-demand | User-defined | S3 + Glacier
Configuration      | On change | Infinite  | Git + S3
```

### Recovery Objectives
- **RTO (Recovery Time Objective)**: 1 hour
- **RPO (Recovery Point Objective)**: 1 hour
- **Degraded Mode**: Core functions in 15 minutes

## Cost Optimization Strategies

### Resource Optimization
1. **VM Pooling**: Pre-warm common configurations
2. **Snapshot Deduplication**: Block-level dedup
3. **Idle Detection**: Suspend inactive VMs
4. **Storage Tiering**: Hot/cold data separation
5. **Spot Instances**: For non-critical workloads

### Monitoring Costs
- Track per-user resource consumption
- Alert on unusual usage patterns
- Implement hard limits
- Regular cost reviews

## Integration Points

### External Services
1. **CloudFlare**: CDN and DDoS protection
2. **SendGrid**: Email notifications
3. **Stripe**: Billing (future)
4. **GitHub**: OAuth provider
5. **Datadog**: Additional monitoring (optional)

### Webhooks & Callbacks
1. **VM State Changes**: POST to user-defined URLs
2. **Git Push Events**: Trigger CI/CD
3. **Snapshot Complete**: Notification system
4. **Session Events**: Analytics pipeline

## Development Workflow

### Local Development Stack
```yaml
# docker-compose.dev.yml summary
services:
  postgres:   # Single instance
  redis:      # Single instance
  minio:      # Single node
  softserve:  # Git server
  api:        # Hot reload
  frontend:   # Vite dev server
  firecracker: # Mock for testing
```

### Testing Environments
1. **Unit Tests**: Mock all external services
2. **Integration Tests**: Real services, fake Firecracker
3. **E2E Tests**: Full stack with real VMs
4. **Performance Tests**: Dedicated environment
5. **Security Tests**: Isolated penetration testing

## Success Criteria

### Technical KPIs
- System uptime > 99.9%

### Operational KPIs
- Deployment frequency > 1/day
- Mean time to recovery < 30 minutes
- Error rate < 0.1%
- Documentation coverage > 90%

## Risk Mitigation Strategies

### Technical Risks
1. **Firecracker vulnerabilities**: Regular patching, defense in depth
2. **Data corruption**: Checksums, regular verification
3. **Performance degradation**: Capacity planning, monitoring
4. **Security breaches**: Regular audits, penetration testing
5. **Dependency failures**: Vendoring critical dependencies

### Operational Risks
1. **Key person dependency**: Documentation, knowledge sharing
2. **Vendor lock-in**: Abstraction layers, standard protocols
3. **Compliance issues**: Regular reviews, automated checks
4. **Scaling issues**: Gradual rollout, monitoring
5. **Budget overruns**: Cost monitoring, alerts

## Conclusion

This architecture provides a robust, scalable, and secure platform for running isolated development environments. The modular design allows for incremental development while maintaining system integrity. Key success factors include:

1. **Security-first design** at every layer
2. **Clear service boundaries** and dependencies
3. **Comprehensive monitoring** and observability
4. **Automated testing** and deployment
5. **Disaster recovery** planning

The implementation sessions build upon each other logically, allowing for continuous delivery of value while maintaining system stability.