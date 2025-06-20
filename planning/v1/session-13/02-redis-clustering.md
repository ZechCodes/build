# Session 13.2: Redis High Availability & Sentinel Clustering

## Objective
Implement Redis high availability through Sentinel clustering with automatic master election, ensuring session persistence and cache availability with seamless failover and data consistency across all platform components.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for Redis cluster monitoring and failover event tracking
- **Session 6**: Ensures session state persistence during Redis failovers
- **Session 12**: Maintains rate limiting data availability during Redis cluster changes
- **Session 13.1**: Coordinates with database HA for complete infrastructure resilience
- **All Sessions**: Provides Redis-based caching and session storage high availability

## Core Implementation

### Redis Sentinel Cluster Configuration
**Location**: `infrastructure/redis/redis-ha.yml`

```yaml
# infrastructure/redis/redis-ha.yml
version: '3.8'

services:
  redis-master:
    image: redis:7-alpine
    container_name: redis-master
    command: redis-server /etc/redis/redis.conf
    volumes:
      - redis_master_data:/data
      - ./configs/redis-master.conf:/etc/redis/redis.conf
      - ./configs/redis-init.sh:/usr/local/bin/redis-init.sh
      - ./ssl:/etc/ssl/redis
    environment:
      - REDIS_REPLICATION_MODE=master
      - REDIS_PASSWORD=${REDIS_PASSWORD}
      - REDIS_MASTER_PASSWORD=${REDIS_PASSWORD}
    networks:
      redis_network:
        ipv4_address: 172.21.0.10
    ports:
      - "6379:6379"
    healthcheck:
      test: ["CMD", "redis-cli", "-a", "${REDIS_PASSWORD}", "ping"]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 30s
    deploy:
      resources:
        limits:
          memory: 512M
          cpus: '0.5'
        reservations:
          memory: 256M
          cpus: '0.25'
    restart: unless-stopped

  redis-slave-1:
    image: redis:7-alpine
    container_name: redis-slave-1
    command: redis-server /etc/redis/redis.conf
    volumes:
      - redis_slave1_data:/data
      - ./configs/redis-slave.conf:/etc/redis/redis.conf
      - ./ssl:/etc/ssl/redis
    environment:
      - REDIS_REPLICATION_MODE=slave
      - REDIS_MASTER_HOST=redis-master
      - REDIS_MASTER_PORT=6379
      - REDIS_PASSWORD=${REDIS_PASSWORD}
      - REDIS_MASTER_PASSWORD=${REDIS_PASSWORD}
    networks:
      redis_network:
        ipv4_address: 172.21.0.11
    ports:
      - "6380:6379"
    depends_on:
      redis-master:
        condition: service_healthy
    healthcheck:
      test: ["CMD", "redis-cli", "-a", "${REDIS_PASSWORD}", "ping"]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 30s
    deploy:
      resources:
        limits:
          memory: 512M
          cpus: '0.5'
        reservations:
          memory: 256M
          cpus: '0.25'
    restart: unless-stopped

  redis-slave-2:
    image: redis:7-alpine
    container_name: redis-slave-2
    command: redis-server /etc/redis/redis.conf
    volumes:
      - redis_slave2_data:/data
      - ./configs/redis-slave.conf:/etc/redis/redis.conf
      - ./ssl:/etc/ssl/redis
    environment:
      - REDIS_REPLICATION_MODE=slave
      - REDIS_MASTER_HOST=redis-master
      - REDIS_MASTER_PORT=6379
      - REDIS_PASSWORD=${REDIS_PASSWORD}
      - REDIS_MASTER_PASSWORD=${REDIS_PASSWORD}
    networks:
      redis_network:
        ipv4_address: 172.21.0.12
    ports:
      - "6381:6379"
    depends_on:
      redis-master:
        condition: service_healthy
    healthcheck:
      test: ["CMD", "redis-cli", "-a", "${REDIS_PASSWORD}", "ping"]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 30s
    deploy:
      resources:
        limits:
          memory: 512M
          cpus: '0.5'
        reservations:
          memory: 256M
          cpus: '0.25'
    restart: unless-stopped

  redis-sentinel-1:
    image: redis:7-alpine
    container_name: redis-sentinel-1
    command: redis-sentinel /etc/redis/sentinel.conf
    volumes:
      - redis_sentinel1_data:/data
      - ./configs/sentinel.conf:/etc/redis/sentinel.conf
      - ./ssl:/etc/ssl/redis
    environment:
      - REDIS_MASTER_NAME=mymaster
      - REDIS_MASTER_HOST=redis-master
      - REDIS_MASTER_PORT=6379
      - REDIS_PASSWORD=${REDIS_PASSWORD}
      - REDIS_SENTINEL_QUORUM=2
    networks:
      redis_network:
        ipv4_address: 172.21.0.20
    ports:
      - "26379:26379"
    depends_on:
      - redis-master
      - redis-slave-1
      - redis-slave-2
    healthcheck:
      test: ["CMD", "redis-cli", "-p", "26379", "ping"]
      interval: 10s
      timeout: 5s
      retries: 5
    deploy:
      resources:
        limits:
          memory: 128M
          cpus: '0.25'
        reservations:
          memory: 64M
          cpus: '0.1'
    restart: unless-stopped

  redis-sentinel-2:
    image: redis:7-alpine
    container_name: redis-sentinel-2
    command: redis-sentinel /etc/redis/sentinel.conf
    volumes:
      - redis_sentinel2_data:/data
      - ./configs/sentinel.conf:/etc/redis/sentinel.conf
      - ./ssl:/etc/ssl/redis
    environment:
      - REDIS_MASTER_NAME=mymaster
      - REDIS_MASTER_HOST=redis-master
      - REDIS_MASTER_PORT=6379
      - REDIS_PASSWORD=${REDIS_PASSWORD}
      - REDIS_SENTINEL_QUORUM=2
    networks:
      redis_network:
        ipv4_address: 172.21.0.21
    ports:
      - "26380:26379"
    depends_on:
      - redis-master
      - redis-slave-1
      - redis-slave-2
    healthcheck:
      test: ["CMD", "redis-cli", "-p", "26379", "ping"]
      interval: 10s
      timeout: 5s
      retries: 5
    deploy:
      resources:
        limits:
          memory: 128M
          cpus: '0.25'
        reservations:
          memory: 64M
          cpus: '0.1'
    restart: unless-stopped

  redis-sentinel-3:
    image: redis:7-alpine
    container_name: redis-sentinel-3
    command: redis-sentinel /etc/redis/sentinel.conf
    volumes:
      - redis_sentinel3_data:/data
      - ./configs/sentinel.conf:/etc/redis/sentinel.conf
      - ./ssl:/etc/ssl/redis
    environment:
      - REDIS_MASTER_NAME=mymaster
      - REDIS_MASTER_HOST=redis-master
      - REDIS_MASTER_PORT=6379
      - REDIS_PASSWORD=${REDIS_PASSWORD}
      - REDIS_SENTINEL_QUORUM=2
    networks:
      redis_network:
        ipv4_address: 172.21.0.22
    ports:
      - "26381:26379"
    depends_on:
      - redis-master
      - redis-slave-1
      - redis-slave-2
    healthcheck:
      test: ["CMD", "redis-cli", "-p", "26379", "ping"]
      interval: 10s
      timeout: 5s
      retries: 5
    deploy:
      resources:
        limits:
          memory: 128M
          cpus: '0.25'
        reservations:
          memory: 64M
          cpus: '0.1'
    restart: unless-stopped

volumes:
  redis_master_data:
    driver: local
    driver_opts:
      type: none
      o: bind
      device: /var/lib/redis/master
  redis_slave1_data:
    driver: local
    driver_opts:
      type: none
      o: bind
      device: /var/lib/redis/slave1
  redis_slave2_data:
    driver: local
    driver_opts:
      type: none
      o: bind
      device: /var/lib/redis/slave2
  redis_sentinel1_data:
    driver: local
  redis_sentinel2_data:
    driver: local
  redis_sentinel3_data:
    driver: local

networks:
  redis_network:
    driver: bridge
    ipam:
      config:
        - subnet: 172.21.0.0/16
```

