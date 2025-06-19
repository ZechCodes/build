"""Database operation tracking with Logfire."""

import time
import structlog
from typing import Any, Dict, Optional, List
from sqlalchemy import event
from sqlalchemy.engine import Engine
from sqlalchemy.pool import Pool

try:
    import logfire
    LOGFIRE_AVAILABLE = True
except ImportError:
    LOGFIRE_AVAILABLE = False

from .logfire_setup import create_span, log_performance_metric

logger = structlog.get_logger(__name__)


class DatabaseTracker:
    """Database operation tracker with Logfire integration."""
    
    def __init__(self):
        self.query_count = 0
        self.total_query_time = 0.0
        self.slow_query_threshold = 1.0  # 1 second
        self.active_queries: Dict[str, Dict[str, Any]] = {}
    
    def track_query_start(self, query_id: str, statement: str, parameters: Any = None):
        """Track the start of a database query."""
        self.active_queries[query_id] = {
            "statement": statement,
            "parameters": parameters,
            "start_time": time.time(),
            "span": None
        }
        
        if LOGFIRE_AVAILABLE:
            try:
                span = create_span(
                    "Database Query",
                    query_id=query_id,
                    statement=self._sanitize_query(statement),
                    has_parameters=parameters is not None
                )
                self.active_queries[query_id]["span"] = span.__enter__()
            except Exception as e:
                logger.debug("Failed to create query span", error=str(e))
    
    def track_query_end(self, query_id: str, success: bool = True, error: Optional[Exception] = None):
        """Track the end of a database query."""
        if query_id not in self.active_queries:
            return
        
        query_info = self.active_queries.pop(query_id)
        duration = time.time() - query_info["start_time"]
        
        self.query_count += 1
        self.total_query_time += duration
        
        # Update span if available
        span = query_info.get("span")
        if span:
            try:
                span.set_attribute("query.duration_ms", round(duration * 1000, 2))
                span.set_attribute("query.success", success)
                
                if error:
                    span.record_exception(error)
                    span.set_attribute("query.error", str(error))
                
                span.__exit__(None, None, None)
            except Exception as e:
                logger.debug("Failed to update query span", error=str(e))
        
        # Log slow queries
        if duration > self.slow_query_threshold:
            logger.warning(
                "Slow database query",
                query_id=query_id,
                duration_ms=round(duration * 1000, 2),
                statement=self._sanitize_query(query_info["statement"]),
                success=success,
                error=str(error) if error else None
            )
        
        # Record performance metric
        log_performance_metric(
            "database_query_duration",
            duration * 1000,
            unit="ms",
            tags={
                "success": str(success),
                "query_type": self._get_query_type(query_info["statement"])
            }
        )
    
    def get_stats(self) -> Dict[str, Any]:
        """Get database operation statistics."""
        avg_query_time = (
            self.total_query_time / self.query_count 
            if self.query_count > 0 else 0
        )
        
        return {
            "total_queries": self.query_count,
            "total_query_time_ms": round(self.total_query_time * 1000, 2),
            "average_query_time_ms": round(avg_query_time * 1000, 2),
            "active_queries": len(self.active_queries)
        }
    
    def reset_stats(self):
        """Reset tracking statistics."""
        self.query_count = 0
        self.total_query_time = 0.0
        self.active_queries.clear()
    
    def _sanitize_query(self, statement: str) -> str:
        """Sanitize SQL statement for logging."""
        # Remove excessive whitespace
        statement = " ".join(statement.split())
        
        # Truncate very long statements
        if len(statement) > 500:
            statement = statement[:497] + "..."
        
        return statement
    
    def _get_query_type(self, statement: str) -> str:
        """Extract query type from SQL statement."""
        statement = statement.strip().upper()
        if statement.startswith("SELECT"):
            return "SELECT"
        elif statement.startswith("INSERT"):
            return "INSERT"
        elif statement.startswith("UPDATE"):
            return "UPDATE"
        elif statement.startswith("DELETE"):
            return "DELETE"
        elif statement.startswith("CREATE"):
            return "CREATE"
        elif statement.startswith("ALTER"):
            return "ALTER"
        elif statement.startswith("DROP"):
            return "DROP"
        else:
            return "OTHER"


# Global database tracker instance
db_tracker = DatabaseTracker()


