/**
 * Enhanced Test Utilities for 100% Success Rate
 * Comprehensive testing helpers and custom render functions
 */

import React, { ReactElement } from 'react';
import { render, RenderOptions, RenderResult } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import { vi } from 'vitest';
import { TEST_CONSTANTS } from '../constants';

// Mock contexts for testing
const MockAuthContext = React.createContext({
  user: {
    id: TEST_CONSTANTS.MOCK_USER_ID,
    username: 'testuser',
    email: 'test@example.com',
    permissions: ['terminal:access', 'vm:manage']
  },
  token: TEST_CONSTANTS.MOCK_TOKEN,
  login: vi.fn(),
  logout: vi.fn(),
  refreshToken: vi.fn(),
  isAuthenticated: true,
  isLoading: false
});

// Test providers wrapper
interface TestProvidersProps {
  children: React.ReactNode;
  initialEntries?: string[];
  queryClient?: QueryClient;
  authContextValue?: any;
}

export const TestProviders: React.FC<TestProvidersProps> = ({
  children,
  initialEntries = ['/'],
  queryClient,
  authContextValue
}) => {
  const defaultQueryClient = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        gcTime: 0,
      },
      mutations: {
        retry: false,
      }
    }
  });

  const client = queryClient || defaultQueryClient;

  const defaultAuthValue = {
    user: {
      id: TEST_CONSTANTS.MOCK_USER_ID,
      username: 'testuser',
      email: 'test@example.com',
      permissions: ['terminal:access', 'vm:manage']
    },
    token: TEST_CONSTANTS.MOCK_TOKEN,
    login: vi.fn(),
    logout: vi.fn(),
    refreshToken: vi.fn(),
    isAuthenticated: true,
    isLoading: false,
    ...authContextValue
  };

  return (
    <MemoryRouter initialEntries={initialEntries}>
      <QueryClientProvider client={client}>
        <MockAuthContext.Provider value={defaultAuthValue}>
          {children}
        </MockAuthContext.Provider>
      </QueryClientProvider>
    </MemoryRouter>
  );
};

// Custom render function with providers
interface CustomRenderOptions extends Omit<RenderOptions, 'wrapper'> {
  initialEntries?: string[];
  queryClient?: QueryClient;
  authContextValue?: any;
}

export const renderWithProviders = (
  ui: ReactElement,
  options: CustomRenderOptions = {}
): RenderResult => {
  const { initialEntries, queryClient, authContextValue, ...renderOptions } = options;

  const Wrapper: React.FC<{ children: React.ReactNode }> = ({ children }) => (
    <TestProviders
      initialEntries={initialEntries}
      queryClient={queryClient}
      authContextValue={authContextValue}
    >
      {children}
    </TestProviders>
  );

  return render(ui, { wrapper: Wrapper, ...renderOptions });
};

// Terminal-specific test utilities
export const terminalTestUtils = {
  // Create a mock terminal instance
  createMockTerminal: () => ({
    open: vi.fn(),
    write: vi.fn(),
    writeln: vi.fn(),
    clear: vi.fn(),
    dispose: vi.fn(),
    focus: vi.fn(),
    blur: vi.fn(),
    paste: vi.fn(),
    selectAll: vi.fn(),
    getSelection: vi.fn(() => 'mock selection'),
    hasSelection: vi.fn(() => true),
    onData: vi.fn(),
    onResize: vi.fn(),
    onTitleChange: vi.fn(),
    onSelectionChange: vi.fn(),
    onBell: vi.fn(),
    attachCustomKeyEventHandler: vi.fn(),
    loadAddon: vi.fn(),
    setOption: vi.fn(),
    cols: 80,
    rows: 24,
    element: document.createElement('div'),
    textarea: document.createElement('textarea'),
    unicode: { activeVersion: '11' }
  }),

  // Simulate terminal input
  simulateTerminalInput: (terminal: any, input: string) => {
    const dataHandler = terminal.onData.mock.calls[0]?.[0];
    if (dataHandler) {
      dataHandler(input);
    }
  },

  // Simulate terminal resize
  simulateTerminalResize: (terminal: any, cols: number, rows: number) => {
    const resizeHandler = terminal.onResize.mock.calls[0]?.[0];
    if (resizeHandler) {
      resizeHandler({ cols, rows });
    }
  },

  // Simulate terminal bell
  simulateTerminalBell: (terminal: any) => {
    const bellHandler = terminal.onBell.mock.calls[0]?.[0];
    if (bellHandler) {
      bellHandler();
    }
  },

  // Simulate keyboard shortcut
  simulateKeyboardShortcut: (element: HTMLElement, key: string, modifiers: any = {}) => {
    const keyEvent = new KeyboardEvent('keydown', {
      key,
      ctrlKey: modifiers.ctrlKey || false,
      shiftKey: modifiers.shiftKey || false,
      altKey: modifiers.altKey || false,
      metaKey: modifiers.metaKey || false,
      bubbles: true
    });
    
    element.dispatchEvent(keyEvent);
  }
};

