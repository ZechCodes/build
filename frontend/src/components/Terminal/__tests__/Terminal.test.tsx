/**
 * Terminal Component Tests - TDD Approach
 * Ensuring 100% test success rate with comprehensive coverage
 */

import React from 'react';
import { describe, test, expect, vi, beforeEach, afterEach } from 'vitest';
import { screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { renderWithProviders, terminalTestUtils, TEST_CONSTANTS } from '../../../test/utils/test-utils';
import { Terminal } from '../Terminal';
import type { Terminal as XTerm } from '@xterm/xterm';

// Mock xterm.js completely for reliable testing
const mockTerminal = terminalTestUtils.createMockTerminal();

vi.mock('@xterm/xterm', () => ({
  Terminal: vi.fn().mockImplementation(() => mockTerminal)
}));

vi.mock('@xterm/addon-fit', () => ({
  FitAddon: vi.fn().mockImplementation(() => ({
    fit: vi.fn(),
    dispose: vi.fn()
  }))
}));

vi.mock('@xterm/addon-search', () => ({
  SearchAddon: vi.fn().mockImplementation(() => ({
    findNext: vi.fn(),
    findPrevious: vi.fn(),
    dispose: vi.fn()
  }))
}));

vi.mock('@xterm/addon-web-links', () => ({
  WebLinksAddon: vi.fn().mockImplementation(() => ({
    dispose: vi.fn()
  }))
}));

vi.mock('@xterm/addon-canvas', () => ({
  CanvasAddon: vi.fn().mockImplementation(() => ({
    dispose: vi.fn()
  }))
}));

vi.mock('@xterm/addon-webgl', () => ({
  WebglAddon: vi.fn().mockImplementation(() => ({
    dispose: vi.fn()
  }))
}));

// Mock WebSocket Manager
const mockWebSocketManager = {
  connect: vi.fn(),
  disconnect: vi.fn(),
  sendTerminalData: vi.fn(),
  sendResize: vi.fn(),
  createSession: vi.fn(),
  joinSession: vi.fn(),
  getConnectionState: vi.fn(() => true),
  isConnected: vi.fn(() => true)
};

vi.mock('../../services/WebSocketManager', () => ({
  WebSocketManager: vi.fn().mockImplementation(() => mockWebSocketManager)
}));

// Mock hooks
vi.mock('../../hooks/useTerminalSession', () => ({
  useTerminalSession: vi.fn(() => ({
    createSession: vi.fn(),
    restoreSession: vi.fn(),
    session: null,
    loading: false,
    error: null
  }))
}));

describe('Terminal Component - TDD Cycle 1: Basic Initialization', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    cleanup();
    
    // Reset mock terminal state
    Object.assign(mockTerminal, terminalTestUtils.createMockTerminal());
  });

  afterEach(() => {
    cleanup();
  });

  describe('RED Phase - Failing Tests', () => {
    test('should render terminal container with proper accessibility attributes', () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);
      
      // This test will fail initially because the component needs to be enhanced
      const terminalContainer = screen.getByRole('terminal');
      expect(terminalContainer).toBeInTheDocument();
      expect(terminalContainer).toHaveAttribute('aria-label', `Terminal for VM ${TEST_CONSTANTS.MOCK_VM_ID}`);
      expect(terminalContainer).toHaveAttribute('tabIndex', '0');
    });

    test('should initialize xterm.js with correct configuration', () => {
      const { Terminal: XTermConstructor } = require('@xterm/xterm');
      
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          theme="dark"
          readOnly={false}
        />
      );

      // Verify XTerm constructor was called with proper config
      expect(XTermConstructor).toHaveBeenCalledWith(
        expect.objectContaining({
          cursorBlink: true,
          cursorStyle: 'block',
          scrollback: 10000,
          allowTransparency: true,
          convertEol: true,
          allowProposedApi: true
        })
      );
    });

    test('should load required xterm addons', () => {
      const { FitAddon } = require('@xterm/addon-fit');
      const { SearchAddon } = require('@xterm/addon-search');
      const { WebLinksAddon } = require('@xterm/addon-web-links');

      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Verify addons were created
      expect(FitAddon).toHaveBeenCalled();
      expect(SearchAddon).toHaveBeenCalled();
      expect(WebLinksAddon).toHaveBeenCalled();

      // Verify addons were loaded
      expect(mockTerminal.loadAddon).toHaveBeenCalledTimes(3);
    });

    test('should open terminal in DOM element', () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Verify terminal.open was called
      expect(mockTerminal.open).toHaveBeenCalledTimes(1);
      expect(mockTerminal.open).toHaveBeenCalledWith(expect.any(HTMLElement));
    });

    test('should set up terminal event handlers', () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Verify essential event handlers are set up
      expect(mockTerminal.onData).toHaveBeenCalled();
      expect(mockTerminal.onResize).toHaveBeenCalled();
      expect(mockTerminal.onSelectionChange).toHaveBeenCalled();
      expect(mockTerminal.onBell).toHaveBeenCalled();
    });

    test('should display connection status indicator', () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Look for connection status elements
      const statusElements = screen.getAllByText(/connected|disconnected/i);
      expect(statusElements.length).toBeGreaterThan(0);
    });

    test('should apply theme configuration', () => {
      const customTheme = 'light';
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          theme={customTheme}
        />
      );

      // Verify theme was applied through xterm configuration
      const { Terminal: XTermConstructor } = require('@xterm/xterm');
      const lastCall = XTermConstructor.mock.calls[XTermConstructor.mock.calls.length - 1];
      
      expect(lastCall[0]).toHaveProperty('background');
      expect(lastCall[0]).toHaveProperty('foreground');
    });

    test('should handle readOnly mode', () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          readOnly={true}
        />
      );

      // Simulate user input
      terminalTestUtils.simulateTerminalInput(mockTerminal, 'test input');

      // In readOnly mode, input should not be sent to WebSocket
      expect(mockWebSocketManager.sendTerminalData).not.toHaveBeenCalled();
    });

    test('should cleanup on unmount', () => {
      const { unmount } = renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Unmount component
      unmount();

      // Verify cleanup was performed
      expect(mockTerminal.dispose).toHaveBeenCalled();
    });

    test('should handle terminal resize events', () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate terminal resize
      const newSize = { cols: 100, rows: 30 };
      terminalTestUtils.simulateTerminalResize(mockTerminal, newSize.cols, newSize.rows);

      // Verify resize was handled (will fail until implemented)
      expect(mockWebSocketManager.sendResize).toHaveBeenCalledWith(newSize.cols, newSize.rows);
    });

    test('should handle terminal input data', () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate user typing
      const inputData = 'ls -la\r';
      terminalTestUtils.simulateTerminalInput(mockTerminal, inputData);

      // Verify input was sent to WebSocket (will fail until implemented)
      expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledWith(inputData);
    });

    test('should handle terminal bell events', () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Mock document.title
      const originalTitle = document.title;
      document.title = 'Test Terminal';

      // Simulate bell
      terminalTestUtils.simulateTerminalBell(mockTerminal);

      // Verify bell was handled (visual indicator or title change)
      expect(document.title).toContain('🔔');

      // Cleanup
      document.title = originalTitle;
    });

    test('should initialize WebSocket connection', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Wait for component to initialize
      await waitFor(() => {
        expect(mockWebSocketManager.createSession).toHaveBeenCalledWith(TEST_CONSTANTS.MOCK_VM_ID);
      });
    });

    test('should handle WebSocket connection state changes', async () => {
      // Start with disconnected state
      mockWebSocketManager.isConnected.mockReturnValue(false);
      
      const { rerender } = renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Should show disconnected state
      expect(screen.getByText(/disconnected/i)).toBeInTheDocument();

      // Change to connected state
      mockWebSocketManager.isConnected.mockReturnValue(true);
      rerender(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Should update to connected state
      await waitFor(() => {
        expect(screen.getByText(/connected/i)).toBeInTheDocument();
      });
    });

    test('should display terminal size information', () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Should display current terminal dimensions
      const sizeInfo = screen.getByText(`${mockTerminal.cols}×${mockTerminal.rows}`);
      expect(sizeInfo).toBeInTheDocument();
    });

    test('should handle window resize events', () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Get the fit addon mock
      const { FitAddon } = require('@xterm/addon-fit');
      const fitAddonInstance = FitAddon.mock.results[0].value;

      // Simulate window resize
      fireEvent(window, new Event('resize'));

      // Should trigger terminal fit
      expect(fitAddonInstance.fit).toHaveBeenCalled();
    });

    test('should support custom dimensions', () => {
      const customHeight = '400px';
      const customWidth = '600px';

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

    test('should handle theme switching', () => {
      const { rerender } = renderWithProviders(
        <Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} theme="dark" />
      );

      // Switch theme
      rerender(
        <Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} theme="light" />
      );

      // Should update terminal options (will need implementation)
      expect(mockTerminal.setOption).toHaveBeenCalled();
    });

    test('should handle terminal focus and blur', () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const terminalElement = screen.getByRole('terminal');
      
      // Simulate focus
      fireEvent.focus(terminalElement);
      expect(mockTerminal.focus).toHaveBeenCalled();

      // Simulate blur
      fireEvent.blur(terminalElement);
      expect(mockTerminal.blur).toHaveBeenCalled();
    });
  });

  describe('Performance Requirements', () => {
    test('should initialize within performance threshold', async () => {
      const startTime = performance.now();
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);
      
      await waitFor(() => {
        expect(mockTerminal.open).toHaveBeenCalled();
      });
      
      const endTime = performance.now();
      const initTime = endTime - startTime;
      
      // Should initialize within 500ms threshold
      expect(initTime).toBeLessThan(TEST_CONSTANTS.PERFORMANCE_THRESHOLDS.RENDER_TIME_MS);
    });

    test('should handle rapid input without performance degradation', () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Simulate rapid typing
      const rapidInputs = ['a', 'b', 'c', 'd', 'e'];
      rapidInputs.forEach(input => {
        terminalTestUtils.simulateTerminalInput(mockTerminal, input);
      });

      // All inputs should be processed
      expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledTimes(rapidInputs.length);
    });
  });

  describe('Error Handling', () => {
    test('should handle xterm initialization failure gracefully', () => {
      // Mock xterm constructor to throw
      const { Terminal: XTermConstructor } = require('@xterm/xterm');
      XTermConstructor.mockImplementationOnce(() => {
        throw new Error('XTerm initialization failed');
      });

      // Should not crash the application
      expect(() => {
        renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);
      }).not.toThrow();
    });

    test('should handle WebSocket connection failure', () => {
      mockWebSocketManager.createSession.mockRejectedValue(new Error('Connection failed'));

      // Should render without crashing
      expect(() => {
        renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);
      }).not.toThrow();
    });

    test('should handle addon loading failure gracefully', () => {
      // Mock addon to throw during load
      mockTerminal.loadAddon.mockImplementationOnce(() => {
        throw new Error('Addon load failed');
      });

      // Should continue initialization
      expect(() => {
        renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);
      }).not.toThrow();
    });
  });
});