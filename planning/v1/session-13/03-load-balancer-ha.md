# Session 13.3: Load Balancer High Availability & Traffic Distribution

## Objective
Implement highly available load balancer configuration with HAProxy, providing intelligent traffic distribution, health checking, SSL termination, and seamless failover to ensure continuous service availability across multiple application instances.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for load balancer performance monitoring and traffic analytics
- **Session 8**: Distributes frontend traffic across multiple React application instances
- **Session 12**: Integrates with rate limiting for distributed traffic management
- **Session 13.1-13.2**: Coordinates with database and Redis HA for complete infrastructure resilience
- **All Sessions**: Provides high availability entry point for all platform services

## Core Implementation

### HAProxy Load Balancer Configuration
**Location**: `infrastructure/load_balancer/haproxy.cfg`

```haproxy
# infrastructure/load_balancer/haproxy.cfg
# HAProxy High Availability Configuration

global
    # Process management
    daemon
    user haproxy
    group haproxy
    
    # Connection limits
    maxconn 4096
    ulimit-n 65536
    
    # Logging
    log stdout local0 info
    
    # SSL configuration
    ssl-default-bind-ciphers ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256
    ssl-default-bind-options ssl-min-ver TLSv1.2 no-tls-tickets
    ssl-default-server-ciphers ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256
    ssl-default-server-options ssl-min-ver TLSv1.2 no-tls-tickets
    
    # Performance tuning
    tune.ssl.default-dh-param 2048
    tune.ssl.capture-cipherlist-size 1
    tune.ssl.capture-buffer-size 1
    
    # Stats socket for management
    stats socket /var/run/haproxy.sock mode 600 level admin
    stats timeout 2m
    
    # CPU affinity for performance
    nbproc 1
    nbthread 4
    cpu-map auto:1/1-4 0-3

defaults
    mode http
    
    # Timeouts
    timeout connect 5000ms
    timeout client 50000ms
    timeout server 50000ms
    timeout http-request 10000ms
    timeout http-keep-alive 2000ms
    timeout check 3000ms
    timeout tunnel 3600000ms
    
    # Logging
    option httplog
    option dontlognull
    option log-health-checks
    
    # Error handling
    option redispatch
    retries 3
    maxconn 2000
    
    # Compression
    compression algo gzip
    compression type text/html text/plain text/css text/javascript application/javascript application/json application/xml
    
    # Headers
    option forwardfor
    option httpchk GET /health
    
    # Error pages
    errorfile 400 /etc/haproxy/errors/400.http
    errorfile 403 /etc/haproxy/errors/403.http
    errorfile 408 /etc/haproxy/errors/408.http
    errorfile 500 /etc/haproxy/errors/500.http
    errorfile 502 /etc/haproxy/errors/502.http
    errorfile 503 /etc/haproxy/errors/503.http
    errorfile 504 /etc/haproxy/errors/504.http

# ==========================================
# FRONTEND CONFIGURATIONS
# ==========================================

# Main HTTPS Frontend
frontend main_https
    bind *:443 ssl crt /etc/ssl/certs/platform.pem alpn h2,http/1.1
    bind *:80
    
    # Redirect HTTP to HTTPS
    redirect scheme https if !{ ssl_fc }
    
    # Security headers
    http-response set-header Strict-Transport-Security "max-age=31536000; includeSubDomains; preload"
    http-response set-header X-Frame-Options "DENY"
    http-response set-header X-Content-Type-Options "nosniff"
    http-response set-header X-XSS-Protection "1; mode=block"
    http-response set-header Referrer-Policy "strict-origin-when-cross-origin"
    http-response set-header Content-Security-Policy "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'"
    
    # Rate limiting using stick tables
    stick-table type ip size 100k expire 30s store http_req_rate(10s),http_err_rate(10s),conn_cur
    http-request track-sc0 src
    
    # Basic rate limiting - 100 requests per 10 seconds per IP
    http-request deny if { sc_http_req_rate(0) gt 100 }
    
    # Block IPs with high error rates
    http-request deny if { sc_http_err_rate(0) gt 10 }
    
    # Request size limits
    http-request deny if { req.body_size gt 10485760 }  # 10MB limit
    
    # Capture headers for logging
    capture request header Host len 64
    capture request header User-Agent len 128
    capture request header X-Forwarded-For len 64
    
    # ACLs for routing
    acl is_api path_beg /api/
    acl is_websocket hdr(Connection) -i upgrade
    acl is_websocket hdr(Upgrade) -i websocket
    acl is_websocket path_beg /ws/
    acl is_static path_beg /static/ /assets/ /images/ /css/ /js/
    acl is_health path_beg /health /status
    
    # Routing decisions
    use_backend websocket_servers if is_websocket
    use_backend api_servers if is_api
    use_backend static_servers if is_static
    use_backend health_check if is_health
    default_backend frontend_servers

# WebSocket Frontend (separate port for better isolation)
frontend websocket_frontend
    bind *:8443 ssl crt /etc/ssl/certs/platform.pem alpn h2,http/1.1
    bind *:8080
    
    # Redirect HTTP to HTTPS
    redirect scheme https if !{ ssl_fc }
    
    # WebSocket specific headers
    http-request set-header X-Real-IP %[src]
    http-request set-header X-Forwarded-Proto https if { ssl_fc }
    http-request set-header X-Forwarded-Proto http if !{ ssl_fc }
    
    # Rate limiting for WebSocket connections
    stick-table type ip size 50k expire 60s store conn_rate(10s),conn_cur
    http-request track-sc0 src
    http-request deny if { sc_conn_rate(0) gt 10 }  # 10 connections per 10 seconds
    
    # WebSocket upgrade handling
    acl is_websocket hdr(Connection) -i upgrade
    acl is_websocket hdr(Upgrade) -i websocket
    
    use_backend websocket_servers if is_websocket
    default_backend websocket_servers

# API Management Frontend
frontend api_management
    bind *:9443 ssl crt /etc/ssl/certs/platform.pem
    
    # Admin access restrictions
    acl admin_networks src 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16
    http-request deny if !admin_networks
    
    # API management routes
    acl is_metrics path_beg /metrics
    acl is_stats path_beg /stats
    acl is_admin path_beg /admin/
    
    use_backend prometheus_exporter if is_metrics
    use_backend stats_backend if is_stats
    use_backend admin_servers if is_admin
    default_backend api_servers

# ==========================================
# BACKEND CONFIGURATIONS
# ==========================================

# API Servers Backend
backend api_servers
    balance roundrobin
    option httpchk GET /health
    http-check expect status 200
    
    # Health check configuration
    default-server check inter 5s fall 3 rise 2 maxconn 100
    
    # Sticky sessions based on session cookie
    cookie SERVERID insert indirect nocache
    
    # Backend servers
    server api1 api-server-1:8000 check cookie api1 weight 100
    server api2 api-server-2:8000 check cookie api2 weight 100
    server api3 api-server-3:8000 check cookie api3 weight 100
    
    # Backup server
    server api_backup api-backup:8000 check backup
    
    # Backend options
    option httpchk GET /health HTTP/1.1\r\nHost:\ api.example.com
    http-check send-state
    
    # Request modifications
    http-request set-header X-Forwarded-Port %[dst_port]
    http-request set-header X-Forwarded-Proto https if { ssl_fc }
    
    # Response modifications
    http-response set-header X-Backend-Server %[srv_name]

# Frontend/React Servers Backend
backend frontend_servers
    balance roundrobin
    option httpchk GET /health
    
    # Health check for React apps
    http-check expect status 200
    default-server check inter 10s fall 2 rise 2 maxconn 50
    
    # Frontend servers
    server frontend1 frontend-server-1:3000 check weight 100
    server frontend2 frontend-server-2:3000 check weight 100
    server frontend3 frontend-server-3:3000 check weight 100
    
    # Static content caching headers
    http-response set-header Cache-Control "public, max-age=31536000" if { path_beg /static/ }
    http-response set-header Cache-Control "public, max-age=86400" if { path_beg /assets/ }

# WebSocket Servers Backend
backend websocket_servers
    balance source  # Source IP hash for session affinity
    option httpchk GET /ws/health
    
    # WebSocket specific timeouts
    timeout server 3600000ms  # 1 hour for long-lived connections
    timeout tunnel 3600000ms
    
    # Health checks
    default-server check inter 10s fall 2 rise 2
    
    # WebSocket servers
    server ws1 websocket-server-1:8000 check weight 100
    server ws2 websocket-server-2:8000 check weight 100
    server ws3 websocket-server-3:8000 check weight 100
    
    # Connection management
    option http-server-close
    option forceclose

# Static Content Servers Backend
backend static_servers
    balance roundrobin
    option httpchk GET /health
    
    # Optimized for static content
    timeout server 30s
    default-server check inter 30s fall 2 rise 1
    
    # Static content servers (could be CDN or file servers)
    server static1 static-server-1:80 check weight 100
    server static2 static-server-2:80 check weight 100
    
    # Caching headers for static content
    http-response set-header Cache-Control "public, max-age=31536000"
    http-response set-header Expires "%[date(31536000),http_date]"

# Health Check Backend
backend health_check
    http-request return status 200 content-type "application/json" string '{"status":"healthy","load_balancer":"active","timestamp":"%[date()]"}'

# Statistics Backend
backend stats_backend
    stats enable
    stats uri /stats
    stats refresh 30s
    stats admin if TRUE
    stats realm "HAProxy Statistics"
    stats auth admin:${HAPROXY_STATS_PASSWORD}
    
    # Enhanced stats
    stats show-desc "Build Platform Load Balancer"
    stats show-legends
    stats show-node

# Prometheus Metrics Backend
backend prometheus_exporter
    http-request return status 200 content-type "text/plain" string "# Prometheus metrics endpoint\n# Implement actual metrics collection here"

# Admin Backend
backend admin_servers
    balance roundrobin
    option httpchk GET /admin/health
    
    # Admin servers
    server admin1 admin-server-1:8000 check
    server admin2 admin-server-2:8000 check backup

# ==========================================
# LISTEN CONFIGURATIONS
# ==========================================

# Statistics interface
listen stats
    bind *:8404 ssl crt /etc/ssl/certs/platform.pem
    mode http
    
    # Access control
    acl admin_networks src 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16
    http-request deny if !admin_networks
    
    # Stats configuration
    stats enable
    stats uri /
    stats refresh 5s
    stats realm "HAProxy Load Balancer Statistics"
    stats auth admin:${HAPROXY_STATS_PASSWORD}
    stats admin if TRUE
    
    # Additional stats options
    stats show-desc "Build Platform HA Load Balancer"
    stats show-legends
    stats show-node
    stats hide-version

# Health monitoring endpoint
listen health_monitor
    bind *:8405
    mode http
    
    # Simple health check
    monitor-uri /health
    
    # Detailed health information
    http-request return status 200 content-type "application/json" string '{"status":"healthy","timestamp":"%[date()]","connections":"%[fe_conn()]","version":"HAProxy 2.x"}' if { path /health }
    
    # Return 503 if no backend servers are available
    monitor fail if { nbsrv(api_servers) eq 0 }
    monitor fail if { nbsrv(frontend_servers) eq 0 }
```

