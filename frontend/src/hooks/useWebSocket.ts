import { useState, useEffect, useRef, useCallback } from 'react';
import { WebSocketManager } from '../services/WebSocketManager';

interface UseWebSocketConfig {
  url: string;
  token: string;
  onConnect?: () => void;
  onDisconnect?: () => void;
  onMessage?: (message: any) => void;
  onError?: (error: Error) => void;
}

interface UseWebSocketReturn {
  isConnected: boolean;
  isReconnecting: boolean;
  connectionError: string | null;
  sendMessage: (message: any) => void;
  disconnect: () => void;
  wsManager: WebSocketManager | null;
}

export function useWebSocket(config: UseWebSocketConfig): UseWebSocketReturn {
  const [isConnected, setIsConnected] = useState(false);
  const [isReconnecting, setIsReconnecting] = useState(false);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const wsManagerRef = useRef<WebSocketManager | null>(null);

  const handleConnect = useCallback(() => {
    setIsConnected(true);
    setIsReconnecting(false);
    setConnectionError(null);
    if (config.onConnect) {
      config.onConnect();
    }
  }, [config.onConnect]);

  const handleDisconnect = useCallback(() => {
    setIsConnected(false);
    if (config.onDisconnect) {
      config.onDisconnect();
    }
  }, [config.onDisconnect]);

  const handleReconnecting = useCallback(() => {
    setIsReconnecting(true);
    setConnectionError(null);
  }, []);

  const handleMessage = useCallback((message: any) => {
    if (config.onMessage) {
      config.onMessage(message);
    }
  }, [config.onMessage]);

  const handleError = useCallback((error: Error) => {
    setConnectionError(error.message);
    if (config.onError) {
      config.onError(error);
    }
  }, [config.onError]);

  // Initialize WebSocket connection
  useEffect(() => {
    if (!config.url || !config.token) {
      return;
    }

    wsManagerRef.current = new WebSocketManager({
      apiUrl: config.url,
      token: config.token,
      onConnect: handleConnect,
      onDisconnect: handleDisconnect,
      onReconnecting: handleReconnecting,
      onMessage: handleMessage,
      onError: handleError,
    });

    return () => {
      if (wsManagerRef.current) {
        wsManagerRef.current.disconnect();
        wsManagerRef.current = null;
      }
    };
  }, [config.url, config.token, handleConnect, handleDisconnect, handleReconnecting, handleMessage, handleError]);

  const sendMessage = useCallback((message: any) => {
    if (wsManagerRef.current && isConnected) {
      wsManagerRef.current.sendTerminalData(JSON.stringify(message));
    }
  }, [isConnected]);

  const disconnect = useCallback(() => {
    if (wsManagerRef.current) {
      wsManagerRef.current.disconnect();
    }
  }, []);

  return {
    isConnected,
    isReconnecting,
    connectionError,
    sendMessage,
    disconnect,
    wsManager: wsManagerRef.current,
  };
}