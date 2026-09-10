# Session 13.4: Comprehensive Health Monitoring & Automated Recovery

## Objective
Implement comprehensive health monitoring system with automated recovery procedures, proactive alerting, and intelligent incident response to ensure platform reliability, minimize downtime, and enable rapid issue resolution across all infrastructure components.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for centralized health monitoring and incident correlation
- **Session 11**: Enhances metrics collection with infrastructure health monitoring
- **Session 13.1-13.3**: Monitors database, Redis, and load balancer health for complete HA coverage
- **All Sessions**: Provides comprehensive health monitoring for entire platform stack

## Core Implementation

### Comprehensive Health Monitoring System
**Location**: `infrastructure/health_monitoring/health_monitor.py`

```python
# infrastructure/health_monitoring/health_monitor.py
import asyncio
import aiohttp
import asyncpg
import time
import json
import psutil
import redis.asyncio as redis
from typing import Dict, List, Optional, Any, Callable, Union, Tuple
from dataclasses import dataclass, field, asdict
from enum import Enum
from datetime import datetime, timedelta
import structlog
import logfire
from concurrent.futures import ThreadPoolExecutor

logger = structlog.get_logger()

class HealthStatus(Enum):
    HEALTHY = "healthy"
    DEGRADED = "degraded"
    UNHEALTHY = "unhealthy"
    CRITICAL = "critical"
    UNKNOWN = "unknown"
    MAINTENANCE = "maintenance"

class ComponentType(Enum):
    DATABASE = "database"
    REDIS = "redis"
    LOAD_BALANCER = "load_balancer"
    API_SERVER = "api_server"
    FRONTEND_SERVER = "frontend_server"
    WEBSOCKET_SERVER = "websocket_server"
    STORAGE = "storage"
    EXTERNAL_SERVICE = "external_service"
    NETWORK = "network"
    SYSTEM = "system"

class AlertSeverity(Enum):
    INFO = "info"
    WARNING = "warning"
    ERROR = "error"
    CRITICAL = "critical"
    EMERGENCY = "emergency"

@dataclass
class HealthCheck:
    check_id: str
    name: str
    component_type: ComponentType
    status: HealthStatus
    last_check: float
    response_time_ms: float
    error_message: Optional[str] = None
    details: Dict[str, Any] = field(default_factory=dict)
    consecutive_failures: int = 0
    check_count: int = 0
    success_rate: float = 100.0
    
@dataclass
class SystemMetrics:
    cpu_usage_percent: float
    memory_usage_percent: float
    disk_usage_percent: float
    network_io_mbps: float
    load_average: Tuple[float, float, float]
    open_file_descriptors: int
    process_count: int
    uptime_seconds: float
    timestamp: float = field(default_factory=time.time)

@dataclass
class HealthAlert:
    alert_id: str
    component_id: str
    component_type: ComponentType
    severity: AlertSeverity
    message: str
    details: Dict[str, Any]
    created_at: float
    resolved_at: Optional[float] = None
    acknowledged_at: Optional[float] = None
    escalated: bool = False
    notification_sent: bool = False

@dataclass
class RecoveryAction:
    action_id: str
    component_id: str
    action_type: str
    description: str
    executed_at: float
    success: bool
    error_message: Optional[str] = None
    execution_time_ms: float = 0.0

class ComprehensiveHealthMonitor:
    def __init__(self, config: Dict[str, Any], notification_service=None, 
                 database_manager=None, redis_manager=None, lb_manager=None):
        self.config = config
        self.notification_service = notification_service
        self.database_manager = database_manager
        self.redis_manager = redis_manager
        self.lb_manager = lb_manager
        
        # Health check tracking
        self.health_checks: Dict[str, HealthCheck] = {}
        self.system_metrics: List[SystemMetrics] = []
        self.active_alerts: Dict[str, HealthAlert] = {}
        self.recovery_actions: List[RecoveryAction] = []
        
        # Configuration
        self.check_interval = config.get('check_interval', 30)
        self.critical_threshold = config.get('critical_threshold', 3)
        self.degraded_threshold = config.get('degraded_threshold', 2)
        self.metrics_retention_hours = config.get('metrics_retention_hours', 24)
        
        # Monitoring state
        self.monitoring_task: Optional[asyncio.Task] = None
        self.metrics_task: Optional[asyncio.Task] = None
        self.recovery_task: Optional[asyncio.Task] = None
        self.is_monitoring = False
        
        # Thread pool for system operations
        self.thread_pool = ThreadPoolExecutor(max_workers=6)
        
        # Recovery procedures
        self.recovery_procedures = self._initialize_recovery_procedures()
        
        # Component configurations
        self.components_config = config.get('components', {})
        
    def _initialize_recovery_procedures(self) -> Dict[str, Callable]:
        """Initialize automated recovery procedures"""
        return {
            'database_connection_recovery': self._recover_database_connection,
            'redis_connection_recovery': self._recover_redis_connection,
            'api_server_restart': self._restart_api_server,
            'clear_cache_recovery': self._clear_cache_recovery,
            'disk_space_cleanup': self._cleanup_disk_space,
            'memory_pressure_relief': self._relieve_memory_pressure,
            'network_connectivity_fix': self._fix_network_connectivity,
            'load_balancer_config_reload': self._reload_load_balancer_config
        }
    
    async def initialize(self):
        """Initialize the comprehensive health monitoring system"""
        try:
            # Initialize health checks for all components
            await self._initialize_health_checks()
            
            # Start monitoring tasks
            self.monitoring_task = asyncio.create_task(self._monitoring_loop())
            self.metrics_task = asyncio.create_task(self._system_metrics_loop())
            self.recovery_task = asyncio.create_task(self._recovery_loop())
            
            self.is_monitoring = True
            
            logfire.info("Comprehensive health monitoring initialized",
                       total_checks=len(self.health_checks),
                       check_interval=self.check_interval)
            
            logger.info("Comprehensive health monitoring system initialized",
                      health_checks=len(self.health_checks))
            
        except Exception as e:
            logger.error("Failed to initialize health monitoring system", error=str(e))
            raise
    
    async def _initialize_health_checks(self):
        """Initialize health checks for all platform components"""
        # Database health checks
        if self.database_manager:
            self.health_checks['database_primary'] = HealthCheck(
                check_id='database_primary',
                name='Database Primary Connection',
                component_type=ComponentType.DATABASE,
                status=HealthStatus.UNKNOWN,
                last_check=0,
                response_time_ms=0
            )
            
            self.health_checks['database_replication'] = HealthCheck(
                check_id='database_replication',
                name='Database Replication Status',
                component_type=ComponentType.DATABASE,
                status=HealthStatus.UNKNOWN,
                last_check=0,
                response_time_ms=0
            )
        
        # Redis health checks
        if self.redis_manager:
            self.health_checks['redis_master'] = HealthCheck(
                check_id='redis_master',
                name='Redis Master Connection',
                component_type=ComponentType.REDIS,
                status=HealthStatus.UNKNOWN,
                last_check=0,
                response_time_ms=0
            )
            
            self.health_checks['redis_sentinel'] = HealthCheck(
                check_id='redis_sentinel',
                name='Redis Sentinel Cluster',
                component_type=ComponentType.REDIS,
                status=HealthStatus.UNKNOWN,
                last_check=0,
                response_time_ms=0
            )
        
        # API server health checks
        api_servers = self.components_config.get('api_servers', [])
        for i, server_config in enumerate(api_servers):
            check_id = f"api_server_{i+1}"
            self.health_checks[check_id] = HealthCheck(
                check_id=check_id,
                name=f"API Server {i+1}",
                component_type=ComponentType.API_SERVER,
                status=HealthStatus.UNKNOWN,
                last_check=0,
                response_time_ms=0,
                details={'host': server_config.get('host'), 'port': server_config.get('port')}
            )
        
        # Load balancer health checks
        if self.lb_manager:
            self.health_checks['load_balancer'] = HealthCheck(
                check_id='load_balancer',
                name='Load Balancer Health',
                component_type=ComponentType.LOAD_BALANCER,
                status=HealthStatus.UNKNOWN,
                last_check=0,
                response_time_ms=0
            )
        
        # System health checks
        self.health_checks['system_resources'] = HealthCheck(
            check_id='system_resources',
            name='System Resource Utilization',
            component_type=ComponentType.SYSTEM,
            status=HealthStatus.UNKNOWN,
            last_check=0,
            response_time_ms=0
        )
        
        # Storage health checks
        self.health_checks['storage_availability'] = HealthCheck(
            check_id='storage_availability',
            name='Storage System Availability',
            component_type=ComponentType.STORAGE,
            status=HealthStatus.UNKNOWN,
            last_check=0,
            response_time_ms=0
        )
        
        # Network connectivity checks
        self.health_checks['network_connectivity'] = HealthCheck(
            check_id='network_connectivity',
            name='Network Connectivity',
            component_type=ComponentType.NETWORK,
            status=HealthStatus.UNKNOWN,
            last_check=0,
            response_time_ms=0
        )
    
    async def stop(self):
        """Stop the health monitoring system"""
        try:
            self.is_monitoring = False
            
            # Stop monitoring tasks
            for task in [self.monitoring_task, self.metrics_task, self.recovery_task]:
                if task and not task.done():
                    task.cancel()
                    try:
                        await task
                    except asyncio.CancelledError:
                        pass
            
            # Shutdown thread pool
            self.thread_pool.shutdown(wait=True)
            
            logger.info("Health monitoring system stopped")
            
        except Exception as e:
            logger.error("Error stopping health monitoring system", error=str(e))
    
    async def _monitoring_loop(self):
        """Main health monitoring loop"""
        while self.is_monitoring:
            try:
                await asyncio.sleep(self.check_interval)
                
                # Run all health checks
                await self._run_all_health_checks()
                
                # Evaluate system health
                await self._evaluate_system_health()
                
                # Check for alerts
                await self._check_for_alerts()
                
                # Cleanup old data
                await self._cleanup_old_data()
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Health monitoring loop error", error=str(e))
                await asyncio.sleep(5)
    
    async def _run_all_health_checks(self):
        """Run all configured health checks"""
        check_tasks = []
        
        for check_id, health_check in self.health_checks.items():
            task = asyncio.create_task(self._run_health_check(check_id, health_check))
            check_tasks.append(task)
        
        # Wait for all checks to complete
        await asyncio.gather(*check_tasks, return_exceptions=True)
    
    async def _run_health_check(self, check_id: str, health_check: HealthCheck):
        """Run individual health check"""
        start_time = time.time()
        
        try:
            # Route to appropriate check method
            if health_check.component_type == ComponentType.DATABASE:
                await self._check_database_health(check_id, health_check)
            elif health_check.component_type == ComponentType.REDIS:
                await self._check_redis_health(check_id, health_check)
            elif health_check.component_type == ComponentType.API_SERVER:
                await self._check_api_server_health(check_id, health_check)
            elif health_check.component_type == ComponentType.LOAD_BALANCER:
                await self._check_load_balancer_health(check_id, health_check)
            elif health_check.component_type == ComponentType.SYSTEM:
                await self._check_system_health(check_id, health_check)
            elif health_check.component_type == ComponentType.STORAGE:
                await self._check_storage_health(check_id, health_check)
            elif health_check.component_type == ComponentType.NETWORK:
                await self._check_network_health(check_id, health_check)
            
            # Update check metadata
            health_check.response_time_ms = (time.time() - start_time) * 1000
            health_check.last_check = time.time()
            health_check.check_count += 1
            
            # Update success rate
            if health_check.status in [HealthStatus.HEALTHY, HealthStatus.DEGRADED]:
                health_check.consecutive_failures = 0
            else:
                health_check.consecutive_failures += 1
            
            health_check.success_rate = (
                (health_check.check_count - health_check.consecutive_failures) / 
                max(health_check.check_count, 1)
            ) * 100
            
        except Exception as e:
            health_check.status = HealthStatus.UNKNOWN
            health_check.error_message = str(e)
            health_check.consecutive_failures += 1
            health_check.last_check = time.time()
            
            logger.error("Health check failed", check_id=check_id, error=str(e))
    
    async def _check_database_health(self, check_id: str, health_check: HealthCheck):
        """Check database health"""
        if check_id == 'database_primary':
            # Check primary database connection
            if self.database_manager:
                primary_status = self.database_manager.get_failover_status()
                
                if primary_status['current_primary']:
                    health_check.status = HealthStatus.HEALTHY
                    health_check.details = {
                        'primary_node': primary_status['current_primary'],
                        'healthy_nodes': sum(1 for node in primary_status['nodes'].values() 
                                           if node['is_healthy'])
                    }
                else:
                    health_check.status = HealthStatus.CRITICAL
                    health_check.error_message = "No primary database available"
            
        elif check_id == 'database_replication':
            # Check database replication status
            if self.database_manager:
                status = self.database_manager.get_failover_status()
                nodes = status.get('nodes', {})
                
                replica_nodes = [node for node in nodes.values() if node['role'] == 'replica']
                healthy_replicas = [node for node in replica_nodes if node['is_healthy']]
                
                if len(healthy_replicas) >= 1:
                    health_check.status = HealthStatus.HEALTHY
                elif len(healthy_replicas) == 0 and len(replica_nodes) > 0:
                    health_check.status = HealthStatus.CRITICAL
                    health_check.error_message = "All database replicas are unhealthy"
                else:
                    health_check.status = HealthStatus.DEGRADED
                    health_check.error_message = "No database replicas configured"
                
                health_check.details = {
                    'total_replicas': len(replica_nodes),
                    'healthy_replicas': len(healthy_replicas)
                }
    
    async def _check_redis_health(self, check_id: str, health_check: HealthCheck):
        """Check Redis health"""
        if check_id == 'redis_master':
            # Check Redis master connectivity
            if self.redis_manager:
                status = self.redis_manager.get_cluster_status()
                
                if status['current_master']:
                    health_check.status = HealthStatus.HEALTHY
                    health_check.details = {
                        'master_host': status['current_master'].get('ip'),
                        'master_port': status['current_master'].get('port')
                    }
                else:
                    health_check.status = HealthStatus.CRITICAL
                    health_check.error_message = "No Redis master available"
        
        elif check_id == 'redis_sentinel':
            # Check Redis Sentinel cluster health
            if self.redis_manager:
                status = self.redis_manager.get_cluster_status()
                sentinel_status = status.get('sentinel_nodes', {})
                
                healthy_sentinels = sentinel_status.get('healthy', 0)
                total_sentinels = sentinel_status.get('total', 0)
                
                if healthy_sentinels >= 2:  # Quorum
                    health_check.status = HealthStatus.HEALTHY
                elif healthy_sentinels >= 1:
                    health_check.status = HealthStatus.DEGRADED
                    health_check.error_message = "Redis Sentinel quorum at risk"
                else:
                    health_check.status = HealthStatus.CRITICAL
                    health_check.error_message = "Redis Sentinel cluster failed"
                
                health_check.details = {
                    'healthy_sentinels': healthy_sentinels,
                    'total_sentinels': total_sentinels
                }
    
    async def _check_api_server_health(self, check_id: str, health_check: HealthCheck):
        """Check API server health"""
        host = health_check.details.get('host')
        port = health_check.details.get('port')
        
        if not host or not port:
            health_check.status = HealthStatus.UNKNOWN
            health_check.error_message = "Missing server configuration"
            return
        
        try:
            async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=10)) as session:
                url = f"http://{host}:{port}/health"
                
                async with session.get(url) as response:
                    if response.status == 200:
                        health_data = await response.json()
                        health_check.status = HealthStatus.HEALTHY
                        health_check.details.update(health_data)
                    else:
                        health_check.status = HealthStatus.UNHEALTHY
                        health_check.error_message = f"Health check returned status {response.status}"
                        
        except Exception as e:
            health_check.status = HealthStatus.UNHEALTHY
            health_check.error_message = str(e)
    
    async def _check_load_balancer_health(self, check_id: str, health_check: HealthCheck):
        """Check load balancer health"""
        if self.lb_manager:
            status = self.lb_manager.get_health_summary()
            lb_instances = status.get('load_balancer_instances', {})
            
            healthy_instances = lb_instances.get('healthy', 0)
            total_instances = lb_instances.get('total', 0)
            
            if healthy_instances >= 1:
                health_check.status = HealthStatus.HEALTHY
            elif total_instances > 0:
                health_check.status = HealthStatus.CRITICAL
                health_check.error_message = "All load balancer instances are unhealthy"
            else:
                health_check.status = HealthStatus.UNKNOWN
                health_check.error_message = "No load balancer instances configured"
            
            health_check.details = {
                'healthy_instances': healthy_instances,
                'total_instances': total_instances,
                'current_metrics': status.get('current_metrics')
            }
    
    async def _check_system_health(self, check_id: str, health_check: HealthCheck):
        """Check system resource health"""
        try:
            # Get current system metrics
            metrics = await self._collect_system_metrics()
            
            # Evaluate system health based on resource usage
            critical_issues = []
            degraded_issues = []
            
            if metrics.cpu_usage_percent > 90:
                critical_issues.append(f"High CPU usage: {metrics.cpu_usage_percent:.1f}%")
            elif metrics.cpu_usage_percent > 80:
                degraded_issues.append(f"Elevated CPU usage: {metrics.cpu_usage_percent:.1f}%")
            
            if metrics.memory_usage_percent > 95:
                critical_issues.append(f"High memory usage: {metrics.memory_usage_percent:.1f}%")
            elif metrics.memory_usage_percent > 85:
                degraded_issues.append(f"Elevated memory usage: {metrics.memory_usage_percent:.1f}%")
            
            if metrics.disk_usage_percent > 95:
                critical_issues.append(f"High disk usage: {metrics.disk_usage_percent:.1f}%")
            elif metrics.disk_usage_percent > 85:
                degraded_issues.append(f"Elevated disk usage: {metrics.disk_usage_percent:.1f}%")
            
            # Determine overall status
            if critical_issues:
                health_check.status = HealthStatus.CRITICAL
                health_check.error_message = "; ".join(critical_issues)
            elif degraded_issues:
                health_check.status = HealthStatus.DEGRADED
                health_check.error_message = "; ".join(degraded_issues)
            else:
                health_check.status = HealthStatus.HEALTHY
            
            health_check.details = asdict(metrics)
            
        except Exception as e:
            health_check.status = HealthStatus.UNKNOWN
            health_check.error_message = str(e)
    
    async def _check_storage_health(self, check_id: str, health_check: HealthCheck):
        """Check storage system health"""
        try:
            # Check disk space on critical mount points
            critical_mounts = ['/var/lib/postgresql', '/var/lib/redis', '/var/log']
            storage_issues = []
            
            for mount_point in critical_mounts:
                try:
                    disk_usage = psutil.disk_usage(mount_point)
                    usage_percent = (disk_usage.used / disk_usage.total) * 100
                    
                    if usage_percent > 95:
                        storage_issues.append(f"{mount_point}: {usage_percent:.1f}% full")
                    
                except FileNotFoundError:
                    # Mount point doesn't exist, skip
                    continue
            
            if storage_issues:
                health_check.status = HealthStatus.CRITICAL
                health_check.error_message = "Storage space critical: " + "; ".join(storage_issues)
            else:
                health_check.status = HealthStatus.HEALTHY
            
        except Exception as e:
            health_check.status = HealthStatus.UNKNOWN
            health_check.error_message = str(e)
    
    async def _check_network_health(self, check_id: str, health_check: HealthCheck):
        """Check network connectivity health"""
        try:
            # Test connectivity to critical external services
            test_endpoints = [
                ('8.8.8.8', 53),  # DNS
                ('1.1.1.1', 53),  # Backup DNS
            ]
            
            connectivity_issues = []
            
            for host, port in test_endpoints:
                try:
                    future = asyncio.open_connection(host, port)
                    reader, writer = await asyncio.wait_for(future, timeout=5)
                    writer.close()
                    await writer.wait_closed()
                except Exception:
                    connectivity_issues.append(f"{host}:{port}")
            
            if connectivity_issues:
                health_check.status = HealthStatus.DEGRADED
                health_check.error_message = f"Network connectivity issues: {', '.join(connectivity_issues)}"
            else:
                health_check.status = HealthStatus.HEALTHY
            
        except Exception as e:
            health_check.status = HealthStatus.UNKNOWN
            health_check.error_message = str(e)
    
    async def _system_metrics_loop(self):
        """Collect system metrics periodically"""
        while self.is_monitoring:
            try:
                await asyncio.sleep(60)  # Collect every minute
                
                metrics = await self._collect_system_metrics()
                self.system_metrics.append(metrics)
                
                # Limit metrics history
                max_metrics = self.metrics_retention_hours * 60  # minutes
                if len(self.system_metrics) > max_metrics:
                    self.system_metrics = self.system_metrics[-max_metrics:]
                
                # Log metrics to Logfire
                logfire.info("System metrics collected",
                           cpu_usage=metrics.cpu_usage_percent,
                           memory_usage=metrics.memory_usage_percent,
                           disk_usage=metrics.disk_usage_percent,
                           load_average=metrics.load_average[0])
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("System metrics collection error", error=str(e))
    
    async def _collect_system_metrics(self) -> SystemMetrics:
        """Collect current system metrics"""
        # CPU usage
        cpu_percent = psutil.cpu_percent(interval=1)
        
        # Memory usage
        memory = psutil.virtual_memory()
        memory_percent = memory.percent
        
        # Disk usage (root filesystem)
        disk = psutil.disk_usage('/')
        disk_percent = (disk.used / disk.total) * 100
        
        # Network I/O
        net_io = psutil.net_io_counters()
        network_mbps = (net_io.bytes_sent + net_io.bytes_recv) / (1024 * 1024)  # Rough estimate
        
        # Load average
        load_avg = psutil.getloadavg()
        
        # Process info
        process_count = len(psutil.pids())
        
        # File descriptors
        try:
            fd_count = len(psutil.Process().open_files())
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            fd_count = 0
        
        # System uptime
        uptime = time.time() - psutil.boot_time()
        
        return SystemMetrics(
            cpu_usage_percent=cpu_percent,
            memory_usage_percent=memory_percent,
            disk_usage_percent=disk_percent,
            network_io_mbps=network_mbps,
            load_average=load_avg,
            open_file_descriptors=fd_count,
            process_count=process_count,
            uptime_seconds=uptime
        )
    
    async def _recovery_loop(self):
        """Automated recovery loop"""
        while self.is_monitoring:
            try:
                await asyncio.sleep(60)  # Check for recovery needs every minute
                
                # Check for components that need recovery
                await self._check_recovery_needs()
                
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Recovery loop error", error=str(e))
    
    async def _check_recovery_needs(self):
        """Check if any components need automated recovery"""
        for check_id, health_check in self.health_checks.items():
            if (health_check.status in [HealthStatus.CRITICAL, HealthStatus.UNHEALTHY] and
                health_check.consecutive_failures >= self.critical_threshold):
                
                # Attempt automated recovery
                await self._attempt_recovery(check_id, health_check)
    
    async def _attempt_recovery(self, check_id: str, health_check: HealthCheck):
        """Attempt automated recovery for a failed component"""
        recovery_action_id = f"recovery_{check_id}_{int(time.time())}"
        
        logger.warning("Attempting automated recovery", 
                     check_id=check_id, 
                     consecutive_failures=health_check.consecutive_failures)
        
        # Determine recovery procedure
        recovery_procedure = None
        action_type = "unknown"
        
        if health_check.component_type == ComponentType.DATABASE:
            recovery_procedure = self.recovery_procedures.get('database_connection_recovery')
            action_type = "database_connection_recovery"
        elif health_check.component_type == ComponentType.REDIS:
            recovery_procedure = self.recovery_procedures.get('redis_connection_recovery')
            action_type = "redis_connection_recovery"
        elif health_check.component_type == ComponentType.API_SERVER:
            recovery_procedure = self.recovery_procedures.get('api_server_restart')
            action_type = "api_server_restart"
        elif health_check.component_type == ComponentType.SYSTEM:
            if "memory" in health_check.error_message.lower():
                recovery_procedure = self.recovery_procedures.get('memory_pressure_relief')
                action_type = "memory_pressure_relief"
            elif "disk" in health_check.error_message.lower():
                recovery_procedure = self.recovery_procedures.get('disk_space_cleanup')
                action_type = "disk_space_cleanup"
        
        if recovery_procedure:
            start_time = time.time()
            
            try:
                success = await recovery_procedure(check_id, health_check)
                execution_time = (time.time() - start_time) * 1000
                
                # Record recovery action
                recovery_action = RecoveryAction(
                    action_id=recovery_action_id,
                    component_id=check_id,
                    action_type=action_type,
                    description=f"Automated recovery for {health_check.name}",
                    executed_at=time.time(),
                    success=success,
                    execution_time_ms=execution_time
                )
                
                self.recovery_actions.append(recovery_action)
                
                if success:
                    logger.info("Automated recovery successful", 
                              check_id=check_id, action_type=action_type)
                    
                    logfire.info("Automated recovery completed",
                               component_id=check_id,
                               action_type=action_type,
                               execution_time_ms=execution_time)
                else:
                    logger.error("Automated recovery failed", 
                               check_id=check_id, action_type=action_type)
                
            except Exception as e:
                execution_time = (time.time() - start_time) * 1000
                
                recovery_action = RecoveryAction(
                    action_id=recovery_action_id,
                    component_id=check_id,
                    action_type=action_type,
                    description=f"Automated recovery for {health_check.name}",
                    executed_at=time.time(),
                    success=False,
                    error_message=str(e),
                    execution_time_ms=execution_time
                )
                
                self.recovery_actions.append(recovery_action)
                
                logger.error("Recovery procedure failed", 
                           check_id=check_id, action_type=action_type, error=str(e))
    
    # Recovery procedure implementations
    async def _recover_database_connection(self, check_id: str, health_check: HealthCheck) -> bool:
        """Attempt to recover database connection"""
        try:
            if self.database_manager:
                # Try to reinitialize database connections
                await self.database_manager._initialize_connection_pools()
                return True
            return False
        except Exception:
            return False
    
    async def _recover_redis_connection(self, check_id: str, health_check: HealthCheck) -> bool:
        """Attempt to recover Redis connection"""
        try:
            if self.redis_manager:
                # Try to reinitialize Redis connections
                await self.redis_manager._initialize_redis_connections()
                return True
            return False
        except Exception:
            return False
    
    async def _restart_api_server(self, check_id: str, health_check: HealthCheck) -> bool:
        """Attempt to restart API server (placeholder)"""
        # In a real implementation, this would trigger a container restart
        # or send a restart signal to the process manager
        logger.info("API server restart requested", check_id=check_id)
        return False  # Placeholder - would implement actual restart logic
    
    async def _clear_cache_recovery(self, check_id: str, health_check: HealthCheck) -> bool:
        """Clear cache to free up memory"""
        try:
            if self.redis_manager and self.redis_manager.master_client:
                # Clear non-critical cache data
                await self.redis_manager.master_client.flushdb()
                return True
            return False
        except Exception:
            return False
    
    async def _cleanup_disk_space(self, check_id: str, health_check: HealthCheck) -> bool:
        """Clean up disk space"""
        try:
            # Clean up log files older than 7 days
            import glob
            import os
            
            log_patterns = ['/var/log/*.log.*', '/var/log/*/*.log.*']
            cleaned_files = 0
            
            for pattern in log_patterns:
                for log_file in glob.glob(pattern):
                    try:
                        if os.path.getmtime(log_file) < time.time() - (7 * 24 * 3600):
                            os.remove(log_file)
                            cleaned_files += 1
                    except (OSError, IOError):
                        continue
            
            logger.info("Disk cleanup completed", cleaned_files=cleaned_files)
            return cleaned_files > 0
            
        except Exception:
            return False
    
    async def _relieve_memory_pressure(self, check_id: str, health_check: HealthCheck) -> bool:
        """Attempt to relieve memory pressure"""
        try:
            # Trigger garbage collection
            import gc
            gc.collect()
            
            # Clear Redis cache if available
            if self.redis_manager:
                await self._clear_cache_recovery(check_id, health_check)
            
            return True
            
        except Exception:
            return False
    
    def get_comprehensive_health_status(self) -> Dict[str, Any]:
        """Get comprehensive health status for all components"""
        # Component health summary
        component_status = {}
        healthy_count = 0
        degraded_count = 0
        unhealthy_count = 0
        critical_count = 0
        
        for check_id, health_check in self.health_checks.items():
            component_status[check_id] = {
                "name": health_check.name,
                "component_type": health_check.component_type.value,
                "status": health_check.status.value,
                "response_time_ms": health_check.response_time_ms,
                "consecutive_failures": health_check.consecutive_failures,
                "success_rate": health_check.success_rate,
                "last_check": health_check.last_check,
                "error_message": health_check.error_message,
                "details": health_check.details
            }
            
            # Count by status
            if health_check.status == HealthStatus.HEALTHY:
                healthy_count += 1
            elif health_check.status == HealthStatus.DEGRADED:
                degraded_count += 1
            elif health_check.status == HealthStatus.UNHEALTHY:
                unhealthy_count += 1
            elif health_check.status == HealthStatus.CRITICAL:
                critical_count += 1
        
        # Overall system status
        if critical_count > 0:
            overall_status = HealthStatus.CRITICAL
        elif unhealthy_count > 0:
            overall_status = HealthStatus.UNHEALTHY
        elif degraded_count > 0:
            overall_status = HealthStatus.DEGRADED
        else:
            overall_status = HealthStatus.HEALTHY
        
        # Active alerts summary
        active_alerts_summary = {
            "total": len(self.active_alerts),
            "by_severity": {
                "emergency": len([a for a in self.active_alerts.values() if a.severity == AlertSeverity.EMERGENCY]),
                "critical": len([a for a in self.active_alerts.values() if a.severity == AlertSeverity.CRITICAL]),
                "error": len([a for a in self.active_alerts.values() if a.severity == AlertSeverity.ERROR]),
                "warning": len([a for a in self.active_alerts.values() if a.severity == AlertSeverity.WARNING]),
                "info": len([a for a in self.active_alerts.values() if a.severity == AlertSeverity.INFO])
            }
        }
        
        # Recent recovery actions
        recent_recovery_actions = [
            {
                "action_id": action.action_id,
                "component_id": action.component_id,
                "action_type": action.action_type,
                "success": action.success,
                "executed_at": action.executed_at,
                "execution_time_ms": action.execution_time_ms
            }
            for action in self.recovery_actions[-10:]  # Last 10 actions
        ]
        
        # Current system metrics
        latest_metrics = self.system_metrics[-1] if self.system_metrics else None
        
        return {
            "overall_status": overall_status.value,
            "component_summary": {
                "total": len(self.health_checks),
                "healthy": healthy_count,
                "degraded": degraded_count,
                "unhealthy": unhealthy_count,
                "critical": critical_count
            },
            "components": component_status,
            "active_alerts": active_alerts_summary,
            "recent_recovery_actions": recent_recovery_actions,
            "current_system_metrics": asdict(latest_metrics) if latest_metrics else None,
            "monitoring_config": {
                "check_interval_seconds": self.check_interval,
                "is_monitoring": self.is_monitoring,
                "total_checks_performed": sum(hc.check_count for hc in self.health_checks.values())
            }
        }
```