### HAProxy Docker Configuration
**Location**: `infrastructure/load_balancer/docker-compose.yml`

```yaml
# infrastructure/load_balancer/docker-compose.yml
version: '3.8'

services:
  haproxy-primary:
    image: haproxy:2.8-alpine
    container_name: haproxy-primary
    volumes:
      - ./haproxy.cfg:/usr/local/etc/haproxy/haproxy.cfg:ro
      - ./ssl:/etc/ssl/certs:ro
      - ./errors:/etc/haproxy/errors:ro
      - haproxy_logs:/var/log/haproxy
    environment:
      - HAPROXY_STATS_PASSWORD=${HAPROXY_STATS_PASSWORD}
    networks:
      - load_balancer_network
      - frontend_network
      - backend_network
    ports:
      - "80:80"
      - "443:443"
      - "8080:8080"
      - "8443:8443"
      - "8404:8404"
      - "8405:8405"
      - "9443:9443"
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8405/health"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 30s
    deploy:
      resources:
        limits:
          memory: 512M
          cpus: '1.0'
        reservations:
          memory: 256M
          cpus: '0.5'
    restart: unless-stopped
    logging:
      driver: "json-file"
      options:
        max-size: "10m"
        max-file: "3"

  haproxy-backup:
    image: haproxy:2.8-alpine
    container_name: haproxy-backup
    volumes:
      - ./haproxy-backup.cfg:/usr/local/etc/haproxy/haproxy.cfg:ro
      - ./ssl:/etc/ssl/certs:ro
      - ./errors:/etc/haproxy/errors:ro
      - haproxy_backup_logs:/var/log/haproxy
    environment:
      - HAPROXY_STATS_PASSWORD=${HAPROXY_STATS_PASSWORD}
    networks:
      - load_balancer_network
      - frontend_network
      - backend_network
    ports:
      - "8080:80"   # Different ports to avoid conflicts
      - "8443:443"
      - "18080:8080"
      - "18443:8443"
      - "18404:8404"
      - "18405:8405"
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8405/health"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 30s
    deploy:
      resources:
        limits:
          memory: 512M
          cpus: '1.0'
        reservations:
          memory: 256M
          cpus: '0.5'
    restart: unless-stopped
    profiles:
      - backup  # Only start when backup profile is specified
    logging:
      driver: "json-file"
      options:
        max-size: "10m"
        max-file: "3"

  keepalived:
    image: osixia/keepalived:2.0.20
    container_name: keepalived
    privileged: true
    network_mode: host
    environment:
      - KEEPALIVED_INTERFACE=eth0
      - KEEPALIVED_VIRTUAL_IPS=192.168.1.100
      - KEEPALIVED_UNICAST_PEERS=#PYTHON2BASH:["192.168.1.10", "192.168.1.11"]
      - KEEPALIVED_PRIORITY=100
      - KEEPALIVED_PASSWORD=changeme
    volumes:
      - ./keepalived.conf:/container/service/keepalived/assets/keepalived.conf:ro
    restart: unless-stopped
    depends_on:
      - haproxy-primary

volumes:
  haproxy_logs:
    driver: local
  haproxy_backup_logs:
    driver: local

networks:
  load_balancer_network:
    driver: bridge
    ipam:
      config:
        - subnet: 172.22.0.0/16
  frontend_network:
    external: true
  backend_network:
    external: true
```

