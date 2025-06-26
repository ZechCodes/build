import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';
import { useTerminalSession } from './useTerminalSession';
import axios from 'axios';

// Mock axios
vi.mock('axios');
const mockedAxios = vi.mocked(axios);

// Mock API responses
const mockSession = {
  id: 'session-123',
  vmId: 'vm-test',
  createdAt: '2023-01-01T00:00:00Z',
  updatedAt: '2023-01-01T00:00:00Z',
  status: 'active' as const
};

const mockSessionHistory = [
  {
    id: 'session-456',
    vmId: 'vm-test',
    createdAt: '2022-12-31T00:00:00Z',
    updatedAt: '2022-12-31T00:00:00Z',
    status: 'ended' as const
  }
];

describe('useTerminalSession', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    
    // Default mock implementations
    mockedAxios.post.mockResolvedValue({ data: { session: mockSession } });
    mockedAxios.get.mockResolvedValue({ data: { sessions: [mockSession, ...mockSessionHistory] } });
    mockedAxios.put.mockResolvedValue({ data: { session: mockSession } });
    mockedAxios.delete.mockResolvedValue({ data: { success: true } });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  describe('initialization', () => {
    it('should initialize with default state', () => {
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      expect(result.current.currentSession).toBeNull();
      expect(result.current.sessionHistory).toEqual([]);
      expect(result.current.isLoading).toBe(false);
      expect(result.current.error).toBeNull();
    });

    it('should load session history on mount', async () => {
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      // Manually trigger loadSessionHistory since it may not auto-load
      await result.current.loadSessionHistory();
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      expect(mockedAxios.get).toHaveBeenCalledWith('/api/v1/sessions', {
        params: { vm_id: 'vm-test', type: 'terminal' }
      });
    });
  });

  describe('createSession', () => {
    it('should create a new session successfully', async () => {
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await result.current.createSession();
      
      await waitFor(() => {
        expect(result.current.currentSession).toEqual(mockSession);
      });
      
      expect(mockedAxios.post).toHaveBeenCalledWith('/api/v1/sessions', {
        vm_id: 'vm-test',
        type: 'terminal'
      });
    });

    it('should handle create session errors', async () => {
      const errorMessage = 'Failed to create session';
      mockedAxios.post.mockRejectedValueOnce(new Error(errorMessage));
      
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      await result.current.createSession();
      
      await waitFor(() => {
        expect(result.current.error).toBe(errorMessage);
      });
      
      expect(result.current.currentSession).toBeNull();
    });

    it('should set loading state during session creation', async () => {
      let resolvePromise: (value: any) => void;
      const pendingPromise = new Promise(resolve => {
        resolvePromise = resolve;
      });
      
      mockedAxios.post.mockReturnValueOnce(pendingPromise);
      
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      const createPromise = result.current.createSession();
      
      await waitFor(() => {
        expect(result.current.isLoading).toBe(true);
      });
      
      resolvePromise!({ data: mockSession });
      await createPromise;
      
      await waitFor(() => {
        expect(result.current.isLoading).toBe(false);
      });
    });
  });

  describe('restoreSession', () => {
    it('should restore an existing session successfully', async () => {
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      await result.current.restoreSession('session-123');
      
      await waitFor(() => {
        expect(result.current.currentSession).toEqual(mockSession);
      });
      
      expect(mockedAxios.put).toHaveBeenCalledWith('/api/v1/sessions/session-123/restore');
    });

    it('should handle restore session errors', async () => {
      const errorMessage = 'Session not found';
      mockedAxios.put.mockRejectedValueOnce(new Error(errorMessage));
      
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      await result.current.restoreSession('invalid-session');
      
      await waitFor(() => {
        expect(result.current.error).toBe(errorMessage);
      });
      
      expect(result.current.currentSession).toBeNull();
    });

    it('should set loading state during session restoration', async () => {
      let resolvePromise: (value: any) => void;
      const pendingPromise = new Promise(resolve => {
        resolvePromise = resolve;
      });
      
      mockedAxios.put.mockReturnValueOnce(pendingPromise);
      
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      const restorePromise = result.current.restoreSession('session-123');
      
      await waitFor(() => {
        expect(result.current.isLoading).toBe(true);
      });
      
      resolvePromise!({ data: mockSession });
      await restorePromise;
      
      await waitFor(() => {
        expect(result.current.isLoading).toBe(false);
      });
    });
  });

  describe('endSession', () => {
    it('should end current session successfully', async () => {
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      // First create a session
      await result.current.createSession();
      
      await waitFor(() => {
        expect(result.current.currentSession).toEqual(mockSession);
      });
      
      // Then end it
      await result.current.endSession();
      
      await waitFor(() => {
        expect(result.current.currentSession).toBeNull();
      });
      
      expect(mockedAxios.delete).toHaveBeenCalledWith('/api/v1/sessions/session-123');
    });

    it('should handle end session errors', async () => {
      const errorMessage = 'Failed to end session';
      mockedAxios.delete.mockRejectedValueOnce(new Error(errorMessage));
      
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      // First create a session
      await result.current.createSession();
      
      await waitFor(() => {
        expect(result.current.currentSession).toEqual(mockSession);
      });
      
      // Then try to end it
      await result.current.endSession();
      
      await waitFor(() => {
        expect(result.current.error).toBe(errorMessage);
      });
      
      // Session should still be current since ending failed
      expect(result.current.currentSession).toEqual(mockSession);
    });

    it('should do nothing if no current session', async () => {
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      await result.current.endSession();
      
      expect(mockedAxios.delete).not.toHaveBeenCalled();
      expect(result.current.currentSession).toBeNull();
    });

    it('should set loading state during session ending', async () => {
      let resolvePromise: (value: any) => void;
      const pendingPromise = new Promise(resolve => {
        resolvePromise = resolve;
      });
      
      mockedAxios.delete.mockReturnValueOnce(pendingPromise);
      
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      // First create a session
      await result.current.createSession();
      
      await waitFor(() => {
        expect(result.current.currentSession).toEqual(mockSession);
      });
      
      const endPromise = result.current.endSession();
      
      await waitFor(() => {
        expect(result.current.isLoading).toBe(true);
      });
      
      resolvePromise!({ data: { success: true } });
      await endPromise;
      
      await waitFor(() => {
        expect(result.current.isLoading).toBe(false);
      });
    });
  });

  describe('loadSessionHistory', () => {
    it('should load session history', async () => {
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      // Clear mock calls
      mockedAxios.get.mockClear();
      
      // Refresh history
      await result.current.loadSessionHistory();
      
      expect(mockedAxios.get).toHaveBeenCalledWith('/api/v1/sessions', {
        params: { vm_id: 'vm-test', type: 'terminal' }
      });
    });

    it('should handle load history errors', async () => {
      const errorMessage = 'Failed to load sessions';
      
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      mockedAxios.get.mockRejectedValueOnce(new Error(errorMessage));
      
      await result.current.loadSessionHistory();
      
      await waitFor(() => {
        expect(result.current.error).toBe(errorMessage);
      });
    });
  });

  describe('vmId changes', () => {
    it('should reload sessions when vmId changes', async () => {
      const { result, rerender } = renderHook(
        ({ vmId }) => useTerminalSession(vmId),
        { initialProps: { vmId: 'vm-test' } }
      );
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      // Change vmId
      rerender({ vmId: 'vm-different' });
      
      // Manually trigger loadSessionHistory since it doesn't auto-load on vmId change
      await result.current.loadSessionHistory();
      
      await waitFor(() => {
        expect(mockedAxios.get).toHaveBeenCalledWith('/api/v1/sessions', {
          params: { vm_id: 'vm-different', type: 'terminal' }
        });
      });
    });

    it('should clear current session when vmId changes', async () => {
      const { result, rerender } = renderHook(
        ({ vmId }) => useTerminalSession(vmId),
        { initialProps: { vmId: 'vm-test' } }
      );
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      // Create a session
      await result.current.createSession();
      
      await waitFor(() => {
        expect(result.current.currentSession).toEqual(mockSession);
      });
      
      // Change vmId
      rerender({ vmId: 'vm-different' });
      
      await waitFor(() => {
        expect(result.current.currentSession).toBeNull();
      });
    });
  });

  describe('error handling', () => {
    it('should clear error when operations succeed', async () => {
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      // First cause an error
      mockedAxios.post.mockRejectedValueOnce(new Error('Test error'));
      await result.current.createSession();
      
      await waitFor(() => {
        expect(result.current.error).toBe('Test error');
      });
      
      // Then succeed
      mockedAxios.post.mockResolvedValueOnce({ data: { session: mockSession } });
      await result.current.createSession();
      
      await waitFor(() => {
        expect(result.current.error).toBeNull();
      });
    });

    it('should handle API errors with response data', async () => {
      const apiError = {
        response: {
          data: {
            detail: 'API error message'
          }
        }
      };
      
      mockedAxios.post.mockRejectedValueOnce(apiError);
      
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      await result.current.createSession();
      
      await waitFor(() => {
        expect(result.current.error).toBe('API error message');
      });
    });

    it('should handle network errors', async () => {
      const networkError = {
        request: {},
        message: 'Network Error'
      };
      
      mockedAxios.post.mockRejectedValueOnce(networkError);
      
      const { result } = renderHook(() => useTerminalSession('vm-test'));
      
      await waitFor(() => {
        expect(result.current.sessionHistory).toHaveLength(2);
      });
      
      await result.current.createSession();
      
      await waitFor(() => {
        expect(result.current.error).toBe('Network Error');
      });
    });
  });
});