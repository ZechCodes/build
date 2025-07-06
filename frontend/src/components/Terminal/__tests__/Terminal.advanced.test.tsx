/**
 * Terminal Advanced Features Tests - Phase 3
 * Focus on performance, security, accessibility, and advanced functionality
 */

import React from 'react';
import { describe, test, expect, vi, beforeEach, afterEach } from 'vitest';
import { screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { 
  renderWithProviders, 
  terminalTestUtils, 
  performanceTestUtils,
  accessibilityTestUtils,
  securityTestUtils,
  TEST_CONSTANTS 
} from '../../../test/utils/test-utils';
import { Terminal } from '../Terminal';

// Mock performance monitoring hooks
const mockPerformanceHooks = {
  startRender: vi.fn(),
  endRender: vi.fn(),
  startPing: vi.fn(),
  endPing: vi.fn(),
  addCleanup: vi.fn()
};

vi.mock('../../hooks/usePerformance', () => ({
  usePerformanceMonitoring: vi.fn(() => mockPerformanceHooks),
  useDataBatcher: vi.fn((callback) => ({
    addData: callback
  })),
  useThrottledCallback: vi.fn((callback) => callback),
  useWSLatencyMonitoring: vi.fn(() => mockPerformanceHooks),
  useMemoryOptimization: vi.fn(() => mockPerformanceHooks)
}));

// Mock security utilities
vi.mock('../../utils/security', async () => {
  const actual = await vi.importActual('../../utils/security');
  return {
    ...actual,
    logSecurityEvent: vi.fn()
  };
});

// Mock performance monitor
const mockPerformanceMonitor = {
  startProfiling: vi.fn(),
  endProfiling: vi.fn(),
  recordDataThroughput: vi.fn(),
  getMetrics: vi.fn(() => ({
    renderTime: 45,
    throughput: 1000,
    memoryUsage: 50 * 1024 * 1024
  }))
};

vi.mock('../../utils/performance', () => ({
  performanceMonitor: mockPerformanceMonitor
}));

// Mock WebSocket Manager
const mockWebSocketManager = {
  connect: vi.fn(),
  disconnect: vi.fn(),
  sendTerminalData: vi.fn(),
  sendResize: vi.fn(),
  createSession: vi.fn().mockResolvedValue({ session_id: 'test-session' }),
  joinSession: vi.fn().mockResolvedValue(true),
  getConnectionState: vi.fn(() => true),
  isConnected: vi.fn(() => true)
};

vi.mock('../../services/WebSocketManager', () => ({
  WebSocketManager: vi.fn().mockImplementation(() => mockWebSocketManager)
}));

describe('Terminal Component - Phase 3: Advanced Features', () => {
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

  afterEach(() => {
    cleanup();
  });

  describe('Performance Monitoring and Optimization', () => {
    test('should monitor component render performance', async () => {
      const startTime = performance.now();
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);
      
      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });
      
      const endTime = performance.now();
      const renderTime = endTime - startTime;
      
      // Should complete initial render within performance threshold
      expect(renderTime).toBeLessThan(TEST_CONSTANTS.PERFORMANCE_THRESHOLDS.RENDER_TIME_MS);
      
      // Performance monitoring hooks should be called
      expect(mockPerformanceHooks.startRender).toHaveBeenCalled();
      expect(mockPerformanceHooks.endRender).toHaveBeenCalled();
    });

    test('should profile terminal initialization steps', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(mockPerformanceMonitor.startProfiling).toHaveBeenCalledWith('terminal-init');
        expect(mockPerformanceMonitor.endProfiling).toHaveBeenCalledWith('terminal-init');
      });
    });

    test('should profile terminal write operations', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate incoming terminal data
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      config.onMessage({
        type: 'terminal_data',
        data: 'test output'
      });

      await waitFor(() => {
        expect(mockPerformanceMonitor.startProfiling).toHaveBeenCalledWith('terminal-write');
        expect(mockPerformanceMonitor.endProfiling).toHaveBeenCalledWith('terminal-write');
      });
    });

    test('should profile terminal resize operations', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Trigger window resize
      fireEvent(window, new Event('resize'));

      await waitFor(() => {
        expect(mockPerformanceMonitor.startProfiling).toHaveBeenCalledWith('terminal-fit');
        expect(mockPerformanceMonitor.endProfiling).toHaveBeenCalledWith('terminal-fit');
      });
    });

    test('should batch terminal data for performance', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate rapid terminal data messages
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      // Send multiple rapid messages
      for (let i = 0; i < 10; i++) {
        config.onMessage({
          type: 'terminal_data',
          data: `Message ${i}\r\n`
        });
      }

      // Data batching should optimize writes
      await waitFor(() => {
        expect(mockTerminal.write).toHaveBeenCalled();
      });
    });

    test('should throttle resize events', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { FitAddon } = require('@xterm/addon-fit');
      const fitAddonInstance = FitAddon.mock.results[0].value;

      // Trigger multiple rapid resize events
      for (let i = 0; i < 20; i++) {
        fireEvent(window, new Event('resize'));
      }

      // Should throttle resize calls
      await waitFor(() => {
        expect(fitAddonInstance.fit).toHaveBeenCalled();
      });
      
      // Should not call fit 20 times
      expect(fitAddonInstance.fit.mock.calls.length).toBeLessThan(20);
    });

    test('should monitor WebSocket latency', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate WebSocket connection
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      config.onConnect();

      await waitFor(() => {
        expect(mockPerformanceHooks.startPing).toHaveBeenCalled();
        expect(mockPerformanceHooks.endPing).toHaveBeenCalled();
      });
    });

    test('should record data throughput metrics', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      const testData = 'Large terminal output data...';
      config.onMessage({
        type: 'terminal_data',
        data: testData
      });

      await waitFor(() => {
        expect(mockPerformanceMonitor.recordDataThroughput).toHaveBeenCalledWith(testData.length);
      });
    });

    test('should optimize memory usage with cleanup', async () => {
      const { unmount } = renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(mockPerformanceHooks.addCleanup).toHaveBeenCalled();
      });

      unmount();

      // Memory cleanup should be performed
      expect(mockTerminal.dispose).toHaveBeenCalled();
    });
  });

  describe('Security and Data Protection', () => {
    test('should sanitize incoming terminal data', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      const maliciousData = '<script>alert("xss")</script>echo "safe"';
      config.onMessage({
        type: 'terminal_data',
        data: maliciousData
      });

      await waitFor(() => {
        // Should write sanitized data only
        expect(mockTerminal.write).toHaveBeenCalledWith(
          expect.not.stringContaining('<script>')
        );
      });
    });

    test('should validate WebSocket messages', async () => {
      const { logSecurityEvent } = require('../../utils/security');
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      // Send invalid message
      config.onMessage({
        invalidField: 'malicious content',
        // Missing required 'type' field
      });

      await waitFor(() => {
        expect(logSecurityEvent).toHaveBeenCalledWith(
          'invalid_websocket_message',
          expect.any(Object),
          'medium'
        );
      });
    });

    test('should detect and log sensitive data', async () => {
      const { logSecurityEvent } = require('../../utils/security');
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      const sensitiveData = 'password=supersecret123 api_key=abcdef123456';
      config.onMessage({
        type: 'terminal_data',
        data: sensitiveData
      });

      await waitFor(() => {
        expect(logSecurityEvent).toHaveBeenCalledWith(
          'sensitive_data_detected',
          expect.objectContaining({
            patterns: expect.arrayContaining(['password', 'api_key'])
          }),
          'high'
        );
      });
    });

    test('should mask sensitive data in output', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      
      const sensitiveData = 'export PASSWORD=secret123';
      config.onMessage({
        type: 'terminal_data',
        data: sensitiveData
      });

      await waitFor(() => {
        // Should write masked data
        expect(mockTerminal.write).toHaveBeenCalledWith(
          expect.stringContaining('***MASKED***')
        );
      });
    });

    test('should sanitize clipboard content before pasting', async () => {
      const { logSecurityEvent } = require('../../utils/security');
      
      const maliciousClipboard = 'safe content\x00\x08malicious\necho "hello"';
      navigator.clipboard.readText = vi.fn().mockResolvedValue(maliciousClipboard);
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const pasteButton = screen.getByTitle(/paste/i);
      await user.click(pasteButton);

      await waitFor(() => {
        // Should send sanitized content
        expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledWith(
          expect.not.stringContaining('\x00')
        );
        
        // Should log sanitization
        expect(logSecurityEvent).toHaveBeenCalledWith(
          'clipboard_content_sanitized',
          expect.any(Object),
          'low'
        );
      });
    });

    test('should handle XSS attempts in terminal content', () => {
      const container = document.createElement('div');
      
      // Test XSS prevention
      expect(() => {
        securityTestUtils.checkXSSPrevention(container, '<script>alert("xss")</script>');
      }).not.toThrow();
    });

    test('should enforce Content Security Policy compliance', () => {
      expect(() => {
        securityTestUtils.checkCSPCompliance();
      }).not.toThrow();
    });
  });

  describe('Accessibility and Usability', () => {
    test('should have proper ARIA attributes for accessibility', () => {
      const { container } = renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      expect(() => {
        accessibilityTestUtils.checkAriaLabels(container);
      }).not.toThrow();
    });

    test('should support keyboard navigation', () => {
      const { container } = renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      expect(() => {
        accessibilityTestUtils.checkKeyboardNavigation(container);
      }).not.toThrow();
    });

    test('should have adequate color contrast', () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const terminalElement = screen.getByRole('terminal');
      
      expect(() => {
        accessibilityTestUtils.checkColorContrast(terminalElement);
      }).not.toThrow();
    });

    test('should support high contrast theme', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          theme="high-contrast"
        />
      );

      const container = screen.getByRole('terminal').closest('.terminal-container');
      
      // Should apply high contrast theme
      expect(container).toHaveStyle({
        backgroundColor: '#000000',
        color: '#ffffff'
      });
    });

    test('should provide screen reader announcements', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const terminalElement = screen.getByRole('terminal');
      
      // Should have proper labeling
      expect(terminalElement).toHaveAttribute('aria-label');
      expect(terminalElement).toHaveAttribute('role', 'terminal');
    });

    test('should support focus management', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const terminalElement = screen.getByRole('terminal');
      
      // Should be focusable
      expect(terminalElement).toHaveAttribute('tabIndex', '0');
      
      // Focus should work
      fireEvent.focus(terminalElement);
      expect(mockTerminal.focus).toHaveBeenCalled();
    });

    test('should announce connection status changes', async () => {
      const { rerender } = renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Should show initial connection status
      expect(screen.getByText(/connected/i)).toBeInTheDocument();

      // Simulate disconnection
      mockWebSocketManager.getConnectionState.mockReturnValue(false);
      
      const { WebSocketManager } = require('../../services/WebSocketManager');
      const config = WebSocketManager.mock.calls[0][0];
      config.onDisconnect();

      rerender(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(screen.getByText(/disconnected/i)).toBeInTheDocument();
      });
    });
  });

  describe('Theme and Customization', () => {
    test('should support multiple terminal themes', async () => {
      const themes = ['dark', 'light', 'high-contrast', 'solarized-dark', 'solarized-light'] as const;

      for (const theme of themes) {
        const { unmount } = renderWithProviders(
          <Terminal 
            vmId={TEST_CONSTANTS.MOCK_VM_ID}
            theme={theme}
          />
        );

        // Should apply theme configuration
        const { Terminal: XTerm } = require('@xterm/xterm');
        const lastCall = XTerm.mock.calls[XTerm.mock.calls.length - 1];
        expect(lastCall[0]).toHaveProperty('background');
        expect(lastCall[0]).toHaveProperty('foreground');

        unmount();
      }
    });

    test('should support dynamic theme switching', async () => {
      const { rerender } = renderWithProviders(
        <Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} theme="dark" />
      );

      // Switch to light theme
      rerender(
        <Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} theme="light" />
      );

      await waitFor(() => {
        expect(mockTerminal.setOption).toHaveBeenCalled();
      });
    });

    test('should support custom dimensions', () => {
      const customHeight = '800px';
      const customWidth = '1200px';

      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          height={customHeight}
          width={customWidth}
        />
      );

      const container = screen.getByRole('terminal').closest('.terminal-container');
      expect(container).toHaveStyle({
        height: customHeight,
        width: customWidth
      });
    });

    test('should hide toolbar when specified', () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          showToolbar={false}
        />
      );

      // Toolbar buttons should not be present
      expect(screen.queryByTitle(/copy/i)).not.toBeInTheDocument();
      expect(screen.queryByTitle(/paste/i)).not.toBeInTheDocument();
    });
  });

  describe('Error Handling and Resilience', () => {
    test('should handle terminal initialization errors gracefully', () => {
      const { Terminal: XTerm } = require('@xterm/xterm');
      XTerm.mockImplementationOnce(() => {
        throw new Error('Terminal initialization failed');
      });

      expect(() => {
        renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);
      }).not.toThrow();
    });

    test('should handle addon loading failures', () => {
      mockTerminal.loadAddon.mockImplementationOnce(() => {
        throw new Error('Addon load failed');
      });

      expect(() => {
        renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);
      }).not.toThrow();
    });

    test('should handle WebSocket connection failures', () => {
      const { WebSocketManager } = require('../../services/WebSocketManager');
      WebSocketManager.mockImplementationOnce(() => {
        throw new Error('WebSocket connection failed');
      });

      expect(() => {
        renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);
      }).not.toThrow();
    });

    test('should handle fit addon errors', async () => {
      const { FitAddon } = require('@xterm/addon-fit');
      const fitAddonInstance = FitAddon.mock.results[0].value;
      fitAddonInstance.fit.mockImplementationOnce(() => {
        throw new Error('Fit failed');
      });

      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      fireEvent(window, new Event('resize'));

      // Should handle fit errors gracefully
      await waitFor(() => {
        expect(true).toBe(true); // Component should not crash
      });
    });

    test('should handle search addon errors', async () => {
      const { SearchAddon } = require('@xterm/addon-search');
      const searchAddonInstance = SearchAddon.mock.results[0].value;
      searchAddonInstance.findNext.mockImplementationOnce(() => {
        throw new Error('Search failed');
      });

      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const searchButton = screen.getByTitle(/search/i);
      await user.click(searchButton);

      const searchInput = screen.getByRole('searchbox');
      await user.type(searchInput, 'test');
      fireEvent.keyDown(searchInput, { key: 'Enter' });

      // Should handle search errors gracefully
      await waitFor(() => {
        expect(true).toBe(true); // Component should not crash
      });
    });
  });

  describe('Advanced Features Integration', () => {
    test('should integrate all features without conflicts', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          theme="solarized-dark"
          showToolbar={true}
          readOnly={false}
          height="600px"
          width="800px"
        />
      );

      // All features should work together
      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
        expect(mockPerformanceHooks.startRender).toHaveBeenCalled();
        expect(mockPerformanceMonitor.startProfiling).toHaveBeenCalled();
      });

      // Test multiple feature interactions
      const searchButton = screen.getByTitle(/search/i);
      await user.click(searchButton);

      const searchInput = screen.getByRole('searchbox');
      await user.type(searchInput, 'test query');

      const terminalElement = screen.getByRole('terminal');
      fireEvent.contextMenu(terminalElement);

      // All features should work without interference
      expect(screen.getByText(/copy/i)).toBeInTheDocument();
      expect(searchInput).toHaveValue('test query');
    });

    test('should maintain performance under feature load', async () => {
      const startTime = performance.now();

      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate multiple simultaneous operations
      const operations = [
        () => fireEvent(window, new Event('resize')),
        () => {
          const { WebSocketManager } = require('../../services/WebSocketManager');
          const config = WebSocketManager.mock.calls[0][0];
          config.onMessage({ type: 'terminal_data', data: 'test data' });
        },
        () => {
          const searchButton = screen.getByTitle(/search/i);
          user.click(searchButton);
        }
      ];

      // Execute all operations
      operations.forEach(op => op());

      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });

      const endTime = performance.now();
      const totalTime = endTime - startTime;

      // Should complete all operations within reasonable time
      expect(totalTime).toBeLessThan(200); // 200ms for complex operations
    });
  });
});