### Load Balancer Health Monitor
**Location**: `infrastructure/load_balancer/health_monitor.py`

```python
# infrastructure/load_balancer/health_monitor.py
import asyncio
import aiohttp
import time
import json
from typing import Dict, List, Optional, Any, Tuple
from dataclasses import dataclass, field
from enum import Enum
import structlog
import logfire
from concurrent.futures import ThreadPoolExecutor

logger = structlog.get_logger()

class ServerStatus(Enum):
    HEALTHY = "healthy"
    UNHEALTHY = "unhealthy"
    MAINTENANCE = "maintenance"
    UNKNOWN = "unknown"

class LoadBalancerStatus(Enum):
    ACTIVE = "active"
    STANDBY = "standby"
    FAILED = "failed"
    MAINTENANCE = "maintenance"

@dataclass
class BackendServer:
    server_id: str
    host: str
    port: int
    backend_name: str
    status: ServerStatus = ServerStatus.UNKNOWN
    response_time_ms: float = 0.0
    last_check: float = field(default_factory=time.time)
    consecutive_failures: int = 0
    total_requests: int = 0
    error_rate: float = 0.0
    weight: int = 100
    is_backup: bool = False

@dataclass
class LoadBalancerInstance:
    instance_id: str
    host: str
    stats_port: int
    health_port: int
    status: LoadBalancerStatus = LoadBalancerStatus.UNKNOWN
    is_primary: bool = False
    last_check: float = field(default_factory=time.time)
    total_connections: int = 0
    current_connections: int = 0
    backend_servers: Dict[str, BackendServer] = field(default_factory=dict)

@dataclass
class TrafficMetrics:
    requests_per_second: float = 0.0
    bytes_per_second: float = 0.0
    active_connections: int = 0
    error_rate: float = 0.0
    avg_response_time_ms: float = 0.0
    timestamp: float = field(default_factory=time.time)

class LoadBalancerHealthMonitor:
    def __init__(self, config: Dict[str, Any], notification_service=None):
        self.config = config
        self.notification_service = notification_service
        
        # Load balancer instances
        self.lb_instances: Dict[str, LoadBalancerInstance] = {}
        
        # Monitoring configuration
        self.check_interval = config.get('check_interval', 30)
        self.failure_threshold = config.get('failure_threshold', 3)
        self.timeout = config.get('timeout', 10)
        
        # Metrics tracking
        self.traffic_metrics: List[TrafficMetrics] = []
        self.max_metrics_history = 1440  # 24 hours of minute-by-minute data
        
        # Background tasks
        self.monitoring_task: Optional[asyncio.Task] = None
        self.metrics_task: Optional[asyncio.Task] = None
        
        # Thread pool for blocking operations
        self.thread_pool = ThreadPoolExecutor(max_workers=4)
        
    async def initialize(self):
        """Initialize the load balancer health monitor"""
        try:
            # Configure load balancer instances
            await self._configure_lb_instances()
            
            # Start monitoring tasks
            self.monitoring_task = asyncio.create_task(self._monitoring_loop())
            self.metrics_task = asyncio.create_task(self._metrics_collection_loop())
            
            logfire.info("Load balancer health monitor initialized",
                       instances_count=len(self.lb_instances))
            
            logger.info("Load balancer health monitor initialized")
            
        except Exception as e:
            logger.error("Failed to initialize load balancer monitor", error=str(e))
            raise
    
    async def _configure_lb_instances(self):
        """Configure load balancer instances from config"""
        lb_configs = self.config.get('load_balancers', [])
        
        for lb_config in lb_configs:
            instance = LoadBalancerInstance(
                instance_id=lb_config['id'],
                host=lb_config['host'],
                stats_port=lb_config.get('stats_port', 8404),
                health_port=lb_config.get('health_port', 8405),
                is_primary=lb_config.get('is_primary', False)
            )
            
            self.lb_instances[instance.instance_id] = instance
            
            # Configure backend servers for this instance
            await self._configure_backend_servers(instance, lb_config.get('backends', []))
    
    async def _configure_backend_servers(self, lb_instance: LoadBalancerInstance, 
                                       backends_config: List[Dict[str, Any]]):
        """Configure backend servers for load balancer instance"""
        for backend_config in backends_config:
            backend_name = backend_config['name']
            servers = backend_config.get('servers', [])
            
            for server_config in servers:
                server = BackendServer(
                    server_id=f"{backend_name}_{server_config['id']}",
                    host=server_config['host'],
                    port=server_config['port'],
                    backend_name=backend_name,
                    weight=server_config.get('weight', 100),
                    is_backup=server_config.get('is_backup', False)
                )
                
                lb_instance.backend_servers[server.server_id] = server
    
    async def stop(self):
        """Stop the load balancer health monitor"""
        try:
            # Stop monitoring tasks
            for task in [self.monitoring_task, self.metrics_task]:
                if task and not task.done():
                    task.cancel()
                    try:
                        await task
                    except asyncio.CancelledError:
                        pass
            
            # Shutdown thread pool
            self.thread_pool.shutdown(wait=True)
            
            logger.info("Load balancer health monitor stopped")
            
        except Exception as e:
            logger.error("Error stopping load balancer monitor", error=str(e))
    
    async def _monitoring_loop(self):
        """Main monitoring loop for load balancer health"""
        while True:
            try:
                await asyncio.sleep(self.check_interval)
                
                # Check health of all load balancer instances
                await self._check_all_lb_instances()
                
                # Check backend servers health
                await self._check_all_backend_servers()
                
                # Evaluate failover needs
                await self._evaluate_failover_needs()
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Load balancer monitoring loop error", error=str(e))
                await asyncio.sleep(5)
    
    async def _check_all_lb_instances(self):
        """Check health of all load balancer instances"""
        check_tasks = []
        for instance_id, instance in self.lb_instances.items():
            task = asyncio.create_task(self._check_lb_instance_health(instance))
            check_tasks.append(task)
        
        await asyncio.gather(*check_tasks, return_exceptions=True)
    
    async def _check_lb_instance_health(self, instance: LoadBalancerInstance):
        """Check health of a specific load balancer instance"""
        try:
            async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=self.timeout)) as session:
                # Check health endpoint
                health_url = f"http://{instance.host}:{instance.health_port}/health"
                
                start_time = time.time()
                async with session.get(health_url) as response:
                    response_time = (time.time() - start_time) * 1000
                    
                    if response.status == 200:
                        health_data = await response.json()
                        
                        instance.status = LoadBalancerStatus.ACTIVE
                        instance.last_check = time.time()
                        
                        # Get statistics
                        await self._get_lb_statistics(instance, session)
                        
                        logfire.debug("Load balancer health check successful",
                                    instance_id=instance.instance_id,
                                    response_time_ms=response_time,
                                    connections=instance.current_connections)
                    else:
                        raise Exception(f"Health check failed with status {response.status}")
                        
        except Exception as e:
            instance.status = LoadBalancerStatus.FAILED
            instance.last_check = time.time()
            
            logger.warning("Load balancer health check failed",
                         instance_id=instance.instance_id,
                         host=instance.host,
                         error=str(e))
            
            logfire.warning("Load balancer unhealthy",
                          instance_id=instance.instance_id,
                          error=str(e))
    
    async def _get_lb_statistics(self, instance: LoadBalancerInstance, session: aiohttp.ClientSession):
        """Get statistics from load balancer instance"""
        try:
            # Get HAProxy stats (this would need to parse CSV format)
            stats_url = f"http://{instance.host}:{instance.stats_port}/stats?stats;csv"
            
            async with session.get(stats_url) as response:
                if response.status == 200:
                    stats_data = await response.text()
                    await self._parse_haproxy_stats(instance, stats_data)
                    
        except Exception as e:
            logger.debug("Failed to get load balancer statistics", 
                       instance_id=instance.instance_id, error=str(e))
    
    async def _parse_haproxy_stats(self, instance: LoadBalancerInstance, stats_data: str):
        """Parse HAProxy statistics CSV format"""
        try:
            lines = stats_data.strip().split('\n')
            if len(lines) < 2:
                return
            
            # First line contains headers
            headers = lines[0].split(',')
            
            # Parse each data line
            for line in lines[1:]:
                if not line:
                    continue
                    
                values = line.split(',')
                if len(values) != len(headers):
                    continue
                
                stats = dict(zip(headers, values))
                
                # Update backend server information
                server_name = stats.get('svname', '')
                backend_name = stats.get('pxname', '')
                
                if server_name and server_name != 'BACKEND' and server_name != 'FRONTEND':
                    server_id = f"{backend_name}_{server_name}"
                    
                    if server_id in instance.backend_servers:
                        server = instance.backend_servers[server_id]
                        
                        # Update server statistics
                        server.total_requests = int(stats.get('stot', 0) or 0)
                        server.error_rate = float(stats.get('eresp', 0) or 0)
                        
                        # Determine server status
                        status = stats.get('status', 'UNK')
                        if status == 'UP':
                            server.status = ServerStatus.HEALTHY
                            server.consecutive_failures = 0
                        elif status == 'DOWN':
                            server.status = ServerStatus.UNHEALTHY
                            server.consecutive_failures += 1
                        elif status == 'MAINT':
                            server.status = ServerStatus.MAINTENANCE
                
                # Update instance-level statistics
                if stats.get('pxname') == 'main_https' and stats.get('svname') == 'FRONTEND':
                    instance.current_connections = int(stats.get('scur', 0) or 0)
                    instance.total_connections = int(stats.get('stot', 0) or 0)
                    
        except Exception as e:
            logger.error("Failed to parse HAProxy statistics", 
                       instance_id=instance.instance_id, error=str(e))
    
    async def _check_all_backend_servers(self):
        """Check health of all backend servers across all load balancer instances"""
        check_tasks = []
        
        for instance in self.lb_instances.values():
            for server in instance.backend_servers.values():
                task = asyncio.create_task(self._check_backend_server_health(server))
                check_tasks.append(task)
        
        await asyncio.gather(*check_tasks, return_exceptions=True)
    
    async def _check_backend_server_health(self, server: BackendServer):
        """Check health of a specific backend server"""
        try:
            async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=self.timeout)) as session:
                # Determine health check URL based on backend type
                if server.backend_name == 'api_servers':
                    health_url = f"http://{server.host}:{server.port}/health"
                elif server.backend_name == 'frontend_servers':
                    health_url = f"http://{server.host}:{server.port}/"
                elif server.backend_name == 'websocket_servers':
                    health_url = f"http://{server.host}:{server.port}/ws/health"
                else:
                    health_url = f"http://{server.host}:{server.port}/health"
                
                start_time = time.time()
                async with session.get(health_url) as response:
                    response_time = (time.time() - start_time) * 1000
                    
                    if response.status == 200:
                        server.status = ServerStatus.HEALTHY
                        server.response_time_ms = response_time
                        server.consecutive_failures = 0
                        server.last_check = time.time()
                    else:
                        raise Exception(f"Health check failed with status {response.status}")
                        
        except Exception as e:
            server.status = ServerStatus.UNHEALTHY
            server.consecutive_failures += 1
            server.last_check = time.time()
            
            if server.consecutive_failures >= self.failure_threshold:
                logger.warning("Backend server marked as unhealthy",
                             server_id=server.server_id,
                             backend=server.backend_name,
                             consecutive_failures=server.consecutive_failures,
                             error=str(e))
    
    async def _metrics_collection_loop(self):
        """Collect traffic metrics periodically"""
        while True:
            try:
                await asyncio.sleep(60)  # Collect metrics every minute
                
                # Collect current metrics
                current_metrics = await self._collect_traffic_metrics()
                
                if current_metrics:
                    self.traffic_metrics.append(current_metrics)
                    
                    # Limit history size
                    if len(self.traffic_metrics) > self.max_metrics_history:
                        self.traffic_metrics = self.traffic_metrics[-self.max_metrics_history:]
                    
                    # Log metrics to Logfire
                    logfire.info("Load balancer traffic metrics",
                               requests_per_second=current_metrics.requests_per_second,
                               active_connections=current_metrics.active_connections,
                               error_rate=current_metrics.error_rate,
                               avg_response_time_ms=current_metrics.avg_response_time_ms)
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Metrics collection loop error", error=str(e))
    
    async def _collect_traffic_metrics(self) -> Optional[TrafficMetrics]:
        """Collect current traffic metrics from all load balancer instances"""
        try:
            total_connections = 0
            total_requests = 0
            total_errors = 0
            response_times = []
            
            for instance in self.lb_instances.values():
                if instance.status == LoadBalancerStatus.ACTIVE:
                    total_connections += instance.current_connections
                    
                    for server in instance.backend_servers.values():
                        if server.status == ServerStatus.HEALTHY:
                            total_requests += server.total_requests
                            total_errors += server.error_rate
                            if server.response_time_ms > 0:
                                response_times.append(server.response_time_ms)
            
            # Calculate averages
            error_rate = (total_errors / max(total_requests, 1)) * 100
            avg_response_time = sum(response_times) / len(response_times) if response_times else 0
            
            return TrafficMetrics(
                requests_per_second=total_requests / 60,  # Rough estimation
                active_connections=total_connections,
                error_rate=error_rate,
                avg_response_time_ms=avg_response_time
            )
            
        except Exception as e:
            logger.error("Failed to collect traffic metrics", error=str(e))
            return None
    
    def get_health_summary(self) -> Dict[str, Any]:
        """Get comprehensive health summary"""
        # Load balancer instances status
        lb_status = {}
        healthy_instances = 0
        
        for instance_id, instance in self.lb_instances.items():
            lb_status[instance_id] = {
                "status": instance.status.value,
                "is_primary": instance.is_primary,
                "current_connections": instance.current_connections,
                "last_check": instance.last_check
            }
            
            if instance.status == LoadBalancerStatus.ACTIVE:
                healthy_instances += 1
        
        # Backend servers status
        backend_status = {}
        for instance in self.lb_instances.values():
            for server_id, server in instance.backend_servers.items():
                if server.backend_name not in backend_status:
                    backend_status[server.backend_name] = {
                        "total_servers": 0,
                        "healthy_servers": 0,
                        "servers": {}
                    }
                
                backend_status[server.backend_name]["total_servers"] += 1
                if server.status == ServerStatus.HEALTHY:
                    backend_status[server.backend_name]["healthy_servers"] += 1
                
                backend_status[server.backend_name]["servers"][server_id] = {
                    "status": server.status.value,
                    "response_time_ms": server.response_time_ms,
                    "consecutive_failures": server.consecutive_failures,
                    "is_backup": server.is_backup
                }
        
        # Recent metrics
        recent_metrics = self.traffic_metrics[-1] if self.traffic_metrics else None
        
        return {
            "load_balancer_instances": {
                "total": len(self.lb_instances),
                "healthy": healthy_instances,
                "status": lb_status
            },
            "backend_servers": backend_status,
            "current_metrics": {
                "requests_per_second": recent_metrics.requests_per_second if recent_metrics else 0,
                "active_connections": recent_metrics.active_connections if recent_metrics else 0,
                "error_rate": recent_metrics.error_rate if recent_metrics else 0,
                "avg_response_time_ms": recent_metrics.avg_response_time_ms if recent_metrics else 0
            } if recent_metrics else None,
            "monitoring_config": {
                "check_interval_seconds": self.check_interval,
                "failure_threshold": self.failure_threshold,
                "timeout_seconds": self.timeout
            }
        }
```

