# Session 13.1: PostgreSQL High Availability & Streaming Replication

## Objective
Implement PostgreSQL streaming replication with automated failover management, ensuring database high availability with 99.9%+ uptime through primary-replica architecture, health monitoring, and seamless failover procedures.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for database replication monitoring and failover event logging
- **Session 2**: Ensures user authentication data remains available during database failovers
- **Session 3-12**: Maintains data availability for all platform components during database failures
- **All Sessions**: Provides foundational data layer high availability for entire platform

## Core Implementation

### PostgreSQL Streaming Replication Setup
**Location**: `infrastructure/database/postgresql-ha.yml`

```yaml
# infrastructure/database/postgresql-ha.yml
version: '3.8'

services:
  postgres-primary:
    image: postgres:16-alpine
    environment:
      POSTGRES_DB: build_platform
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD}
      POSTGRES_REPLICATION_USER: replicator
      POSTGRES_REPLICATION_PASSWORD: ${REPLICATION_PASSWORD}
    volumes:
      - postgres_primary_data:/var/lib/postgresql/data
      - ./postgresql-primary.conf:/etc/postgresql/postgresql.conf
      - ./pg_hba.conf:/etc/postgresql/pg_hba.conf
      - ./init-replication.sh:/docker-entrypoint-initdb.d/init-replication.sh
    command: >
      postgres
      -c config_file=/etc/postgresql/postgresql.conf
      -c hba_file=/etc/postgresql/pg_hba.conf
    networks:
      - database_network
    ports:
      - "5432:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres"]
      interval: 10s
      timeout: 5s
      retries: 5
    deploy:
      resources:
        limits:
          memory: 1G
          cpus: '1.0'
        reservations:
          memory: 512M
          cpus: '0.5'

  postgres-replica-1:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD}
      PGUSER: postgres
      POSTGRES_PRIMARY_HOST: postgres-primary
      POSTGRES_PRIMARY_PORT: 5432
      POSTGRES_REPLICATION_USER: replicator
      POSTGRES_REPLICATION_PASSWORD: ${REPLICATION_PASSWORD}
    volumes:
      - postgres_replica1_data:/var/lib/postgresql/data
      - ./postgresql-replica.conf:/etc/postgresql/postgresql.conf
      - ./setup-replica.sh:/docker-entrypoint-initdb.d/setup-replica.sh
    command: >
      bash -c '
        if [ ! -s "$$PGDATA/PG_VERSION" ]; then
          echo "Setting up replica from primary..."
          pg_basebackup -h $$POSTGRES_PRIMARY_HOST -D $$PGDATA -U $$POSTGRES_REPLICATION_USER -v -P -W
          echo "standby_mode = on" >> $$PGDATA/recovery.conf
          echo "primary_conninfo = 'host=$$POSTGRES_PRIMARY_HOST port=$$POSTGRES_PRIMARY_PORT user=$$POSTGRES_REPLICATION_USER'" >> $$PGDATA/recovery.conf
          echo "trigger_file = '/tmp/postgresql.trigger.5432'" >> $$PGDATA/recovery.conf
          echo "recovery_target_timeline = 'latest'" >> $$PGDATA/recovery.conf
        fi
        postgres -c config_file=/etc/postgresql/postgresql.conf
      '
    depends_on:
      postgres-primary:
        condition: service_healthy
    networks:
      - database_network
    ports:
      - "5433:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres"]
      interval: 10s
      timeout: 5s
      retries: 5
    deploy:
      resources:
        limits:
          memory: 1G
          cpus: '1.0'
        reservations:
          memory: 512M
          cpus: '0.5'

  postgres-replica-2:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD}
      PGUSER: postgres
      POSTGRES_PRIMARY_HOST: postgres-primary
      POSTGRES_PRIMARY_PORT: 5432
      POSTGRES_REPLICATION_USER: replicator
      POSTGRES_REPLICATION_PASSWORD: ${REPLICATION_PASSWORD}
    volumes:
      - postgres_replica2_data:/var/lib/postgresql/data
      - ./postgresql-replica.conf:/etc/postgresql/postgresql.conf
      - ./setup-replica.sh:/docker-entrypoint-initdb.d/setup-replica.sh
    command: >
      bash -c '
        if [ ! -s "$$PGDATA/PG_VERSION" ]; then
          echo "Setting up replica from primary..."
          pg_basebackup -h $$POSTGRES_PRIMARY_HOST -D $$PGDATA -U $$POSTGRES_REPLICATION_USER -v -P -W
          echo "standby_mode = on" >> $$PGDATA/recovery.conf
          echo "primary_conninfo = 'host=$$POSTGRES_PRIMARY_HOST port=$$POSTGRES_PRIMARY_PORT user=$$POSTGRES_REPLICATION_USER'" >> $$PGDATA/recovery.conf
          echo "trigger_file = '/tmp/postgresql.trigger.5432'" >> $$PGDATA/recovery.conf
          echo "recovery_target_timeline = 'latest'" >> $$PGDATA/recovery.conf
        fi
        postgres -c config_file=/etc/postgresql/postgresql.conf
      '
    depends_on:
      postgres-primary:
        condition: service_healthy
    networks:
      - database_network
    ports:
      - "5434:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres"]
      interval: 10s
      timeout: 5s
      retries: 5
    deploy:
      resources:
        limits:
          memory: 1G
          cpus: '1.0'
        reservations:
          memory: 512M
          cpus: '0.5'

volumes:
  postgres_primary_data:
    driver: local
    driver_opts:
      type: none
      o: bind
      device: /var/lib/postgresql/primary
  postgres_replica1_data:
    driver: local
    driver_opts:
      type: none
      o: bind
      device: /var/lib/postgresql/replica1
  postgres_replica2_data:
    driver: local
    driver_opts:
      type: none
      o: bind
      device: /var/lib/postgresql/replica2

networks:
  database_network:
    driver: bridge
    ipam:
      config:
        - subnet: 172.20.0.0/16
```

