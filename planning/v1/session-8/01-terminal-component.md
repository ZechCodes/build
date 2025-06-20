# Session 8.1: React Terminal Component with xterm.js

## Objective
Implement a comprehensive React terminal component using xterm.js, providing full-featured terminal emulation with modern UI/UX, accessibility features, and seamless integration with the backend terminal sessions.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for frontend terminal interaction monitoring and debugging
- **Session 2**: Integrates with authentication system for terminal access control
- **Session 4**: Connects to PTY layer for terminal data communication
- **Session 6**: Integrates with session management for terminal state persistence
- **Session 7**: Supports snapshot operations directly from terminal interface

## Core Implementation

### Terminal Component Architecture
**Location**: `frontend/src/components/Terminal/`

```tsx
// frontend/src/components/Terminal/Terminal.tsx
import React, { useEffect, useRef, useState, useCallback } from 'react';
import { Terminal as XTerm } from 'xterm';
import { FitAddon } from 'xterm-addon-fit';
import { WebLinksAddon } from 'xterm-addon-web-links';
import { SearchAddon } from 'xterm-addon-search';
import { Unicode11Addon } from 'xterm-addon-unicode11';
import { AttachAddon } from 'xterm-addon-attach';
import { WebglAddon } from 'xterm-addon-webgl';
import 'xterm/css/xterm.css';
import { useWebSocket } from '../hooks/useWebSocket';
import { useTerminalSession } from '../hooks/useTerminalSession';
import { useAuth } from '../contexts/AuthContext';
import { logfire } from '../utils/logfire';
import './Terminal.css';

interface TerminalProps {
  sessionId?: string;
  vmId: string;
  className?: string;
  onResize?: (cols: number, rows: number) => void;
  onTitleChange?: (title: string) => void;
  theme?: 'dark' | 'light' | 'custom';
  fontSize?: number;
  fontFamily?: string;
  readOnly?: boolean;
}

interface TerminalTheme {
  background: string;
  foreground: string;
  cursor: string;
  cursorAccent: string;
  selection: string;
  black: string;
  red: string;
  green: string;
  yellow: string;
  blue: string;
  magenta: string;
  cyan: string;
  white: string;
  brightBlack: string;
  brightRed: string;
  brightGreen: string;
  brightYellow: string;
  brightBlue: string;
  brightMagenta: string;
  brightCyan: string;
  brightWhite: string;
}

export const Terminal: React.FC<TerminalProps> = ({
  sessionId,
  vmId,
  className = '',
  onResize,
  onTitleChange,
  theme = 'dark',
  fontSize = 14,
  fontFamily = '"Fira Code", "SF Mono", Monaco, Inconsolata, "Roboto Mono", monospace',
  readOnly = false
}) => {
  const terminalRef = useRef<HTMLDivElement>(null);
  const xtermRef = useRef<XTerm | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const webglAddonRef = useRef<WebglAddon | null>(null);
  const searchAddonRef = useRef<SearchAddon | null>(null);
  
  const [isConnected, setIsConnected] = useState(false);
  const [terminalSize, setTerminalSize] = useState({ cols: 80, rows: 24 });
  const [isWebGLEnabled, setIsWebGLEnabled] = useState(true);
  
  const { user } = useAuth();
  const { session, createSession, attachSession } = useTerminalSession();
  const { 
    connect, 
    disconnect, 
    sendMessage, 
    lastMessage, 
    connectionState 
  } = useWebSocket();

  // Terminal themes
  const themes: Record<string, TerminalTheme> = {
    dark: {
      background: '#1e1e1e',
      foreground: '#d4d4d4',
      cursor: '#d4d4d4',
      cursorAccent: '#1e1e1e',
      selection: '#add6ff26',
      black: '#000000',
      red: '#f14c4c',
      green: '#23d18b',
      yellow: '#f5f543',
      blue: '#3b8eea',
      magenta: '#d670d6',
      cyan: '#29b8db',
      white: '#e5e5e5',
      brightBlack: '#666666',
      brightRed: '#f14c4c',
      brightGreen: '#23d18b',
      brightYellow: '#f5f543',
      brightBlue: '#3b8eea',
      brightMagenta: '#d670d6',
      brightCyan: '#29b8db',
      brightWhite: '#ffffff'
    },
    light: {
      background: '#ffffff',
      foreground: '#333333',
      cursor: '#333333',
      cursorAccent: '#ffffff',
      selection: '#3390ff44',
      black: '#000000',
      red: '#cd3131',
      green: '#00bc00',
      yellow: '#949800',
      blue: '#0451a5',
      magenta: '#bc05bc',
      cyan: '#0598bc',
      white: '#555555',
      brightBlack: '#666666',
      brightRed: '#cd3131',
      brightGreen: '#00bc00',
      brightYellow: '#949800',
      brightBlue: '#0451a5',
      brightMagenta: '#bc05bc',
      brightCyan: '#0598bc',
      brightWhite: '#ffffff'
    }
  };

  // Initialize terminal
  useEffect(() => {
    if (!terminalRef.current || xtermRef.current) return;

    try {
      // Create xterm instance
      const xterm = new XTerm({
        theme: themes[theme],
        fontSize,
        fontFamily,
        cursorBlink: true,
        cursorStyle: 'block',
        scrollback: 10000,
        tabStopWidth: 4,
        allowTransparency: false,
        convertEol: true,
        disableStdin: readOnly,
        allowProposedApi: true
      });

      // Create addons
      const fitAddon = new FitAddon();
      const webLinksAddon = new WebLinksAddon();
      const searchAddon = new SearchAddon();
      const unicode11Addon = new Unicode11Addon();

      // Load addons
      xterm.loadAddon(fitAddon);
      xterm.loadAddon(webLinksAddon);
      xterm.loadAddon(searchAddon);
      xterm.loadAddon(unicode11Addon);

      // Try to load WebGL addon for better performance
      try {
        const webglAddon = new WebglAddon();
        xterm.loadAddon(webglAddon);
        webglAddonRef.current = webglAddon;
        setIsWebGLEnabled(true);
        
        logfire.info('Terminal WebGL acceleration enabled', {
          sessionId,
          vmId,
          userId: user?.id
        });
      } catch (error) {
        console.warn('WebGL addon failed to load, falling back to canvas renderer:', error);
        setIsWebGLEnabled(false);
        
        logfire.warning('Terminal WebGL fallback', {
          sessionId,
          vmId,
          error: error instanceof Error ? error.message : 'Unknown error'
        });
      }

      // Open terminal
      xterm.open(terminalRef.current);
      
      // Activate unicode support
      xterm.unicode.activeVersion = '11';

      // Set up event handlers
      setupTerminalEventHandlers(xterm, fitAddon, searchAddon);

      // Store references
      xtermRef.current = xterm;
      fitAddonRef.current = fitAddon;
      searchAddonRef.current = searchAddon;

      // Initial fit
      setTimeout(() => {
        fitAddon.fit();
        const { cols, rows } = xterm;
        setTerminalSize({ cols, rows });
        onResize?.(cols, rows);
      }, 100);

      logfire.info('Terminal component initialized', {
        sessionId,
        vmId,
        theme,
        fontSize,
        webglEnabled: isWebGLEnabled,
        userId: user?.id
      });

    } catch (error) {
      console.error('Failed to initialize terminal:', error);
      logfire.error('Terminal initialization failed', {
        sessionId,
        vmId,
        error: error instanceof Error ? error.message : 'Unknown error'
      });
    }

    return () => {
      if (xtermRef.current) {
        xtermRef.current.dispose();
        xtermRef.current = null;
      }
    };
  }, [terminalRef.current, theme, fontSize, fontFamily, readOnly]);

  const setupTerminalEventHandlers = useCallback((
    xterm: XTerm, 
    fitAddon: FitAddon, 
    searchAddon: SearchAddon
  ) => {
    // Data input handler
    xterm.onData((data) => {
      if (!readOnly && isConnected) {
        sendMessage({
          type: 'terminal_input',
          sessionId: sessionId || session?.id,
          data: data
        });
      }
    });

    // Title change handler
    xterm.onTitleChange((title) => {
      onTitleChange?.(title);
      logfire.info('Terminal title changed', {
        sessionId: sessionId || session?.id,
        title,
        vmId
      });
    });

    // Resize handler
    xterm.onResize(({ cols, rows }) => {
      setTerminalSize({ cols, rows });
      onResize?.(cols, rows);
      
      // Send resize to backend
      if (isConnected) {
        sendMessage({
          type: 'terminal_resize',
          sessionId: sessionId || session?.id,
          cols,
          rows
        });
      }
    });

    // Selection change handler
    xterm.onSelectionChange(() => {
      const selection = xterm.getSelection();
      if (selection.length > 0) {
        logfire.debug('Terminal text selected', {
          sessionId: sessionId || session?.id,
          selectionLength: selection.length
        });
      }
    });

    // Bell handler
    xterm.onBell(() => {
      // Visual bell effect
      const element = terminalRef.current;
      if (element) {
        element.classList.add('terminal-bell');
        setTimeout(() => {
          element.classList.remove('terminal-bell');
        }, 150);
      }
    });

    // Key handler for shortcuts
    xterm.attachCustomKeyEventHandler((event) => {
      // Ctrl+Shift+F for search
      if (event.ctrlKey && event.shiftKey && event.key === 'F') {
        event.preventDefault();
        openSearch();
        return false;
      }
      
      // Ctrl+Shift+C for copy
      if (event.ctrlKey && event.shiftKey && event.key === 'C') {
        event.preventDefault();
        copySelection();
        return false;
      }
      
      // Ctrl+Shift+V for paste
      if (event.ctrlKey && event.shiftKey && event.key === 'V') {
        event.preventDefault();
        pasteFromClipboard();
        return false;
      }

      return true;
    });

    // Window resize handler
    const handleResize = () => {
      setTimeout(() => {
        fitAddon.fit();
        const { cols, rows } = xterm;
        setTerminalSize({ cols, rows });
        onResize?.(cols, rows);
      }, 100);
    };

    window.addEventListener('resize', handleResize);
    
    return () => {
      window.removeEventListener('resize', handleResize);
    };
  }, [isConnected, readOnly, sessionId, session, sendMessage, onResize, onTitleChange]);

  // Handle WebSocket messages
  useEffect(() => {
    if (!lastMessage || !xtermRef.current) return;

    try {
      const message = JSON.parse(lastMessage.data);
      
      switch (message.type) {
        case 'terminal_output':
          if (message.sessionId === (sessionId || session?.id)) {
            xtermRef.current.write(message.data);
          }
          break;
          
        case 'terminal_connected':
          setIsConnected(true);
          logfire.info('Terminal connected', {
            sessionId: message.sessionId,
            vmId
          });
          break;
          
        case 'terminal_disconnected':
          setIsConnected(false);
          xtermRef.current.write('\r\n\x1b[31mTerminal disconnected\x1b[0m\r\n');
          logfire.warning('Terminal disconnected', {
            sessionId: message.sessionId,
            vmId
          });
          break;
          
        case 'session_restored':
          if (message.bufferData && xtermRef.current) {
            xtermRef.current.clear();
            xtermRef.current.write(message.bufferData);
            logfire.info('Terminal session restored', {
              sessionId: message.sessionId,
              bufferSize: message.bufferData.length
            });
          }
          break;
      }
    } catch (error) {
      console.error('Failed to process WebSocket message:', error);
    }
  }, [lastMessage, sessionId, session, vmId]);

  // Terminal utility functions
  const copySelection = useCallback(() => {
    if (xtermRef.current) {
      const selection = xtermRef.current.getSelection();
      if (selection) {
        navigator.clipboard.writeText(selection).then(() => {
          logfire.debug('Terminal text copied', {
            sessionId: sessionId || session?.id,
            textLength: selection.length
          });
        }).catch(console.error);
      }
    }
  }, [sessionId, session]);

  const pasteFromClipboard = useCallback(async () => {
    if (xtermRef.current && !readOnly) {
      try {
        const text = await navigator.clipboard.readText();
        xtermRef.current.paste(text);
        
        logfire.debug('Terminal text pasted', {
          sessionId: sessionId || session?.id,
          textLength: text.length
        });
      } catch (error) {
        console.error('Failed to paste from clipboard:', error);
      }
    }
  }, [readOnly, sessionId, session]);

  const openSearch = useCallback(() => {
    if (searchAddonRef.current && xtermRef.current) {
      // This would trigger a search modal or panel
      console.log('Search functionality would be implemented here');
    }
  }, []);

  const clearTerminal = useCallback(() => {
    if (xtermRef.current) {
      xtermRef.current.clear();
      logfire.info('Terminal cleared', {
        sessionId: sessionId || session?.id,
        vmId
      });
    }
  }, [sessionId, session, vmId]);

  const fitTerminal = useCallback(() => {
    if (fitAddonRef.current && xtermRef.current) {
      fitAddonRef.current.fit();
      const { cols, rows } = xtermRef.current;
      setTerminalSize({ cols, rows });
      onResize?.(cols, rows);
    }
  }, [onResize]);

  // Public interface for parent components
  const terminalAPI = {
    clear: clearTerminal,
    fit: fitTerminal,
    copy: copySelection,
    paste: pasteFromClipboard,
    search: openSearch,
    write: (data: string) => xtermRef.current?.write(data),
    getSelection: () => xtermRef.current?.getSelection() || '',
    focus: () => xtermRef.current?.focus(),
    blur: () => xtermRef.current?.blur(),
    getSize: () => terminalSize,
    isConnected
  };

  // Expose API to parent via ref
  React.useImperativeHandle(React.forwardRef(() => terminalAPI), () => terminalAPI);

  return (
    <div className={`terminal-container ${className}`}>
      <div 
        ref={terminalRef}
        className="terminal-content"
        style={{
          width: '100%',
          height: '100%',
          overflow: 'hidden'
        }}
        role="terminal"
        aria-label={`Terminal for VM ${vmId}`}
        tabIndex={0}
      />
      
      {/* Connection status indicator */}
      <div className={`terminal-status ${isConnected ? 'connected' : 'disconnected'}`}>
        <span className="status-indicator" />
        <span className="status-text">
          {isConnected ? 'Connected' : 'Disconnected'}
        </span>
        {isWebGLEnabled && (
          <span className="webgl-indicator" title="Hardware acceleration enabled">
            ⚡
          </span>
        )}
      </div>
      
      {/* Terminal info overlay */}
      <div className="terminal-info">
        <span className="terminal-size">
          {terminalSize.cols}×{terminalSize.rows}
        </span>
      </div>
    </div>
  );
};

export default Terminal;
```

