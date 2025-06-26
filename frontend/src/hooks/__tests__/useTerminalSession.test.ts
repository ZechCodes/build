import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useTerminalSession } from '../useTerminalSession';

// Mock axios
vi.mock('axios', () => ({
  default: {
    post: vi.fn(),
    get: vi.fn(),
    delete: vi.fn(),
  },
}));

import axios from 'axios';
const mockAxios = axios as any;

describe('useTerminalSession', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockAxios.post.mockResolvedValue({ data: {} });
    mockAxios.get.mockResolvedValue({ data: {} });
    mockAxios.delete.mockResolvedValue({ data: {} });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it('should initialize with default state', () => {
    const { result } = renderHook(() => useTerminalSession('vm-123'));

    expect(result.current.currentSession).toBeNull();
    expect(result.current.sessionHistory).toEqual([]);
    expect(result.current.isLoading).toBe(false);
    expect(result.current.error).toBeNull();
  });

  it('should create a new session', async () => {
    const mockSession = {
      id: 'session-123',
      vmId: 'vm-123',
      status: 'active',
      createdAt: new Date().toISOString(),
    };

    mockAxios.post.mockResolvedValueOnce({
      data: { session: mockSession }
    });

    const { result } = renderHook(() => useTerminalSession('vm-123'));

    await act(async () => {
      await result.current.createSession();
    });

    expect(mockAxios.post).toHaveBeenCalledWith('/api/v1/sessions', {
      vm_id: 'vm-123',
      type: 'terminal'
    });
    expect(result.current.currentSession).toEqual(mockSession);
    expect(result.current.isLoading).toBe(false);
  });

  it('should restore an existing session', async () => {
    const mockSession = {
      id: 'session-456',
      vmId: 'vm-123',
      status: 'active',
      createdAt: new Date().toISOString(),
    };

    mockAxios.post.mockResolvedValueOnce({
      data: { session: mockSession }
    });

    const { result } = renderHook(() => useTerminalSession('vm-123'));

    await act(async () => {
      await result.current.restoreSession('session-456');
    });

    expect(mockAxios.post).toHaveBeenCalledWith('/api/v1/sessions/session-456/restore', {});
    expect(result.current.currentSession).toEqual(mockSession);
  });

  it('should end a session', async () => {
    const mockSession = {
      id: 'session-789',
      vmId: 'vm-123',
      status: 'active',
      createdAt: new Date().toISOString(),
    };

    // First create a session
    mockAxios.post.mockResolvedValueOnce({
      data: { session: mockSession }
    });

    const { result } = renderHook(() => useTerminalSession('vm-123'));

    await act(async () => {
      await result.current.createSession();
    });

    expect(result.current.currentSession).toEqual(mockSession);

    // Now end the session
    mockAxios.delete.mockResolvedValueOnce({ data: { success: true } });

    await act(async () => {
      await result.current.endSession();
    });

    expect(mockAxios.delete).toHaveBeenCalledWith('/api/v1/sessions/session-789');
    expect(result.current.currentSession).toBeNull();
  });

  it('should load session history', async () => {
    const mockHistory = [
      {
        id: 'session-1',
        vmId: 'vm-123',
        status: 'ended',
        createdAt: new Date(Date.now() - 86400000).toISOString(), // 1 day ago
      },
      {
        id: 'session-2',
        vmId: 'vm-123',
        status: 'ended',
        createdAt: new Date(Date.now() - 3600000).toISOString(), // 1 hour ago
      },
    ];

    mockAxios.get.mockResolvedValueOnce({
      data: { sessions: mockHistory }
    });

    const { result } = renderHook(() => useTerminalSession('vm-123'));

    await act(async () => {
      await result.current.loadSessionHistory();
    });

    expect(mockAxios.get).toHaveBeenCalledWith('/api/v1/sessions?vm_id=vm-123&limit=10');
    expect(result.current.sessionHistory).toEqual(mockHistory);
  });

  it('should handle loading states correctly', async () => {
    let resolvePromise: (value: any) => void;
    const pendingPromise = new Promise((resolve) => {
      resolvePromise = resolve;
    });

    mockAxios.post.mockReturnValueOnce(pendingPromise);

    const { result } = renderHook(() => useTerminalSession('vm-123'));

    expect(result.current.isLoading).toBe(false);

    act(() => {
      result.current.createSession();
    });

    expect(result.current.isLoading).toBe(true);

    await act(async () => {
      resolvePromise!({ data: { session: { id: 'test' } } });
    });

    expect(result.current.isLoading).toBe(false);
  });

  it('should handle API errors gracefully', async () => {
    const mockError = new Error('Session creation failed');
    mockAxios.post.mockRejectedValueOnce(mockError);

    const { result } = renderHook(() => useTerminalSession('vm-123'));

    await act(async () => {
      await result.current.createSession();
    });

    expect(result.current.error).toBe('Session creation failed');
    expect(result.current.currentSession).toBeNull();
    expect(result.current.isLoading).toBe(false);
  });

  it('should clear error on successful operation', async () => {
    const mockError = new Error('Initial error');
    mockAxios.post.mockRejectedValueOnce(mockError);

    const { result } = renderHook(() => useTerminalSession('vm-123'));

    // First operation fails
    await act(async () => {
      await result.current.createSession();
    });

    expect(result.current.error).toBe('Initial error');

    // Second operation succeeds
    mockAxios.post.mockResolvedValueOnce({
      data: { session: { id: 'session-success' } }
    });

    await act(async () => {
      await result.current.createSession();
    });

    expect(result.current.error).toBeNull();
    expect(result.current.currentSession).toEqual({ id: 'session-success' });
  });

  it('should update vm id and reset state when vm changes', async () => {
    const { result, rerender } = renderHook(
      ({ vmId }) => useTerminalSession(vmId),
      { initialProps: { vmId: 'vm-123' } }
    );

    // Create a session first
    mockAxios.post.mockResolvedValueOnce({
      data: { session: { id: 'session-old', status: 'active' } }
    });

    await act(async () => {
      await result.current.createSession();
    });

    expect(result.current.currentSession).toEqual({ id: 'session-old', status: 'active' });

    // Change VM ID
    rerender({ vmId: 'vm-456' });

    expect(result.current.currentSession).toBeNull();
    expect(result.current.sessionHistory).toEqual([]);
    expect(result.current.error).toBeNull();
  });

  it('should provide session status helpers', async () => {
    const { result } = renderHook(() => useTerminalSession('vm-123'));

    // No session
    expect(result.current.isSessionActive).toBe(false);
    expect(result.current.canRestore).toBe(false);

    // Active session
    mockAxios.post.mockResolvedValueOnce({
      data: { session: { id: 'session-active', status: 'active' } }
    });

    await act(async () => {
      await result.current.createSession();
    });

    expect(result.current.isSessionActive).toBe(true);
    expect(result.current.canRestore).toBe(false);

    // Test suspended session by restoring a suspended session
    mockAxios.post.mockResolvedValueOnce({
      data: { session: { id: 'session-suspended', status: 'suspended' } }
    });

    await act(async () => {
      await result.current.restoreSession('session-suspended');
    });

    expect(result.current.isSessionActive).toBe(false);
    expect(result.current.canRestore).toBe(true);
  });
});