### PostgreSQL Configuration Files
**Location**: `infrastructure/database/configs/`

```ini
# infrastructure/database/configs/postgresql-primary.conf
# PostgreSQL Primary Configuration

# Connection settings
listen_addresses = '*'
port = 5432
max_connections = 200
superuser_reserved_connections = 3

# Memory settings
shared_buffers = 256MB
effective_cache_size = 512MB
maintenance_work_mem = 64MB
work_mem = 4MB

# WAL settings for replication
wal_level = replica
max_wal_senders = 5
max_replication_slots = 5
wal_keep_segments = 32
wal_sender_timeout = 60s

# Archive settings
archive_mode = on
archive_command = 'cp %p /var/lib/postgresql/archive/%f'
archive_timeout = 300

# Checkpoint settings
checkpoint_timeout = 5min
checkpoint_completion_target = 0.9
wal_buffers = 16MB

# Logging
log_destination = 'stderr'
logging_collector = on
log_directory = 'pg_log'
log_filename = 'postgresql-%Y-%m-%d_%H%M%S.log'
log_truncate_on_rotation = on
log_rotation_age = 1d
log_rotation_size = 100MB
log_line_prefix = '%t [%p]: [%l-1] user=%u,db=%d,app=%a,client=%h '
log_checkpoints = on
log_connections = on
log_disconnections = on
log_lock_waits = on
log_temp_files = 0
log_autovacuum_min_duration = 0
log_error_verbosity = default

# Performance monitoring
track_activities = on
track_counts = on
track_io_timing = on
track_functions = all
stats_temp_directory = 'pg_stat_tmp'

# Security
ssl = on
ssl_cert_file = '/etc/ssl/certs/server.crt'
ssl_key_file = '/etc/ssl/private/server.key'
ssl_ca_file = '/etc/ssl/certs/ca.crt'
ssl_crl_file = ''
password_encryption = scram-sha-256

# Autovacuum
autovacuum = on
autovacuum_max_workers = 3
autovacuum_naptime = 1min
autovacuum_vacuum_threshold = 50
autovacuum_analyze_threshold = 50
autovacuum_vacuum_scale_factor = 0.2
autovacuum_analyze_scale_factor = 0.1
autovacuum_freeze_max_age = 200000000
autovacuum_multixact_freeze_max_age = 400000000
autovacuum_vacuum_cost_delay = 20ms
autovacuum_vacuum_cost_limit = 200
```