## TDD Implementation Cycle

### Red Phase: Health Monitoring Test Creation
```python
# infrastructure/tests/test_health_monitoring.py
import pytest
import asyncio
from infrastructure.health_monitoring.health_monitor import ComprehensiveHealthMonitor, HealthStatus

@pytest.mark.asyncio
async def test_health_monitor_initialization():
    """Test health monitor initializes correctly"""
    # This test should initially fail (Red phase)
    assert False, "Health monitor initialization not implemented yet"

@pytest.mark.asyncio
async def test_database_health_checking():
    """Test database health checking functionality"""
    # This test should initially fail (Red phase)
    assert False, "Database health checking not implemented yet"

@pytest.mark.asyncio
async def test_system_metrics_collection():
    """Test system metrics collection"""
    # This test should initially fail (Red phase)
    assert False, "System metrics collection not implemented yet"

@pytest.mark.asyncio
async def test_automated_recovery_procedures():
    """Test automated recovery procedures"""
    # This test should initially fail (Red phase)
    assert False, "Automated recovery procedures not implemented yet"

@pytest.mark.asyncio
async def test_health_alert_generation():
    """Test health alert generation and management"""
    # This test should initially fail (Red phase)
    assert False, "Health alert generation not implemented yet"
```

### Green Phase: Health Monitoring Implementation
```python
# Implement health monitoring features to make tests pass
# This involves adding health checks, metrics collection, and recovery procedures
```

