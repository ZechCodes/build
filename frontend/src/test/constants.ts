/**
 * Test constants for consistent test data across all test files
 */

export const TEST_CONSTANTS = {
  MOCK_USER_ID: 'test-user-123',
  MOCK_VM_ID: 'test-vm-456',
  MOCK_SESSION_ID: 'test-session-789',
  MOCK_TOKEN: 'mock-jwt-token',
  WEBSOCKET_URL: 'ws://localhost:8000/ws/terminal',
  API_BASE_URL: 'http://localhost:8000/api/v1',
  DEFAULT_TERMINAL_SIZE: { cols: 80, rows: 24 },
  PERFORMANCE_THRESHOLDS: {
    RENDER_TIME_MS: 500,
    KEYSTROKE_LATENCY_MS: 16,
    MEMORY_USAGE_MB: 100,
  }
};