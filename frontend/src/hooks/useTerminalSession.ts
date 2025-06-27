import { useState, useEffect, useCallback } from 'react';
// Removed axios import since we're using WebSocket for session management

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
      // Create a local session for WebSocket demo
      const newSession: TerminalSession = {
        id: `session-${Date.now()}`,
        vmId,
        status: 'active',
        createdAt: new Date().toISOString(),
        metadata: {
          type: 'terminal',
          demo: true
        }
      };

      setCurrentSession(newSession);
      console.log('Demo session created:', newSession.id);
    } catch (err: any) {
      const errorMessage = err.message || 'Session creation failed';
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
      // Simulate session restore for demo
      const restoredSession: TerminalSession = {
        id: sessionId,
        vmId,
        status: 'active',
        createdAt: new Date().toISOString(),
        metadata: {
          type: 'terminal',
          demo: true,
          restored: true
        }
      };
      setCurrentSession(restoredSession);
      console.log('Demo session restored:', sessionId);
    } catch (err: any) {
      const errorMessage = err.message || 'Session restore failed';
      setError(errorMessage);
    } finally {
      setIsLoading(false);
    }
  }, [vmId, clearError]);

  const endSession = useCallback(async () => {
    if (!currentSession?.id) {
      setError('No active session to end');
      return;
    }

    setIsLoading(true);
    clearError();

    try {
      // End session locally for demo
      setCurrentSession(null);
      console.log('Demo session ended:', currentSession.id);
    } catch (err: any) {
      const errorMessage = err.message || 'Failed to end session';
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
      // Return empty history for demo
      setSessionHistory([]);
      console.log('Demo session history loaded for VM:', vmId);
    } catch (err: any) {
      const errorMessage = err.message || 'Failed to load session history';
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