// WebSocket test utilities
export const websocketTestUtils = {
  // Create mock WebSocket
  createMockWebSocket: () => {
    const mockWS = {
      send: vi.fn(),
      close: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      readyState: WebSocket.OPEN,
      url: TEST_CONSTANTS.WEBSOCKET_URL,
      onopen: null,
      onclose: null,
      onmessage: null,
      onerror: null,
      CONNECTING: 0,
      OPEN: 1,
      CLOSING: 2,
      CLOSED: 3
    };

    return mockWS;
  },

  // Simulate WebSocket message
  simulateWSMessage: (ws: any, message: any) => {
    const messageEvent = {
      data: JSON.stringify(message),
      origin: TEST_CONSTANTS.WEBSOCKET_URL,
      source: ws
    };

    // Call onmessage handler if set
    if (ws.onmessage) {
      ws.onmessage(messageEvent);
    }

    // Call event listeners
    const messageListeners = ws.addEventListener.mock.calls
      .filter(call => call[0] === 'message')
      .map(call => call[1]);
    
    messageListeners.forEach(listener => listener(messageEvent));
  },

  // Simulate WebSocket connection
  simulateWSConnection: (ws: any) => {
    ws.readyState = WebSocket.OPEN;
    
    const openEvent = { type: 'open' };
    
    if (ws.onopen) {
      ws.onopen(openEvent);
    }

    const openListeners = ws.addEventListener.mock.calls
      .filter(call => call[0] === 'open')
      .map(call => call[1]);
    
    openListeners.forEach(listener => listener(openEvent));
  },

  // Simulate WebSocket disconnection
  simulateWSDisconnection: (ws: any, code: number = 1000, reason: string = 'Normal closure') => {
    ws.readyState = WebSocket.CLOSED;
    
    const closeEvent = { type: 'close', code, reason };
    
    if (ws.onclose) {
      ws.onclose(closeEvent);
    }

    const closeListeners = ws.addEventListener.mock.calls
      .filter(call => call[0] === 'close')
      .map(call => call[1]);
    
    closeListeners.forEach(listener => listener(closeEvent));
  },

  // Simulate WebSocket error
  simulateWSError: (ws: any, error: Error) => {
    const errorEvent = { type: 'error', error };
    
    if (ws.onerror) {
      ws.onerror(errorEvent);
    }

    const errorListeners = ws.addEventListener.mock.calls
      .filter(call => call[0] === 'error')
      .map(call => call[1]);
    
    errorListeners.forEach(listener => listener(errorEvent));
  }
};

// Performance testing utilities
export const performanceTestUtils = {
  // Measure component render time
  measureRenderTime: async (renderFn: () => any) => {
    const start = performance.now();
    const result = renderFn();
    
    // Wait for any async operations
    await new Promise(resolve => setTimeout(resolve, 0));
    
    const end = performance.now();
    return {
      renderTime: end - start,
      result
    };
  },

  // Measure memory usage during test
  measureMemoryUsage: () => {
    // Mock implementation - in real browser this would use performance.memory
    return {
      usedJSHeapSize: Math.random() * 50 * 1024 * 1024, // Random up to 50MB
      totalJSHeapSize: 100 * 1024 * 1024, // 100MB
      jsHeapSizeLimit: 200 * 1024 * 1024 // 200MB
    };
  },

  // Assert performance thresholds
  assertPerformanceThreshold: (actualTime: number, thresholdMs: number, operation: string) => {
    if (actualTime > thresholdMs) {
      throw new Error(
        `Performance threshold exceeded for ${operation}: ${actualTime}ms > ${thresholdMs}ms`
      );
    }
  }
};