### Terminal Hooks
**Location**: `frontend/src/hooks/useTerminalSession.ts`

```typescript
// frontend/src/hooks/useTerminalSession.ts
import { useState, useEffect, useCallback } from 'react';
import { useAuth } from '../contexts/AuthContext';
import { terminalAPI } from '../api/terminal';
import { logfire } from '../utils/logfire';

interface TerminalSession {
  id: string;
  vmId: string;
  userId: string;
  state: 'active' | 'idle' | 'suspended' | 'terminated';
  createdAt: string;
  lastActivity: string;
  terminalSize: {
    cols: number;
    rows: number;
  };
}

interface UseTerminalSessionReturn {
  session: TerminalSession | null;
  sessions: TerminalSession[];
  loading: boolean;
  error: string | null;
  createSession: (vmId: string, cols?: number, rows?: number) => Promise<TerminalSession>;
  attachSession: (sessionId: string) => Promise<void>;
  detachSession: (sessionId: string) => Promise<void>;
  terminateSession: (sessionId: string) => Promise<void>;
  listSessions: (vmId?: string) => Promise<void>;
  restoreSession: (sessionId: string) => Promise<void>;
}

export const useTerminalSession = (): UseTerminalSessionReturn => {
  const [session, setSession] = useState<TerminalSession | null>(null);
  const [sessions, setSessions] = useState<TerminalSession[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  
  const { user, token } = useAuth();

  const createSession = useCallback(async (
    vmId: string, 
    cols: number = 80, 
    rows: number = 24
  ): Promise<TerminalSession> => {
    if (!user || !token) {
      throw new Error('Authentication required');
    }

    setLoading(true);
    setError(null);

    try {
      logfire.info('Creating terminal session', {
        vmId,
        userId: user.id,
        terminalSize: { cols, rows }
      });

      const newSession = await terminalAPI.createSession(token, {
        vmId,
        terminalSize: { cols, rows }
      });

      setSession(newSession);
      setSessions(prev => [...prev, newSession]);

      logfire.info('Terminal session created', {
        sessionId: newSession.id,
        vmId,
        userId: user.id
      });

      return newSession;
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to create session';
      setError(errorMessage);
      
      logfire.error('Terminal session creation failed', {
        vmId,
        userId: user.id,
        error: errorMessage
      });
      
      throw err;
    } finally {
      setLoading(false);
    }
  }, [user, token]);

  const attachSession = useCallback(async (sessionId: string): Promise<void> => {
    if (!user || !token) {
      throw new Error('Authentication required');
    }

    setLoading(true);
    setError(null);

    try {
      logfire.info('Attaching to terminal session', {
        sessionId,
        userId: user.id
      });

      const sessionData = await terminalAPI.attachSession(token, sessionId);
      setSession(sessionData);

      logfire.info('Attached to terminal session', {
        sessionId,
        userId: user.id
      });
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to attach to session';
      setError(errorMessage);
      
      logfire.error('Terminal session attach failed', {
        sessionId,
        userId: user.id,
        error: errorMessage
      });
      
      throw err;
    } finally {
      setLoading(false);
    }
  }, [user, token]);

  const detachSession = useCallback(async (sessionId: string): Promise<void> => {
    if (!user || !token) {
      throw new Error('Authentication required');
    }

    try {
      logfire.info('Detaching from terminal session', {
        sessionId,
        userId: user.id
      });

      await terminalAPI.detachSession(token, sessionId);
      
      if (session?.id === sessionId) {
        setSession(null);
      }

      logfire.info('Detached from terminal session', {
        sessionId,
        userId: user.id
      });
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to detach from session';
      setError(errorMessage);
      
      logfire.error('Terminal session detach failed', {
        sessionId,
        userId: user.id,
        error: errorMessage
      });
      
      throw err;
    }
  }, [user, token, session]);

  const terminateSession = useCallback(async (sessionId: string): Promise<void> => {
    if (!user || !token) {
      throw new Error('Authentication required');
    }

    try {
      logfire.info('Terminating terminal session', {
        sessionId,
        userId: user.id
      });

      await terminalAPI.terminateSession(token, sessionId);
      
      setSessions(prev => prev.filter(s => s.id !== sessionId));
      
      if (session?.id === sessionId) {
        setSession(null);
      }

      logfire.info('Terminal session terminated', {
        sessionId,
        userId: user.id
      });
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to terminate session';
      setError(errorMessage);
      
      logfire.error('Terminal session termination failed', {
        sessionId,
        userId: user.id,
        error: errorMessage
      });
      
      throw err;
    }
  }, [user, token, session]);

  const listSessions = useCallback(async (vmId?: string): Promise<void> => {
    if (!user || !token) {
      throw new Error('Authentication required');
    }

    setLoading(true);
    setError(null);

    try {
      const sessionList = await terminalAPI.listSessions(token, vmId);
      setSessions(sessionList);

      logfire.debug('Terminal sessions listed', {
        userId: user.id,
        vmId,
        sessionCount: sessionList.length
      });
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to list sessions';
      setError(errorMessage);
      
      logfire.error('Terminal session listing failed', {
        userId: user.id,
        vmId,
        error: errorMessage
      });
    } finally {
      setLoading(false);
    }
  }, [user, token]);

  const restoreSession = useCallback(async (sessionId: string): Promise<void> => {
    if (!user || !token) {
      throw new Error('Authentication required');
    }

    try {
      logfire.info('Restoring terminal session', {
        sessionId,
        userId: user.id
      });

      const restoredSession = await terminalAPI.restoreSession(token, sessionId);
      setSession(restoredSession);

      logfire.info('Terminal session restored', {
        sessionId,
        userId: user.id
      });
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to restore session';
      setError(errorMessage);
      
      logfire.error('Terminal session restore failed', {
        sessionId,
        userId: user.id,
        error: errorMessage
      });
      
      throw err;
    }
  }, [user, token]);

  return {
    session,
    sessions,
    loading,
    error,
    createSession,
    attachSession,
    detachSession,
    terminateSession,
    listSessions,
    restoreSession
  };
};
```

