import { useState, useEffect, useCallback } from 'react';
import axios from 'axios';

interface TerminalSession {
  id: string;
  vmId: string;
  status: 'active' | 'suspended' | 'ended' | 'error';
  createdAt: string;
  updatedAt?: string;
  metadata?: Record<string, any>;
}

interface UseTerminalSessionReturn {
  // State
  currentSession: TerminalSession | null;
  sessionHistory: TerminalSession[];
  isLoading: boolean;
  error: string | null;
  
  // Actions
  createSession: () => Promise<void>;
  restoreSession: (sessionId: string) => Promise<void>;
  endSession: () => Promise<void>;
  loadSessionHistory: () => Promise<void>;
  
  // Computed properties
  isSessionActive: boolean;
  canRestore: boolean;
}

export function useTerminalSession(vmId: string): UseTerminalSessionReturn {
  const [currentSession, setCurrentSession] = useState<TerminalSession | null>(null);
  const [sessionHistory, setSessionHistory] = useState<TerminalSession[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Reset state when VM ID changes
  useEffect(() => {
    setCurrentSession(null);
    setSessionHistory([]);
    setError(null);
  }, [vmId]);

  const clearError = useCallback(() => {
    setError(null);
  }, []);

  const createSession = useCallback(async () => {
    if (!vmId) {
      setError('VM ID is required to create a session');
      return;
    }

    setIsLoading(true);
    clearError();

    try {
      const response = await axios.post('/api/v1/sessions', {
        vm_id: vmId,
        type: 'terminal'
      });

      setCurrentSession(response.data.session);
    } catch (err: any) {
      const errorMessage = err.response?.data?.detail || err.message || 'Session creation failed';
      setError(errorMessage);
    } finally {
      setIsLoading(false);
    }
  }, [vmId, clearError]);

  const restoreSession = useCallback(async (sessionId: string) => {
    if (!sessionId) {
      setError('Session ID is required to restore a session');
      return;
    }

    setIsLoading(true);
    clearError();

    try {
      const response = await axios.post(`/api/v1/sessions/${sessionId}/restore`, {});
      setCurrentSession(response.data.session);
    } catch (err: any) {
      const errorMessage = err.response?.data?.detail || err.message || 'Session restore failed';
      setError(errorMessage);
    } finally {
      setIsLoading(false);
    }
  }, [clearError]);

  const endSession = useCallback(async () => {
    if (!currentSession?.id) {
      setError('No active session to end');
      return;
    }

    setIsLoading(true);
    clearError();

    try {
      await axios.delete(`/api/v1/sessions/${currentSession.id}`);
      setCurrentSession(null);
    } catch (err: any) {
      const errorMessage = err.response?.data?.detail || err.message || 'Failed to end session';
      setError(errorMessage);
    } finally {
      setIsLoading(false);
    }
  }, [currentSession?.id, clearError]);

  const loadSessionHistory = useCallback(async () => {
    if (!vmId) {
      setError('VM ID is required to load session history');
      return;
    }

    setIsLoading(true);
    clearError();

    try {
      const response = await axios.get(`/api/v1/sessions?vm_id=${vmId}&limit=10`);
      setSessionHistory(response.data.sessions || []);
    } catch (err: any) {
      const errorMessage = err.response?.data?.detail || err.message || 'Failed to load session history';
      setError(errorMessage);
    } finally {
      setIsLoading(false);
    }
  }, [vmId, clearError]);

  // Computed properties
  const isSessionActive = currentSession?.status === 'active';
  const canRestore = currentSession?.status === 'suspended';

  return {
    // State
    currentSession,
    sessionHistory,
    isLoading,
    error,
    
    // Actions
    createSession,
    restoreSession,
    endSession,
    loadSessionHistory,
    
    // Computed properties
    isSessionActive,
    canRestore,
  };
}