### Redis Configuration Files
**Location**: `infrastructure/redis/configs/`

```conf
# infrastructure/redis/configs/redis-master.conf
# Redis Master Configuration

# Network
bind 0.0.0.0
port 6379
protected-mode yes
tcp-backlog 511
tcp-keepalive 300

# Security
requirepass ${REDIS_PASSWORD}
masterauth ${REDIS_PASSWORD}

# SSL/TLS Configuration
tls-port 6380
tls-cert-file /etc/ssl/redis/redis.crt
tls-key-file /etc/ssl/redis/redis.key
tls-ca-cert-file /etc/ssl/redis/ca.crt
tls-protocols "TLSv1.2 TLSv1.3"

# General
daemonize no
supervised no
pidfile /var/run/redis_6379.pid
loglevel notice
logfile "/var/log/redis/redis-server.log"
databases 16

# Snapshotting
save 900 1
save 300 10
save 60 10000
stop-writes-on-bgsave-error yes
rdbcompression yes
rdbchecksum yes
dbfilename dump.rdb
dir /data

# Replication
replica-serve-stale-data yes
replica-read-only yes
repl-diskless-sync no
repl-diskless-sync-delay 5
repl-ping-replica-period 10
repl-timeout 60
repl-disable-tcp-nodelay no
repl-backlog-size 1mb
repl-backlog-ttl 3600
replica-priority 100
min-replicas-to-write 1
min-replicas-max-lag 10

# Memory Management
maxmemory 256mb
maxmemory-policy allkeys-lru
maxmemory-samples 5

# Lazy Freeing
lazyfree-lazy-eviction no
lazyfree-lazy-expire no
lazyfree-lazy-server-del no
replica-lazy-flush no

# Append Only File
appendonly yes
appendfilename "appendonly.aof"
appendfsync everysec
no-appendfsync-on-rewrite no
auto-aof-rewrite-percentage 100
auto-aof-rewrite-min-size 64mb
aof-load-truncated yes
aof-use-rdb-preamble yes

# Lua scripting
lua-time-limit 5000

# Redis slow log
slowlog-log-slower-than 10000
slowlog-max-len 128

# Latency monitor
latency-monitor-threshold 100

# Event notification
notify-keyspace-events ""

# Hashes
hash-max-ziplist-entries 512
hash-max-ziplist-value 64

# Lists
list-max-ziplist-size -2
list-compress-depth 0

# Sets
set-max-intset-entries 512

# Sorted Sets
zset-max-ziplist-entries 128
zset-max-ziplist-value 64

# HyperLogLog
hll-sparse-max-bytes 3000

# Streams
stream-node-max-bytes 4096
stream-node-max-entries 100

# Active rehashing
activerehashing yes

# Client output buffer limits
client-output-buffer-limit normal 0 0 0
client-output-buffer-limit replica 256mb 64mb 60
client-output-buffer-limit pubsub 32mb 8mb 60

# Client query buffer limit
client-query-buffer-limit 1gb

# Protocol
proto-max-bulk-len 512mb

# Frequency
hz 10

# ACL
aclfile /etc/redis/users.acl

# GEOIP
# geoip-db /path/to/GeoLite2-City.mmdb
```

