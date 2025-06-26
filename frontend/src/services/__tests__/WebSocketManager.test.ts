import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { WebSocketManager } from '../WebSocketManager';

// Mock WebSocket
class MockWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;

  url: string;
  readyState: number = MockWebSocket.CONNECTING;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;

  constructor(url: string) {
    this.url = url;
    // Simulate async connection
    setTimeout(() => {
      this.readyState = MockWebSocket.OPEN;
      if (this.onopen) {
        this.onopen(new Event('open'));
      }
    }, 10);
  }

  send(data: string) {
    if (this.readyState !== MockWebSocket.OPEN) {
      throw new Error('WebSocket is not open');
    }
  }

  close(code?: number, reason?: string) {
    this.readyState = MockWebSocket.CLOSED;
    if (this.onclose) {
      this.onclose(new CloseEvent('close', { code: code || 1000, reason }));
    }
  }

  // Test helper methods
  simulateMessage(data: any) {
    if (this.onmessage) {
      this.onmessage(new MessageEvent('message', { data: JSON.stringify(data) }));
    }
  }

  simulateError() {
    if (this.onerror) {
      this.onerror(new Event('error'));
    }
  }

  simulateClose(code: number = 1000, reason: string = '') {
    this.readyState = MockWebSocket.CLOSED;
    if (this.onclose) {
      this.onclose(new CloseEvent('close', { code, reason }));
    }
  }
}

// Replace global WebSocket
(global as any).WebSocket = MockWebSocket;