```ini
# infrastructure/database/configs/postgresql-replica.conf
# PostgreSQL Replica Configuration

# Connection settings
listen_addresses = '*'
port = 5432
max_connections = 100
superuser_reserved_connections = 3

# Memory settings (reduced for replica)
shared_buffers = 128MB
effective_cache_size = 256MB
maintenance_work_mem = 32MB
work_mem = 2MB

# Standby settings
hot_standby = on
max_standby_archive_delay = 30s
max_standby_streaming_delay = 30s
wal_receiver_status_interval = 10s
hot_standby_feedback = on
wal_receiver_timeout = 60s
wal_retrieve_retry_interval = 5s

# Logging (reduced for replica)
log_destination = 'stderr'
logging_collector = on
log_directory = 'pg_log'
log_filename = 'postgresql-replica-%Y-%m-%d_%H%M%S.log'
log_truncate_on_rotation = on
log_rotation_age = 1d
log_rotation_size = 50MB
log_line_prefix = '%t [%p]: [%l-1] user=%u,db=%d,app=%a,client=%h '
log_connections = on
log_disconnections = on

# Performance monitoring
track_activities = on
track_counts = on
track_io_timing = on
stats_temp_directory = 'pg_stat_tmp'

# Security
ssl = on
ssl_cert_file = '/etc/ssl/certs/server.crt'
ssl_key_file = '/etc/ssl/private/server.key'
ssl_ca_file = '/etc/ssl/certs/ca.crt'
password_encryption = scram-sha-256
```

### Database Failover Manager
**Location**: `infrastructure/database/failover_manager.py`