```conf
# infrastructure/redis/configs/redis-slave.conf
# Redis Slave Configuration

# Network
bind 0.0.0.0
port 6379
protected-mode yes
tcp-backlog 511
tcp-keepalive 300

# Security
requirepass ${REDIS_PASSWORD}
masterauth ${REDIS_MASTER_PASSWORD}

# SSL/TLS Configuration
tls-port 6380
tls-cert-file /etc/ssl/redis/redis.crt
tls-key-file /etc/ssl/redis/redis.key
tls-ca-cert-file /etc/ssl/redis/ca.crt
tls-protocols "TLSv1.2 TLSv1.3"

# General
daemonize no
supervised no
pidfile /var/run/redis_6379.pid
loglevel notice
logfile "/var/log/redis/redis-server.log"
databases 16

# Replication
replicaof ${REDIS_MASTER_HOST} ${REDIS_MASTER_PORT}
replica-serve-stale-data yes
replica-read-only yes
repl-diskless-sync no
repl-diskless-sync-delay 5
repl-ping-replica-period 10
repl-timeout 60
repl-disable-tcp-nodelay no
replica-priority 100

# Snapshotting (reduced for slaves)
save 1800 1
save 600 10
save 120 10000
stop-writes-on-bgsave-error yes
rdbcompression yes
rdbchecksum yes
dbfilename dump.rdb
dir /data

# Memory Management
maxmemory 128mb
maxmemory-policy allkeys-lru
maxmemory-samples 5

# Append Only File
appendonly yes
appendfilename "appendonly.aof"
appendfsync everysec
no-appendfsync-on-rewrite no
auto-aof-rewrite-percentage 100
auto-aof-rewrite-min-size 32mb
aof-load-truncated yes
aof-use-rdb-preamble yes

# Other settings same as master but optimized for replica role
lua-time-limit 5000
slowlog-log-slower-than 10000
slowlog-max-len 128
latency-monitor-threshold 100
notify-keyspace-events ""
hz 10
```