## TDD Implementation Cycle

### Red Phase: Load Balancer HA Test Creation
```python
# infrastructure/tests/test_load_balancer_ha.py
import pytest
import asyncio
from infrastructure.load_balancer.health_monitor import LoadBalancerHealthMonitor, ServerStatus

@pytest.mark.asyncio
async def test_load_balancer_health_monitor_initialization():
    """Test load balancer health monitor initializes correctly"""
    # This test should initially fail (Red phase)
    assert False, "Load balancer health monitor initialization not implemented yet"

@pytest.mark.asyncio
async def test_backend_server_health_checking():
    """Test backend server health checking"""
    # This test should initially fail (Red phase)
    assert False, "Backend server health checking not implemented yet"

@pytest.mark.asyncio
async def test_haproxy_statistics_parsing():
    """Test HAProxy statistics parsing"""
    # This test should initially fail (Red phase)
    assert False, "HAProxy statistics parsing not implemented yet"

@pytest.mark.asyncio
async def test_traffic_metrics_collection():
    """Test traffic metrics collection"""
    # This test should initially fail (Red phase)
    assert False, "Traffic metrics collection not implemented yet"

@pytest.mark.asyncio
async def test_load_balancer_failover():
    """Test load balancer failover scenarios"""
    # This test should initially fail (Red phase)
    assert False, "Load balancer failover not implemented yet"
```