```python
# infrastructure/database/failover_manager.py
import asyncio
import asyncpg
import time
import json
import subprocess
from typing import Dict, List, Optional, Tuple, Any
from dataclasses import dataclass, field
from enum import Enum
import structlog
import logfire
from concurrent.futures import ThreadPoolExecutor

logger = structlog.get_logger()

class DatabaseRole(Enum):
    PRIMARY = "primary"
    REPLICA = "replica"
    FAILED = "failed"
    PROMOTING = "promoting"
    UNKNOWN = "unknown"

class FailoverReason(Enum):
    PRIMARY_FAILURE = "primary_failure"
    MANUAL_FAILOVER = "manual_failover"
    MAINTENANCE = "maintenance"
    NETWORK_PARTITION = "network_partition"
    REPLICA_PROMOTION = "replica_promotion"

@dataclass
class DatabaseNode:
    node_id: str
    host: str
    port: int
    role: DatabaseRole
    lag_bytes: int = 0
    lag_seconds: float = 0.0
    last_check: float = field(default_factory=time.time)
    consecutive_failures: int = 0
    is_healthy: bool = True
    connection_pool: Optional[asyncpg.Pool] = None
    last_seen_lsn: str = "0/0"
    promotion_priority: int = 100  # Lower is higher priority

@dataclass
class FailoverEvent:
    event_id: str
    reason: FailoverReason
    old_primary: Optional[str]
    new_primary: Optional[str]
    started_at: float
    completed_at: Optional[float] = None
    success: bool = False
    error_message: Optional[str] = None
    affected_nodes: List[str] = field(default_factory=list)
    downtime_seconds: Optional[float] = None

class DatabaseFailoverManager:
    def __init__(self, connection_config: Dict[str, str], notification_service=None):
        self.connection_config = connection_config
        self.notification_service = notification_service
        
        # Node management
        self.nodes: Dict[str, DatabaseNode] = {}
        self.current_primary: Optional[str] = None
        
        # Monitoring configuration
        self.check_interval = 10  # seconds
        self.failure_threshold = 3  # consecutive failures before failover
        self.lag_threshold_bytes = 10 * 1024 * 1024  # 10MB
        self.lag_threshold_seconds = 30  # 30 seconds
        
        # State management
        self.failover_in_progress = False
        self.monitoring_task: Optional[asyncio.Task] = None
        self.thread_pool = ThreadPoolExecutor(max_workers=4)
        
        # Event tracking
        self.failover_history: List[FailoverEvent] = []
        self.health_metrics: Dict[str, List[Dict[str, Any]]] = {}
        
    async def initialize(self):
        """Initialize the failover manager"""
        try:
            # Configure database nodes
            await self._configure_nodes()
            
            # Initialize connection pools
            await self._initialize_connection_pools()
            
            # Identify current primary
            await self._identify_current_primary()
            
            # Start health monitoring
            self.monitoring_task = asyncio.create_task(self._monitoring_loop())
            
            logfire.info("Database failover manager initialized",
                       primary=self.current_primary,
                       total_nodes=len(self.nodes))
            
            logger.info("Database failover manager initialized", 
                      primary=self.current_primary)
            
        except Exception as e:
            logger.error("Failed to initialize failover manager", error=str(e))
            raise
    
    async def _configure_nodes(self):
        """Configure database nodes from environment or config"""
        # Primary node
        self.nodes["primary"] = DatabaseNode(
            node_id="primary",
            host="postgres-primary",
            port=5432,
            role=DatabaseRole.PRIMARY,
            promotion_priority=1
        )
        
        # Replica nodes
        self.nodes["replica1"] = DatabaseNode(
            node_id="replica1",
            host="postgres-replica-1",
            port=5432,
            role=DatabaseRole.REPLICA,
            promotion_priority=2
        )
        
        self.nodes["replica2"] = DatabaseNode(
            node_id="replica2",
            host="postgres-replica-2",
            port=5432,
            role=DatabaseRole.REPLICA,
            promotion_priority=3
        )
    
    async def _initialize_connection_pools(self):
        """Initialize connection pools for all nodes"""
        for node_id, node in self.nodes.items():
            try:
                node.connection_pool = await asyncpg.create_pool(
                    host=node.host,
                    port=node.port,
                    user=self.connection_config['user'],
                    password=self.connection_config['password'],
                    database=self.connection_config['database'],
                    min_size=1,
                    max_size=5,
                    timeout=10,
                    command_timeout=30
                )
                logger.debug("Connection pool created", node_id=node_id)
                
            except Exception as e:
                logger.warning("Failed to create connection pool", 
                             node_id=node_id, error=str(e))
    
    async def stop(self):
        """Stop the failover manager and cleanup resources"""
        try:
            # Stop monitoring
            if self.monitoring_task and not self.monitoring_task.done():
                self.monitoring_task.cancel()
                try:
                    await self.monitoring_task
                except asyncio.CancelledError:
                    pass
            
            # Close connection pools
            for node in self.nodes.values():
                if node.connection_pool:
                    await node.connection_pool.close()
            
            # Shutdown thread pool
            self.thread_pool.shutdown(wait=True)
            
            logger.info("Database failover manager stopped")
            
        except Exception as e:
            logger.error("Error stopping failover manager", error=str(e))
    
    async def _monitoring_loop(self):
        """Main monitoring loop for database health"""
        while True:
            try:
                await asyncio.sleep(self.check_interval)
                
                # Check health of all nodes
                await self._check_all_nodes_health()
                
                # Evaluate need for failover
                await self._evaluate_failover_need()
                
                # Record health metrics
                await self._record_health_metrics()
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Database monitoring loop error", error=str(e))
                await asyncio.sleep(5)  # Brief pause before retry
    
    async def _check_all_nodes_health(self):
        """Check health of all database nodes concurrently"""
        health_check_tasks = []
        for node_id, node in self.nodes.items():
            task = asyncio.create_task(self._check_node_health(node_id, node))
            health_check_tasks.append(task)
        
        # Wait for all health checks to complete
        results = await asyncio.gather(*health_check_tasks, return_exceptions=True)
        
        # Log any exceptions
        for i, result in enumerate(results):
            if isinstance(result, Exception):
                node_id = list(self.nodes.keys())[i]
                logger.error("Health check exception", node_id=node_id, error=str(result))
    
    async def _check_node_health(self, node_id: str, node: DatabaseNode):
        """Check health of a specific database node"""
        try:
            if not node.connection_pool:
                await self._initialize_connection_pools()
                if not node.connection_pool:
                    raise Exception("No connection pool available")
            
            async with node.connection_pool.acquire() as conn:
                start_time = time.time()
                
                # Basic connectivity test
                await conn.fetchval("SELECT 1")
                
                # Check if node is in recovery (replica) or primary
                is_in_recovery = await conn.fetchval("SELECT pg_is_in_recovery()")
                response_time = time.time() - start_time
                
                # Update role based on recovery status
                if is_in_recovery:
                    if node.role == DatabaseRole.PRIMARY:
                        logger.warning("Primary node now in recovery", node_id=node_id)
                    node.role = DatabaseRole.REPLICA
                    
                    # Get replication lag for replicas
                    await self._check_replication_lag(conn, node)
                else:
                    if node.role == DatabaseRole.REPLICA:
                        logger.warning("Replica node now primary", node_id=node_id)
                    node.role = DatabaseRole.PRIMARY
                
                # Node is healthy
                node.is_healthy = True
                node.consecutive_failures = 0
                node.last_check = time.time()
                
                logfire.debug("Node health check successful",
                            node_id=node_id,
                            role=node.role.value,
                            response_time_ms=response_time * 1000,
                            lag_bytes=node.lag_bytes,
                            lag_seconds=node.lag_seconds)
                
        except Exception as e:
            # Node health check failed
            node.is_healthy = False
            node.consecutive_failures += 1
            node.last_check = time.time()
            
            logger.warning("Node health check failed",
                         node_id=node_id,
                         host=node.host,
                         consecutive_failures=node.consecutive_failures,
                         error=str(e))
            
            logfire.warning("Database node unhealthy",
                          node_id=node_id,
                          consecutive_failures=node.consecutive_failures,
                          error=str(e))
    
    async def _check_replication_lag(self, conn: asyncpg.Connection, node: DatabaseNode):
        """Check replication lag for replica nodes"""
        try:
            # Get replication lag in bytes and time
            lag_query = """
                SELECT 
                    CASE 
                        WHEN pg_last_wal_receive_lsn() = pg_last_wal_replay_lsn() 
                        THEN 0
                        ELSE EXTRACT(EPOCH FROM (now() - pg_last_xact_replay_timestamp()))::int
                    END as lag_seconds,
                    CASE 
                        WHEN pg_last_wal_receive_lsn() = pg_last_wal_replay_lsn() 
                        THEN 0
                        ELSE pg_wal_lsn_diff(pg_last_wal_receive_lsn(), pg_last_wal_replay_lsn())
                    END as lag_bytes,
                    pg_last_wal_replay_lsn() as current_lsn
            """
            
            result = await conn.fetchrow(lag_query)
            
            if result:
                node.lag_seconds = result['lag_seconds'] or 0
                node.lag_bytes = int(result['lag_bytes'] or 0)
                node.last_seen_lsn = result['current_lsn'] or "0/0"
                
                # Log significant lag
                if (node.lag_bytes > self.lag_threshold_bytes or 
                    node.lag_seconds > self.lag_threshold_seconds):
                    
                    logger.warning("High replication lag detected",
                                 node_id=node.node_id,
                                 lag_bytes=node.lag_bytes,
                                 lag_seconds=node.lag_seconds)
                    
                    logfire.warning("High database replication lag",
                                  node_id=node.node_id,
                                  lag_bytes=node.lag_bytes,
                                  lag_seconds=node.lag_seconds)
            
        except Exception as e:
            logger.error("Failed to check replication lag", 
                       node_id=node.node_id, error=str(e))
    
    async def _evaluate_failover_need(self):
        """Evaluate if failover is needed based on node health"""
        if self.failover_in_progress:
            return
        
        # Check if current primary is healthy
        if self.current_primary:
            primary_node = self.nodes.get(self.current_primary)
            
            if (primary_node and 
                (primary_node.consecutive_failures >= self.failure_threshold or 
                 not primary_node.is_healthy)):
                
                logger.critical("Primary database failure detected",
                              primary=self.current_primary,
                              consecutive_failures=primary_node.consecutive_failures)
                
                await self._initiate_failover(FailoverReason.PRIMARY_FAILURE)
    
    async def _initiate_failover(self, reason: FailoverReason):
        """Initiate database failover process"""
        if self.failover_in_progress:
            logger.warning("Failover already in progress")
            return
        
        self.failover_in_progress = True
        event_id = f"failover_{int(time.time())}"
        
        failover_event = FailoverEvent(
            event_id=event_id,
            reason=reason,
            old_primary=self.current_primary,
            new_primary=None,
            started_at=time.time()
        )
        
        logger.critical("Initiating database failover",
                       event_id=event_id,
                       reason=reason.value,
                       failed_primary=self.current_primary)
        
        logfire.error("Database failover initiated",
                    event_id=event_id,
                    reason=reason.value,
                    old_primary=self.current_primary)
        
        try:
            # Select best replica for promotion
            best_replica = await self._select_best_replica()
            
            if not best_replica:
                error_msg = "No healthy replica available for failover"
                logger.error(error_msg)
                failover_event.error_message = error_msg
                return
            
            failover_event.new_primary = best_replica
            
            # Promote replica to primary
            await self._promote_replica_to_primary(best_replica)
            
            # Update node roles
            await self._update_node_roles_after_failover(best_replica)
            
            # Update application configuration
            await self._update_application_database_config(best_replica)
            
            # Complete failover event
            failover_event.completed_at = time.time()
            failover_event.success = True
            failover_event.downtime_seconds = failover_event.completed_at - failover_event.started_at
            
            logger.info("Database failover completed successfully",
                      event_id=event_id,
                      new_primary=best_replica,
                      downtime_seconds=failover_event.downtime_seconds)
            
            logfire.info("Database failover completed",
                       event_id=event_id,
                       new_primary=best_replica,
                       downtime_seconds=failover_event.downtime_seconds)
            
            # Send notification
            if self.notification_service:
                await self.notification_service.send_critical_alert(
                    f"Database failover completed: {self.current_primary} -> {best_replica}",
                    {
                        "event_id": event_id,
                        "old_primary": failover_event.old_primary,
                        "new_primary": best_replica,
                        "downtime_seconds": failover_event.downtime_seconds,
                        "reason": reason.value
                    }
                )
            
        except Exception as e:
            error_msg = f"Database failover failed: {str(e)}"
            logger.error(error_msg, event_id=event_id)
            
            failover_event.completed_at = time.time()
            failover_event.success = False
            failover_event.error_message = error_msg
            
            logfire.error("Database failover failed",
                        event_id=event_id,
                        error=str(e))
            
        finally:
            self.failover_history.append(failover_event)
            self.failover_in_progress = False
    
    async def _select_best_replica(self) -> Optional[str]:
        """Select the best replica for promotion based on health and lag"""
        healthy_replicas = [
            (node_id, node) for node_id, node in self.nodes.items()
            if (node.role == DatabaseRole.REPLICA and 
                node.is_healthy and 
                node.consecutive_failures == 0)
        ]
        
        if not healthy_replicas:
            return None
        
        # Sort by promotion priority, then by lag
        healthy_replicas.sort(key=lambda x: (x[1].promotion_priority, x[1].lag_bytes))
        
        selected_replica = healthy_replicas[0][0]
        logger.info("Selected replica for promotion", 
                  replica=selected_replica,
                  lag_bytes=healthy_replicas[0][1].lag_bytes,
                  priority=healthy_replicas[0][1].promotion_priority)
        
        return selected_replica
    
    async def _promote_replica_to_primary(self, replica_id: str):
        """Promote replica to primary"""
        replica_node = self.nodes[replica_id]
        replica_node.role = DatabaseRole.PROMOTING
        
        try:
            # Create trigger file to promote replica
            trigger_command = [
                "docker", "exec", f"postgres-{replica_id}",
                "touch", "/tmp/postgresql.trigger.5432"
            ]
            
            result = subprocess.run(trigger_command, capture_output=True, text=True, timeout=30)
            
            if result.returncode != 0:
                raise Exception(f"Failed to create trigger file: {result.stderr}")
            
            # Wait for promotion to complete
            await self._wait_for_promotion_completion(replica_node)
            
            logger.info("Replica promoted to primary successfully", replica_id=replica_id)
            
        except Exception as e:
            logger.error("Failed to promote replica", replica_id=replica_id, error=str(e))
            replica_node.role = DatabaseRole.REPLICA
            raise
    
    async def _wait_for_promotion_completion(self, node: DatabaseNode, timeout: int = 60):
        """Wait for replica promotion to complete"""
        start_time = time.time()
        
        while time.time() - start_time < timeout:
            try:
                if node.connection_pool:
                    async with node.connection_pool.acquire() as conn:
                        is_in_recovery = await conn.fetchval("SELECT pg_is_in_recovery()")
                        
                        if not is_in_recovery:
                            # Promotion completed
                            node.role = DatabaseRole.PRIMARY
                            return
                
                await asyncio.sleep(2)
                
            except Exception as e:
                logger.debug("Waiting for promotion", node_id=node.node_id, error=str(e))
                await asyncio.sleep(2)
        
        raise Exception(f"Promotion timeout after {timeout} seconds")
    
    def get_failover_status(self) -> Dict[str, Any]:
        """Get current failover status and health information"""
        nodes_status = {}
        for node_id, node in self.nodes.items():
            nodes_status[node_id] = {
                "role": node.role.value,
                "is_healthy": node.is_healthy,
                "consecutive_failures": node.consecutive_failures,
                "lag_bytes": node.lag_bytes,
                "lag_seconds": node.lag_seconds,
                "last_check": node.last_check,
                "promotion_priority": node.promotion_priority
            }
        
        recent_failovers = [
            {
                "event_id": event.event_id,
                "reason": event.reason.value,
                "old_primary": event.old_primary,
                "new_primary": event.new_primary,
                "success": event.success,
                "downtime_seconds": event.downtime_seconds,
                "started_at": event.started_at
            }
            for event in self.failover_history[-10:]  # Last 10 events
        ]
        
        return {
            "current_primary": self.current_primary,
            "failover_in_progress": self.failover_in_progress,
            "nodes": nodes_status,
            "recent_failovers": recent_failovers,
            "check_interval_seconds": self.check_interval,
            "failure_threshold": self.failure_threshold
        }
```

