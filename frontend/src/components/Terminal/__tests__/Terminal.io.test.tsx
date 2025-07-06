/**
 * Terminal I/O Tests - TDD Cycle 3
 * Focus on input/output handling, keyboard shortcuts, clipboard operations
 */

import React from 'react';
import { describe, test, expect, vi, beforeEach, afterEach } from 'vitest';
import { screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { renderWithProviders, terminalTestUtils, TEST_CONSTANTS } from '../../../test/utils/test-utils';
import { Terminal } from '../Terminal';

// Mock clipboard API
const mockClipboard = {
  writeText: vi.fn().mockResolvedValue(undefined),
  readText: vi.fn().mockResolvedValue('test clipboard content')
};

Object.defineProperty(navigator, 'clipboard', {
  value: mockClipboard,
  writable: true,
});

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

// Mock terminal hooks
vi.mock('../../hooks/useTerminalSession', () => ({
  useTerminalSession: vi.fn(() => ({
    createSession: vi.fn(),
    restoreSession: vi.fn(),
    session: null,
    loading: false,
    error: null
  }))
}));

describe('Terminal Component - TDD Cycle 3: I/O Handling', () => {
  let mockTerminal: any;
  let user: any;

  beforeEach(() => {
    vi.clearAllMocks();
    cleanup();
    
    // Create fresh mock terminal for each test
    mockTerminal = terminalTestUtils.createMockTerminal();
    
    // Mock XTerm constructor
    const { Terminal: XTerm } = require('@xterm/xterm');
    XTerm.mockImplementation(() => mockTerminal);
    
    user = userEvent.setup();
  });

  afterEach(() => {
    cleanup();
  });

  describe('Keyboard Input Handling', () => {
    test('should handle basic character input', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      // Simulate typing characters
      const characters = ['h', 'e', 'l', 'l', 'o'];
      const dataHandler = mockTerminal.onData.mock.calls[0][0];
      
      characters.forEach(char => {
        dataHandler(char);
      });

      // Each character should be sent via WebSocket
      expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledTimes(5);
      characters.forEach(char => {
        expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledWith(char);
      });
    });

    test('should handle special keys (Enter, Tab, Arrow keys)', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      const dataHandler = mockTerminal.onData.mock.calls[0][0];
      
      // Test special keys
      const specialKeys = [
        { key: 'Enter', code: '\r' },
        { key: 'Tab', code: '\t' },
        { key: 'ArrowUp', code: '\x1b[A' },
        { key: 'ArrowDown', code: '\x1b[B' },
        { key: 'ArrowLeft', code: '\x1b[D' },
        { key: 'ArrowRight', code: '\x1b[C' },
        { key: 'Backspace', code: '\x7f' },
        { key: 'Delete', code: '\x1b[3~' }
      ];

      specialKeys.forEach(({ key, code }) => {
        dataHandler(code);
      });

      // All special keys should be sent
      expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledTimes(specialKeys.length);
    });

    test('should handle Ctrl key combinations', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      const dataHandler = mockTerminal.onData.mock.calls[0][0];
      
      // Test Ctrl combinations (common ones)
      const ctrlCombinations = [
        { combination: 'Ctrl+C', code: '\x03' },
        { combination: 'Ctrl+D', code: '\x04' },
        { combination: 'Ctrl+Z', code: '\x1a' },
        { combination: 'Ctrl+L', code: '\x0c' },
        { combination: 'Ctrl+A', code: '\x01' },
        { combination: 'Ctrl+E', code: '\x05' }
      ];

      ctrlCombinations.forEach(({ combination, code }) => {
        dataHandler(code);
      });

      expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledTimes(ctrlCombinations.length);
    });

    test('should handle function keys', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      const dataHandler = mockTerminal.onData.mock.calls[0][0];
      
      // Test function keys (F1-F12)
      const functionKeys = [
        { key: 'F1', code: '\x1bOP' },
        { key: 'F2', code: '\x1bOQ' },
        { key: 'F3', code: '\x1bOR' },
        { key: 'F4', code: '\x1bOS' },
        { key: 'F5', code: '\x1b[15~' }
      ];

      functionKeys.forEach(({ key, code }) => {
        dataHandler(code);
      });

      expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledTimes(functionKeys.length);
    });

    test('should not send input when terminal is readonly', async () => {
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          readOnly={true}
        />
      );

      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      const dataHandler = mockTerminal.onData.mock.calls[0][0];
      
      // Try to send input in readonly mode
      dataHandler('hello world');

      // Should not send any data
      expect(mockWebSocketManager.sendTerminalData).not.toHaveBeenCalled();
    });

    test('should handle rapid input without loss', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      const dataHandler = mockTerminal.onData.mock.calls[0][0];
      
      // Send rapid input
      const rapidInput = Array.from({ length: 100 }, (_, i) => String(i % 10));
      
      rapidInput.forEach(char => {
        dataHandler(char);
      });

      // All inputs should be sent
      expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledTimes(100);
    });
  });

  describe('Clipboard Operations', () => {
    test('should copy selected text to clipboard', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Mock terminal has selection
      mockTerminal.hasSelection.mockReturnValue(true);
      mockTerminal.getSelection.mockReturnValue('selected text');

      // Find and click copy button
      const copyButton = screen.getByTitle(/copy/i);
      await user.click(copyButton);

      expect(mockClipboard.writeText).toHaveBeenCalledWith('selected text');
    });

    test('should not copy when no text is selected', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Mock terminal has no selection
      mockTerminal.hasSelection.mockReturnValue(false);

      // Find and click copy button
      const copyButton = screen.getByTitle(/copy/i);
      await user.click(copyButton);

      expect(mockClipboard.writeText).not.toHaveBeenCalled();
    });

    test('should paste clipboard content', async () => {
      mockClipboard.readText.mockResolvedValue('pasted content');
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Find and click paste button
      const pasteButton = screen.getByTitle(/paste/i);
      await user.click(pasteButton);

      await waitFor(() => {
        expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledWith('pasted content');
      });
    });

    test('should not paste in readonly mode', async () => {
      mockClipboard.readText.mockResolvedValue('should not paste');
      
      renderWithProviders(
        <Terminal 
          vmId={TEST_CONSTANTS.MOCK_VM_ID}
          readOnly={true}
        />
      );

      // Find and click paste button
      const pasteButton = screen.getByTitle(/paste/i);
      await user.click(pasteButton);

      await waitFor(() => {
        expect(mockWebSocketManager.sendTerminalData).not.toHaveBeenCalled();
      });
    });

    test('should sanitize pasted content', async () => {
      const maliciousContent = '<script>alert("xss")</script>\necho "safe content"';
      mockClipboard.readText.mockResolvedValue(maliciousContent);
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const pasteButton = screen.getByTitle(/paste/i);
      await user.click(pasteButton);

      await waitFor(() => {
        // Should send sanitized content
        expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledWith(
          expect.not.stringContaining('<script>')
        );
        expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledWith(
          expect.stringContaining('echo "safe content"')
        );
      });
    });

    test('should handle clipboard access errors gracefully', async () => {
      mockClipboard.readText.mockRejectedValue(new Error('Clipboard access denied'));
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const pasteButton = screen.getByTitle(/paste/i);
      await user.click(pasteButton);

      // Should not crash or send anything
      await waitFor(() => {
        expect(mockWebSocketManager.sendTerminalData).not.toHaveBeenCalled();
      });
    });
  });

  describe('Context Menu Operations', () => {
    test('should show context menu on right click', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const terminalElement = screen.getByRole('terminal');
      
      // Right click to show context menu
      fireEvent.contextMenu(terminalElement, {
        clientX: 100,
        clientY: 100
      });

      // Context menu should be visible
      await waitFor(() => {
        expect(screen.getByText(/copy/i)).toBeInTheDocument();
        expect(screen.getByText(/paste/i)).toBeInTheDocument();
        expect(screen.getByText(/select all/i)).toBeInTheDocument();
      });
    });

    test('should hide context menu on click outside', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const terminalElement = screen.getByRole('terminal');
      
      // Show context menu
      fireEvent.contextMenu(terminalElement, {
        clientX: 100,
        clientY: 100
      });

      await waitFor(() => {
        expect(screen.getByText(/copy/i)).toBeInTheDocument();
      });

      // Click outside
      fireEvent.click(document.body);

      await waitFor(() => {
        expect(screen.queryByText(/copy/i)).not.toBeInTheDocument();
      });
    });

    test('should perform copy from context menu', async () => {
      mockTerminal.hasSelection.mockReturnValue(true);
      mockTerminal.getSelection.mockReturnValue('context menu copy');
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const terminalElement = screen.getByRole('terminal');
      fireEvent.contextMenu(terminalElement);

      await waitFor(() => {
        expect(screen.getByText(/copy/i)).toBeInTheDocument();
      });

      const copyOption = screen.getByText(/copy/i);
      await user.click(copyOption);

      expect(mockClipboard.writeText).toHaveBeenCalledWith('context menu copy');
    });

    test('should perform paste from context menu', async () => {
      mockClipboard.readText.mockResolvedValue('context menu paste');
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const terminalElement = screen.getByRole('terminal');
      fireEvent.contextMenu(terminalElement);

      await waitFor(() => {
        expect(screen.getByText(/paste/i)).toBeInTheDocument();
      });

      const pasteOption = screen.getByText(/paste/i);
      await user.click(pasteOption);

      await waitFor(() => {
        expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledWith('context menu paste');
      });
    });

    test('should select all text from context menu', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const terminalElement = screen.getByRole('terminal');
      fireEvent.contextMenu(terminalElement);

      await waitFor(() => {
        expect(screen.getByText(/select all/i)).toBeInTheDocument();
      });

      const selectAllOption = screen.getByText(/select all/i);
      await user.click(selectAllOption);

      expect(mockTerminal.selectAll).toHaveBeenCalled();
    });
  });

  describe('Terminal Actions and Controls', () => {
    test('should clear terminal content', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const clearButton = screen.getByTitle(/clear/i);
      await user.click(clearButton);

      expect(mockTerminal.clear).toHaveBeenCalled();
    });

    test('should fit terminal to container', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Mock fit addon
      const { FitAddon } = require('@xterm/addon-fit');
      const fitAddonInstance = FitAddon.mock.results[0].value;

      const fitButton = screen.getByTitle(/fit/i);
      await user.click(fitButton);

      expect(fitAddonInstance.fit).toHaveBeenCalled();
    });

    test('should open search interface', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const searchButton = screen.getByTitle(/search/i);
      await user.click(searchButton);

      // Search interface should be visible
      await waitFor(() => {
        expect(screen.getByRole('searchbox')).toBeInTheDocument();
      });
    });

    test('should handle search functionality', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Open search
      const searchButton = screen.getByTitle(/search/i);
      await user.click(searchButton);

      await waitFor(() => {
        expect(screen.getByRole('searchbox')).toBeInTheDocument();
      });

      // Mock search addon
      const { SearchAddon } = require('@xterm/addon-search');
      const searchAddonInstance = SearchAddon.mock.results[0].value;

      // Type search query
      const searchInput = screen.getByRole('searchbox');
      await user.type(searchInput, 'test query');

      // Trigger search
      fireEvent.keyDown(searchInput, { key: 'Enter' });

      expect(searchAddonInstance.findNext).toHaveBeenCalledWith(
        'test query',
        expect.any(Object)
      );
    });

    test('should close search interface', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Open search
      const searchButton = screen.getByTitle(/search/i);
      await user.click(searchButton);

      await waitFor(() => {
        expect(screen.getByRole('searchbox')).toBeInTheDocument();
      });

      // Close search
      const closeButton = screen.getByRole('button', { name: /close/i });
      await user.click(closeButton);

      await waitFor(() => {
        expect(screen.queryByRole('searchbox')).not.toBeInTheDocument();
      });
    });
  });

  describe('Terminal Resize Handling', () => {
    test('should handle manual terminal resize', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(mockTerminal.onResize).toHaveBeenCalled();
      });

      // Simulate resize event
      const resizeHandler = mockTerminal.onResize.mock.calls[0][0];
      resizeHandler({ cols: 120, rows: 40 });

      expect(mockWebSocketManager.sendResize).toHaveBeenCalledWith(120, 40);
    });

    test('should handle window resize events', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Mock fit addon
      const { FitAddon } = require('@xterm/addon-fit');
      const fitAddonInstance = FitAddon.mock.results[0].value;

      // Trigger window resize
      fireEvent(window, new Event('resize'));

      // Should trigger fit
      await waitFor(() => {
        expect(fitAddonInstance.fit).toHaveBeenCalled();
      });
    });

    test('should update terminal size display', async () => {
      mockTerminal.cols = 100;
      mockTerminal.rows = 30;
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      // Should display current terminal size
      expect(screen.getByText('100×30')).toBeInTheDocument();
    });

    test('should handle resize throttling', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const { FitAddon } = require('@xterm/addon-fit');
      const fitAddonInstance = FitAddon.mock.results[0].value;

      // Trigger multiple rapid resize events
      for (let i = 0; i < 10; i++) {
        fireEvent(window, new Event('resize'));
      }

      // Should throttle calls (not call fit 10 times immediately)
      await waitFor(() => {
        expect(fitAddonInstance.fit).toHaveBeenCalled();
      });

      // Exact call count depends on throttling implementation
      expect(fitAddonInstance.fit.mock.calls.length).toBeLessThan(10);
    });
  });

  describe('Focus and Accessibility', () => {
    test('should focus terminal on container focus', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const terminalContainer = screen.getByRole('terminal');
      
      fireEvent.focus(terminalContainer);

      expect(mockTerminal.focus).toHaveBeenCalled();
    });

    test('should blur terminal on container blur', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const terminalContainer = screen.getByRole('terminal');
      
      fireEvent.blur(terminalContainer);

      expect(mockTerminal.blur).toHaveBeenCalled();
    });

    test('should be keyboard navigable', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const terminalContainer = screen.getByRole('terminal');
      
      // Should be focusable
      expect(terminalContainer).toHaveAttribute('tabIndex', '0');
      
      // Should have proper ARIA label
      expect(terminalContainer).toHaveAttribute('aria-label');
    });
  });

  describe('Bell and Visual Notifications', () => {
    test('should handle terminal bell events', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(mockTerminal.onBell).toHaveBeenCalled();
      });

      // Mock document title
      const originalTitle = document.title;
      document.title = 'Terminal Test';

      // Trigger bell
      const bellHandler = mockTerminal.onBell.mock.calls[0][0];
      bellHandler();

      // Should update document title
      expect(document.title).toContain('🔔');

      // Cleanup
      document.title = originalTitle;
    });

    test('should reset bell notification after timeout', async () => {
      vi.useFakeTimers();
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(mockTerminal.onBell).toHaveBeenCalled();
      });

      const originalTitle = document.title;
      document.title = 'Terminal Test';

      // Trigger bell
      const bellHandler = mockTerminal.onBell.mock.calls[0][0];
      bellHandler();

      expect(document.title).toContain('🔔');

      // Fast forward timer
      vi.advanceTimersByTime(1000);

      // Bell notification should be cleared
      expect(document.title).not.toContain('🔔');

      document.title = originalTitle;
      vi.useRealTimers();
    });
  });

  describe('Performance and Error Handling', () => {
    test('should handle clipboard API errors gracefully', async () => {
      mockClipboard.writeText.mockRejectedValue(new Error('Clipboard write failed'));
      mockTerminal.hasSelection.mockReturnValue(true);
      mockTerminal.getSelection.mockReturnValue('test selection');
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      const copyButton = screen.getByTitle(/copy/i);
      
      // Should not throw even if clipboard fails
      expect(async () => {
        await user.click(copyButton);
      }).not.toThrow();
    });

    test('should handle WebSocket disconnection during input', async () => {
      mockWebSocketManager.getConnectionState.mockReturnValue(false);
      
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      const dataHandler = mockTerminal.onData.mock.calls[0][0];
      dataHandler('test input while disconnected');

      // Should handle gracefully, not send data
      expect(mockWebSocketManager.sendTerminalData).not.toHaveBeenCalled();
    });

    test('should maintain input responsiveness under load', async () => {
      renderWithProviders(<Terminal vmId={TEST_CONSTANTS.MOCK_VM_ID} />);

      await waitFor(() => {
        expect(mockTerminal.onData).toHaveBeenCalled();
      });

      const dataHandler = mockTerminal.onData.mock.calls[0][0];
      
      // Simulate high-frequency input
      const startTime = performance.now();
      
      for (let i = 0; i < 1000; i++) {
        dataHandler(String(i % 10));
      }
      
      const endTime = performance.now();
      const processingTime = endTime - startTime;
      
      // Should process input quickly (less than 100ms for 1000 inputs)
      expect(processingTime).toBeLessThan(100);
      expect(mockWebSocketManager.sendTerminalData).toHaveBeenCalledTimes(1000);
    });
  });
});