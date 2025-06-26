import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { WebSocketManager, WebSocketManagerConfig } from './WebSocketManager';

// Mock WebSocket
class MockWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;

  readyState = MockWebSocket.CONNECTING;
  onopen: ((event: Event) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;

  constructor(public url: string, public protocols?: string | string[]) {
    // Simulate async connection
    setTimeout(() => {
      this.readyState = MockWebSocket.OPEN;
      this.onopen?.(new Event('open'));
    }, 10);
  }

  send(data: string) {
    if (this.readyState !== MockWebSocket.OPEN) {
      throw new Error('WebSocket is not open');
    }
    // Mock echo for testing
    setTimeout(() => {
      this.onmessage?.(new MessageEvent('message', { data }));
    }, 5);
  }

  close(code?: number, reason?: string) {
    this.readyState = MockWebSocket.CLOSING;
    setTimeout(() => {
      this.readyState = MockWebSocket.CLOSED;
      this.onclose?.(new CloseEvent('close', { code: code || 1000, reason: reason || '' }));
    }, 5);
  }

  // Helper methods for testing
  simulateMessage(data: any) {
    const messageEvent = new MessageEvent('message', {
      data: typeof data === 'string' ? data : JSON.stringify(data)
    });
    this.onmessage?.(messageEvent);
  }

  simulateError() {
    this.onerror?.(new Event('error'));
  }

  simulateClose(code = 1000, reason = '') {
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.(new CloseEvent('close', { code, reason }));
  }
}

// Global WebSocket mock
(global as any).WebSocket = MockWebSocket;