class SQLAlchemyLogfirePlugin:
    """SQLAlchemy plugin for Logfire integration."""
    
    def __init__(self, engine: Engine):
        self.engine = engine
        self.setup_event_listeners()
    
    def setup_event_listeners(self):
        """Setup SQLAlchemy event listeners for tracking."""
        
        @event.listens_for(self.engine, "before_cursor_execute")
        def before_cursor_execute(conn, cursor, statement, parameters, context, executemany):
            """Track query start."""
            query_id = f"{id(conn)}_{id(cursor)}_{time.time()}"
            context._logfire_query_id = query_id
            context._logfire_start_time = time.time()
            
            db_tracker.track_query_start(query_id, statement, parameters)
        
        @event.listens_for(self.engine, "after_cursor_execute")
        def after_cursor_execute(conn, cursor, statement, parameters, context, executemany):
            """Track successful query completion."""
            query_id = getattr(context, "_logfire_query_id", None)
            if query_id:
                db_tracker.track_query_end(query_id, success=True)
        
        @event.listens_for(self.engine, "handle_error")
        def handle_error(exception_context):
            """Track query errors."""
            context = exception_context.execution_context
            query_id = getattr(context, "_logfire_query_id", None) if context else None
            
            if query_id:
                db_tracker.track_query_end(
                    query_id, 
                    success=False, 
                    error=exception_context.original_exception
                )


class ConnectionPoolTracker:
    """Track database connection pool metrics."""
    
    def __init__(self, pool: Pool):
        self.pool = pool
        self.setup_pool_monitoring()
    
    def setup_pool_monitoring(self):
        """Setup connection pool monitoring."""
        
        @event.listens_for(self.pool, "connect")
        def on_connect(dbapi_conn, connection_record):
            """Track new connections."""
            logger.debug("Database connection established")
            
            if LOGFIRE_AVAILABLE:
                try:
                    logfire.log(
                        "Database Connection",
                        event="connect",
                        pool_size=self.pool.size(),
                        checked_out=self.pool.checkedout(),
                        overflow=self.pool.overflow(),
                        checked_in=self.pool.checkedin()
                    )
                except Exception as e:
                    logger.debug("Failed to log connection event", error=str(e))
        
        @event.listens_for(self.pool, "checkout")
        def on_checkout(dbapi_conn, connection_record, connection_proxy):
            """Track connection checkout."""
            pool_stats = self.get_pool_stats()
            
            # Log if pool utilization is high
            if pool_stats["utilization"] > 0.8:
                logger.warning(
                    "High database connection pool utilization",
                    **pool_stats
                )
        
        @event.listens_for(self.pool, "checkin")
        def on_checkin(dbapi_conn, connection_record):
            """Track connection checkin."""
            logger.debug("Database connection returned to pool")
    
    def get_pool_stats(self) -> Dict[str, Any]:
        """Get connection pool statistics."""
        try:
            pool_size = self.pool.size()
            checked_out = self.pool.checkedout()
            overflow = self.pool.overflow()
            checked_in = self.pool.checkedin()
            
            utilization = checked_out / pool_size if pool_size > 0 else 0
            
            return {
                "pool_size": pool_size,
                "checked_out": checked_out,
                "checked_in": checked_in,
                "overflow": overflow,
                "utilization": round(utilization, 2),
                "available": pool_size - checked_out
            }
        except Exception as e:
            logger.error("Failed to get pool stats", error=str(e))
            return {}


def setup_database_tracking(engine: Engine) -> SQLAlchemyLogfirePlugin:
    """Setup database tracking for SQLAlchemy engine."""
    plugin = SQLAlchemyLogfirePlugin(engine)
    
    # Also setup pool tracking if available
    if hasattr(engine, "pool"):
        ConnectionPoolTracker(engine.pool)
    
    logger.info("Database tracking configured with Logfire")
    return plugin


def log_database_operation(
    operation: str,
    table: str,
    record_id: Optional[str] = None,
    user_id: Optional[str] = None,
    metadata: Optional[Dict[str, Any]] = None
) -> None:
    """Log database operation for audit purposes."""
    operation_data = {
        "operation": operation,
        "table": table,
        "record_id": record_id,
        "user_id": user_id,
        "metadata": metadata or {}
    }
    
    if LOGFIRE_AVAILABLE:
        try:
            with create_span("Database Operation", **operation_data):
                logger.info("Database operation", **operation_data)
        except Exception as e:
            logger.debug("Failed to create database operation span", error=str(e))
            logger.info("Database operation", **operation_data)
    else:
        logger.info("Database operation", **operation_data)


def get_database_metrics() -> Dict[str, Any]:
    """Get comprehensive database metrics."""
    return {
        "query_stats": db_tracker.get_stats(),
        "timestamp": time.time()
    }