### Green Phase: Load Balancer HA Implementation
```python
# Implement load balancer HA features to make tests pass
# This involves adding HAProxy configuration, health monitoring, and failover logic
```

### Refactor Phase: Load Balancer HA Optimization
```python
# Optimize load balancer HA for performance and reliability
# Add advanced load balancing algorithms and enhanced monitoring
# Improve error handling and failover procedures
```

## Security Checklist ✅

### Load Balancer Security
- [ ] SSL/TLS configuration with strong ciphers and protocols
- [ ] Security headers implementation (HSTS, CSP, etc.)
- [ ] Rate limiting and DDoS protection configuration
- [ ] Access controls for administrative interfaces
- [ ] Secure certificate management and rotation
- [ ] Protection against SSL/TLS vulnerabilities
- [ ] Load balancer configuration file security
- [ ] Network security for load balancer communications
- [ ] Security logging and monitoring
- [ ] Regular security updates and patching

### Traffic Security
- [ ] HTTP to HTTPS redirection enforcement
- [ ] Request size limits and input validation
- [ ] Protection against common web attacks (XSS, CSRF, etc.)
- [ ] Secure session handling and cookie management
- [ ] IP-based access controls and whitelisting
- [ ] Geographic traffic filtering if required
- [ ] Content filtering and malware protection
- [ ] Request logging for security analysis
- [ ] Traffic encryption between load balancer and backends
- [ ] Protection against request smuggling attacks

