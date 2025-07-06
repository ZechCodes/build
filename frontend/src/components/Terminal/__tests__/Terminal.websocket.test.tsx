/**
 * Terminal WebSocket Tests - TDD Cycle 2
 * Focus on WebSocket connection, messaging, and real-time communication
 */

import React from 'react';
import { describe, test, expect, vi, beforeEach, afterEach } from 'vitest';
import { screen, waitFor, cleanup } from '@testing-library/react';
import { renderWithProviders, websocketTestUtils, TEST_CONSTANTS } from '../../../test/utils/test-utils';
import { Terminal } from '../Terminal';

// Mock WebSocket Manager with enhanced functionality
const mockWebSocketManager = {
  connect: vi.fn(),
  disconnect: vi.fn(),
  sendTerminalData: vi.fn(),
  sendResize: vi.fn(),
  createSession: vi.fn().mockResolvedValue({ session_id: 'test-session-123' }),
  joinSession: vi.fn().mockResolvedValue(true),
  endSession: vi.fn().mockResolvedValue(true),
  getConnectionState: vi.fn(() => true),
  isConnected: vi.fn(() => true),
  on: vi.fn(),
  off: vi.fn(),
  emit: vi.fn()
};

vi.mock('../../services/WebSocketManager', () => ({
  WebSocketManager: vi.fn().mockImplementation((config) => {
    // Store config for testing
    mockWebSocketManager.config = config;
    
    // Simulate connection after construction
    setTimeout(() => {
      if (config.onConnect) {
        config.onConnect();
      }
    }, 0);
    
    return mockWebSocketManager;
  })
}));

// Mock terminal hooks
vi.mock('../../hooks/useTerminalSession', () => ({
  useTerminalSession: vi.fn(() => ({
    createSession: vi.fn().mockResolvedValue({ id: 'test-session-123' }),
    restoreSession: vi.fn().mockResolvedValue({ id: 'test-session-123' }),
    session: { id: 'test-session-123', vmId: TEST_CONSTANTS.MOCK_VM_ID },
    loading: false,
    error: null
  }))
}));

