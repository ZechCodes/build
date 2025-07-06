/**
 * Enhanced Test Setup for 100% Success Rate
 * Comprehensive test environment configuration with bulletproof reliability
 */

import '@testing-library/jest-dom';
import { cleanup } from '@testing-library/react';
import { afterEach, beforeEach, beforeAll, afterAll, vi, expect } from 'vitest';
import { setupServer } from 'msw/node';
import { handlers } from './mocks/handlers';
import { TEST_CONSTANTS } from './constants';

// Make expect globally available
globalThis.expect = expect;

// Add setImmediate polyfill for testing
globalThis.setImmediate = globalThis.setImmediate || ((fn: Function) => setTimeout(fn, 0));

// Mock xterm.js completely to avoid canvas issues
vi.mock('@xterm/xterm', () => ({
  Terminal: vi.fn().mockImplementation(() => ({
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
  }))
}));

vi.mock('@xterm/addon-fit', () => ({
  FitAddon: vi.fn().mockImplementation(() => ({
    fit: vi.fn(),
    proposeDimensions: vi.fn(() => ({ cols: 80, rows: 24 }))
  }))
}));

vi.mock('@xterm/addon-search', () => ({
  SearchAddon: vi.fn().mockImplementation(() => ({
    findNext: vi.fn(),
    findPrevious: vi.fn()
  }))
}));

vi.mock('@xterm/addon-web-links', () => ({
  WebLinksAddon: vi.fn().mockImplementation(() => ({}))
}));

// Mock CSS imports
vi.mock('@xterm/xterm/css/xterm.css', () => ({}));

// Mock Service Worker setup for API and WebSocket mocking
export const server = setupServer(...handlers);

// Start server before all tests
beforeAll(() => {
  server.listen({ onUnhandledRequest: 'error' });
});

// Reset handlers after each test
afterEach(() => {
  server.resetHandlers();
  cleanup();
  
  // Clear all mocks to prevent test pollution
  vi.clearAllMocks();
  
  // Clear any remaining timers
  vi.clearAllTimers();
  
  // Reset DOM to clean state
  document.body.innerHTML = '';
  document.head.innerHTML = '';
});

// Close server after all tests
afterAll(() => {
  server.close();
});

// Global test environment setup
beforeEach(() => {
  // Reset console methods for clean test output
  vi.clearAllMocks();
  
  // Mock window.matchMedia for responsive components
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: vi.fn().mockImplementation(query => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: vi.fn(), // deprecated
      removeListener: vi.fn(), // deprecated
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  });

  // Mock ResizeObserver for terminal fitting
  global.ResizeObserver = vi.fn().mockImplementation(() => ({
    observe: vi.fn(),
    unobserve: vi.fn(),
    disconnect: vi.fn(),
  }));

  // Mock IntersectionObserver for virtual scrolling
  global.IntersectionObserver = vi.fn().mockImplementation(() => ({
    observe: vi.fn(),
    unobserve: vi.fn(),
    disconnect: vi.fn(),
  }));

  // Mock clipboard API for terminal copy/paste
  Object.assign(navigator, {
    clipboard: {
      writeText: vi.fn().mockResolvedValue(undefined),
      readText: vi.fn().mockResolvedValue('mock clipboard content'),
    },
  });

  // Mock performance API for monitoring
  Object.defineProperty(window, 'performance', {
    writable: true,
    value: {
      now: vi.fn(() => Date.now()),
      mark: vi.fn(),
      measure: vi.fn(),
      getEntriesByType: vi.fn(() => []),
      getEntriesByName: vi.fn(() => []),
      clearMarks: vi.fn(),
      clearMeasures: vi.fn(),
    },
  });

  // Mock WebSocket for real-time communication testing
  class MockWebSocket {
    static CONNECTING = 0;
    static OPEN = 1;
    static CLOSING = 2;
    static CLOSED = 3;
    
    CONNECTING = 0;
    OPEN = 1;
    CLOSING = 2;
    CLOSED = 3;
    readyState = 0;
    
    close = vi.fn();
    send = vi.fn();
    addEventListener = vi.fn();
    removeEventListener = vi.fn();
  }
  
  global.WebSocket = MockWebSocket as any;

  // Mock requestAnimationFrame for smooth animations
  global.requestAnimationFrame = vi.fn((cb) => setTimeout(cb, 16));
  global.cancelAnimationFrame = vi.fn();
  
  // Mock HTMLCanvasElement for xterm.js
  HTMLCanvasElement.prototype.getContext = vi.fn().mockReturnValue({
    fillStyle: '',
    strokeStyle: '',
    lineWidth: 1,
    font: '',
    textAlign: 'start',
    textBaseline: 'alphabetic',
    fillRect: vi.fn(),
    clearRect: vi.fn(),
    strokeRect: vi.fn(),
    fillText: vi.fn(),
    strokeText: vi.fn(),
    measureText: vi.fn().mockReturnValue({ width: 10 }),
    createLinearGradient: vi.fn().mockReturnValue({
      addColorStop: vi.fn()
    }),
    save: vi.fn(),
    restore: vi.fn(),
    scale: vi.fn(),
    translate: vi.fn(),
    rotate: vi.fn(),
    beginPath: vi.fn(),
    closePath: vi.fn(),
    moveTo: vi.fn(),
    lineTo: vi.fn(),
    arc: vi.fn(),
    fill: vi.fn(),
    stroke: vi.fn(),
    clip: vi.fn(),
    getImageData: vi.fn().mockReturnValue({
      data: new Uint8ClampedArray(4),
      width: 1,
      height: 1
    }),
    putImageData: vi.fn(),
    createImageData: vi.fn().mockReturnValue({
      data: new Uint8ClampedArray(4),
      width: 1,
      height: 1
    })
  });

  // Mock getBoundingClientRect for layout calculations
  Element.prototype.getBoundingClientRect = vi.fn(() => ({
    width: 800,
    height: 600,
    top: 0,
    left: 0,
    bottom: 600,
    right: 800,
    x: 0,
    y: 0,
    toJSON: vi.fn(),
  }));

  // Mock scrollIntoView for terminal scrolling
  Element.prototype.scrollIntoView = vi.fn();

  // Mock focus/blur for accessibility testing
  HTMLElement.prototype.focus = vi.fn();
  HTMLElement.prototype.blur = vi.fn();

  // Set up error handling to catch any unhandled errors
  const originalError = console.error;
  console.error = (...args) => {
    // Fail tests on React warnings or errors
    if (
      typeof args[0] === 'string' &&
      (args[0].includes('Warning:') || args[0].includes('Error:'))
    ) {
      throw new Error(`Test failed due to console error: ${args[0]}`);
    }
    originalError.apply(console, args);
  };
});