### High Availability Security
- [ ] Failover process security validation
- [ ] Backup load balancer security configuration
- [ ] Secure communication between HA components
- [ ] Protection against split-brain scenarios
- [ ] Health check endpoint security
- [ ] Monitoring system security and access controls
- [ ] Secure configuration synchronization
- [ ] HA system audit logging and monitoring
- [ ] Emergency access procedures security
- [ ] Regular security assessment of HA infrastructure

### Administrative Security
- [ ] Administrative interface access controls and MFA
- [ ] Statistics and monitoring interface security
- [ ] Configuration management security procedures
- [ ] Secure backup and recovery of load balancer configuration
- [ ] Change management security for load balancer updates
- [ ] Incident response procedures for load balancer security
- [ ] Security training for operations team
- [ ] Documentation security and access controls
- [ ] Vendor security assessment and management
- [ ] Compliance validation for load balancer security

## Performance Requirements

### Load Balancing Performance
- Request processing latency < 1ms overhead
- SSL termination processing < 5ms
- Connection handling > 100,000 concurrent connections
- Throughput > 1M requests/minute
- Memory usage optimization for high-traffic scenarios
- CPU usage efficiency for load balancing operations

### High Availability Performance
- Health check response time < 100ms
- Failover detection time < 10 seconds
- Traffic redirection time < 5 seconds
- Backend server recovery detection < 30 seconds
- Monitoring overhead < 1% of system resources
- Configuration reload time < 1 second