### Refactor Phase: Health Monitoring Optimization
```python
# Optimize health monitoring for performance and reliability
# Add advanced health assessment algorithms and recovery strategies
# Enhance alerting and notification systems
```

## Security Checklist ✅

### Health Monitoring Security
- [ ] Health monitoring system access controls and authentication
- [ ] Secure health data collection and storage
- [ ] Protection against health monitoring system compromise
- [ ] Health check endpoint security and validation
- [ ] Monitoring data encryption and protection
- [ ] Secure communication between monitoring components
- [ ] Health monitoring audit logging and monitoring
- [ ] Protection against false health status injection
- [ ] Monitoring system network isolation and hardening
- [ ] Regular security assessment of monitoring infrastructure

### Recovery System Security
- [ ] Automated recovery procedure security validation
- [ ] Protection against malicious recovery triggers
- [ ] Secure execution of recovery actions
- [ ] Recovery system access controls and authorization
- [ ] Audit logging for all recovery actions
- [ ] Protection against recovery system abuse
- [ ] Secure recovery procedure configuration
- [ ] Recovery action validation and verification
- [ ] Emergency recovery override security
- [ ] Recovery system integration security

### Alert and Notification Security
- [ ] Secure alert generation and processing
- [ ] Alert data protection and confidentiality
- [ ] Notification system security and authentication
- [ ] Protection against alert flooding or manipulation
- [ ] Secure alert escalation procedures
- [ ] Alert acknowledgment security and validation
- [ ] Protection against unauthorized alert access
- [ ] Secure integration with external alerting systems
- [ ] Alert data retention policy enforcement
- [ ] Regular security review of alerting infrastructure