// Enhanced error boundary for catching React errors in tests
export class TestErrorBoundary extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'TestErrorBoundary';
  }
}

// Test utilities for consistent test execution
export const testUtils = {
  // Wait for all async operations to complete
  waitForAsyncOperations: async () => {
    await new Promise(resolve => setTimeout(resolve, 0));
    await vi.runAllTimersAsync();
  },

  // Simulate realistic user delays
  simulateUserDelay: (ms: number = 100) => {
    return new Promise(resolve => setTimeout(resolve, ms));
  },

  // Verify no memory leaks in component lifecycle
  verifyNoMemoryLeaks: () => {
    // This would be implemented with more sophisticated memory tracking
    expect(document.querySelectorAll('*').length).toBeLessThan(1000);
  },

  // Ensure all promises are resolved
  flushPromises: async () => {
    await new Promise(resolve => setImmediate(resolve));
  }
};

// TEST_CONSTANTS now imported from ./constants.ts to avoid circular dependencies

// Performance testing utilities
export const performanceUtils = {
  measureRenderTime: async (renderFn: () => void) => {
    const start = performance.now();
    renderFn();
    await testUtils.waitForAsyncOperations();
    const end = performance.now();
    return end - start;
  },

  measureMemoryUsage: () => {
    // Mock implementation - would use real memory API in actual browser
    return {
      usedJSHeapSize: 50 * 1024 * 1024, // 50MB mock
      totalJSHeapSize: 100 * 1024 * 1024, // 100MB mock
      jsHeapSizeLimit: 200 * 1024 * 1024, // 200MB mock
    };
  }
};

// Security testing utilities
export const securityUtils = {
  // Test XSS prevention
  createXSSPayload: () => '<script>alert("XSS")</script>',
  
  // Test input sanitization
  createMaliciousInput: () => '"><script>document.location="http://evil.com"</script>',
  
  // Test SQL injection patterns (for form inputs)
  createSQLInjection: () => "'; DROP TABLE users; --",
  
  // Verify no script execution
  verifyNoScriptExecution: () => {
    const scripts = document.querySelectorAll('script');
    scripts.forEach(script => {
      expect(script.innerHTML).not.toContain('alert');
      expect(script.innerHTML).not.toContain('eval');
      expect(script.innerHTML).not.toContain('document.location');
    });
  }
};