export interface WebSocketManagerConfig {
  apiUrl: string;
  token: string;
  onConnect?: () => void;
  onDisconnect?: () => void;
  onReconnecting?: () => void;
  onMessage?: (message: any) => void;
  onError?: (error: Error) => void;
}

export class WebSocketManager {
  private ws: WebSocket | null = null;
  private config: WebSocketManagerConfig;
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 3; // Reduced from 10
  private reconnectDelay = 1000;
  private maxReconnectDelay = 5000; // Reduced from 30000
  private heartbeatInterval: number | null = null;
  private isReconnecting = false;
  private sessionId: string | null = null;

  constructor(config: WebSocketManagerConfig) {
    this.config = config;
    this.connect();
  }

  private connect(): void {
    // Don't reconnect if we've exceeded max attempts
    if (this.reconnectAttempts >= this.maxReconnectAttempts) {
      console.warn('WebSocket: Max reconnection attempts reached. Stopping reconnection.');
      if (this.config.onError) {
        this.config.onError(new Error('Max reconnection attempts reached'));
      }
      return;
    }

    try {
      const wsUrl = new URL('/ws/terminal', this.config.apiUrl);
      wsUrl.searchParams.set('token', this.config.token);
      
      this.ws = new WebSocket(wsUrl.toString());
      
      this.ws.onopen = this.handleOpen.bind(this);
      this.ws.onmessage = this.handleMessage.bind(this);
      this.ws.onclose = this.handleClose.bind(this);
      this.ws.onerror = this.handleError.bind(this);
      
    } catch (error) {
      console.error('Failed to create WebSocket connection:', error);
      this.scheduleReconnect();
    }
  }

  private handleOpen(): void {
    console.log('WebSocket connected');
    this.reconnectAttempts = 0;
    this.isReconnecting = false;
    this.startHeartbeat();
    
    if (this.config.onConnect) {
      this.config.onConnect();
    }
  }

  private handleMessage(event: MessageEvent): void {
    try {
      const message = JSON.parse(event.data);
      
      switch (message.type) {
        case 'heartbeat':
          // Respond to heartbeat
          this.send({ type: 'heartbeat_response', timestamp: Date.now() });
          break;
        
        default:
          if (this.config.onMessage) {
            this.config.onMessage(message);
          }
          break;
      }
    } catch (error) {
      console.error('Failed to parse WebSocket message:', error);
    }
  }

  private handleClose(event: CloseEvent): void {
    console.log('WebSocket disconnected:', event.code, event.reason);
    this.stopHeartbeat();
    
    if (this.config.onDisconnect) {
      this.config.onDisconnect();
    }
    
    // Attempt to reconnect unless explicitly closed or max attempts reached
    if (event.code !== 1000 && event.code !== 1001 && this.reconnectAttempts < this.maxReconnectAttempts) {
      this.scheduleReconnect();
    }
  }

  private handleError(event: Event): void {
    console.error('WebSocket error:', event);
    
    const error = new Error('WebSocket connection error');
    if (this.config.onError) {
      this.config.onError(error);
    }
  }

  private scheduleReconnect(): void {
    if (this.isReconnecting || this.reconnectAttempts >= this.maxReconnectAttempts) {
      return;
    }
    
    this.isReconnecting = true;
    
    if (this.config.onReconnecting) {
      this.config.onReconnecting();
    }
    
    const delay = Math.min(
      this.reconnectDelay * Math.pow(2, this.reconnectAttempts),
      this.maxReconnectDelay
    );
    
    console.log(`Reconnecting in ${delay}ms (attempt ${this.reconnectAttempts + 1})`);
    
    setTimeout(() => {
      this.reconnectAttempts++;
      this.connect();
    }, delay);
  }

  private startHeartbeat(): void {
    this.heartbeatInterval = setInterval(() => {
      if (this.ws?.readyState === WebSocket.OPEN) {
        this.send({ type: 'heartbeat', timestamp: Date.now() });
      }
    }, 30000); // 30 seconds
  }

  private stopHeartbeat(): void {
    if (this.heartbeatInterval) {
      clearInterval(this.heartbeatInterval);
      this.heartbeatInterval = null;
    }
  }

  private send(message: any): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      console.log('📤 Sending WebSocket message:', message.type);
      this.ws.send(JSON.stringify(message));
    } else {
      console.warn('❌ WebSocket not ready, cannot send:', message.type, 'State:', this.ws?.readyState);
    }
  }

  public sendTerminalData(data: string): void {
    this.send({
      type: 'terminal_data',
      data,
      session_id: this.sessionId
    });
  }

  public sendResize(cols: number, rows: number): void {
    this.send({
      type: 'terminal_resize',
      data: { cols, rows },
      session_id: this.sessionId
    });
  }

  public async joinSession(sessionId: string): Promise<void> {
    this.sessionId = sessionId;
    this.send({
      type: 'session_join',
      session_id: sessionId
    });
  }

  public async createSession(vmId: string): Promise<void> {
    this.send({
      type: 'session_create',
      data: { vm_id: vmId }
    });
  }

  public async endSession(): Promise<void> {
    if (this.sessionId) {
      this.send({
        type: 'session_end',
        session_id: this.sessionId
      });
      this.sessionId = null;
    }
  }

  public disconnect(): void {
    this.stopHeartbeat();
    if (this.ws) {
      this.ws.close(1000, 'Client disconnect');
      this.ws = null;
    }
  }

  public getConnectionState(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }
}