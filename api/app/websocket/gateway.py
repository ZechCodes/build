"""WebSocket gateway for terminal connections."""

import asyncio
import json
from fastapi import WebSocket, WebSocketDisconnect, Depends, HTTPException
import structlog

from .connections import get_connection_manager, TerminalConnectionManager
from .handlers import MessageRouter
from app.core.deps import get_current_user
from app.models.user import User

logger = structlog.get_logger(__name__)

# Global message router instance
message_router: MessageRouter = None


def get_message_router() -> MessageRouter:
    """Get the global message router instance."""
    global message_router
    if message_router is None:
        connection_manager = get_connection_manager()
        message_router = MessageRouter(connection_manager)
    return message_router


async def websocket_terminal_endpoint(
    websocket: WebSocket,
    token: str = None
):
    """WebSocket endpoint for terminal connections."""
    connection_manager = get_connection_manager()
    router = get_message_router()
    connection_id = None
    
    try:
        # Accept the WebSocket connection
        connection_id = await connection_manager.connect(websocket)
        
        logger.info("WebSocket connection established", connection_id=connection_id)
        
        # Authenticate the connection
        if not await connection_manager.authenticate(connection_id):
            logger.warning("WebSocket authentication failed", connection_id=connection_id)
            await websocket.close(code=1008)  # Policy violation
            return
        
        connection = connection_manager.get_connection(connection_id)
        logger.info("WebSocket authenticated successfully", 
                   connection_id=connection_id,
                   user_id=connection.user_id)
        
        # Main message handling loop
        while True:
            try:
                # Receive message from client
                data = await websocket.receive_text()
                
                # Route the message
                await router.route_message(connection_id, data)
                
            except WebSocketDisconnect:
                logger.info("WebSocket client disconnected", connection_id=connection_id)
                break
            except json.JSONDecodeError as e:
                logger.warning("Invalid JSON received", 
                             connection_id=connection_id,
                             error=str(e))
                # Send error message
                error_msg = {
                    "type": "error",
                    "data": {"code": "INVALID_JSON", "message": "Invalid JSON format"}
                }
                await websocket.send_json(error_msg)
            except Exception as e:
                logger.error("Error handling WebSocket message", 
                           connection_id=connection_id,
                           error=str(e))
                # Send error message
                error_msg = {
                    "type": "error", 
                    "data": {"code": "INTERNAL_ERROR", "message": "Internal server error"}
                }
                try:
                    await websocket.send_json(error_msg)
                except:
                    # If we can't send error, connection is probably broken
                    break
    
    except Exception as e:
        logger.error("WebSocket connection error", 
                    connection_id=connection_id,
                    error=str(e))
    
    finally:
        # Clean up connection
        if connection_id:
            await connection_manager.disconnect(connection_id)
            logger.info("WebSocket connection cleaned up", connection_id=connection_id)


async def start_websocket_services():
    """Start WebSocket-related services."""
    try:
        connection_manager = get_connection_manager()
        await connection_manager.start()
        
        # Start PTY manager
        router = get_message_router()
        await router.pty_manager.start()
        
        logger.info("WebSocket services started successfully")
        
    except Exception as e:
        logger.error("Failed to start WebSocket services", error=str(e))
        raise


async def stop_websocket_services():
    """Stop WebSocket-related services."""
    try:
        connection_manager = get_connection_manager()
        await connection_manager.stop()
        
        # Stop PTY manager
        router = get_message_router()
        await router.pty_manager.stop()
        
        logger.info("WebSocket services stopped successfully")
        
    except Exception as e:
        logger.error("Failed to stop WebSocket services", error=str(e))