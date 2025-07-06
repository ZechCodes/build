/**
 * MSW Handlers for API and WebSocket Mocking
 * Provides bulletproof mocking for 100% test success rate
 */

import { http, HttpResponse } from 'msw';
import { TEST_CONSTANTS } from '../constants';

// Mock API responses
export const apiHandlers = [
  // Authentication endpoints
  http.post(`${TEST_CONSTANTS.API_BASE_URL}/auth/login`, () => {
    return HttpResponse.json({
      access_token: TEST_CONSTANTS.MOCK_TOKEN,
      token_type: 'bearer',
      user: {
        id: TEST_CONSTANTS.MOCK_USER_ID,
        username: 'testuser',
        email: 'test@example.com',
        permissions: ['terminal:access', 'vm:manage']
      }
    });
  }),

  // Terminal session endpoints
  http.post(`${TEST_CONSTANTS.API_BASE_URL}/sessions`, () => {
    return HttpResponse.json({
      id: TEST_CONSTANTS.MOCK_SESSION_ID,
      vm_id: TEST_CONSTANTS.MOCK_VM_ID,
      user_id: TEST_CONSTANTS.MOCK_USER_ID,
      state: 'active',
      created_at: new Date().toISOString(),
      terminal_size: TEST_CONSTANTS.DEFAULT_TERMINAL_SIZE
    });
  }),

  http.get(`${TEST_CONSTANTS.API_BASE_URL}/sessions/:sessionId`, ({ params }) => {
    return HttpResponse.json({
      id: params.sessionId,
      vm_id: TEST_CONSTANTS.MOCK_VM_ID,
      user_id: TEST_CONSTANTS.MOCK_USER_ID,
      state: 'active',
      created_at: new Date().toISOString(),
      terminal_size: TEST_CONSTANTS.DEFAULT_TERMINAL_SIZE,
      last_activity: new Date().toISOString()
    });
  }),

  http.post(`${TEST_CONSTANTS.API_BASE_URL}/sessions/:sessionId/attach`, () => {
    return HttpResponse.json({
      success: true,
      session_id: TEST_CONSTANTS.MOCK_SESSION_ID,
      buffer_data: 'Welcome to the terminal!\r\n$ '
    });
  }),

  http.delete(`${TEST_CONSTANTS.API_BASE_URL}/sessions/:sessionId`, () => {
    return HttpResponse.json({ success: true });
  }),

  // VM management endpoints
  http.get(`${TEST_CONSTANTS.API_BASE_URL}/vms/:vmId`, ({ params }) => {
    return HttpResponse.json({
      id: params.vmId,
      user_id: TEST_CONSTANTS.MOCK_USER_ID,
      state: 'running',
      created_at: new Date().toISOString(),
      config: {
        vcpus: 2,
        memory_mb: 1024,
        disk_gb: 10
      }
    });
  }),

  // Health check endpoint
  http.get(`${TEST_CONSTANTS.API_BASE_URL}/health`, () => {
    return HttpResponse.json({
      status: 'healthy',
      timestamp: new Date().toISOString(),
      services: {
        database: 'healthy',
        redis: 'healthy',
        websocket: 'healthy'
      }
    });
  }),

  // Error scenarios for testing
  http.get(`${TEST_CONSTANTS.API_BASE_URL}/error/500`, () => {
    return new HttpResponse(null, { status: 500 });
  }),

  http.get(`${TEST_CONSTANTS.API_BASE_URL}/error/401`, () => {
    return new HttpResponse(null, { status: 401 });
  }),

  http.get(`${TEST_CONSTANTS.API_BASE_URL}/error/timeout`, () => {
    return new Promise(() => {
      // Never resolves to simulate timeout
    });
  })
];

// WebSocket mock implementation
export class MockWebSocket {
  static instance: MockWebSocket | null = null;
  
  public readyState: number = WebSocket.CONNECTING;
  public url: string;
  public onopen: ((event: Event) => void) | null = null;
  public onclose: ((event: CloseEvent) => void) | null = null;
  public onmessage: ((event: MessageEvent) => void) | null = null;
  public onerror: ((event: Event) => void) | null = null;

  private listeners: Map<string, Function[]> = new Map();

  constructor(url: string) {
    this.url = url;
    MockWebSocket.instance = this;
    
    // Simulate connection opening after a short delay
    setTimeout(() => {
      this.readyState = WebSocket.OPEN;
      this.dispatchEvent('open', {});
    }, 10);
  }