// Accessibility testing utilities
export const accessibilityTestUtils = {
  // Check for ARIA labels
  checkAriaLabels: (container: HTMLElement) => {
    const interactiveElements = container.querySelectorAll(
      'button, input, select, textarea, [role="button"], [role="textbox"], [role="terminal"]'
    );
    
    interactiveElements.forEach(element => {
      const hasLabel = 
        element.hasAttribute('aria-label') ||
        element.hasAttribute('aria-labelledby') ||
        element.hasAttribute('title') ||
        (element as HTMLElement).textContent?.trim();
      
      if (!hasLabel) {
        throw new Error(`Interactive element missing accessible name: ${element.tagName}`);
      }
    });
  },

  // Check color contrast (simplified)
  checkColorContrast: (element: HTMLElement) => {
    const styles = window.getComputedStyle(element);
    const color = styles.color;
    const backgroundColor = styles.backgroundColor;
    
    // Simple check - in reality would calculate actual contrast ratio
    if (color === backgroundColor) {
      throw new Error('Insufficient color contrast detected');
    }
  },

  // Check keyboard navigation
  checkKeyboardNavigation: (container: HTMLElement) => {
    const focusableElements = container.querySelectorAll(
      'button, input, select, textarea, a[href], [tabindex]:not([tabindex="-1"])'
    );
    
    if (focusableElements.length === 0) {
      throw new Error('No focusable elements found for keyboard navigation');
    }
    
    // Check tab order
    const tabIndexes = Array.from(focusableElements).map(el => 
      parseInt((el as HTMLElement).getAttribute('tabindex') || '0')
    );
    
    // Ensure logical tab order
    const sortedIndexes = [...tabIndexes].sort((a, b) => a - b);
    if (JSON.stringify(tabIndexes) !== JSON.stringify(sortedIndexes)) {
      console.warn('Tab order may not be logical');
    }
  }
};

// Security testing utilities
export const securityTestUtils = {
  // Check for XSS vulnerabilities
  checkXSSPrevention: (container: HTMLElement, maliciousInput: string) => {
    // Set input and check if script was executed
    const originalAlert = window.alert;
    let alertCalled = false;
    
    window.alert = () => {
      alertCalled = true;
    };
    
    // Simulate setting dangerous content
    const inputs = container.querySelectorAll('input, textarea');
    inputs.forEach(input => {
      (input as HTMLInputElement).value = maliciousInput;
      input.dispatchEvent(new Event('input', { bubbles: true }));
    });
    
    // Check if any scripts were executed
    if (alertCalled) {
      throw new Error('XSS vulnerability detected: malicious script executed');
    }
    
    window.alert = originalAlert;
  },

  // Check for unsafe innerHTML usage
  checkUnsafeInnerHTML: (container: HTMLElement) => {
    const allElements = container.querySelectorAll('*');
    
    allElements.forEach(element => {
      const innerHTML = element.innerHTML;
      if (innerHTML.includes('<script>') || innerHTML.includes('javascript:')) {
        throw new Error('Unsafe innerHTML content detected');
      }
    });
  },

  // Verify Content Security Policy compliance
  checkCSPCompliance: () => {
    // Check if inline scripts are present
    const inlineScripts = document.querySelectorAll('script:not([src])');
    if (inlineScripts.length > 0) {
      console.warn('Inline scripts detected - may violate CSP');
    }
    
    // Check for inline event handlers
    const elementsWithInlineEvents = document.querySelectorAll('*[onclick], *[onload], *[onerror]');
    if (elementsWithInlineEvents.length > 0) {
      console.warn('Inline event handlers detected - may violate CSP');
    }
  }
};

// Export render with providers as default render for testing
export { renderWithProviders as render };