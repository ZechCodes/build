# Session 8.2: WebSocket Client & Connection Management

## Objective
Implement robust WebSocket client with intelligent connection management, automatic reconnection, message queuing, and efficient real-time communication for terminal sessions.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for WebSocket connection monitoring and debugging
- **Session 2**: Integrates with authentication system for secure WebSocket connections
- **Session 6**: Connects to WebSocket gateway for session management communication
- **Session 4**: Coordinates with PTY layer for terminal data transmission

## Core Implementation

### WebSocket Client Manager
**Location**: `frontend/src/services/WebSocketClient.ts`

```typescript
// frontend/src/services/WebSocketClient.ts
import { logfire } from '../utils/logfire';

export enum ConnectionState {
  DISCONNECTED = 'disconnected',
  CONNECTING = 'connecting',
  CONNECTED = 'connected',
  RECONNECTING = 'reconnecting',
  FAILED = 'failed'
}

export interface WebSocketMessage {
  type: string;
  sessionId?: string;
  data?: any;
  timestamp?: number;
  id?: string;
}

export interface WebSocketConfig {
  url: string;
  protocols?: string[];
  reconnectInterval?: number;
  maxReconnectAttempts?: number;
  heartbeatInterval?: number;
  messageTimeout?: number;
  maxQueueSize?: number;
  enableCompression?: boolean;
}

export interface ConnectionMetrics {
  connectTime: number;
  lastMessageTime: number;
  messagesSent: number;
  messagesReceived: number;
  reconnectAttempts: number;
  totalDowntime: number;
  averageLatency: number;
  currentLatency?: number;
}

export class WebSocketClient {
  private ws: WebSocket | null = null;
  private config: Required<WebSocketConfig>;
  private state: ConnectionState = ConnectionState.DISCONNECTED;
  private reconnectTimer: NodeJS.Timeout | null = null;
  private heartbeatTimer: NodeJS.Timeout | null = null;
  private messageQueue: WebSocketMessage[] = [];
  private pendingMessages: Map<string, { 
    resolve: (value: any) => void; 
    reject: (error: Error) => void;
    timeout: NodeJS.Timeout;
  }> = new Map();
  
  private metrics: ConnectionMetrics = {
    connectTime: 0,
    lastMessageTime: 0,
    messagesSent: 0,
    messagesReceived: 0,
    reconnectAttempts: 0,
    totalDowntime: 0,
    averageLatency: 0
  };

  private eventListeners: Map<string, ((data: any) => void)[]> = new Map();
  private authToken: string | null = null;
  private userId: string | null = null;

  constructor(config: WebSocketConfig) {
    this.config = {
      reconnectInterval: 3000,
      maxReconnectAttempts: 10,
      heartbeatInterval: 30000,
      messageTimeout: 5000,
      maxQueueSize: 1000,
      enableCompression: true,
      protocols: [],
      ...config
    };

    // Bind methods to preserve context
    this.handleOpen = this.handleOpen.bind(this);
    this.handleMessage = this.handleMessage.bind(this);
    this.handleError = this.handleError.bind(this);
    this.handleClose = this.handleClose.bind(this);
    this.sendHeartbeat = this.sendHeartbeat.bind(this);
  }

  public async connect(authToken: string, userId: string): Promise<void> {
    if (this.state === ConnectionState.CONNECTED || this.state === ConnectionState.CONNECTING) {
      return;
    }

    this.authToken = authToken;
    this.userId = userId;
    this.setState(ConnectionState.CONNECTING);

    try {
      logfire.info('Initiating WebSocket connection', {
        url: this.config.url,
        userId,
        protocols: this.config.protocols
      });

      const wsUrl = this.buildConnectionUrl();
      this.ws = new WebSocket(wsUrl, this.config.protocols);

      // Configure WebSocket
      if (this.config.enableCompression) {
        // Note: Compression is handled by the browser automatically
        // when supported by both client and server
      }

      this.setupEventHandlers();

      // Connection timeout
      const connectTimeout = setTimeout(() => {
        if (this.state === ConnectionState.CONNECTING) {
          this.handleConnectionTimeout();
        }
      }, 10000);

      return new Promise((resolve, reject) => {
        const onConnect = () => {
          clearTimeout(connectTimeout);
          this.removeEventListener('connected', onConnect);
          this.removeEventListener('error', onError);
          resolve();
        };

        const onError = (error: Error) => {
          clearTimeout(connectTimeout);
          this.removeEventListener('connected', onConnect);
          this.removeEventListener('error', onError);
          reject(error);
        };

        this.addEventListener('connected', onConnect);
        this.addEventListener('error', onError);
      });

    } catch (error) {
      this.setState(ConnectionState.FAILED);
      const errorMessage = error instanceof Error ? error.message : 'Unknown connection error';
      
      logfire.error('WebSocket connection failed', {
        url: this.config.url,
        userId,
        error: errorMessage
      });
      
      throw new Error(`WebSocket connection failed: ${errorMessage}`);
    }
  }

  public disconnect(): void {
    logfire.info('Disconnecting WebSocket', {
      userId: this.userId,
      state: this.state
    });

    this.clearTimers();
    this.setState(ConnectionState.DISCONNECTED);

    if (this.ws) {
      this.ws.close(1000, 'Client disconnect');
      this.ws = null;
    }

    // Clear pending messages
    this.pendingMessages.forEach(({ reject, timeout }) => {
      clearTimeout(timeout);
      reject(new Error('Connection closed'));
    });
    this.pendingMessages.clear();

    // Clear message queue
    this.messageQueue = [];
  }

  public async sendMessage(message: WebSocketMessage): Promise<any> {
    const messageWithId = {
      ...message,
      id: this.generateMessageId(),
      timestamp: Date.now()
    };

    if (this.state !== ConnectionState.CONNECTED) {
      // Queue message for later delivery
      if (this.messageQueue.length < this.config.maxQueueSize) {
        this.messageQueue.push(messageWithId);
        logfire.debug('Message queued for delivery', {
          messageType: message.type,
          queueSize: this.messageQueue.length,
          userId: this.userId
        });
        return;
      } else {
        throw new Error('Message queue is full');
      }
    }

    return this.sendMessageDirectly(messageWithId);
  }

  private async sendMessageDirectly(message: WebSocketMessage): Promise<any> {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      throw new Error('WebSocket not connected');
    }

    try {
      const messageString = JSON.stringify(message);
      this.ws.send(messageString);

      this.metrics.messagesSent++;
      this.metrics.lastMessageTime = Date.now();

      logfire.debug('WebSocket message sent', {
        messageType: message.type,
        messageId: message.id,
        size: messageString.length,
        userId: this.userId
      });

      // Handle response-required messages
      if (message.type.endsWith('_request') || message.id) {
        return new Promise((resolve, reject) => {
          const timeout = setTimeout(() => {
            this.pendingMessages.delete(message.id!);
            reject(new Error('Message timeout'));
          }, this.config.messageTimeout);

          this.pendingMessages.set(message.id!, { resolve, reject, timeout });
        });
      }

    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : 'Unknown send error';
      logfire.error('Failed to send WebSocket message', {
        messageType: message.type,
        messageId: message.id,
        error: errorMessage,
        userId: this.userId
      });
      throw error;
    }
  }

  private buildConnectionUrl(): string {
    const url = new URL(this.config.url);
    
    if (this.authToken) {
      url.searchParams.set('token', this.authToken);
    }
    
    if (this.userId) {
      url.searchParams.set('userId', this.userId);
    }

    // Add protocol version
    url.searchParams.set('version', '1.0');

    return url.toString();
  }

  private setupEventHandlers(): void {
    if (!this.ws) return;

    this.ws.addEventListener('open', this.handleOpen);
    this.ws.addEventListener('message', this.handleMessage);
    this.ws.addEventListener('error', this.handleError);
    this.ws.addEventListener('close', this.handleClose);
  }

  private handleOpen(): void {
    this.setState(ConnectionState.CONNECTED);
    this.metrics.connectTime = Date.now();
    this.metrics.reconnectAttempts = 0;

    logfire.info('WebSocket connected successfully', {
      url: this.config.url,
      userId: this.userId,
      reconnectAttempts: this.metrics.reconnectAttempts
    });

    // Start heartbeat
    this.startHeartbeat();

    // Process queued messages
    this.processQueuedMessages();

    // Emit connected event
    this.emit('connected', {
      timestamp: Date.now(),
      reconnectAttempts: this.metrics.reconnectAttempts
    });
  }

  private handleMessage(event: MessageEvent): void {
    try {
      const message: WebSocketMessage = JSON.parse(event.data);
      this.metrics.messagesReceived++;
      this.metrics.lastMessageTime = Date.now();

      // Calculate latency if message has timestamp
      if (message.timestamp) {
        const latency = Date.now() - message.timestamp;
        this.updateLatencyMetrics(latency);
      }

      logfire.debug('WebSocket message received', {
        messageType: message.type,
        messageId: message.id,
        userId: this.userId
      });

      // Handle response messages
      if (message.id && this.pendingMessages.has(message.id)) {
        const pending = this.pendingMessages.get(message.id)!;
        clearTimeout(pending.timeout);
        this.pendingMessages.delete(message.id);
        pending.resolve(message.data);
        return;
      }

      // Handle specific message types
      switch (message.type) {
        case 'pong':
          this.handlePongMessage(message);
          break;
        case 'error':
          this.handleErrorMessage(message);
          break;
        case 'session_state_changed':
          this.handleSessionStateChanged(message);
          break;
        default:
          // Emit to listeners
          this.emit(message.type, message);
          this.emit('message', message);
      }

    } catch (error) {
      logfire.error('Failed to process WebSocket message', {
        error: error instanceof Error ? error.message : 'Unknown error',
        rawMessage: event.data,
        userId: this.userId
      });
    }
  }

  private handleError(event: Event): void {
    const error = new Error('WebSocket error occurred');
    
    logfire.error('WebSocket error', {
      userId: this.userId,
      state: this.state,
      readyState: this.ws?.readyState
    });

    this.emit('error', error);

    // Trigger reconnection if not manually disconnected
    if (this.state !== ConnectionState.DISCONNECTED) {
      this.handleConnectionLoss();
    }
  }

  private handleClose(event: CloseEvent): void {
    const wasConnected = this.state === ConnectionState.CONNECTED;
    
    logfire.info('WebSocket connection closed', {
      code: event.code,
      reason: event.reason,
      wasClean: event.wasClean,
      userId: this.userId,
      wasConnected
    });

    this.clearTimers();
    this.ws = null;

    if (event.code !== 1000 && this.state !== ConnectionState.DISCONNECTED) {
      // Unexpected disconnection
      this.handleConnectionLoss();
    } else {
      this.setState(ConnectionState.DISCONNECTED);
    }

    this.emit('disconnected', {
      code: event.code,
      reason: event.reason,
      wasClean: event.wasClean,
      timestamp: Date.now()
    });
  }

  private handleConnectionLoss(): void {
    if (this.state === ConnectionState.DISCONNECTED) {
      return;
    }

    this.setState(ConnectionState.RECONNECTING);
    this.scheduleReconnect();
  }

  private handleConnectionTimeout(): void {
    logfire.warning('WebSocket connection timeout', {
      userId: this.userId,
      url: this.config.url
    });

    if (this.ws) {
      this.ws.close();
    }

    this.handleConnectionLoss();
  }

  private scheduleReconnect(): void {
    if (this.metrics.reconnectAttempts >= this.config.maxReconnectAttempts) {
      this.setState(ConnectionState.FAILED);
      
      logfire.error('Max reconnection attempts reached', {
        attempts: this.metrics.reconnectAttempts,
        maxAttempts: this.config.maxReconnectAttempts,
        userId: this.userId
      });

      this.emit('maxReconnectAttemptsReached', {
        attempts: this.metrics.reconnectAttempts,
        timestamp: Date.now()
      });
      return;
    }

    const delay = Math.min(
      this.config.reconnectInterval * Math.pow(2, this.metrics.reconnectAttempts),
      30000 // Max 30 seconds
    );

    logfire.info('Scheduling WebSocket reconnection', {
      attempt: this.metrics.reconnectAttempts + 1,
      delay,
      userId: this.userId
    });

    this.reconnectTimer = setTimeout(async () => {
      if (this.state === ConnectionState.RECONNECTING && this.authToken && this.userId) {
        this.metrics.reconnectAttempts++;
        
        try {
          await this.connect(this.authToken, this.userId);
        } catch (error) {
          logfire.error('Reconnection attempt failed', {
            attempt: this.metrics.reconnectAttempts,
            error: error instanceof Error ? error.message : 'Unknown error',
            userId: this.userId
          });
          
          this.scheduleReconnect();
        }
      }
    }, delay);
  }

  private startHeartbeat(): void {
    this.heartbeatTimer = setInterval(this.sendHeartbeat, this.config.heartbeatInterval);
  }

  private sendHeartbeat(): void {
    if (this.state === ConnectionState.CONNECTED) {
      const pingMessage: WebSocketMessage = {
        type: 'ping',
        timestamp: Date.now()
      };

      this.sendMessageDirectly(pingMessage).catch((error) => {
        logfire.warning('Heartbeat failed', {
          error: error.message,
          userId: this.userId
        });
      });
    }
  }

  private handlePongMessage(message: WebSocketMessage): void {
    if (message.timestamp) {
      const latency = Date.now() - message.timestamp;
      this.updateLatencyMetrics(latency);
      
      logfire.debug('Heartbeat pong received', {
        latency,
        userId: this.userId
      });
    }
  }

  private handleErrorMessage(message: WebSocketMessage): void {
    logfire.error('Server error message received', {
      error: message.data,
      userId: this.userId
    });

    this.emit('serverError', message.data);
  }

  private handleSessionStateChanged(message: WebSocketMessage): void {
    logfire.info('Session state changed', {
      sessionId: message.sessionId,
      newState: message.data?.state,
      userId: this.userId
    });

    this.emit('sessionStateChanged', message.data);
  }

  private async processQueuedMessages(): Promise<void> {
    const queuedMessages = [...this.messageQueue];
    this.messageQueue = [];

    logfire.info('Processing queued messages', {
      count: queuedMessages.length,
      userId: this.userId
    });

    for (const message of queuedMessages) {
      try {
        await this.sendMessageDirectly(message);
      } catch (error) {
        logfire.error('Failed to send queued message', {
          messageType: message.type,
          error: error instanceof Error ? error.message : 'Unknown error',
          userId: this.userId
        });
        
        // Re-queue if connection is still active
        if (this.state === ConnectionState.CONNECTED && this.messageQueue.length < this.config.maxQueueSize) {
          this.messageQueue.push(message);
        }
      }
    }
  }

  private updateLatencyMetrics(latency: number): void {
    this.metrics.currentLatency = latency;
    
    // Calculate rolling average
    const alpha = 0.125; // EMA smoothing factor
    if (this.metrics.averageLatency === 0) {
      this.metrics.averageLatency = latency;
    } else {
      this.metrics.averageLatency = (alpha * latency) + ((1 - alpha) * this.metrics.averageLatency);
    }
  }

  private clearTimers(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }

    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  private setState(newState: ConnectionState): void {
    const oldState = this.state;
    this.state = newState;

    if (oldState !== newState) {
      logfire.debug('WebSocket state changed', {
        from: oldState,
        to: newState,
        userId: this.userId
      });

      this.emit('stateChanged', { from: oldState, to: newState });
    }
  }

  private generateMessageId(): string {
    return `msg_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
  }

  // Event system
  public addEventListener(event: string, listener: (data: any) => void): void {
    if (!this.eventListeners.has(event)) {
      this.eventListeners.set(event, []);
    }
    this.eventListeners.get(event)!.push(listener);
  }

  public removeEventListener(event: string, listener: (data: any) => void): void {
    const listeners = this.eventListeners.get(event);
    if (listeners) {
      const index = listeners.indexOf(listener);
      if (index > -1) {
        listeners.splice(index, 1);
      }
    }
  }

  private emit(event: string, data?: any): void {
    const listeners = this.eventListeners.get(event);
    if (listeners) {
      listeners.forEach(listener => {
        try {
          listener(data);
        } catch (error) {
          logfire.error('Event listener error', {
            event,
            error: error instanceof Error ? error.message : 'Unknown error',
            userId: this.userId
          });
        }
      });
    }
  }

  // Public getters
  public getState(): ConnectionState {
    return this.state;
  }

  public getMetrics(): ConnectionMetrics {
    return { ...this.metrics };
  }

  public isConnected(): boolean {
    return this.state === ConnectionState.CONNECTED;
  }

  public getQueueSize(): number {
    return this.messageQueue.length;
  }

  public getPendingMessageCount(): number {
    return this.pendingMessages.size;
  }
}
```

### WebSocket React Hook
**Location**: `frontend/src/hooks/useWebSocket.ts`

```typescript
// frontend/src/hooks/useWebSocket.ts
import { useState, useEffect, useCallback, useRef, useContext } from 'react';
import { WebSocketClient, ConnectionState, WebSocketMessage, ConnectionMetrics } from '../services/WebSocketClient';
import { AuthContext } from '../contexts/AuthContext';
import { logfire } from '../utils/logfire';