  send(data: string) {
    // Simulate server response to terminal input
    if (this.readyState === WebSocket.OPEN) {
      const message = JSON.parse(data);
      
      switch (message.type) {
        case 'session_create':
          this.simulateMessage({
            type: 'session_created',
            data: {
              session_id: TEST_CONSTANTS.MOCK_SESSION_ID,
              vm_id: message.vm_id
            }
          });
          break;
          
        case 'session_join':
          this.simulateMessage({
            type: 'session_joined',
            session_id: message.session_id,
            buffer_data: 'Welcome to the terminal!\r\n$ '
          });
          break;
          
        case 'terminal_data':
          // Echo the input back as terminal output
          this.simulateMessage({
            type: 'terminal_data',
            session_id: message.session_id,
            data: message.data
          });
          break;
          
        case 'terminal_resize':
          this.simulateMessage({
            type: 'terminal_resized',
            session_id: message.session_id,
            cols: message.data.cols,
            rows: message.data.rows
          });
          break;
          
        case 'heartbeat':
          this.simulateMessage({
            type: 'heartbeat_response',
            timestamp: Date.now()
          });
          break;
      }
    }
  }

  close(code?: number, reason?: string) {
    this.readyState = WebSocket.CLOSED;
    this.dispatchEvent('close', { code: code || 1000, reason: reason || 'Normal closure' });
  }

  addEventListener(event: string, listener: Function) {
    if (!this.listeners.has(event)) {
      this.listeners.set(event, []);
    }
    this.listeners.get(event)!.push(listener);
  }

  removeEventListener(event: string, listener: Function) {
    const eventListeners = this.listeners.get(event);
    if (eventListeners) {
      const index = eventListeners.indexOf(listener);
      if (index > -1) {
        eventListeners.splice(index, 1);
      }
    }
  }

  private dispatchEvent(event: string, data: any) {
    // Call specific handler if exists
    switch (event) {
      case 'open':
        this.onopen?.(data);
        break;
      case 'close':
        this.onclose?.(data);
        break;
      case 'message':
        this.onmessage?.(data);
        break;
      case 'error':
        this.onerror?.(data);
        break;
    }

    // Call generic listeners
    const listeners = this.listeners.get(event);
    listeners?.forEach(listener => listener(data));
  }

  private simulateMessage(message: any) {
    setTimeout(() => {
      this.dispatchEvent('message', {
        data: JSON.stringify(message),
        origin: this.url,
        source: this
      });
    }, 5); // Small delay to simulate network latency
  }

  // Static methods for test control
  static simulateConnectionError() {
    if (MockWebSocket.instance) {
      MockWebSocket.instance.readyState = WebSocket.CLOSED;
      MockWebSocket.instance.dispatchEvent('error', new Error('Connection failed'));
    }
  }

  static simulateServerMessage(message: any) {
    if (MockWebSocket.instance && MockWebSocket.instance.readyState === WebSocket.OPEN) {
      MockWebSocket.instance.simulateMessage(message);
    }
  }

  static reset() {
    MockWebSocket.instance = null;
  }
}

// WebSocket handler for MSW
export const websocketHandlers = [
  // WebSocket connection upgrade simulation
  http.get(TEST_CONSTANTS.WEBSOCKET_URL, () => {
    return HttpResponse.json({
      message: 'WebSocket upgrade would happen here',
      protocols: ['terminal']
    });
  })
];

// Combined handlers export
export const handlers = [...apiHandlers, ...websocketHandlers];

// Test utilities for handler control
export const mockHandlerUtils = {
  // Simulate server error
  simulateServerError: (endpoint: string, status: number = 500) => {
    return http.get(endpoint, () => {
      return new HttpResponse(null, { status });
    });
  },

  // Simulate slow response
  simulateSlowResponse: (endpoint: string, delay: number = 2000) => {
    return http.get(endpoint, async () => {
      await new Promise(resolve => setTimeout(resolve, delay));
      return HttpResponse.json({ success: true });
    });
  },

  // Simulate authentication failure
  simulateAuthFailure: () => {
    return http.post(`${TEST_CONSTANTS.API_BASE_URL}/auth/login`, () => {
      return new HttpResponse(null, { 
        status: 401,
        headers: {
          'Content-Type': 'application/json'
        }
      });
    });
  }
};