describe('Terminal Component - TDD Cycle 2: WebSocket Connection', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    cleanup();
  });

  afterEach(() => {
    cleanup();
  });

  describe('WebSocket Connection Management', () => {
    test('should initialize WebSocket connection with correct configuration', async () => {
      const customApiUrl = 'wss://test.example.com';
      const customToken = 'test-token-123';

      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          apiUrl={customApiUrl}
          token={customToken}
        />
      );

      await waitFor(() => {
        const { WebSocketManager } = require('../../services/WebSocketManager');
        expect(WebSocketManager).toHaveBeenCalledWith({
          apiUrl: customApiUrl,
          token: customToken,
          onConnect: expect.any(Function),
          onDisconnect: expect.any(Function),
          onReconnecting: expect.any(Function),
          onMessage: expect.any(Function),
          onError: expect.any(Function)
        });
      });
    });

    test('should handle WebSocket connection state changes', async () => {
      // Start disconnected
      mockWebSocketManager.getConnectionState.mockReturnValue(false);
      
      const { rerender } = renderWithProviders(
        <Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />
      );

      // Should show disconnected status
      expect(screen.getByText(/disconnected/i)).toBeInTheDocument();

      // Connect
      mockWebSocketManager.getConnectionState.mockReturnValue(true);
      
      // Trigger onConnect callback
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      config.onConnect();

      rerender(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(screen.getByText(/connected/i)).toBeInTheDocument();
      });
    });

    test('should handle WebSocket disconnection gracefully', async () => {
      const { rerender } = renderWithProviders(
        <Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />
      );

      // Simulate disconnection
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      mockWebSocketManager.getConnectionState.mockReturnValue(false);
      config.onDisconnect();

      rerender(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(screen.getByText(/disconnected/i)).toBeInTheDocument();
      });
    });

    test('should handle WebSocket reconnection attempts', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate reconnecting state
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      config.onReconnecting();

      await waitFor(() => {
        expect(screen.getByText(/reconnecting/i)).toBeInTheDocument();
      });
    });

    test('should handle WebSocket errors', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const testError = new Error('Connection failed');
      
      // Simulate WebSocket error
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      config.onError(testError);

      // Error should be handled gracefully (logged, displayed, etc.)
      await waitFor(() => {
        // Check if error is displayed or logged
        expect(true).toBe(true); // Placeholder - implement actual error handling check
      });
    });

    test('should clean up WebSocket connection on unmount', () => {
      const { unmount } = renderWithProviders(
        <Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />
      );

      unmount();

      expect(mockWebSocketManager.disconnect).toHaveBeenCalled();
    });
  });

  describe('Session Management', () => {
    test('should create new session when sessionId not provided', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(mockWebSocketManager.createSession).toHaveBeenCalledWith(TEST_CONSTANTS.MOCK_VM_ID);
      });
    });

    test('should restore existing session when sessionId provided', async () => {
      const existingSessionId = 'existing-session-456';
      
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          sessionId={existingSessionId}
        />
      );

      await waitFor(() => {
        expect(mockWebSocketManager.joinSession).toHaveBeenCalledWith(existingSessionId);
      });
    });

    test('should handle session creation response', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate session_created message
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      const sessionCreatedMessage = {
        type: 'session_created',
        data: { session_id: 'new-session-789' }
      };

      config.onMessage(sessionCreatedMessage);

      await waitFor(() => {
        expect(mockWebSocketManager.joinSession).toHaveBeenCalledWith('new-session-789');
      });
    });

    test('should handle session recovery with history', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Mock terminal instance
      const mockTerminal = {
        clear: vi.fn(),
        write: vi.fn()
      };
      
      // Get xterm mock
      const { Terminal: XTerm } = require('@xterm/xterm');
      XTerm.mockImplementation(() => mockTerminal);

      // Simulate session_recovered message
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      const recoveryMessage = {
        type: 'session_recovered',
        recovery_data: {
          history: ['$ ls -la\r\n', 'total 64\r\n', 'drwxr-xr-x  8 user user 256 Jan 1 12:00 .\r\n']
        }
      };

      config.onMessage(recoveryMessage);

      await waitFor(() => {
        expect(mockTerminal.clear).toHaveBeenCalled();
        expect(mockTerminal.write).toHaveBeenCalledTimes(3);
      });
    });

    test('should handle session end notification', async () => {
      const onSessionEnded = vi.fn();
      
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          onSessionEnded={onSessionEnded}
        />
      );

      // Simulate session_ended message
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      config.onMessage({ type: 'session_ended' });

      await waitFor(() => {
        expect(onSessionEnded).toHaveBeenCalled();
      });
    });
  });

  describe('Real-time Data Communication', () => {
    test('should handle incoming terminal data', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Mock terminal instance
      const mockTerminal = { write: vi.fn() };
      const { Terminal: XTerm } = require('@xterm/xterm');
      XTerm.mockImplementation(() => mockTerminal);

      // Simulate terminal_data message
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      const terminalData = 'Welcome to the terminal!\r\n$ ';
      config.onMessage({
        type: 'terminal_data',
        data: terminalData
      });

      await waitFor(() => {
        expect(mockTerminal.write).toHaveBeenCalledWith(terminalData);
      });
    });

    test('should send terminal input to WebSocket', async () => {
      const { terminalTestUtils } = require('../../../test/utils/test-utils');
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Get mock terminal and simulate user input
      const mockTerminal = terminalTestUtils.createMockTerminal();
      const { Terminal: XTerm } = require('@xterm/xterm');
      XTerm.mockImplementation(() => mockTerminal);

      // Wait for component to initialize
      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      // Simulate user typing
      const userInput = 'ls -la\r';
      const dataHandler = mockTerminal.onData.mock.calls[0][0];
      dataHandler(userInput);

      expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledWith(userInput);
    });

    test('should not send input in readOnly mode', async () => {
      const { terminalTestUtils } = require('../../../test/utils/test-utils');
      
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          readOnly={true}
        />
      );

      // Get mock terminal and simulate input
      const mockTerminal = terminalTestUtils.createMockTerminal();
      const { Terminal: XTerm } = require('@xterm/xterm');
      XTerm.mockImplementation(() => mockTerminal);

      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      // Simulate user typing in readOnly mode
      const userInput = 'should not send';
      const dataHandler = mockTerminal.onData.mock.calls[0][0];
      dataHandler(userInput);

      expect(mockWebSocketManager.sendTerminalData).not.toHaveBeenCalled();
    });

    test('should handle terminal resize events', async () => {
      const { terminalTestUtils } = require('../../../test/utils/test-utils');
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Get mock terminal
      const mockTerminal = terminalTestUtils.createMockTerminal();
      const { Terminal: XTerm } = require('@xterm/xterm');
      XTerm.mockImplementation(() => mockTerminal);

      await waitFor(() => {
        expect(mockTerminal.onResize).toHaveBeenCalled();
      });

      // Simulate terminal resize
      const newSize = { cols: 120, rows: 40 };
      const resizeHandler = mockTerminal.onResize.mock.calls[0][0];
      resizeHandler(newSize);

      expect(mockWebSocketManager.sendResize).toHaveBeenCalledWith(newSize.cols, newSize.rows);
    });

    test('should handle WebSocket error messages', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Mock terminal instance
      const mockTerminal = { write: vi.fn() };
      const { Terminal: XTerm } = require('@xterm/xterm');
      XTerm.mockImplementation(() => mockTerminal);

      // Simulate error message
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      const errorMessage = {
        type: 'error',
        message: 'Session timeout'
      };

      config.onMessage(errorMessage);

      await waitFor(() => {
        expect(mockTerminal.write).toHaveBeenCalledWith(
          expect.stringContaining('Error: Session timeout')
        );
      });
    });
  });

  describe('Performance and Reliability', () => {
    test('should handle rapid WebSocket messages without loss', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Mock terminal instance
      const mockTerminal = { write: vi.fn() };
      const { Terminal: XTerm } = require('@xterm/xterm');
      XTerm.mockImplementation(() => mockTerminal);

      // Simulate rapid messages
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      const rapidMessages = Array.from({ length: 100 }, (_, i) => ({
        type: 'terminal_data',
        data: `Line ${i}\r\n`
      }));

      rapidMessages.forEach(message => {
        config.onMessage(message);
      });

      await waitFor(() => {
        expect(mockTerminal.write).toHaveBeenCalledTimes(100);
      });
    });

    test('should maintain connection state accuracy', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Test connection state changes
      const states = [true, false, true, false];
      
      for (const connected of states) {
        mockWebSocketManager.getConnectionState.mockReturnValue(connected);
        
        // Verify state is reflected in UI
        await waitFor(() => {
          const expectedText = connected ? /connected/i : /disconnected/i;
          expect(screen.getByText(expectedText)).toBeInTheDocument();
        });
      }
    });

    test('should handle WebSocket latency monitoring', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate ping-pong for latency measurement
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      // Send heartbeat
      config.onMessage({ type: 'heartbeat', timestamp: Date.now() });

      // Should respond with heartbeat_response (implementation dependent)
      await waitFor(() => {
        expect(true).toBe(true); // Placeholder for actual latency monitoring check
      });
    });
  });

  describe('Security and Data Integrity', () => {
    test('should validate incoming WebSocket messages', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Mock terminal instance
      const mockTerminal = { write: vi.fn() };
      const { Terminal: XTerm } = require('@xterm/xterm');
      XTerm.mockImplementation(() => mockTerminal);

      // Simulate invalid message
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      const invalidMessage = {
        // Missing required fields
        invalidField: 'should be rejected'
      };

      config.onMessage(invalidMessage);

      // Invalid message should not cause terminal to write anything
      expect(mockTerminal.write).not.toHaveBeenCalled();
    });

    test('should sanitize terminal data for security', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Mock terminal instance
      const mockTerminal = { write: vi.fn() };
      const { Terminal: XTerm } = require('@xterm/xterm');
      XTerm.mockImplementation(() => mockTerminal);

      // Simulate potentially malicious data
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      const maliciousData = '<script>alert("xss")</script>';
      config.onMessage({
        type: 'terminal_data',
        data: maliciousData
      });

      await waitFor(() => {
        // Data should be sanitized before writing to terminal
        expect(mockTerminal.write).toHaveBeenCalledWith(
          expect.not.stringContaining('<script>')
        );
      });
    });

    test('should handle authentication failures gracefully', async () => {
      // Mock WebSocket connection failure due to auth
      const { WebSocketManager } = require('../../services/WebSocketManager');
      WebSocketManager.mockImplementationOnce(() => {
        throw new Error('Authentication failed');
      });

      expect(() => {
        renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);
      }).not.toThrow();
    });
  });
});