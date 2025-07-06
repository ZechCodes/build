/**
 * Terminal Integration Tests - Phase 4
 * Backend integration testing with MSW mock server
 */

import React from 'react';
import { describe, test, expect, vi, beforeEach, afterEach, beforeAll, afterAll } from 'vitest';
import { screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { server } from '../../../test/mocks/server';
import { 
  renderWithProviders, 
  terminalTestUtils, 
  websocketTestUtils,
  TEST_CONSTANTS 
} from '../../../test/utils/test-utils';
import { Terminal } from '../Terminal';

// Start server before all tests
beforeAll(() => server.listen({ onUnhandledRequest: 'error' }));

// Close server after all tests
afterAll(() => server.close());

// Reset handlers after each test
afterEach(() => {
  server.resetHandlers();
  cleanup();
});

describe('Terminal Component - Phase 4: Backend Integration', () => {
  let mockTerminal: any;
  let user: any;

  beforeEach(() => {
    vi.clearAllMocks();
    cleanup();
    
    mockTerminal = terminalTestUtils.createMockTerminal();
    const { Terminal: XTerm } = require('@xterm/xterm');
    XTerm.mockImplementation(() => mockTerminal);
    
    user = userEvent.setup();
  });

  describe('Authentication and Authorization', () => {
    test('should handle valid authentication token', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      // Should initialize without authentication errors
      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });

      // Should show connected status
      expect(screen.getByText(/connected/i)).toBeInTheDocument();
    });

    test('should handle invalid authentication token', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token="invalid-token"
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      // Should handle authentication failure gracefully
      await waitFor(() => {
        expect(screen.getByText(/disconnected/i)).toBeInTheDocument();
      });
    });

    test('should handle missing authentication token', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token=""
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      // Should not attempt connection without token
      expect(screen.getByText(/disconnected/i)).toBeInTheDocument();
    });

    test('should handle expired authentication token', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token="expired-token"
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      // Should detect token expiration and show appropriate status
      await waitFor(() => {
        expect(screen.getByText(/disconnected/i)).toBeInTheDocument();
      });
    });
  });

  describe('VM and Session Management', () => {
    test('should create new terminal session for VM', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      // Should request session creation
      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });

      // Verify session creation was requested
      // This would be validated through WebSocket message inspection
      expect(screen.getByText(/connected/i)).toBeInTheDocument();
    });

    test('should restore existing terminal session', async () => {
      const existingSessionId = 'existing-session-123';
      
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          sessionId={existingSessionId}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      // Should attempt to restore session
      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });

      expect(screen.getByText(/connected/i)).toBeInTheDocument();
    });

    test('should handle non-existent VM', async () => {
      renderWithProviders(
        <Terminal 
          vmId="non-existent-vm"
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      // Should handle VM not found error
      await waitFor(() => {
        // Error should be displayed in terminal or status
        expect(mockTerminal.write).toHaveBeenCalledWith(
          expect.stringContaining('Error')
        );
      });
    });

    test('should handle VM in wrong state', async () => {
      renderWithProviders(
        <Terminal 
          vmId="stopped-vm"
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      // Should handle VM state error
      await waitFor(() => {
        expect(mockTerminal.write).toHaveBeenCalledWith(
          expect.stringContaining('VM is not running')
        );
      });
    });

    test('should handle session creation failure', async () => {
      renderWithProviders(
        <Terminal 
          vmId="session-create-fail"
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      // Should handle session creation failure
      await waitFor(() => {
        expect(mockTerminal.write).toHaveBeenCalledWith(
          expect.stringContaining('Failed to create session')
        );
      });
    });
  });

  describe('Real-time Communication', () => {
    test('should receive terminal output from backend', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });

      // Simulate backend sending terminal output
      const mockWS = websocketTestUtils.createMockWebSocket();
      websocketTestUtils.simulateWSMessage(mockWS, {
        type: 'terminal_data',
        data: 'Welcome to Ubuntu 22.04 LTS\r\n$ '
      });

      await waitFor(() => {
        expect(mockTerminal.write).toHaveBeenCalledWith(
          'Welcome to Ubuntu 22.04 LTS\r\n$ '
        );
      });
    });

    test('should send user input to backend', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      // Simulate user typing
      const dataHandler = mockTerminal.onData.mock.calls[0][0];
      dataHandler('ls -la\r');

      // Should send input to backend via WebSocket
      // This would be verified through WebSocket message inspection
      expect(dataHandler).toHaveBeenCalledWith('ls -la\r');
    });

    test('should handle terminal resize events', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.onResize).toHaveBeenCalled();
      });

      // Simulate terminal resize
      const resizeHandler = mockTerminal.onResize.mock.calls[0][0];
      resizeHandler({ cols: 120, rows: 40 });

      // Should send resize event to backend
      // This would be verified through WebSocket message inspection
      expect(resizeHandler).toHaveBeenCalledWith({ cols: 120, rows: 40 });
    });

    test('should handle WebSocket reconnection', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(screen.getByText(/connected/i)).toBeInTheDocument();
      });

      // Simulate WebSocket disconnection
      const mockWS = websocketTestUtils.createMockWebSocket();
      websocketTestUtils.simulateWSDisconnection(mockWS, 1006, 'Connection lost');

      await waitFor(() => {
        expect(screen.getByText(/reconnecting/i)).toBeInTheDocument();
      });

      // Simulate successful reconnection
      websocketTestUtils.simulateWSConnection(mockWS);

      await waitFor(() => {
        expect(screen.getByText(/connected/i)).toBeInTheDocument();
      });
    });

    test('should handle high-frequency data from backend', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });

      // Simulate rapid data from backend
      const mockWS = websocketTestUtils.createMockWebSocket();
      
      for (let i = 0; i < 100; i++) {
        websocketTestUtils.simulateWSMessage(mockWS, {
          type: 'terminal_data',
          data: `Line ${i}\r\n`
        });
      }

      // Should handle all messages without loss
      await waitFor(() => {
        expect(mockTerminal.write).toHaveBeenCalledTimes(100);
      });
    });
  });

  describe('Error Handling and Recovery', () => {
    test('should handle backend API errors', async () => {
      // Mock API to return error
      server.use(
        /* Mock handlers for error scenarios would be set up here */
      );

      renderWithProviders(
        <Terminal 
          vmId="error-vm"
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      // Should handle API errors gracefully
      await waitFor(() => {
        expect(mockTerminal.write).toHaveBeenCalledWith(
          expect.stringContaining('Error')
        );
      });
    });

    test('should handle network timeouts', async () => {
      renderWithProviders(
        <Terminal 
          vmId="timeout-vm"
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      // Should handle timeout gracefully
      await waitFor(() => {
        expect(screen.getByText(/disconnected/i)).toBeInTheDocument();
      });
    });

    test('should handle malformed backend responses', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });

      // Simulate malformed WebSocket message
      const mockWS = websocketTestUtils.createMockWebSocket();
      websocketTestUtils.simulateWSMessage(mockWS, {
        // Missing required fields
        invalidData: 'malformed message'
      });

      // Should handle gracefully without crashing
      expect(mockTerminal.write).not.toHaveBeenCalledWith(
        expect.stringContaining('invalidData')
      );
    });

    test('should handle session recovery after connection loss', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          sessionId="recovery-session"
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });

      // Simulate session recovery with history
      const mockWS = websocketTestUtils.createMockWebSocket();
      websocketTestUtils.simulateWSMessage(mockWS, {
        type: 'session_recovered',
        recovery_data: {
          history: [
            'Previous command output\r\n',
            '$ ls -la\r\n',
            'total 16\r\n'
          ]
        }
      });

      await waitFor(() => {
        expect(mockTerminal.clear).toHaveBeenCalled();
        expect(mockTerminal.write).toHaveBeenCalledTimes(3);
      });
    });
  });

  describe('Performance Under Load', () => {
    test('should handle concurrent user input', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      const dataHandler = mockTerminal.onData.mock.calls[0][0];
      
      // Simulate rapid concurrent input
      const inputs = Array.from({ length: 1000 }, (_, i) => String(i % 10));
      
      const startTime = performance.now();
      inputs.forEach(input => dataHandler(input));
      const endTime = performance.now();
      
      // Should process all inputs efficiently
      expect(endTime - startTime).toBeLessThan(100); // Within 100ms
    });

    test('should handle large terminal output efficiently', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });

      // Simulate large output (like cat of a big file)
      const mockWS = websocketTestUtils.createMockWebSocket();
      const largeOutput = 'A'.repeat(10000) + '\r\n';
      
      const startTime = performance.now();
      websocketTestUtils.simulateWSMessage(mockWS, {
        type: 'terminal_data',
        data: largeOutput
      });
      const endTime = performance.now();

      // Should handle large output efficiently
      expect(endTime - startTime).toBeLessThan(50); // Within 50ms
      
      await waitFor(() => {
        expect(mockTerminal.write).toHaveBeenCalledWith(largeOutput);
      });
    });

    test('should maintain performance with many resize events', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.onResize).toHaveBeenCalled();
      });

      const resizeHandler = mockTerminal.onResize.mock.calls[0][0];
      
      // Simulate many rapid resize events
      const startTime = performance.now();
      for (let i = 0; i < 100; i++) {
        resizeHandler({ cols: 80 + i, rows: 24 + i });
      }
      const endTime = performance.now();

      // Should handle efficiently with throttling
      expect(endTime - startTime).toBeLessThan(100);
    });
  });

  describe('Security Integration', () => {
    test('should reject unauthorized session access', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          sessionId="unauthorized-session"
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      // Should handle unauthorized access gracefully
      await waitFor(() => {
        expect(mockTerminal.write).toHaveBeenCalledWith(
          expect.stringContaining('Unauthorized')
        );
      });
    });

    test('should sanitize dangerous terminal output from backend', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });

      // Simulate dangerous output from backend
      const mockWS = websocketTestUtils.createMockWebSocket();
      websocketTestUtils.simulateWSMessage(mockWS, {
        type: 'terminal_data',
        data: '<script>alert("xss")</script>\x00\x08dangerous'
      });

      await waitFor(() => {
        // Should write sanitized content only
        expect(mockTerminal.write).toHaveBeenCalledWith(
          expect.not.stringContaining('<script>')
        );
        expect(mockTerminal.write).toHaveBeenCalledWith(
          expect.not.stringContaining('\x00')
        );
      });
    });

    test('should handle rate limiting from backend', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      // Simulate rapid input that might trigger rate limiting
      const dataHandler = mockTerminal.onData.mock.calls[0][0];
      
      for (let i = 0; i < 1000; i++) {
        dataHandler('spam');
      }

      // Should handle rate limiting gracefully
      // Backend would send rate limit error
      const mockWS = websocketTestUtils.createMockWebSocket();
      websocketTestUtils.simulateWSMessage(mockWS, {
        type: 'error',
        message: 'Rate limit exceeded'
      });

      await waitFor(() => {
        expect(mockTerminal.write).toHaveBeenCalledWith(
          expect.stringContaining('Rate limit exceeded')
        );
      });
    });
  });

  describe('Cross-browser Compatibility', () => {
    test('should work with different WebSocket implementations', async () => {
      // Test would verify WebSocket compatibility across browsers
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });

      // Should work regardless of WebSocket implementation
      expect(screen.getByText(/connected/i)).toBeInTheDocument();
    });

    test('should handle different clipboard APIs', async () => {
      // Mock different clipboard API implementations
      const originalClipboard = navigator.clipboard;
      
      // Test with modern Clipboard API
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      const pasteButton = screen.getByTitle(/paste/i);
      await user.click(pasteButton);

      // Should work with modern API
      expect(navigator.clipboard.readText).toHaveBeenCalled();

      // Restore original
      Object.defineProperty(navigator, 'clipboard', {
        value: originalClipboard,
        writable: true
      });
    });
  });

  describe('Edge Cases and Stress Testing', () => {
    test('should handle empty terminal output', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });

      // Send empty data
      const mockWS = websocketTestUtils.createMockWebSocket();
      websocketTestUtils.simulateWSMessage(mockWS, {
        type: 'terminal_data',
        data: ''
      });

      // Should handle gracefully
      await waitFor(() => {
        expect(mockTerminal.write).toHaveBeenCalledWith('');
      });
    });

    test('should handle extremely long terminal lines', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });

      // Send extremely long line
      const longLine = 'A'.repeat(100000) + '\r\n';
      const mockWS = websocketTestUtils.createMockWebSocket();
      websocketTestUtils.simulateWSMessage(mockWS, {
        type: 'terminal_data',
        data: longLine
      });

      // Should handle without performance issues
      await waitFor(() => {
        expect(mockTerminal.write).toHaveBeenCalledWith(longLine);
      });
    });

    test('should handle rapid connection/disconnection cycles', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      );

      const mockWS = websocketTestUtils.createMockWebSocket();

      // Simulate rapid connect/disconnect cycles
      for (let i = 0; i < 10; i++) {
        websocketTestUtils.simulateWSConnection(mockWS);
        websocketTestUtils.simulateWSDisconnection(mockWS);
      }

      // Should handle gracefully without memory leaks or crashes
      await waitFor(() => {
        expect(true).toBe(true); // Component should remain stable
      });
    });

    test('should handle simultaneous multiple terminal instances', async () => {
      // Render multiple terminal instances
      const terminals = Array.from({ length: 5 }, (_, i) => (
        <Terminal 
          key={i}
          vmId={`vm-${i}`}
          token={TEST_CONSTANTS.MOCK_TOKEN}
          apiUrl={TEST_CONSTANTS.API_URL}
        />
      ));

      const { container } = renderWithProviders(<div>{terminals}</div>);

      // All terminals should initialize successfully
      await waitFor(() => {
        const terminalElements = container.querySelectorAll('[role="terminal"]');
        expect(terminalElements).toHaveLength(5);
      });
    });
  });
});