## TDD Implementation Cycle

### Test-Driven Development Process

1. **Red Phase**: Write failing terminal component tests
   ```bash
   # Create terminal test file
   touch frontend/src/components/Terminal/__tests__/Terminal.test.tsx
   
   # Run failing test
   npm test Terminal.test.tsx
   ```

2. **Green Phase**: Implement basic terminal functionality
   ```bash
   # Implement terminal component
   npm test Terminal.test.tsx
   ```

3. **Refactor Phase**: Optimize terminal performance and UX
   ```bash
   # Add advanced features and optimizations
   npm test -- --coverage
   ```

4. **Commit**: Commit terminal functionality
   ```bash
   git add frontend/src/components/Terminal/ frontend/src/hooks/useTerminalSession.ts
   git commit -m "feat: implement React terminal component with xterm.js integration
   
   - Add comprehensive Terminal component with xterm.js and modern addons
   - Implement terminal session management with hooks
   - Add WebGL acceleration support with canvas fallback
   - Include accessibility features and keyboard shortcuts
   - Integrate with Logfire for terminal interaction monitoring
   
   Tests: Added comprehensive test suite for terminal component and hooks
   Accessibility: ARIA labels, keyboard navigation, and screen reader support
   Performance: WebGL acceleration, efficient rendering, and memory optimization"
   ```