describe('WebSocketManager', () => {
  let wsManager: WebSocketManager;
  let mockConfig: WebSocketManagerConfig;
  let mockOnConnect: ReturnType<typeof vi.fn>;
  let mockOnDisconnect: ReturnType<typeof vi.fn>;
  let mockOnReconnecting: ReturnType<typeof vi.fn>;
  let mockOnMessage: ReturnType<typeof vi.fn>;
  let mockOnError: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.useFakeTimers();
    
    mockOnConnect = vi.fn();
    mockOnDisconnect = vi.fn();
    mockOnReconnecting = vi.fn();
    mockOnMessage = vi.fn();
    mockOnError = vi.fn();

    mockConfig = {
      apiUrl: 'ws://localhost:8000',
      token: 'test-token',
      onConnect: mockOnConnect,
      onDisconnect: mockOnDisconnect,
      onReconnecting: mockOnReconnecting,
      onMessage: mockOnMessage,
      onError: mockOnError,
    };
  });

  afterEach(() => {
    wsManager?.disconnect();
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  describe('constructor and connection', () => {
    it('should create WebSocketManager with config', () => {
      wsManager = new WebSocketManager(mockConfig);
      expect(wsManager).toBeDefined();
    });

    it('should connect to WebSocket server', async () => {
      wsManager = new WebSocketManager(mockConfig);
      
      // Wait for connection
      await vi.advanceTimersByTimeAsync(50);
      
      expect(mockOnConnect).toHaveBeenCalled();
    });

    it('should construct correct WebSocket URL with token', () => {
      wsManager = new WebSocketManager(mockConfig);
      
      // Check that WebSocket was created with correct URL
      expect((global as any).WebSocket).toHaveBeenCalledWith(
        'ws://localhost:8000/ws?token=test-token'
      );
    });

    it('should handle connection errors', async () => {
      wsManager = new WebSocketManager(mockConfig);
      
      // Simulate connection error before open
      const ws = (wsManager as any).ws as MockWebSocket;
      ws.simulateError();
      
      await vi.advanceTimersByTimeAsync(50);
      
      expect(mockOnError).toHaveBeenCalled();
    });
  });

  describe('message handling', () => {
    beforeEach(async () => {
      wsManager = new WebSocketManager(mockConfig);
      await vi.advanceTimersByTimeAsync(50); // Wait for connection
    });

    it('should handle incoming messages', async () => {
      const testMessage = { type: 'terminal_data', data: 'test data' };
      
      const ws = (wsManager as any).ws as MockWebSocket;
      ws.simulateMessage(testMessage);
      
      await vi.advanceTimersByTimeAsync(10);
      
      expect(mockOnMessage).toHaveBeenCalledWith(testMessage);
    });

    it('should handle malformed JSON messages', async () => {
      const ws = (wsManager as any).ws as MockWebSocket;
      ws.simulateMessage('invalid json {');
      
      await vi.advanceTimersByTimeAsync(10);
      
      expect(mockOnError).toHaveBeenCalled();
    });

    it('should handle heartbeat responses', async () => {
      const heartbeatMessage = { type: 'heartbeat_response' };
      
      const ws = (wsManager as any).ws as MockWebSocket;
      ws.simulateMessage(heartbeatMessage);
      
      await vi.advanceTimersByTimeAsync(10);
      
      // Should not call onMessage for heartbeat responses
      expect(mockOnMessage).not.toHaveBeenCalledWith(heartbeatMessage);
    });
  });

  describe('sending messages', () => {
    beforeEach(async () => {
      wsManager = new WebSocketManager(mockConfig);
      await vi.advanceTimersByTimeAsync(50); // Wait for connection
    });

    it('should send terminal data', async () => {
      const spy = vi.spyOn((wsManager as any).ws, 'send');
      
      wsManager.sendTerminalData('test input');
      
      expect(spy).toHaveBeenCalledWith(
        JSON.stringify({
          type: 'terminal_data',
          data: 'test input'
        })
      );
    });

    it('should send resize data', async () => {
      const spy = vi.spyOn((wsManager as any).ws, 'send');
      
      wsManager.sendResize(80, 24);
      
      expect(spy).toHaveBeenCalledWith(
        JSON.stringify({
          type: 'terminal_resize',
          data: { cols: 80, rows: 24 }
        })
      );
    });

    it('should send session join', async () => {
      const spy = vi.spyOn((wsManager as any).ws, 'send');
      
      await wsManager.joinSession('session-123');
      
      expect(spy).toHaveBeenCalledWith(
        JSON.stringify({
          type: 'session_join',
          session_id: 'session-123'
        })
      );
    });

    it('should send session create', async () => {
      const spy = vi.spyOn((wsManager as any).ws, 'send');
      
      await wsManager.createSession();
      
      expect(spy).toHaveBeenCalledWith(
        JSON.stringify({
          type: 'session_create'
        })
      );
    });

    it('should send session end', async () => {
      const spy = vi.spyOn((wsManager as any).ws, 'send');
      
      await wsManager.endSession('session-123');
      
      expect(spy).toHaveBeenCalledWith(
        JSON.stringify({
          type: 'session_end',
          session_id: 'session-123'
        })
      );
    });

    it('should not send messages when disconnected', () => {
      wsManager.disconnect();
      
      const spy = vi.spyOn(console, 'warn');
      wsManager.sendTerminalData('test');
      
      expect(spy).toHaveBeenCalledWith('WebSocket not connected');
    });
  });

  describe('heartbeat mechanism', () => {
    beforeEach(async () => {
      wsManager = new WebSocketManager({
        ...mockConfig,
        heartbeatInterval: 100 // Short interval for testing
      });
      await vi.advanceTimersByTimeAsync(50); // Wait for connection
    });

    it('should send heartbeat messages', async () => {
      const spy = vi.spyOn((wsManager as any).ws, 'send');
      
      // Advance past heartbeat interval
      await vi.advanceTimersByTimeAsync(150);
      
      expect(spy).toHaveBeenCalledWith(
        JSON.stringify({ type: 'heartbeat' })
      );
    });

    it('should handle missed heartbeats', async () => {
      const wsManager = new WebSocketManager({
        ...mockConfig,
        heartbeatInterval: 100,
        heartbeatTimeout: 50
      });
      
      await vi.advanceTimersByTimeAsync(50); // Wait for connection
      
      // Advance past heartbeat timeout without response
      await vi.advanceTimersByTimeAsync(200);
      
      expect(mockOnReconnecting).toHaveBeenCalled();
    });
  });

  describe('reconnection logic', () => {
    it('should attempt reconnection on close', async () => {
      wsManager = new WebSocketManager({
        ...mockConfig,
        reconnectInterval: 100,
        maxReconnectAttempts: 3
      });
      
      await vi.advanceTimersByTimeAsync(50); // Wait for connection
      
      // Simulate close
      const ws = (wsManager as any).ws as MockWebSocket;
      ws.simulateClose(1006, 'Connection lost'); // Abnormal closure
      
      await vi.advanceTimersByTimeAsync(10);
      
      expect(mockOnDisconnect).toHaveBeenCalled();
      expect(mockOnReconnecting).toHaveBeenCalled();
      
      // Should attempt reconnection
      await vi.advanceTimersByTimeAsync(150);
      
      // New WebSocket should be created
      expect((global as any).WebSocket).toHaveBeenCalledTimes(2);
    });

    it('should use exponential backoff for reconnection', async () => {
      wsManager = new WebSocketManager({
        ...mockConfig,
        reconnectInterval: 100,
        maxReconnectAttempts: 3
      });
      
      await vi.advanceTimersByTimeAsync(50); // Wait for connection
      
      // Simulate multiple failed connections
      for (let i = 0; i < 3; i++) {
        const ws = (wsManager as any).ws as MockWebSocket;
        ws.simulateClose(1006, 'Connection lost');
        
        await vi.advanceTimersByTimeAsync(10);
        
        // Wait for reconnection attempt with exponential backoff
        const expectedDelay = 100 * Math.pow(2, i);
        await vi.advanceTimersByTimeAsync(expectedDelay + 50);
      }
      
      // Should have attempted reconnection 3 times (plus initial connection)
      expect((global as any).WebSocket).toHaveBeenCalledTimes(4);
    });

    it('should stop reconnecting after max attempts', async () => {
      wsManager = new WebSocketManager({
        ...mockConfig,
        reconnectInterval: 100,
        maxReconnectAttempts: 2
      });
      
      await vi.advanceTimersByTimeAsync(50); // Wait for connection
      
      // Simulate failed connections exceeding max attempts
      for (let i = 0; i < 3; i++) {
        const ws = (wsManager as any).ws as MockWebSocket;
        ws.simulateClose(1006, 'Connection lost');
        
        await vi.advanceTimersByTimeAsync(10);
        await vi.advanceTimersByTimeAsync(200); // Wait longer than any backoff
      }
      
      // Should have stopped after max attempts (initial + 2 reconnects)
      expect((global as any).WebSocket).toHaveBeenCalledTimes(3);
    });

    it('should not reconnect on normal close', async () => {
      wsManager = new WebSocketManager(mockConfig);
      
      await vi.advanceTimersByTimeAsync(50); // Wait for connection
      
      // Simulate normal close
      const ws = (wsManager as any).ws as MockWebSocket;
      ws.simulateClose(1000, 'Normal closure');
      
      await vi.advanceTimersByTimeAsync(10);
      
      expect(mockOnDisconnect).toHaveBeenCalled();
      expect(mockOnReconnecting).not.toHaveBeenCalled();
      
      // Should not attempt reconnection
      await vi.advanceTimersByTimeAsync(200);
      expect((global as any).WebSocket).toHaveBeenCalledTimes(1);
    });
  });

  describe('cleanup and disconnection', () => {
    beforeEach(async () => {
      wsManager = new WebSocketManager(mockConfig);
      await vi.advanceTimersByTimeAsync(50); // Wait for connection
    });

    it('should disconnect cleanly', () => {
      const ws = (wsManager as any).ws as MockWebSocket;
      const closeSpy = vi.spyOn(ws, 'close');
      
      wsManager.disconnect();
      
      expect(closeSpy).toHaveBeenCalledWith(1000, 'Client disconnect');
    });

    it('should clear timers on disconnect', () => {
      const clearIntervalSpy = vi.spyOn(global, 'clearInterval');
      const clearTimeoutSpy = vi.spyOn(global, 'clearTimeout');
      
      wsManager.disconnect();
      
      expect(clearIntervalSpy).toHaveBeenCalled();
      expect(clearTimeoutSpy).toHaveBeenCalled();
    });

    it('should not attempt operations after disconnect', () => {
      wsManager.disconnect();
      
      const spy = vi.spyOn(console, 'warn');
      wsManager.sendTerminalData('test');
      
      expect(spy).toHaveBeenCalledWith('WebSocket not connected');
    });
  });

  describe('connection state', () => {
    it('should track connection state correctly', async () => {
      wsManager = new WebSocketManager(mockConfig);
      
      expect(wsManager.isConnected()).toBe(false);
      
      await vi.advanceTimersByTimeAsync(50); // Wait for connection
      
      expect(wsManager.isConnected()).toBe(true);
      
      wsManager.disconnect();
      
      expect(wsManager.isConnected()).toBe(false);
    });

    it('should report reconnecting state', async () => {
      wsManager = new WebSocketManager({
        ...mockConfig,
        reconnectInterval: 100,
        maxReconnectAttempts: 3
      });
      
      await vi.advanceTimersByTimeAsync(50); // Wait for connection
      
      expect(wsManager.isReconnecting()).toBe(false);
      
      // Simulate connection loss
      const ws = (wsManager as any).ws as MockWebSocket;
      ws.simulateClose(1006, 'Connection lost');
      
      await vi.advanceTimersByTimeAsync(10);
      
      expect(wsManager.isReconnecting()).toBe(true);
      
      // Wait for reconnection
      await vi.advanceTimersByTimeAsync(150);
      
      expect(wsManager.isReconnecting()).toBe(false);
    });
  });
});