```conf
# infrastructure/redis/configs/sentinel.conf
# Redis Sentinel Configuration

port 26379
bind 0.0.0.0
protected-mode no

# Sentinel announce settings
sentinel announce-ip 172.21.0.20
sentinel announce-port 26379

# Master monitoring
sentinel monitor mymaster ${REDIS_MASTER_HOST} ${REDIS_MASTER_PORT} ${REDIS_SENTINEL_QUORUM}
sentinel auth-pass mymaster ${REDIS_PASSWORD}
sentinel down-after-milliseconds mymaster 5000
sentinel parallel-syncs mymaster 1
sentinel failover-timeout mymaster 60000
sentinel deny-scripts-reconfig yes

# Notification scripts
# sentinel notification-script mymaster /etc/redis/notify.sh
# sentinel client-reconfig-script mymaster /etc/redis/reconfig.sh

# Logging
loglevel notice
logfile "/var/log/redis/sentinel.log"
syslog-enabled yes
syslog-ident sentinel
syslog-facility local0

# ACL for Sentinel
# requirepass ${REDIS_SENTINEL_PASSWORD}

# Sentinel working directory
dir /tmp

# How many sentinels need to agree about the fact the master is not reachable
# in order to really mark the master as failing
sentinel down-after-milliseconds mymaster 5000

# How many replicas we can reconfigure to point to the new master simultaneously
# during the failover. Use a low number if you have a slow network
sentinel parallel-syncs mymaster 1

# Specifies the failover timeout in milliseconds
sentinel failover-timeout mymaster 60000

# The maximum time a sentinel can be disconnected from a Redis instance 
# (either master or replica) before it is considered to be down
sentinel ping-timeout mymaster 30000

# When to start the failover
sentinel quorum-timeout mymaster 5000
```

### Redis Sentinel Manager
**Location**: `infrastructure/redis/sentinel_manager.py`