### Terminal Test Cases

```tsx
// frontend/src/components/Terminal/__tests__/Terminal.test.tsx
import React from 'react';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { Terminal } from '../Terminal';
import { AuthProvider } from '../../contexts/AuthContext';
import { WebSocketProvider } from '../../contexts/WebSocketContext';

// Mock xterm.js
jest.mock('xterm', () => ({
  Terminal: jest.fn().mockImplementation(() => ({
    open: jest.fn(),
    write: jest.fn(),
    onData: jest.fn(),
    onResize: jest.fn(),
    onTitleChange: jest.fn(),
    onSelectionChange: jest.fn(),
    onBell: jest.fn(),
    attachCustomKeyEventHandler: jest.fn(),
    getSelection: jest.fn(() => 'test selection'),
    clear: jest.fn(),
    paste: jest.fn(),
    focus: jest.fn(),
    blur: jest.fn(),
    dispose: jest.fn(),
    unicode: { activeVersion: '11' },
    loadAddon: jest.fn(),
    cols: 80,
    rows: 24
  }))
}));

const renderTerminal = (props = {}) => {
  return render(
    <AuthProvider>
      <WebSocketProvider>
        <Terminal vmId="test-vm" {...props} />
      </WebSocketProvider>
    </AuthProvider>
  );
};

describe('Terminal Component', () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  test('renders terminal container', () => {
    renderTerminal();
    
    expect(screen.getByRole('terminal')).toBeInTheDocument();
    expect(screen.getByLabelText('Terminal for VM test-vm')).toBeInTheDocument();
  });

  test('initializes xterm with correct options', () => {
    const { Terminal: XTermConstructor } = require('xterm');
    
    renderTerminal({
      theme: 'dark',
      fontSize: 16,
      fontFamily: 'monospace'
    });

    expect(XTermConstructor).toHaveBeenCalledWith(
      expect.objectContaining({
        fontSize: 16,
        fontFamily: 'monospace',
        cursorBlink: true,
        scrollback: 10000
      })
    );
  });

  test('displays connection status', () => {
    renderTerminal();
    
    expect(screen.getByText('Disconnected')).toBeInTheDocument();
    expect(screen.getByText('80×24')).toBeInTheDocument();
  });

  test('handles keyboard shortcuts', async () => {
    const mockWrite = jest.fn();
    require('xterm').Terminal.mockImplementation(() => ({
      ...require('xterm').Terminal(),
      write: mockWrite,
      getSelection: () => 'selected text'
    }));

    renderTerminal();
    
    const terminal = screen.getByRole('terminal');
    
    // Test copy shortcut (Ctrl+Shift+C)
    fireEvent.keyDown(terminal, {
      key: 'C',
      ctrlKey: true,
      shiftKey: true
    });

    // Mock clipboard API
    Object.assign(navigator, {
      clipboard: {
        writeText: jest.fn().mockResolvedValue(undefined)
      }
    });

    await waitFor(() => {
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith('selected text');
    });
  });

  test('handles theme changes', () => {
    const { rerender } = renderTerminal({ theme: 'dark' });
    
    rerender(
      <AuthProvider>
        <WebSocketProvider>
          <Terminal vmId="test-vm" theme="light" />
        </WebSocketProvider>
      </AuthProvider>
    );

    // Verify terminal was recreated with new theme
    expect(require('xterm').Terminal).toHaveBeenCalledWith(
      expect.objectContaining({
        theme: expect.objectContaining({
          background: '#ffffff',
          foreground: '#333333'
        })
      })
    );
  });

  test('handles resize events', () => {
    const onResize = jest.fn();
    renderTerminal({ onResize });

    const mockXterm = require('xterm').Terminal.mock.results[0].value;
    
    // Simulate resize event
    const resizeHandler = mockXterm.onResize.mock.calls[0][0];
    resizeHandler({ cols: 100, rows: 30 });

    expect(onResize).toHaveBeenCalledWith(100, 30);
  });

  test('handles WebSocket messages', async () => {
    const mockWrite = jest.fn();
    require('xterm').Terminal.mockImplementation(() => ({
      ...require('xterm').Terminal(),
      write: mockWrite
    }));

    renderTerminal({ sessionId: 'test-session' });

    // Simulate WebSocket message
    const messageEvent = new MessageEvent('message', {
      data: JSON.stringify({
        type: 'terminal_output',
        sessionId: 'test-session',
        data: 'Hello, terminal!'
      })
    });

    window.dispatchEvent(messageEvent);

    await waitFor(() => {
      expect(mockWrite).toHaveBeenCalledWith('Hello, terminal!');
    });
  });
});
```