interface UseWebSocketConfig {
  url?: string;
  autoConnect?: boolean;
  reconnectOnAuthChange?: boolean;
  onMessage?: (message: WebSocketMessage) => void;
  onConnect?: () => void;
  onDisconnect?: () => void;
  onError?: (error: Error) => void;
}

interface UseWebSocketReturn {
  connectionState: ConnectionState;
  lastMessage: WebSocketMessage | null;
  sendMessage: (message: WebSocketMessage) => Promise<any>;
  connect: () => Promise<void>;
  disconnect: () => void;
  isConnected: boolean;
  metrics: ConnectionMetrics;
  queueSize: number;
  pendingMessages: number;
}

const DEFAULT_WS_URL = process.env.REACT_APP_WS_URL || 'ws://localhost:8000/ws';

export const useWebSocket = (config: UseWebSocketConfig = {}): UseWebSocketReturn => {
  const {
    url = DEFAULT_WS_URL,
    autoConnect = true,
    reconnectOnAuthChange = true,
    onMessage,
    onConnect,
    onDisconnect,
    onError
  } = config;

  const [connectionState, setConnectionState] = useState<ConnectionState>(ConnectionState.DISCONNECTED);
  const [lastMessage, setLastMessage] = useState<WebSocketMessage | null>(null);
  const [metrics, setMetrics] = useState<ConnectionMetrics>({
    connectTime: 0,
    lastMessageTime: 0,
    messagesSent: 0,
    messagesReceived: 0,
    reconnectAttempts: 0,
    totalDowntime: 0,
    averageLatency: 0
  });
  const [queueSize, setQueueSize] = useState(0);
  const [pendingMessages, setPendingMessages] = useState(0);

  const clientRef = useRef<WebSocketClient | null>(null);
  const { user, token } = useContext(AuthContext);

  // Initialize WebSocket client
  useEffect(() => {
    if (!clientRef.current) {
      clientRef.current = new WebSocketClient({
        url,
        reconnectInterval: 3000,
        maxReconnectAttempts: 10,
        heartbeatInterval: 30000,
        messageTimeout: 5000,
        maxQueueSize: 100
      });

      const client = clientRef.current;

      // Set up event listeners
      client.addEventListener('stateChanged', ({ to }) => {
        setConnectionState(to);
        
        // Update metrics
        setMetrics(client.getMetrics());
        setQueueSize(client.getQueueSize());
        setPendingMessages(client.getPendingMessageCount());
      });

      client.addEventListener('message', (message: WebSocketMessage) => {
        setLastMessage(message);
        onMessage?.(message);
        
        // Update metrics
        setMetrics(client.getMetrics());
      });

      client.addEventListener('connected', () => {
        logfire.info('WebSocket hook - connected', {
          userId: user?.id,
          url
        });
        onConnect?.();
      });

      client.addEventListener('disconnected', () => {
        logfire.info('WebSocket hook - disconnected', {
          userId: user?.id,
          url
        });
        onDisconnect?.();
      });

      client.addEventListener('error', (error: Error) => {
        logfire.error('WebSocket hook - error', {
          userId: user?.id,
          url,
          error: error.message
        });
        onError?.(error);
      });

      client.addEventListener('maxReconnectAttemptsReached', () => {
        logfire.error('WebSocket hook - max reconnect attempts reached', {
          userId: user?.id,
          url
        });
        onError?.(new Error('Maximum reconnection attempts reached'));
      });
    }

    return () => {
      if (clientRef.current) {
        clientRef.current.disconnect();
        clientRef.current = null;
      }
    };
  }, [url]);

  // Auto-connect when user is authenticated
  useEffect(() => {
    if (autoConnect && user && token && clientRef.current) {
      const client = clientRef.current;
      
      if (client.getState() === ConnectionState.DISCONNECTED) {
        client.connect(token, user.id).catch((error) => {
          logfire.error('Auto-connect failed', {
            userId: user.id,
            error: error.message
          });
        });
      }
    } else if (!user && clientRef.current) {
      // Disconnect when user logs out
      clientRef.current.disconnect();
    }
  }, [user, token, autoConnect]);

  // Reconnect on auth change if enabled
  useEffect(() => {
    if (reconnectOnAuthChange && user && token && clientRef.current) {
      const client = clientRef.current;
      
      if (client.getState() === ConnectionState.CONNECTED) {
        // Reconnect with new credentials
        client.disconnect();
        setTimeout(() => {
          client.connect(token, user.id).catch((error) => {
            logfire.error('Reconnect on auth change failed', {
              userId: user.id,
              error: error.message
            });
          });
        }, 1000);
      }
    }
  }, [token, reconnectOnAuthChange]);

  const connect = useCallback(async (): Promise<void> => {
    if (!user || !token) {
      throw new Error('Authentication required for WebSocket connection');
    }

    if (!clientRef.current) {
      throw new Error('WebSocket client not initialized');
    }

    return clientRef.current.connect(token, user.id);
  }, [user, token]);

  const disconnect = useCallback((): void => {
    if (clientRef.current) {
      clientRef.current.disconnect();
    }
  }, []);

  const sendMessage = useCallback(async (message: WebSocketMessage): Promise<any> => {
    if (!clientRef.current) {
      throw new Error('WebSocket client not initialized');
    }

    try {
      const result = await clientRef.current.sendMessage(message);
      
      // Update metrics after sending
      setMetrics(clientRef.current.getMetrics());
      setQueueSize(clientRef.current.getQueueSize());
      setPendingMessages(clientRef.current.getPendingMessageCount());
      
      return result;
    } catch (error) {
      logfire.error('Failed to send WebSocket message', {
        userId: user?.id,
        messageType: message.type,
        error: error instanceof Error ? error.message : 'Unknown error'
      });
      throw error;
    }
  }, [user]);

  return {
    connectionState,
    lastMessage,
    sendMessage,
    connect,
    disconnect,
    isConnected: connectionState === ConnectionState.CONNECTED,
    metrics,
    queueSize,
    pendingMessages
  };
};
```

## TDD Implementation Cycle

### Test-Driven Development Process

1. **Red Phase**: Write failing WebSocket tests
   ```bash
   # Create WebSocket test files
   touch frontend/src/services/__tests__/WebSocketClient.test.ts
   touch frontend/src/hooks/__tests__/useWebSocket.test.ts
   
   # Run failing tests
   npm test WebSocketClient.test.ts
   ```

2. **Green Phase**: Implement basic WebSocket functionality
   ```bash
   # Implement WebSocket client and hooks
   npm test WebSocketClient.test.ts
   ```

3. **Refactor Phase**: Add reconnection and error handling
   ```bash
   # Add advanced features and optimizations
   npm test -- --coverage
   ```

4. **Commit**: Commit WebSocket functionality
   ```bash
   git add frontend/src/services/WebSocketClient.ts frontend/src/hooks/useWebSocket.ts
   git commit -m "feat: implement WebSocket client with intelligent connection management
   
   - Add robust WebSocketClient with automatic reconnection and message queuing
   - Implement useWebSocket hook for React integration
   - Add comprehensive error handling and connection recovery
   - Include performance metrics and connection monitoring
   - Integrate with Logfire for WebSocket operation tracking
   
   Tests: Added comprehensive test suite for WebSocket client and hooks
   Performance: Message queuing, heartbeat monitoring, and efficient reconnection
   Reliability: Exponential backoff, connection recovery, and error handling"
   ```

### WebSocket Test Cases

```typescript
// frontend/src/services/__tests__/WebSocketClient.test.ts
import { WebSocketClient, ConnectionState } from '../WebSocketClient';