```python
# infrastructure/redis/sentinel_manager.py
import asyncio
import time
import json
from typing import Dict, List, Optional, Any, Tuple
from dataclasses import dataclass, field
from enum import Enum
import structlog
import logfire
import redis.asyncio as redis
from redis.sentinel import Sentinel
from redis.exceptions import ConnectionError, TimeoutError, RedisError

logger = structlog.get_logger()

class RedisRole(Enum):
    MASTER = "master"
    SLAVE = "slave"
    SENTINEL = "sentinel"
    UNKNOWN = "unknown"

class FailoverReason(Enum):
    MASTER_DOWN = "master_down"
    MANUAL_FAILOVER = "manual_failover"
    NETWORK_PARTITION = "network_partition"
    MAINTENANCE = "maintenance"
    SENTINEL_DECISION = "sentinel_decision"

@dataclass
class RedisNode:
    node_id: str
    host: str
    port: int
    role: RedisRole
    is_healthy: bool = True
    last_ping: Optional[float] = None
    info: Dict[str, Any] = field(default_factory=dict)
    connection_pool: Optional[redis.ConnectionPool] = None
    
@dataclass
class SentinelNode:
    host: str
    port: int
    is_healthy: bool = True
    last_check: Optional[float] = None
    master_info: Optional[Dict[str, Any]] = None

@dataclass
class RedisFailoverEvent:
    event_id: str
    reason: FailoverReason
    old_master: Optional[Dict[str, Any]]
    new_master: Optional[Dict[str, Any]]
    sentinels_involved: List[str]
    started_at: float
    completed_at: Optional[float] = None
    success: bool = False
    error_message: Optional[str] = None
    downtime_ms: Optional[float] = None

class RedisSentinelManager:
    def __init__(self, sentinel_hosts: List[Tuple[str, int]], 
                 master_name: str = "mymaster",
                 password: Optional[str] = None,
                 notification_service=None):
        
        self.sentinel_hosts = sentinel_hosts
        self.master_name = master_name
        self.password = password
        self.notification_service = notification_service
        
        # Sentinel and Redis connections
        self.sentinel = None
        self.master_client = None
        self.slave_clients: List[redis.Redis] = []
        
        # Node tracking
        self.sentinel_nodes: Dict[str, SentinelNode] = {}
        self.redis_nodes: Dict[str, RedisNode] = {}
        self.current_master: Optional[Dict[str, Any]] = None
        
        # Monitoring configuration
        self.monitoring_task: Optional[asyncio.Task] = None
        self.check_interval = 10  # seconds
        self.connection_timeout = 5
        self.health_check_timeout = 3
        
        # Event tracking
        self.failover_history: List[RedisFailoverEvent] = []
        self.health_metrics: Dict[str, List[Dict[str, Any]]] = {}
        
        # State
        self.is_monitoring = False
        self.last_master_check = 0
        
    async def initialize(self):
        """Initialize the Redis Sentinel manager"""
        try:
            # Initialize sentinel nodes tracking
            for host, port in self.sentinel_hosts:
                node_key = f"{host}:{port}"
                self.sentinel_nodes[node_key] = SentinelNode(host=host, port=port)
            
            # Create Sentinel connection
            await self._create_sentinel_connection()
            
            # Discover current topology
            await self._discover_redis_topology()
            
            # Initialize Redis connections
            await self._initialize_redis_connections()
            
            # Start monitoring
            self.monitoring_task = asyncio.create_task(self._monitoring_loop())
            self.is_monitoring = True
            
            logfire.info("Redis Sentinel manager initialized",
                       master_name=self.master_name,
                       sentinel_count=len(self.sentinel_hosts),
                       current_master=self.current_master)
            
            logger.info("Redis Sentinel manager initialized",
                      master_name=self.master_name,
                      sentinels=len(self.sentinel_hosts))
            
        except Exception as e:
            logger.error("Failed to initialize Redis Sentinel manager", error=str(e))
            raise
    
    async def _create_sentinel_connection(self):
        """Create connection to Redis Sentinel cluster"""
        try:
            self.sentinel = Sentinel(
                self.sentinel_hosts,
                password=self.password,
                socket_timeout=self.connection_timeout,
                socket_connect_timeout=self.connection_timeout
            )
            
            # Test sentinel connectivity
            sentinels_info = await self._get_sentinels_info()
            logger.info("Connected to Redis Sentinel cluster", 
                      active_sentinels=len(sentinels_info))
            
        except Exception as e:
            logger.error("Failed to connect to Redis Sentinel", error=str(e))
            raise
    
    async def _discover_redis_topology(self):
        """Discover current Redis cluster topology"""
        try:
            # Get master info from sentinel
            master_info = await self._get_master_info()
            if master_info:
                self.current_master = master_info
                
                master_key = f"{master_info['ip']}:{master_info['port']}"
                self.redis_nodes[master_key] = RedisNode(
                    node_id=master_key,
                    host=master_info['ip'],
                    port=int(master_info['port']),
                    role=RedisRole.MASTER,
                    info=master_info
                )
            
            # Get slaves info
            slaves_info = await self._get_slaves_info()
            for slave_info in slaves_info:
                slave_key = f"{slave_info['ip']}:{slave_info['port']}"
                self.redis_nodes[slave_key] = RedisNode(
                    node_id=slave_key,
                    host=slave_info['ip'],
                    port=int(slave_info['port']),
                    role=RedisRole.SLAVE,
                    info=slave_info
                )
            
            logger.info("Redis topology discovered",
                      master=self.current_master,
                      slaves_count=len(slaves_info))
            
        except Exception as e:
            logger.error("Failed to discover Redis topology", error=str(e))
            raise
    
    async def _initialize_redis_connections(self):
        """Initialize connections to Redis master and slaves"""
        try:
            # Create master connection
            if self.current_master:
                self.master_client = self.sentinel.master_for(
                    self.master_name,
                    password=self.password,
                    socket_timeout=self.connection_timeout
                )
                
                # Test master connection
                await self.master_client.ping()
                logger.debug("Master connection established")
            
            # Create slave connections
            slaves = await self._get_slaves_info()
            for slave_info in slaves:
                try:
                    slave_client = redis.Redis(
                        host=slave_info['ip'],
                        port=int(slave_info['port']),
                        password=self.password,
                        socket_timeout=self.connection_timeout
                    )
                    await slave_client.ping()
                    self.slave_clients.append(slave_client)
                    logger.debug("Slave connection established", 
                               host=slave_info['ip'], port=slave_info['port'])
                    
                except Exception as e:
                    logger.warning("Failed to connect to slave",
                                 host=slave_info['ip'], 
                                 port=slave_info['port'], 
                                 error=str(e))
            
        except Exception as e:
            logger.error("Failed to initialize Redis connections", error=str(e))
            raise
    
    async def stop(self):
        """Stop the Redis Sentinel manager"""
        try:
            self.is_monitoring = False
            
            # Stop monitoring task
            if self.monitoring_task and not self.monitoring_task.done():
                self.monitoring_task.cancel()
                try:
                    await self.monitoring_task
                except asyncio.CancelledError:
                    pass
            
            # Close Redis connections
            if self.master_client:
                await self.master_client.close()
            
            for slave_client in self.slave_clients:
                await slave_client.close()
            
            logger.info("Redis Sentinel manager stopped")
            
        except Exception as e:
            logger.error("Error stopping Redis Sentinel manager", error=str(e))
    
    async def _monitoring_loop(self):
        """Main monitoring loop for Redis cluster health"""
        while self.is_monitoring:
            try:
                await asyncio.sleep(self.check_interval)
                
                # Check sentinel health
                await self._check_sentinels_health()
                
                # Check Redis nodes health
                await self._check_redis_nodes_health()
                
                # Check for topology changes
                await self._check_topology_changes()
                
                # Record health metrics
                await self._record_health_metrics()
                
                # Detect potential issues
                await self._detect_potential_issues()
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Redis monitoring loop error", error=str(e))
                await asyncio.sleep(5)  # Brief pause before retry
    
    async def _check_sentinels_health(self):
        """Check health of all Sentinel nodes"""
        for node_key, sentinel_node in self.sentinel_nodes.items():
            try:
                # Create direct connection to this sentinel
                sentinel_client = redis.Redis(
                    host=sentinel_node.host,
                    port=sentinel_node.port,
                    socket_timeout=self.health_check_timeout
                )
                
                # Ping sentinel
                await sentinel_client.ping()
                
                # Get master info from this sentinel
                master_info = await sentinel_client.execute_command(
                    "SENTINEL", "masters"
                )
                
                # Update sentinel health
                sentinel_node.is_healthy = True
                sentinel_node.last_check = time.time()
                sentinel_node.master_info = master_info
                
                await sentinel_client.close()
                
            except Exception as e:
                sentinel_node.is_healthy = False
                sentinel_node.last_check = time.time()
                
                logger.warning("Sentinel health check failed",
                             host=sentinel_node.host,
                             port=sentinel_node.port,
                             error=str(e))
    
    async def _check_redis_nodes_health(self):
        """Check health of all Redis nodes"""
        for node_key, redis_node in self.redis_nodes.items():
            try:
                # Create connection to Redis node
                redis_client = redis.Redis(
                    host=redis_node.host,
                    port=redis_node.port,
                    password=self.password,
                    socket_timeout=self.health_check_timeout
                )
                
                start_time = time.time()
                
                # Ping Redis node
                await redis_client.ping()
                
                # Get node info
                info = await redis_client.info()
                
                response_time = (time.time() - start_time) * 1000
                
                # Update node health
                redis_node.is_healthy = True
                redis_node.last_ping = time.time()
                redis_node.info = info
                
                await redis_client.close()
                
                logfire.debug("Redis node health check successful",
                            node_id=redis_node.node_id,
                            role=redis_node.role.value,
                            response_time_ms=response_time)
                
            except Exception as e:
                redis_node.is_healthy = False
                redis_node.last_ping = time.time()
                
                logger.warning("Redis node health check failed",
                             node_id=redis_node.node_id,
                             host=redis_node.host,
                             port=redis_node.port,
                             error=str(e))
                
                logfire.warning("Redis node unhealthy",
                              node_id=redis_node.node_id,
                              error=str(e))
    
    async def _check_topology_changes(self):
        """Check for Redis topology changes"""
        try:
            # Get current master from sentinel
            current_master = await self._get_master_info()
            
            if current_master and self.current_master:
                # Check if master has changed
                if (current_master['ip'] != self.current_master['ip'] or 
                    current_master['port'] != self.current_master['port']):
                    
                    # Master has changed - failover occurred
                    await self._handle_master_change(current_master)
            
            elif current_master and not self.current_master:
                # Master came online
                logger.info("Master came online", master=current_master)
                self.current_master = current_master
                await self._initialize_redis_connections()
            
            elif not current_master and self.current_master:
                # Master went offline
                logger.error("Master went offline", old_master=self.current_master)
                self.current_master = None
            
        except Exception as e:
            logger.error("Failed to check topology changes", error=str(e))
    
    async def _handle_master_change(self, new_master: Dict[str, Any]):
        """Handle master change event"""
        old_master = self.current_master
        
        # Create failover event
        event_id = f"redis_failover_{int(time.time())}"
        failover_event = RedisFailoverEvent(
            event_id=event_id,
            reason=FailoverReason.SENTINEL_DECISION,
            old_master=old_master,
            new_master=new_master,
            sentinels_involved=[f"{h}:{p}" for h, p in self.sentinel_hosts],
            started_at=time.time()
        )
        
        logger.critical("Redis master failover detected",
                       event_id=event_id,
                       old_master=old_master,
                       new_master=new_master)
        
        logfire.error("Redis master failover",
                    event_id=event_id,
                    old_master_host=old_master['ip'] if old_master else None,
                    new_master_host=new_master['ip'])
        
        try:
            # Update current master
            self.current_master = new_master
            
            # Reinitialize connections
            await self._initialize_redis_connections()
            
            # Update node roles
            await self._update_node_roles_after_failover(new_master)
            
            # Mark failover as successful
            failover_event.completed_at = time.time()
            failover_event.success = True
            failover_event.downtime_ms = (failover_event.completed_at - failover_event.started_at) * 1000
            
            logger.info("Redis failover handling completed",
                      event_id=event_id,
                      downtime_ms=failover_event.downtime_ms)
            
            # Send notification
            if self.notification_service:
                await self.notification_service.send_critical_alert(
                    f"Redis master failover: {old_master['ip'] if old_master else 'unknown'} -> {new_master['ip']}",
                    {
                        "event_id": event_id,
                        "old_master": old_master,
                        "new_master": new_master,
                        "downtime_ms": failover_event.downtime_ms
                    }
                )
            
        except Exception as e:
            failover_event.completed_at = time.time()
            failover_event.success = False
            failover_event.error_message = str(e)
            
            logger.error("Failed to handle Redis master change", 
                       event_id=event_id, error=str(e))
        
        finally:
            self.failover_history.append(failover_event)
    
    async def _get_master_info(self) -> Optional[Dict[str, Any]]:
        """Get master information from Sentinel"""
        try:
            if not self.sentinel:
                await self._create_sentinel_connection()
            
            master_info = self.sentinel.sentinel_masters()
            return master_info.get(self.master_name)
            
        except Exception as e:
            logger.error("Failed to get master info from Sentinel", error=str(e))
            return None
    
    async def _get_slaves_info(self) -> List[Dict[str, Any]]:
        """Get slaves information from Sentinel"""
        try:
            if not self.sentinel:
                await self._create_sentinel_connection()
            
            return self.sentinel.sentinel_slaves(self.master_name)
            
        except Exception as e:
            logger.error("Failed to get slaves info from Sentinel", error=str(e))
            return []
    
    async def _get_sentinels_info(self) -> List[Dict[str, Any]]:
        """Get sentinels information"""
        try:
            if not self.sentinel:
                await self._create_sentinel_connection()
            
            return self.sentinel.sentinel_sentinels(self.master_name)
            
        except Exception as e:
            logger.error("Failed to get sentinels info", error=str(e))
            return []
    
    def get_cluster_status(self) -> Dict[str, Any]:
        """Get comprehensive Redis cluster status"""
        # Sentinel status
        sentinel_status = {}
        healthy_sentinels = 0
        for node_key, sentinel_node in self.sentinel_nodes.items():
            sentinel_status[node_key] = {
                "is_healthy": sentinel_node.is_healthy,
                "last_check": sentinel_node.last_check
            }
            if sentinel_node.is_healthy:
                healthy_sentinels += 1
        
        # Redis nodes status
        redis_status = {}
        for node_key, redis_node in self.redis_nodes.items():
            redis_status[node_key] = {
                "role": redis_node.role.value,
                "is_healthy": redis_node.is_healthy,
                "last_ping": redis_node.last_ping,
                "info_keys": list(redis_node.info.keys()) if redis_node.info else []
            }
        
        # Recent failovers
        recent_failovers = [
            {
                "event_id": event.event_id,
                "reason": event.reason.value,
                "success": event.success,
                "downtime_ms": event.downtime_ms,
                "started_at": event.started_at
            }
            for event in self.failover_history[-5:]  # Last 5 events
        ]
        
        return {
            "master_name": self.master_name,
            "current_master": self.current_master,
            "sentinel_nodes": {
                "total": len(self.sentinel_nodes),
                "healthy": healthy_sentinels,
                "status": sentinel_status
            },
            "redis_nodes": {
                "total": len(self.redis_nodes),
                "status": redis_status
            },
            "recent_failovers": recent_failovers,
            "is_monitoring": self.is_monitoring,
            "check_interval_seconds": self.check_interval
        }
```