## Security Checklist for Terminal Component

### Client-Side Security
- [ ] Input sanitization for all terminal data to prevent XSS attacks
- [ ] WebSocket message validation with type checking and schema validation
- [ ] Content Security Policy (CSP) headers preventing inline script execution
- [ ] Secure handling of clipboard operations with user permission validation
- [ ] Protection against terminal escape sequence injection attacks
- [ ] Validation of terminal size parameters to prevent buffer overflow
- [ ] Secure theme handling preventing CSS injection attacks
- [ ] Authentication token validation before terminal operations
- [ ] Session ownership validation on all terminal operations
- [ ] Rate limiting on terminal input to prevent spam/DoS attacks

### Data Protection
- [ ] Terminal output filtering to prevent sensitive data exposure
- [ ] Clipboard data validation and sanitization
- [ ] Secure storage of terminal preferences in localStorage
- [ ] Protection against terminal session hijacking
- [ ] Validation of font loading sources to prevent external resource attacks
- [ ] Secure handling of terminal search functionality
- [ ] Protection against terminal history enumeration
- [ ] Secure terminal session token management
- [ ] Validation of WebSocket origin and connection security
- [ ] Terminal data encryption for sensitive operations

### UI/UX Security
- [ ] Accessibility features that don't compromise security
- [ ] Secure handling of terminal focus and blur events
- [ ] Protection against UI redressing attacks on terminal
- [ ] Secure error handling that doesn't leak sensitive information
- [ ] Validation of terminal resize operations
- [ ] Protection against terminal cursor manipulation attacks
- [ ] Secure handling of terminal bell and notification events
- [ ] Validation of terminal color and styling to prevent abuse
- [ ] Protection against terminal content injection via styling
- [ ] Secure terminal theme switching without code execution