### Data Protection
- [ ] Health monitoring data encryption at rest and in transit
- [ ] Monitoring data access controls and authorization
- [ ] Health metrics data retention policy enforcement
- [ ] Secure backup and recovery of monitoring data
- [ ] Protection against monitoring data correlation attacks
- [ ] Compliance with data protection regulations
- [ ] Monitoring data anonymization and pseudonymization
- [ ] Secure data sharing between monitoring systems
- [ ] Protection against unauthorized monitoring data export
- [ ] Regular assessment of monitoring data security

## Performance Requirements

### Monitoring Performance
- Health check execution time < 5 seconds per check
- System metrics collection latency < 1 second
- Monitoring overhead < 2% of system resources
- Health status update frequency = 30 seconds
- Alert generation time < 10 seconds
- Recovery action execution time < 2 minutes

### System Performance
- Health monitoring system availability > 99.9%
- Concurrent health checks > 100 simultaneous checks
- Metrics data retention efficiency > 90%
- Alert processing throughput > 1000 alerts/minute
- Recovery success rate > 80% for automated procedures
- Monitoring data query response time < 500ms

### Scalability Requirements
- Support 1000+ health checks across platform
- Handle 100+ components monitoring simultaneously
- Scale to multiple geographic regions
- Support 10TB+ of historical monitoring data
- Handle 10,000+ alerts per day
- Support 100+ concurrent recovery operations