## TDD Implementation Cycle

### Red Phase: Redis Clustering Test Creation
```python
# infrastructure/tests/test_redis_sentinel.py
import pytest
import asyncio
from infrastructure.redis.sentinel_manager import RedisSentinelManager, RedisRole

@pytest.mark.asyncio
async def test_sentinel_manager_initialization():
    """Test Redis Sentinel manager initializes correctly"""
    # This test should initially fail (Red phase)
    assert False, "Redis Sentinel manager initialization not implemented yet"

@pytest.mark.asyncio
async def test_redis_topology_discovery():
    """Test Redis cluster topology discovery"""
    # This test should initially fail (Red phase)
    assert False, "Redis topology discovery not implemented yet"

@pytest.mark.asyncio
async def test_master_failover_detection():
    """Test detection of Redis master failover"""
    # This test should initially fail (Red phase)
    assert False, "Master failover detection not implemented yet"

@pytest.mark.asyncio
async def test_sentinel_health_monitoring():
    """Test Sentinel node health monitoring"""
    # This test should initially fail (Red phase)
    assert False, "Sentinel health monitoring not implemented yet"

@pytest.mark.asyncio
async def test_redis_connection_management():
    """Test Redis connection management during failovers"""
    # This test should initially fail (Red phase)
    assert False, "Redis connection management not implemented yet"
```