## Performance Requirements

### Terminal Rendering Performance
- Initial terminal load time < 1 second
- Terminal resize response time < 100ms
- Text rendering performance > 60 FPS during heavy output
- Memory usage < 100MB for 10,000 lines of scrollback
- WebGL acceleration reducing CPU usage by 30%
- Smooth scrolling performance at > 30 FPS

### User Interaction Performance
- Keystroke to display latency < 16ms (60 FPS)
- Copy/paste operations < 100ms
- Search operations < 200ms for 10,000 lines
- Theme switching < 200ms
- Terminal focus/blur response < 50ms
- Resize operations complete within 2 frames

### Network Performance
- WebSocket message processing < 10ms per message
- Terminal session establishment < 500ms
- Network error recovery < 2 seconds
- Automatic reconnection within 5 seconds
- Bandwidth optimization with efficient data protocols
- Connection state updates < 100ms

## Accessibility Features

### Screen Reader Support
```tsx
// ARIA attributes for screen readers
<div 
  role="terminal"
  aria-label={`Terminal for VM ${vmId}`}
  aria-live="polite"
  aria-atomic="false"
  tabIndex={0}
>
```

### Keyboard Navigation
- Full keyboard accessibility with tab navigation
- Standard terminal shortcuts (Ctrl+C, Ctrl+V, etc.)
- Accessible search functionality with keyboard shortcuts
- Focus management for modal dialogs and overlays
- Screen reader announcements for important state changes