## TDD Implementation Cycle

### Red Phase: Database Replication Test Creation
```python
# infrastructure/tests/test_database_failover.py
import pytest
import asyncio
from infrastructure.database.failover_manager import DatabaseFailoverManager, DatabaseRole

@pytest.mark.asyncio
async def test_failover_manager_initialization():
    """Test failover manager initializes with correct node configuration"""
    # This test should initially fail (Red phase)
    assert False, "Failover manager initialization not implemented yet"

@pytest.mark.asyncio
async def test_database_health_monitoring():
    """Test database health monitoring detects node failures"""
    # This test should initially fail (Red phase)
    assert False, "Database health monitoring not implemented yet"

@pytest.mark.asyncio
async def test_automatic_failover_execution():
    """Test automatic failover when primary fails"""
    # This test should initially fail (Red phase)
    assert False, "Automatic failover execution not implemented yet"

@pytest.mark.asyncio
async def test_replica_selection_algorithm():
    """Test best replica selection for promotion"""
    # This test should initially fail (Red phase)
    assert False, "Replica selection algorithm not implemented yet"

@pytest.mark.asyncio
async def test_replication_lag_monitoring():
    """Test replication lag monitoring and alerting"""
    # This test should initially fail (Red phase)
    assert False, "Replication lag monitoring not implemented yet"
```