describe('WebSocketManager', () => {
  let wsManager: WebSocketManager;
  let mockCallbacks: {
    onConnect: ReturnType<typeof vi.fn>;
    onDisconnect: ReturnType<typeof vi.fn>;
    onReconnecting: ReturnType<typeof vi.fn>;
    onMessage: ReturnType<typeof vi.fn>;
    onError: ReturnType<typeof vi.fn>;
  };

  beforeEach(() => {
    vi.useFakeTimers();
    mockCallbacks = {
      onConnect: vi.fn(),
      onDisconnect: vi.fn(),
      onReconnecting: vi.fn(),
      onMessage: vi.fn(),
      onError: vi.fn(),
    };

    wsManager = new WebSocketManager({
      apiUrl: 'ws://localhost:8000',
      token: 'test-token',
      ...mockCallbacks,
    });
  });

  afterEach(() => {
    wsManager.disconnect();
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it('should initialize with correct configuration', () => {
    expect(wsManager).toBeDefined();
    expect(wsManager.isConnected()).toBe(false);
  });

  it('should connect and call onConnect callback', async () => {
    // Fast-forward past connection timeout
    vi.advanceTimersByTime(20);
    
    expect(mockCallbacks.onConnect).toHaveBeenCalled();
    expect(wsManager.isConnected()).toBe(true);
  });

  it('should handle incoming messages', async () => {
    vi.advanceTimersByTime(20); // Connect first
    
    const testMessage = { type: 'terminal_data', data: 'hello world' };
    
    // Access the underlying WebSocket to simulate message
    const ws = (wsManager as any).ws as MockWebSocket;
    ws.simulateMessage(testMessage);

    expect(mockCallbacks.onMessage).toHaveBeenCalledWith(testMessage);
  });

  it('should handle heartbeat messages automatically', async () => {
    vi.advanceTimersByTime(20); // Connect first
    
    const heartbeatMessage = { type: 'heartbeat', timestamp: Date.now() };
    
    const ws = (wsManager as any).ws as MockWebSocket;
    const sendSpy = vi.spyOn(ws, 'send');
    
    ws.simulateMessage(heartbeatMessage);

    expect(sendSpy).toHaveBeenCalledWith(
      expect.stringContaining('"type":"heartbeat_response"')
    );
    expect(mockCallbacks.onMessage).not.toHaveBeenCalledWith(heartbeatMessage);
  });

  it('should send terminal data correctly', async () => {
    vi.advanceTimersByTime(20); // Connect first
    
    const ws = (wsManager as any).ws as MockWebSocket;
    const sendSpy = vi.spyOn(ws, 'send');
    
    await wsManager.joinSession('test-session-id');
    wsManager.sendTerminalData('test data');

    expect(sendSpy).toHaveBeenCalledWith(
      JSON.stringify({
        type: 'terminal_data',
        data: 'test data',
        session_id: 'test-session-id'
      })
    );
  });

  it('should send resize events correctly', async () => {
    vi.advanceTimersByTime(20); // Connect first
    
    const ws = (wsManager as any).ws as MockWebSocket;
    const sendSpy = vi.spyOn(ws, 'send');
    
    await wsManager.joinSession('test-session-id');
    wsManager.sendResize(80, 24);

    expect(sendSpy).toHaveBeenCalledWith(
      JSON.stringify({
        type: 'terminal_resize',
        data: { cols: 80, rows: 24 },
        session_id: 'test-session-id'
      })
    );
  });

  it('should handle disconnection and call onDisconnect callback', async () => {
    vi.advanceTimersByTime(20); // Connect first
    
    const ws = (wsManager as any).ws as MockWebSocket;
    ws.simulateClose(1006, 'Connection lost'); // Abnormal close

    expect(mockCallbacks.onDisconnect).toHaveBeenCalled();
    expect(wsManager.isConnected()).toBe(false);
  });

  it('should attempt reconnection on abnormal disconnect', async () => {
    vi.advanceTimersByTime(20); // Connect first
    
    const ws = (wsManager as any).ws as MockWebSocket;
    ws.simulateClose(1006, 'Connection lost'); // Abnormal close

    expect(mockCallbacks.onReconnecting).toHaveBeenCalled();
    
    // Should schedule reconnect
    vi.advanceTimersByTime(1000); // Initial reconnect delay
    
    // Should create new connection
    expect((wsManager as any).reconnectAttempts).toBe(1);
  });

  it('should not reconnect on normal close', async () => {
    vi.advanceTimersByTime(20); // Connect first
    
    const ws = (wsManager as any).ws as MockWebSocket;
    ws.simulateClose(1000, 'Normal close'); // Normal close

    expect(mockCallbacks.onReconnecting).not.toHaveBeenCalled();
  });

  it('should handle connection errors', async () => {
    vi.advanceTimersByTime(20); // Connect first
    
    const ws = (wsManager as any).ws as MockWebSocket;
    ws.simulateError();

    expect(mockCallbacks.onError).toHaveBeenCalled();
  });

  it('should start heartbeat after connection', async () => {
    vi.advanceTimersByTime(20); // Connect first
    
    const ws = (wsManager as any).ws as MockWebSocket;
    const sendSpy = vi.spyOn(ws, 'send');
    
    // Advance time to trigger heartbeat
    vi.advanceTimersByTime(30000); // 30 seconds
    
    expect(sendSpy).toHaveBeenCalledWith(
      expect.stringContaining('"type":"heartbeat"')
    );
  });

  it('should clean up properly on disconnect', () => {
    wsManager.disconnect();
    
    expect(wsManager.isConnected()).toBe(false);
    expect((wsManager as any).heartbeatInterval).toBeNull();
  });

  it('should support session management operations', async () => {
    vi.advanceTimersByTime(20); // Connect first
    
    const ws = (wsManager as any).ws as MockWebSocket;
    const sendSpy = vi.spyOn(ws, 'send');
    
    // Test join session
    await wsManager.joinSession('session-123');
    expect(sendSpy).toHaveBeenCalledWith(
      JSON.stringify({ type: 'session_join', session_id: 'session-123' })
    );
    
    // Test create session
    await wsManager.createSession('vm-456');
    expect(sendSpy).toHaveBeenCalledWith(
      JSON.stringify({ type: 'session_create', vm_id: 'vm-456' })
    );
    
    // Test end session
    await wsManager.endSession();
    expect(sendSpy).toHaveBeenCalledWith(
      JSON.stringify({ type: 'session_end', session_id: 'session-123' })
    );
  });

  it('should implement exponential backoff for reconnection', async () => {
    vi.advanceTimersByTime(20); // Connect first
    
    // Force connection failure and verify reconnection attempts
    const ws = (wsManager as any).ws as MockWebSocket;
    ws.simulateClose(1006, 'Connection lost');
    
    // Should trigger reconnecting callback
    expect(mockCallbacks.onReconnecting).toHaveBeenCalled();
    
    // Advance through first reconnect delay (1000ms)
    vi.advanceTimersByTime(1000);
    expect((wsManager as any).reconnectAttempts).toBe(1);
    
    // Second disconnect should have exponentially increased delay
    vi.advanceTimersByTime(20); // Connect
    const ws2 = (wsManager as any).ws as MockWebSocket;
    ws2.simulateClose(1006, 'Connection lost');
    
    // Advance through second reconnect delay (2000ms)
    vi.advanceTimersByTime(2000);
    expect((wsManager as any).reconnectAttempts).toBe(2);
  });

  it('should stop reconnecting after max attempts', async () => {
    vi.advanceTimersByTime(20); // Connect first
    
    const maxAttempts = (wsManager as any).maxReconnectAttempts;
    let reconnectingCallCount = 0;
    
    // Simulate repeated connection failures
    for (let i = 0; i < maxAttempts; i++) {
      const ws = (wsManager as any).ws as MockWebSocket;
      ws.simulateClose(1006, 'Connection lost');
      reconnectingCallCount++;
      
      // Advance time for reconnection delay
      vi.advanceTimersByTime(30000); // Use max delay
      vi.advanceTimersByTime(20); // Allow reconnection
    }
    
    // One more failure should not trigger more reconnection attempts
    const ws = (wsManager as any).ws as MockWebSocket;
    ws.simulateClose(1006, 'Connection lost');
    
    expect((wsManager as any).reconnectAttempts).toBe(maxAttempts);
    expect(mockCallbacks.onReconnecting).toHaveBeenCalledTimes(maxAttempts);
  });
});