### Green Phase: Redis Clustering Implementation
```python
# Implement Redis clustering features to make tests pass
# This involves adding Sentinel configuration, failover detection, and connection management
```

### Refactor Phase: Redis Clustering Optimization
```python
# Optimize Redis clustering for performance and reliability
# Add advanced failover strategies and enhanced monitoring
# Improve connection pooling and error handling
```

## Security Checklist ✅

### Redis Cluster Security
- [ ] Redis authentication with strong passwords
- [ ] SSL/TLS encryption for Redis connections
- [ ] Network security between Redis nodes
- [ ] Redis ACL configuration for user access control
- [ ] Secure Redis configuration file permissions
- [ ] Protection against Redis security vulnerabilities
- [ ] Redis data encryption at rest
- [ ] Secure Redis backup and restoration procedures
- [ ] Redis command filtering and dangerous command disabling
- [ ] Monitoring and logging of Redis security events

### Sentinel Security
- [ ] Sentinel authentication and authorization
- [ ] Secure Sentinel configuration and deployment
- [ ] Network security for Sentinel communications
- [ ] Protection against Sentinel compromise
- [ ] Secure Sentinel decision-making processes
- [ ] Sentinel logging and audit trail
- [ ] Protection against split-brain scenarios
- [ ] Secure Sentinel notification mechanisms
- [ ] Regular security assessment of Sentinel infrastructure
- [ ] Incident response procedures for Sentinel security events