### Green Phase: Database Replication Implementation
```python
# Implement database replication features to make tests pass
# This involves adding PostgreSQL configuration, failover logic, and health monitoring
```

### Refactor Phase: Database Replication Optimization
```python
# Optimize database replication for performance and reliability
# Add advanced failover strategies and enhanced monitoring
# Improve error handling and recovery procedures
```

## Security Checklist ✅

### Database Replication Security
- [ ] Database replication encryption with SSL/TLS
- [ ] Replication user authentication and authorization
- [ ] Network security between primary and replica nodes
- [ ] Secure replication slot management
- [ ] Protection against replication attacks and manipulation
- [ ] Secure WAL archive storage and access controls
- [ ] Database connection encryption and certificate validation
- [ ] Replication monitoring data protection
- [ ] Secure failover trigger mechanisms
- [ ] Access controls for failover management interfaces

### Failover Security
- [ ] Failover process authentication and authorization
- [ ] Secure promotion procedures with validation
- [ ] Protection against unauthorized failover triggers
- [ ] Failover event logging and audit trails
- [ ] Secure communication during failover operations
- [ ] Split-brain prevention and detection mechanisms
- [ ] Failover notification security and encryption
- [ ] Recovery process security validation
- [ ] Emergency access procedures security
- [ ] Failover system isolation and hardening