### Scalability Targets
- Support 1000+ backend servers
- Handle 10x traffic spikes
- Scale to multiple geographic regions
- Auto-scaling response time < 2 minutes
- Load balancer cluster expansion capability
- Resource efficiency > 85%

## Commit Instructions

After implementing the load balancer HA system:

```bash
git add infrastructure/load_balancer/
git commit -m "Add HAProxy high availability with intelligent traffic distribution

- Implement comprehensive HAProxy configuration with SSL termination
- Add LoadBalancerHealthMonitor with backend server health checking
- Include traffic metrics collection and statistics parsing
- Add security headers and rate limiting configuration
- Implement Docker Compose setup for HA load balancer deployment
- Add WebSocket and API traffic routing with session affinity
- Include health monitoring endpoints and statistics interface
- Add backup load balancer configuration and failover procedures
- Implement comprehensive security measures and access controls
- Add TDD cycle with Red-Green-Refactor for load balancer HA
- Ensure >90% load balancer HA test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete load balancer HA test suite:

```bash
# Run all load balancer HA tests
pytest infrastructure/tests/test_load_balancer_ha.py -v --timeout=300

# Run specific load balancer HA test categories
pytest infrastructure/tests/load_balancer/ -k "health_monitoring" -v
pytest infrastructure/tests/load_balancer/ -k "traffic_distribution" -v
pytest infrastructure/tests/load_balancer/ -k "failover" -v

# Run load balancer performance tests
pytest infrastructure/tests/load_balancer/performance/ -v

# Run load balancer integration tests
pytest infrastructure/tests/load_balancer/test_integration.py -v
```

Validate load balancer HA test coverage:
```bash
pytest infrastructure/tests/load_balancer/ --cov=infrastructure.load_balancer --cov-report=html --cov-fail-under=90
```

## Integration Testing

Test load balancer HA integration with platform components:
```bash
# Test integration with Session 8 (Frontend)
pytest infrastructure/tests/integration/test_load_balancer_frontend_integration.py -v

# Test integration with Session 12 (Rate Limiting)
pytest infrastructure/tests/integration/test_load_balancer_rate_limiting_integration.py -v

# Test load balancer HA with complete infrastructure stack
pytest infrastructure/tests/integration/test_load_balancer_full_stack_integration.py -v
```