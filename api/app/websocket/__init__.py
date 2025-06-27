"""WebSocket gateway for real-time terminal communication."""

from .auth import WebSocketAuthenticator, WebSocketConnectionManager as BasicConnectionManager
from .gateway import websocket_terminal_endpoint, start_websocket_services, stop_websocket_services
from .connections import TerminalConnectionManager, get_connection_manager
from .protocols import MessageProtocol, MessageType
from .handlers import MessageRouter

__all__ = [
    "WebSocketAuthenticator",
    "BasicConnectionManager", 
    "websocket_terminal_endpoint",
    "start_websocket_services",
    "stop_websocket_services",
    "TerminalConnectionManager",
    "get_connection_manager", 
    "MessageProtocol",
    "MessageType",
    "MessageRouter"
]