### Configuration Security
- [ ] PostgreSQL configuration file protection
- [ ] Secure storage of database credentials
- [ ] Configuration change management and validation
- [ ] Database parameter security hardening
- [ ] Backup configuration security
- [ ] Log file protection and secure storage
- [ ] Database user privilege minimization
- [ ] Secure database initialization procedures
- [ ] Configuration backup and recovery security
- [ ] Database security patch management

### Monitoring Security
- [ ] Health monitoring system security and access controls
- [ ] Monitoring data encryption and protection
- [ ] Secure monitoring agent deployment
- [ ] Health check authentication and authorization
- [ ] Monitoring alert security and validation
- [ ] Performance metrics data protection
- [ ] Monitoring system network isolation
- [ ] Secure integration with external monitoring systems
- [ ] Monitoring data retention policy enforcement
- [ ] Regular security assessment of monitoring infrastructure

## Performance Requirements

### Replication Performance
- Replication lag < 1 second under normal load
- WAL shipping latency < 500ms
- Primary-replica synchronization efficiency > 99%
- Connection pool performance > 1000 connections/second
- Query response time impact < 5% due to replication
- Network bandwidth utilization < 80% for replication

### Failover Performance
- Automatic failover detection time < 30 seconds
- Replica promotion time < 60 seconds
- Total failover time < 2 minutes
- Application reconnection time < 30 seconds
- Data consistency validation < 10 seconds
- Failover success rate > 99%