### High Contrast Support
```css
/* High contrast theme support */
@media (prefers-contrast: high) {
  .terminal-container {
    --terminal-bg: #000000;
    --terminal-fg: #ffffff;
    --terminal-selection: #ffffff;
  }
}

@media (prefers-reduced-motion: reduce) {
  .terminal-container {
    --terminal-cursor-blink: none;
    --terminal-scroll-behavior: auto;
  }
}
```

## Integration Testing

### Component Integration Tests
```tsx
async function testTerminalIntegration() {
  // Test terminal with session management
  const terminal = render(<Terminal vmId="test-vm" />);
  
  // Verify session creation
  await waitFor(() => {
    expect(screen.getByText('Connected')).toBeInTheDocument();
  });
  
  // Test WebSocket integration
  fireEvent.keyDown(terminal.getByRole('terminal'), {
    key: 'a'
  });
  
  // Verify message sent
  expect(mockWebSocket.send).toHaveBeenCalledWith(
    expect.stringContaining('terminal_input')
  );
}
```

### Authentication Integration
```tsx
async function testTerminalAuthentication() {
  // Test with unauthenticated user
  render(
    <AuthProvider value={{ user: null, token: null }}>
      <Terminal vmId="test-vm" />
    </AuthProvider>
  );
  
  expect(screen.getByText('Authentication required')).toBeInTheDocument();
  
  // Test with authenticated user
  render(
    <AuthProvider value={{ user: mockUser, token: 'valid-token' }}>
      <Terminal vmId="test-vm" />
    </AuthProvider>
  );
  
  expect(screen.getByRole('terminal')).toBeInTheDocument();
}
```

## Next Implementation Steps

1. **Complete terminal component implementation** with all xterm.js features
2. **Add advanced accessibility features** for screen readers and keyboard navigation
3. **Implement terminal session persistence** with automatic reconnection
4. **Add comprehensive error handling** with user-friendly error messages
5. **Create terminal themes system** with customizable color schemes
6. **Add performance optimization** with virtualization for large outputs
7. **Implement advanced features** like terminal splitting and tabs

## Commit Guidelines

Each commit should include:
- **Feature implementation** with comprehensive error handling
- **Accessibility validation** with ARIA compliance and keyboard navigation
- **Performance optimization** with rendering efficiency and memory management
- **Test coverage** for terminal component and hooks (>80%)
- **Integration verification** with WebSocket and session management
- **Documentation updates** with usage examples and API documentation