## Commit Instructions

After implementing the health monitoring system:

```bash
git add infrastructure/health_monitoring/
git commit -m "Add comprehensive health monitoring with automated recovery

- Implement ComprehensiveHealthMonitor with multi-component health checking
- Add database, Redis, load balancer, and system health monitoring
- Include automated recovery procedures for common failure scenarios
- Add system metrics collection with historical data retention
- Implement health alert generation and management system
- Add recovery action tracking and execution monitoring
- Include comprehensive component health assessment algorithms
- Add integration with notification services for critical alerts
- Implement proactive monitoring and preventive maintenance capabilities
- Add TDD cycle with Red-Green-Refactor for health monitoring
- Ensure >90% health monitoring test coverage

🤖 Generated with [Claude Code](https://claude.ai/code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

## Testing Instructions

Run the complete health monitoring test suite:

```bash
# Run all health monitoring tests
pytest infrastructure/tests/test_health_monitoring.py -v --timeout=300

# Run specific health monitoring test categories
pytest infrastructure/tests/health_monitoring/ -k "health_checks" -v
pytest infrastructure/tests/health_monitoring/ -k "metrics_collection" -v
pytest infrastructure/tests/health_monitoring/ -k "recovery_procedures" -v

# Run health monitoring performance tests
pytest infrastructure/tests/health_monitoring/performance/ -v

# Run health monitoring integration tests
pytest infrastructure/tests/health_monitoring/test_integration.py -v
```

Validate health monitoring test coverage:
```bash
pytest infrastructure/tests/health_monitoring/ --cov=infrastructure.health_monitoring --cov-report=html --cov-fail-under=90
```

## Integration Testing

Test health monitoring integration with all HA components:
```bash
# Test integration with database HA
pytest infrastructure/tests/integration/test_health_monitoring_database_integration.py -v

# Test integration with Redis HA
pytest infrastructure/tests/integration/test_health_monitoring_redis_integration.py -v

# Test integration with load balancer HA
pytest infrastructure/tests/integration/test_health_monitoring_lb_integration.py -v

# Test complete HA stack monitoring
pytest infrastructure/tests/integration/test_health_monitoring_complete_integration.py -v
```