### Data Protection
- [ ] Redis data backup encryption and secure storage
- [ ] Data replication integrity verification
- [ ] Secure data transfer between Redis nodes
- [ ] Data retention policy enforcement for Redis
- [ ] Secure data disposal procedures
- [ ] Compliance with data protection regulations
- [ ] Redis data access logging and monitoring
- [ ] Protection against data corruption attacks
- [ ] Cross-region data transfer security
- [ ] Redis data recovery testing and validation

### Operational Security
- [ ] Redis cluster administrative access controls
- [ ] Secure Redis cluster configuration management
- [ ] Protection against unauthorized Redis operations
- [ ] Secure Redis monitoring and alerting systems
- [ ] Automated Redis security updates and patching
- [ ] Redis cluster incident response procedures
- [ ] Vendor security assessment for Redis infrastructure
- [ ] Regular security testing of Redis deployment
- [ ] Training and awareness for Redis operations team
- [ ] Documentation security for Redis procedures

## Performance Requirements

### Redis Performance
- Redis master response time < 1ms for simple operations
- Replication lag < 100ms between master and slaves
- Sentinel decision time < 5 seconds for failover
- Connection pool efficiency > 95%
- Memory usage optimization for Redis instances
- Throughput > 100,000 operations/second

### Clustering Performance
- Failover detection time < 10 seconds
- Master promotion time < 30 seconds
- Application reconnection time < 5 seconds
- Cluster health check latency < 50ms
- Monitoring overhead < 1% of Redis performance
- Network bandwidth utilization < 70% for replication

### Availability Targets
- Redis cluster uptime > 99.9%
- Planned maintenance downtime < 30 seconds
- Unplanned outage recovery < 2 minutes
- Data consistency during failovers > 99.99%
- Sentinel quorum availability > 99.95%
- Cross-zone failover capability

## Commit Instructions

After implementing the Redis clustering system:

```bash
git add infrastructure/redis/
git commit -m "Add Redis high availability with Sentinel clustering

- Implement Redis Sentinel cluster with master-slave replication
- Add RedisSentinelManager with automated failover detection
- Include comprehensive Redis topology discovery and monitoring
- Add secure Redis configuration with SSL/TLS and authentication
- Implement connection management and automatic reconnection
- Add failover event tracking and notification integration
- Include Docker Compose configuration for Redis HA cluster
- Add health monitoring for Redis nodes and Sentinels
- Implement performance optimization and resource management
- Add TDD cycle with Red-Green-Refactor for Redis HA
- Ensure >90% Redis clustering test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete Redis clustering test suite:

```bash
# Run all Redis Sentinel tests
pytest infrastructure/tests/test_redis_sentinel.py -v --timeout=300

# Run specific Redis HA test categories
pytest infrastructure/tests/redis/ -k "sentinel" -v
pytest infrastructure/tests/redis/ -k "failover" -v
pytest infrastructure/tests/redis/ -k "topology" -v

# Run Redis HA performance tests
pytest infrastructure/tests/redis/performance/ -v

# Run Redis HA integration tests
pytest infrastructure/tests/redis/test_integration.py -v
```

Validate Redis HA test coverage:
```bash
pytest infrastructure/tests/redis/ --cov=infrastructure.redis --cov-report=html --cov-fail-under=90
```

## Integration Testing

Test Redis HA integration with platform components:
```bash
# Test integration with Session 6 (Session Management)
pytest infrastructure/tests/integration/test_redis_session_integration.py -v

# Test integration with Session 12 (Rate Limiting)
pytest infrastructure/tests/integration/test_redis_rate_limiting_integration.py -v

# Test Redis HA with database HA coordination
pytest infrastructure/tests/integration/test_redis_database_ha_integration.py -v
```