// Mock WebSocket
global.WebSocket = jest.fn().mockImplementation(() => ({
  addEventListener: jest.fn(),
  removeEventListener: jest.fn(),
  send: jest.fn(),
  close: jest.fn(),
  readyState: WebSocket.OPEN
}));

describe('WebSocketClient', () => {
  let client: WebSocketClient;
  let mockWS: any;

  beforeEach(() => {
    jest.clearAllMocks();
    
    client = new WebSocketClient({
      url: 'ws://localhost:8000/ws',
      reconnectInterval: 100,
      maxReconnectAttempts: 3
    });

    mockWS = {
      addEventListener: jest.fn(),
      removeEventListener: jest.fn(),
      send: jest.fn(),
      close: jest.fn(),
      readyState: WebSocket.OPEN
    };

    (WebSocket as jest.Mock).mockReturnValue(mockWS);
  });

  test('initializes with correct configuration', () => {
    expect(client.getState()).toBe(ConnectionState.DISCONNECTED);
    expect(client.getQueueSize()).toBe(0);
    expect(client.getPendingMessageCount()).toBe(0);
  });

  test('connects successfully with authentication', async () => {
    const connectPromise = client.connect('test-token', 'user-123');
    
    // Simulate WebSocket open event
    const openHandler = mockWS.addEventListener.mock.calls.find(
      call => call[0] === 'open'
    )[1];
    openHandler();

    await connectPromise;

    expect(client.getState()).toBe(ConnectionState.CONNECTED);
    expect(WebSocket).toHaveBeenCalledWith(
      expect.stringContaining('token=test-token'),
      expect.any(Array)
    );
  });

  test('handles connection failure and reconnection', async () => {
    const connectPromise = client.connect('test-token', 'user-123');
    
    // Simulate WebSocket error
    const errorHandler = mockWS.addEventListener.mock.calls.find(
      call => call[0] === 'error'
    )[1];
    errorHandler(new Error('Connection failed'));

    await expect(connectPromise).rejects.toThrow();
    expect(client.getState()).toBe(ConnectionState.RECONNECTING);
  });

  test('queues messages when disconnected', async () => {
    const message = { type: 'test', data: 'hello' };
    
    await client.sendMessage(message);
    
    expect(client.getQueueSize()).toBe(1);
  });

  test('sends queued messages after reconnection', async () => {
    // Queue a message while disconnected
    const message = { type: 'test', data: 'hello' };
    await client.sendMessage(message);
    
    // Connect
    const connectPromise = client.connect('test-token', 'user-123');
    const openHandler = mockWS.addEventListener.mock.calls.find(
      call => call[0] === 'open'
    )[1];
    openHandler();
    await connectPromise;

    // Verify message was sent
    expect(mockWS.send).toHaveBeenCalledWith(
      expect.stringContaining('"type":"test"')
    );
    expect(client.getQueueSize()).toBe(0);
  });

  test('handles heartbeat correctly', async () => {
    jest.useFakeTimers();
    
    const connectPromise = client.connect('test-token', 'user-123');
    const openHandler = mockWS.addEventListener.mock.calls.find(
      call => call[0] === 'open'
    )[1];
    openHandler();
    await connectPromise;

    // Fast-forward to trigger heartbeat
    jest.advanceTimersByTime(30000);

    expect(mockWS.send).toHaveBeenCalledWith(
      expect.stringContaining('"type":"ping"')
    );

    jest.useRealTimers();
  });

  test('calculates latency metrics correctly', async () => {
    const connectPromise = client.connect('test-token', 'user-123');
    const openHandler = mockWS.addEventListener.mock.calls.find(
      call => call[0] === 'open'
    )[1];
    openHandler();
    await connectPromise;

    // Simulate pong message with latency
    const messageHandler = mockWS.addEventListener.mock.calls.find(
      call => call[0] === 'message'
    )[1];
    
    const timestamp = Date.now() - 100; // 100ms ago
    messageHandler({
      data: JSON.stringify({
        type: 'pong',
        timestamp
      })
    });

    const metrics = client.getMetrics();
    expect(metrics.currentLatency).toBeCloseTo(100, -1);
  });

  test('disconnects cleanly', () => {
    client.connect('test-token', 'user-123');
    client.disconnect();

    expect(mockWS.close).toHaveBeenCalledWith(1000, 'Client disconnect');
    expect(client.getState()).toBe(ConnectionState.DISCONNECTED);
  });
});
```

## Security Checklist for WebSocket Client

### Connection Security
- [ ] WebSocket connection over WSS (secure WebSocket) in production
- [ ] Authentication token validation before establishing connection
- [ ] Origin validation to prevent cross-origin WebSocket attacks
- [ ] Connection rate limiting to prevent abuse (10 connections per user max)
- [ ] Secure token transmission in connection URL parameters
- [ ] Connection timeout enforcement (10 seconds maximum)
- [ ] Protection against WebSocket hijacking with token validation
- [ ] Secure handling of connection errors without information disclosure
- [ ] Connection state validation before message transmission
- [ ] Automatic disconnection on authentication expiration

### Message Security
- [ ] Message validation and sanitization for all incoming data
- [ ] Message type validation with allow-list approach
- [ ] Protection against message injection attacks with schema validation
- [ ] Rate limiting on message transmission (100 messages/minute per user)
- [ ] Message size limits to prevent DoS attacks (1MB max per message)
- [ ] Secure handling of binary message data
- [ ] Protection against message replay attacks with timestamps
- [ ] Message encryption for sensitive data transmission
- [ ] Validation of message origins and session ownership
- [ ] Secure error handling in message processing

### Client-Side Security
- [ ] Secure storage of authentication tokens in memory only
- [ ] Protection against XSS attacks in WebSocket message handling
- [ ] Secure event listener management without memory leaks
- [ ] Input validation for all user-provided WebSocket parameters
- [ ] Protection against malicious WebSocket server responses
- [ ] Secure handling of reconnection with credential validation
- [ ] Client-side rate limiting and message queuing controls
- [ ] Protection against infinite reconnection loops
- [ ] Secure cleanup of WebSocket resources on disconnect
- [ ] Validation of WebSocket protocol and version support

## Performance Requirements

### Connection Performance
- Connection establishment time < 500ms
- Reconnection time < 2 seconds with exponential backoff
- Heartbeat interval every 30 seconds with 5-second timeout
- Message queuing capacity for 1000 messages maximum
- Memory usage < 10MB for WebSocket client state
- CPU usage optimization for message processing

### Message Throughput
- Message processing rate > 1000 messages/second
- Message send latency < 10ms for queued messages
- Message delivery acknowledgment < 100ms
- Batch message processing for efficiency
- Compression support for large messages
- Message deduplication for reliability

### Reliability Metrics
- Connection uptime > 99% under normal conditions
- Automatic reconnection success rate > 95%
- Message delivery success rate > 99.9%
- Queue overflow protection with oldest message eviction
- Connection recovery time < 5 seconds after network interruption
- Zero message loss during brief network disruptions

## Connection Management Strategies

### Reconnection Logic
```typescript
// Exponential backoff with jitter
private calculateReconnectDelay(attempt: number): number {
  const baseDelay = this.config.reconnectInterval;
  const exponentialDelay = baseDelay * Math.pow(2, attempt);
  const maxDelay = 30000; // 30 seconds max
  const jitter = Math.random() * 1000; // Up to 1 second jitter
  
  return Math.min(exponentialDelay + jitter, maxDelay);
}
```

### Message Queue Management
```typescript
// Priority-based message queuing
private prioritizeMessage(message: WebSocketMessage): number {
  const priorities = {
    'terminal_input': 1,    // Highest priority
    'heartbeat': 2,
    'session_command': 3,
    'notification': 4       // Lowest priority
  };
  
  return priorities[message.type] || 5;
}
```

### Health Monitoring
```typescript
// Connection health assessment
private assessConnectionHealth(): 'healthy' | 'degraded' | 'unhealthy' {
  const metrics = this.getMetrics();
  
  if (metrics.averageLatency > 1000 || metrics.reconnectAttempts > 3) {
    return 'unhealthy';
  } else if (metrics.averageLatency > 500 || metrics.reconnectAttempts > 1) {
    return 'degraded';
  }
  
  return 'healthy';
}
```

## Integration Testing

### Terminal Integration
```typescript
async function testTerminalWebSocketIntegration() {
  const terminal = new Terminal();
  const wsClient = new WebSocketClient({ url: 'ws://localhost:8000/ws' });
  
  await wsClient.connect('test-token', 'user-123');
  
  // Send terminal input
  terminal.onData((data) => {
    wsClient.sendMessage({
      type: 'terminal_input',
      sessionId: 'session-123',
      data
    });
  });
  
  // Receive terminal output
  wsClient.addEventListener('terminal_output', (message) => {
    terminal.write(message.data);
  });
  
  // Test bidirectional communication
  terminal.write('echo "Hello WebSocket"\n');
  
  // Verify message was sent
  expect(wsClient.getMetrics().messagesSent).toBeGreaterThan(0);
}
```

### Authentication Integration
```typescript
async function testWebSocketAuthentication() {
  const wsClient = new WebSocketClient({ url: 'ws://localhost:8000/ws' });
  
  // Test with invalid token
  await expect(wsClient.connect('invalid-token', 'user-123'))
    .rejects.toThrow('Authentication failed');
  
  // Test with valid token
  await expect(wsClient.connect('valid-token', 'user-123'))
    .resolves.toBeUndefined();
  
  expect(wsClient.isConnected()).toBe(true);
}
```

## Next Implementation Steps

1. **Complete WebSocket client implementation** with all features
2. **Add message compression** for large data transfers
3. **Implement advanced error recovery** with multiple strategies
4. **Add WebSocket connection pooling** for multiple sessions
5. **Create comprehensive monitoring** with performance metrics
6. **Add binary message support** for efficient data transfer
7. **Implement WebSocket message encryption** for sensitive data

## Commit Guidelines

Each commit should include:
- **Feature implementation** with comprehensive error handling
- **Performance optimization** with efficient connection management
- **Security validation** for connection and message security
- **Test coverage** for WebSocket client and hooks (>80%)
- **Integration verification** with terminal and authentication systems
- **Documentation updates** with usage examples and API reference