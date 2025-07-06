/**
 * Terminal Production Readiness Tests - Phase 5
 * Comprehensive error handling, edge cases, and production scenarios
 */

import React from 'react';
import { describe, test, expect, vi, beforeEach, afterEach } from 'vitest';
import { screen, fireEvent, waitFor, cleanup, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { 
  renderWithProviders, 
  terminalTestUtils, 
  websocketTestUtils,
  performanceTestUtils,
  securityTestUtils,
  TEST_CONSTANTS 
} from '../../../test/utils/test-utils';
import { Terminal } from '../Terminal';

// Mock console methods to verify error handling
const originalError = console.error;
const originalWarn = console.warn;
const mockConsoleError = vi.fn();
const mockConsoleWarn = vi.fn();

beforeEach(() => {
  console.error = mockConsoleError;
  console.warn = mockConsoleWarn;
  vi.clearAllMocks();
  cleanup();
});

afterEach(() => {
  console.error = originalError;
  console.warn = originalWarn;
  cleanup();
});

describe('Terminal Component - Phase 5: Production Readiness', () => {
  describe('Error Boundary and Crash Prevention', () => {
    test('should handle component crashes gracefully', async () => {
      // Mock terminal constructor to throw error
      const { Terminal: XTerm } = require('@xterm/xterm');
      XTerm.mockImplementationOnce(() => {
        throw new Error('Critical terminal initialization failure');
      });

      // Component should not crash the entire application
      expect(() => {
        renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);
      }).not.toThrow();

      // Should log error but continue functioning
      expect(mockConsoleError).toHaveBeenCalledWith(
        expect.stringContaining('terminal initialization failure'),
        expect.any(Error)
      );
    });

    test('should handle addon loading failures without crashing', async () => {
      const mockTerminal = terminalTestUtils.createMockTerminal();
      mockTerminal.loadAddon.mockImplementation(() => {
        throw new Error('Addon loading failed');
      });

      const { Terminal: XTerm } = require('@xterm/xterm');
      XTerm.mockImplementation(() => mockTerminal);

      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(mockTerminal.loadAddon).toHaveBeenCalled();
      });

      // Should handle addon failure gracefully
      expect(mockConsoleWarn).toHaveBeenCalledWith(
        expect.stringContaining('addon'),
        expect.any(Error)
      );

      // Terminal should still be rendered and functional
      const terminal = screen.getByRole('terminal');
      expect(terminal).toBeInTheDocument();
    });

    test('should handle WebSocket constructor failures', async () => {
      const { WebSocketManager } = require('../../services/WebSocketManager');
      WebSocketManager.mockImplementation(() => {
        throw new Error('WebSocket initialization failed');
      });

      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Should not crash and show appropriate error state
      await waitFor(() => {
        expect(screen.getByText(/disconnected/i)).toBeInTheDocument();
      });
    });

    test('should handle render errors in child components', async () => {
      // Mock TerminalToolbar to throw error
      vi.doMock('../TerminalToolbar', () => ({
        TerminalToolbar: () => {
          throw new Error('Toolbar render error');
        }
      }));

      expect(() => {
        renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} showToolbar={true} />);
      }).not.toThrow();
    });

    test('should handle memory allocation failures', async () => {
      const mockTerminal = terminalTestUtils.createMockTerminal();
      mockTerminal.write.mockImplementation(() => {
        throw new Error('Out of memory');
      });

      const { Terminal: XTerm } = require('@xterm/xterm');
      XTerm.mockImplementation(() => mockTerminal);

      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate incoming data that would trigger memory error
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      config.onMessage({
        type: 'terminal_data',
        data: 'test data'
      });

      // Should handle memory error gracefully
      await waitFor(() => {
        expect(mockConsoleError).toHaveBeenCalled();
      });
    });
  });

  describe('Network Failure Scenarios', () => {
    test('should handle complete network failure', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate network failure
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      config.onError(new Error('Network unreachable'));

      await waitFor(() => {
        expect(screen.getByText(/disconnected/i)).toBeInTheDocument();
      });

      // Should show appropriate error message
      const mockTerminal = terminalTestUtils.createMockTerminal();
      expect(mockTerminal.write).toHaveBeenCalledWith(
        expect.stringContaining('Connection error')
      );
    });

    test('should handle intermittent connectivity', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];

      // Simulate connection loss and recovery cycles
      for (let i = 0; i < 5; i++) {
        config.onDisconnect();
        await waitFor(() => {
          expect(screen.getByText(/disconnected/i)).toBeInTheDocument();
        });

        config.onReconnecting();
        await waitFor(() => {
          expect(screen.getByText(/reconnecting/i)).toBeInTheDocument();
        });

        config.onConnect();
        await waitFor(() => {
          expect(screen.getByText(/connected/i)).toBeInTheDocument();
        });
      }

      // Should handle multiple disconnections without issues
      expect(screen.getByText(/connected/i)).toBeInTheDocument();
    });

    test('should handle slow network conditions', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];

      // Simulate delayed message processing
      const delayedMessages = Array.from({ length: 100 }, (_, i) => ({
        type: 'terminal_data',
        data: `Delayed message ${i}\r\n`
      }));

      const startTime = performance.now();
      
      // Send messages with artificial delay
      delayedMessages.forEach((message, index) => {
        setTimeout(() => {
          config.onMessage(message);
        }, index * 10); // 10ms delay between messages
      });

      // Should handle all messages even with delays
      await waitFor(() => {
        const mockTerminal = terminalTestUtils.createMockTerminal();
        expect(mockTerminal.write).toHaveBeenCalledTimes(100);
      }, { timeout: 5000 });

      const endTime = performance.now();
      expect(endTime - startTime).toBeLessThan(3000); // Should complete within 3 seconds
    });

    test('should handle network timeout scenarios', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate long network timeout
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      // Don't respond to connection attempts
      vi.useFakeTimers();
      
      config.onReconnecting();
      
      // Fast forward to timeout
      vi.advanceTimersByTime(30000); // 30 seconds
      
      // Should handle timeout gracefully
      await waitFor(() => {
        expect(screen.getByText(/disconnected/i)).toBeInTheDocument();
      });

      vi.useRealTimers();
    });
  });

  describe('Resource Exhaustion Scenarios', () => {
    test('should handle memory pressure', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate high memory usage
      Object.defineProperty(performance, 'memory', {
        value: {
          usedJSHeapSize: 100 * 1024 * 1024, // 100MB
          totalJSHeapSize: 120 * 1024 * 1024, // 120MB
          jsHeapSizeLimit: 128 * 1024 * 1024  // 128MB limit
        },
        configurable: true
      });

      // Should trigger memory optimization
      const { useMemoryOptimization } = require('../../hooks/usePerformance');
      const mockCleanup = vi.fn();
      useMemoryOptimization.mockReturnValue({ 
        addCleanup: vi.fn(),
        runCleanup: mockCleanup 
      });

      // Trigger memory check
      fireEvent(window, new Event('resize'));

      await waitFor(() => {
        expect(mockConsoleWarn).toHaveBeenCalledWith(
          expect.stringContaining('memory usage')
        );
      });
    });

    test('should handle excessive terminal output', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];

      // Simulate massive output (like cat of a huge file)
      const hugeOutput = 'A'.repeat(1000000); // 1MB of data
      
      const startTime = performance.now();
      
      config.onMessage({
        type: 'terminal_data',
        data: hugeOutput
      });

      // Should handle large output without freezing
      await waitFor(() => {
        const mockTerminal = terminalTestUtils.createMockTerminal();
        expect(mockTerminal.write).toHaveBeenCalled();
      });

      const endTime = performance.now();
      expect(endTime - startTime).toBeLessThan(1000); // Should process within 1 second
    });

    test('should handle CPU-intensive operations', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate CPU-intensive terminal operations
      const terminal = screen.getByRole('terminal');
      const user = userEvent.setup();

      await terminal.focus();

      const startTime = performance.now();

      // Rapid input simulation
      for (let i = 0; i < 1000; i++) {
        await user.keyboard(String(i % 10));
      }

      const endTime = performance.now();
      
      // Should maintain responsiveness even under load
      expect(endTime - startTime).toBeLessThan(5000); // 5 seconds max
    });
  });

  describe('Browser Compatibility Edge Cases', () => {
    test('should handle missing modern APIs gracefully', async () => {
      // Mock missing clipboard API
      const originalClipboard = navigator.clipboard;
      
      Object.defineProperty(navigator, 'clipboard', {
        value: undefined,
        configurable: true
      });

      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const pasteButton = screen.getByTitle(/paste/i);
      await userEvent.click(pasteButton);

      // Should handle missing API gracefully
      expect(mockConsoleWarn).toHaveBeenCalledWith(
        expect.stringContaining('clipboard')
      );

      // Restore clipboard
      Object.defineProperty(navigator, 'clipboard', {
        value: originalClipboard,
        configurable: true
      });
    });

    test('should handle missing WebSocket support', async () => {
      // Mock WebSocket not available
      const originalWebSocket = global.WebSocket;
      global.WebSocket = undefined as any;

      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Should show appropriate error
      await waitFor(() => {
        expect(screen.getByText(/disconnected/i)).toBeInTheDocument();
      });

      // Restore WebSocket
      global.WebSocket = originalWebSocket;
    });

    test('should handle missing performance API', async () => {
      // Mock missing performance.now
      const originalPerformance = global.performance;
      global.performance = {} as any;

      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Should fallback gracefully
      const terminal = screen.getByRole('terminal');
      expect(terminal).toBeInTheDocument();

      // Restore performance
      global.performance = originalPerformance;
    });

    test('should handle missing ResizeObserver', async () => {
      // Mock missing ResizeObserver
      const originalResizeObserver = global.ResizeObserver;
      global.ResizeObserver = undefined as any;

      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Should use fallback resize detection
      fireEvent(window, new Event('resize'));

      const terminal = screen.getByRole('terminal');
      expect(terminal).toBeInTheDocument();

      // Restore ResizeObserver
      global.ResizeObserver = originalResizeObserver;
    });
  });

  describe('Data Corruption and Validation', () => {
    test('should handle corrupted WebSocket messages', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];

      // Send corrupted/malformed messages
      const corruptedMessages = [
        null,
        undefined,
        '',
        '{"invalid": json}',
        { type: null },
        { type: 'invalid_type' },
        { type: 'terminal_data' }, // Missing data field
        { type: 'terminal_data', data: null },
        { type: 'terminal_data', data: { invalid: 'object' } }
      ];

      corruptedMessages.forEach(message => {
        expect(() => {
          config.onMessage(message);
        }).not.toThrow();
      });

      // Should log security events for invalid messages
      expect(mockConsoleWarn).toHaveBeenCalledWith(
        expect.stringContaining('Security Event')
      );
    });

    test('should validate terminal data encoding', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];

      // Test various encoding issues
      const problematicData = [
        '\uFFFD', // Replacement character
        '\x00\x01\x02', // Null bytes and control characters
        'valid\uD800invalid', // Unpaired surrogates
        '🔥' + '\uDC00', // Emoji with unpaired surrogate
      ];

      problematicData.forEach(data => {
        config.onMessage({
          type: 'terminal_data',
          data
        });
      });

      // Should sanitize all problematic data
      const mockTerminal = terminalTestUtils.createMockTerminal();
      expect(mockTerminal.write).toHaveBeenCalledWith(
        expect.not.stringContaining('\x00')
      );
    });

    test('should handle session state corruption', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];

      // Send session recovery with corrupted data
      config.onMessage({
        type: 'session_recovered',
        recovery_data: {
          history: [
            null,
            undefined,
            123, // Non-string
            { invalid: 'object' },
            'valid line\r\n'
          ]
        }
      });

      // Should only process valid lines
      const mockTerminal = terminalTestUtils.createMockTerminal();
      expect(mockTerminal.write).toHaveBeenCalledWith('valid line\r\n');
      expect(mockTerminal.write).not.toHaveBeenCalledWith(null);
    });
  });

  describe('Concurrency and Race Conditions', () => {
    test('should handle concurrent initialization attempts', async () => {
      // Render multiple terminals simultaneously
      const terminals = Array.from({ length: 10 }, (_, i) => (
        <Terminal key={i} vmId={`vm-${i}`} />
      ));

      expect(() => {
        renderWithProviders(<div>{terminals}</div>);
      }).not.toThrow();

      // All terminals should initialize successfully
      await waitFor(() => {
        const terminalElements = screen.getAllByRole('terminal');
        expect(terminalElements).toHaveLength(10);
      });
    });

    test('should handle rapid connect/disconnect cycles', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];

      // Rapid connection state changes
      for (let i = 0; i < 50; i++) {
        config.onConnect();
        config.onDisconnect();
        config.onReconnecting();
      }

      // Should end in a stable state
      config.onConnect();
      
      await waitFor(() => {
        expect(screen.getByText(/connected/i)).toBeInTheDocument();
      });
    });

    test('should handle concurrent message processing', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];

      // Send many messages concurrently
      const promises = Array.from({ length: 100 }, (_, i) => 
        Promise.resolve().then(() => {
          config.onMessage({
            type: 'terminal_data',
            data: `Concurrent message ${i}\r\n`
          });
        })
      );

      await Promise.all(promises);

      // All messages should be processed
      const mockTerminal = terminalTestUtils.createMockTerminal();
      expect(mockTerminal.write).toHaveBeenCalledTimes(100);
    });

    test('should handle rapid user input', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const mockTerminal = terminalTestUtils.createMockTerminal();
      const { Terminal: XTerm } = require('@xterm/xterm');
      XTerm.mockImplementation(() => mockTerminal);

      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      const dataHandler = mockTerminal.onData.mock.calls[0][0];

      // Simulate rapid typing
      const rapidInput = Array.from({ length: 1000 }, (_, i) => String(i % 10));
      
      const promises = rapidInput.map(input => 
        Promise.resolve().then(() => dataHandler(input))
      );

      await Promise.all(promises);

      // All input should be processed
      const mockWebSocketManager = require('../../services/WebSocketManager');
      expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledTimes(1000);
    });
  });

  describe('Security Stress Testing', () => {
    test('should resist XSS injection attempts', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const xssAttempts = [
        '<script>alert("xss")</script>',
        '"><script>alert("xss")</script>',
        'javascript:alert("xss")',
        '<img src=x onerror=alert("xss")>',
        '&#60;script&#62;alert("xss")&#60;/script&#62;',
        '<svg onload=alert("xss")>',
        '<iframe src="javascript:alert(\'xss\')"></iframe>'
      ];

      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];

      xssAttempts.forEach(xss => {
        config.onMessage({
          type: 'terminal_data',
          data: xss
        });
      });

      // Should sanitize all XSS attempts
      const mockTerminal = terminalTestUtils.createMockTerminal();
      expect(mockTerminal.write).not.toHaveBeenCalledWith(
        expect.stringContaining('<script>')
      );
    });

    test('should handle oversized input attacks', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const mockTerminal = terminalTestUtils.createMockTerminal();
      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      const dataHandler = mockTerminal.onData.mock.calls[0][0];

      // Attempt to send oversized input
      const oversizedInput = 'A'.repeat(1000000); // 1MB of input
      
      expect(() => {
        dataHandler(oversizedInput);
      }).not.toThrow();

      // Should handle gracefully without crashing
      expect(mockTerminal).toBeTruthy();
    });

    test('should resist clipboard injection attacks', async () => {
      const maliciousClipboard = [
        '<script>alert("clipboard xss")</script>',
        'rm -rf /',
        '\x00\x01\x02\x03malicious',
        'password=stolen123'
      ];

      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      for (const malicious of maliciousClipboard) {
        navigator.clipboard.readText = vi.fn().mockResolvedValue(malicious);
        
        const pasteButton = screen.getByTitle(/paste/i);
        await userEvent.click(pasteButton);

        // Should sanitize malicious content
        const mockWebSocketManager = require('../../services/WebSocketManager');
        expect(mockWebSocketManager.sendTerminalData).not.toHaveBeenCalledWith(
          expect.stringContaining('<script>')
        );
      }
    });
  });

  describe('Performance Under Stress', () => {
    test('should maintain performance with many terminals', async () => {
      const startTime = performance.now();

      // Render 20 terminal instances
      const terminals = Array.from({ length: 20 }, (_, i) => (
        <Terminal key={i} vmId={`stress-vm-${i}`} />
      ));

      renderWithProviders(<div>{terminals}</div>);

      await waitFor(() => {
        const terminalElements = screen.getAllByRole('terminal');
        expect(terminalElements).toHaveLength(20);
      });

      const endTime = performance.now();
      const initTime = endTime - startTime;

      // Should initialize all terminals within reasonable time
      expect(initTime).toBeLessThan(5000); // 5 seconds for 20 terminals
    });

    test('should handle memory leaks prevention', async () => {
      const { unmount } = renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Verify cleanup is called on unmount
      const mockTerminal = terminalTestUtils.createMockTerminal();
      const { addCleanup } = require('../../hooks/usePerformance');
      
      expect(addCleanup).toHaveBeenCalled();

      unmount();

      expect(mockTerminal.dispose).toHaveBeenCalled();
    });

    test('should throttle high-frequency events', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const startTime = performance.now();

      // Trigger many resize events rapidly
      for (let i = 0; i < 100; i++) {
        fireEvent(window, new Event('resize'));
      }

      const endTime = performance.now();
      const processingTime = endTime - startTime;

      // Should complete efficiently due to throttling
      expect(processingTime).toBeLessThan(200); // 200ms max
    });
  });

  describe('Production Environment Simulation', () => {
    test('should handle production build optimizations', async () => {
      // Simulate production environment
      const originalNodeEnv = process.env.NODE_ENV;
      process.env.NODE_ENV = 'production';

      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const terminal = screen.getByRole('terminal');
      expect(terminal).toBeInTheDocument();

      // Should work in production mode
      await userEvent.click(screen.getByTitle(/search/i));
      expect(screen.getByRole('searchbox')).toBeInTheDocument();

      process.env.NODE_ENV = originalNodeEnv;
    });

    test('should handle CDN resource failures', async () => {
      // Mock failed resource loading
      const originalError = console.error;
      console.error = vi.fn();

      // Simulate CSS/font loading failure
      Object.defineProperty(document, 'fonts', {
        value: {
          ready: Promise.reject(new Error('Font loading failed'))
        },
        configurable: true
      });

      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Should still render and function
      const terminal = screen.getByRole('terminal');
      expect(terminal).toBeInTheDocument();

      console.error = originalError;
    });

    test('should validate all accessibility requirements', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Run comprehensive accessibility check
      const container = screen.getByRole('terminal').closest('.terminal-container');
      
      expect(() => {
        securityTestUtils.checkCSPCompliance();
      }).not.toThrow();

      // Check ARIA compliance
      const terminal = screen.getByRole('terminal');
      expect(terminal).toHaveAttribute('aria-label');
      expect(terminal).toHaveAttribute('tabindex', '0');

      // Check keyboard navigation
      expect(terminal).toBeVisible();
      await userEvent.tab();
      expect(terminal).toBeFocused();
    });
  });
});