### Monitoring Performance
- Health check response time < 100ms
- Monitoring overhead < 2% of database performance
- Health status update frequency = 10 seconds
- Lag detection accuracy > 99%
- Alert delivery time < 10 seconds
- Monitoring system availability > 99.9%

## Commit Instructions

After implementing the database replication system:

```bash
git add infrastructure/database/
git commit -m "Add PostgreSQL high availability with streaming replication

- Implement PostgreSQL streaming replication with primary-replica architecture
- Add DatabaseFailoverManager with automated failover capabilities
- Include comprehensive health monitoring and lag detection
- Add replica promotion and primary election algorithms
- Implement secure replication configuration with SSL/TLS
- Add failover event tracking and notification integration
- Include Docker Compose configuration for HA database cluster
- Add connection pooling and resource management
- Implement comprehensive monitoring and alerting
- Add TDD cycle with Red-Green-Refactor for database HA
- Ensure >90% database failover test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete database replication test suite:

```bash
# Run all database failover tests
pytest infrastructure/tests/test_database_failover.py -v --timeout=300

# Run specific database HA test categories
pytest infrastructure/tests/database/ -k "replication" -v
pytest infrastructure/tests/database/ -k "failover" -v
pytest infrastructure/tests/database/ -k "health_monitoring" -v

# Run database HA performance tests
pytest infrastructure/tests/database/performance/ -v

# Run database HA integration tests
pytest infrastructure/tests/database/test_integration.py -v
```

Validate database HA test coverage:
```bash
pytest infrastructure/tests/database/ --cov=infrastructure.database --cov-report=html --cov-fail-under=90
```

## Integration Testing

Test database HA integration with platform components:
```bash
# Test integration with Session 1 (Logfire)
pytest infrastructure/tests/integration/test_database_logfire_integration.py -v

# Test integration with application layer
pytest infrastructure/tests/integration/test_database_application_integration.py -v

# Test failover impact on all platform sessions
pytest infrastructure/tests/integration/test_database_platform_integration.py -v
```