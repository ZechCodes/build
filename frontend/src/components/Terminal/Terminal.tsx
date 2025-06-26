import React, { useEffect, useRef, useState, useCallback } from 'react';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { SearchAddon } from '@xterm/addon-search';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { WebglAddon } from '@xterm/addon-webgl';
import { CanvasAddon } from '@xterm/addon-canvas';
import '@xterm/xterm/css/xterm.css';
import './Terminal.css';

import { WebSocketManager } from '../../services/WebSocketManager';
import { TerminalToolbar } from './TerminalToolbar';
import { TerminalContextMenu } from './TerminalContextMenu';
import { TerminalSearch } from './TerminalSearch';
import { useTerminalSession } from '../../hooks/useTerminalSession';
import { TerminalTheme, getTerminalTheme } from './themes';

interface TerminalProps {
  sessionId?: string;
  vmId: string;
  onSessionCreated?: (sessionId: string) => void;
  onSessionEnded?: () => void;
  height?: string;
  width?: string;
  theme?: TerminalTheme;
  readOnly?: boolean;
  showToolbar?: boolean;
  apiUrl?: string;
  token?: string;
}

export const Terminal: React.FC<TerminalProps> = ({
  sessionId,
  vmId,
  onSessionCreated,
  onSessionEnded,
  height = '600px',
  width = '100%',
  theme = 'dark',
  readOnly = false,
  showToolbar = true,
  apiUrl = 'ws://localhost:8000',
  token = 'demo-token', // In real app, this would come from auth context
}) => {
  const terminalRef = useRef<HTMLDivElement>(null);
  const xtermRef = useRef<XTerm | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const searchAddonRef = useRef<SearchAddon | null>(null);
  const wsManagerRef = useRef<WebSocketManager | null>(null);
  
  const [isConnected, setIsConnected] = useState(false);
  const [isReconnecting, setIsReconnecting] = useState(false);
  const [searchVisible, setSearchVisible] = useState(false);
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null);
  const [currentTheme, setCurrentTheme] = useState<TerminalTheme>(theme);

  const {
    currentSession,
    createSession,
    restoreSession,
    endSession,
    sessionHistory
  } = useTerminalSession(vmId);

  // Initialize terminal
  useEffect(() => {
    if (!terminalRef.current) return;

    const terminal = new XTerm({
      ...getTerminalTheme(currentTheme),
      cursorBlink: true,
      cursorStyle: 'block',
      scrollback: 10000,
      allowTransparency: true,
      fontFamily: 'Menlo, Monaco, "Courier New", monospace',
      fontSize: 14,
      lineHeight: 1.2,
      rightClickSelectsWord: true,
      convertEol: true,
      allowProposedApi: true
    });

    // Add addons
    const fitAddon = new FitAddon();
    const searchAddon = new SearchAddon();
    const webLinksAddon = new WebLinksAddon();
    
    terminal.loadAddon(fitAddon);
    terminal.loadAddon(searchAddon);
    terminal.loadAddon(webLinksAddon);
    
    // Try WebGL, fallback to Canvas
    try {
      terminal.loadAddon(new WebglAddon());
    } catch {
      try {
        terminal.loadAddon(new CanvasAddon());
      } catch {
        console.warn('Unable to load WebGL or Canvas addon, using DOM renderer');
      }
    }

    terminal.open(terminalRef.current);
    fitAddon.fit();

    // Store references
    xtermRef.current = terminal;
    fitAddonRef.current = fitAddon;
    searchAddonRef.current = searchAddon;

    // Set up event handlers
    setupTerminalEventHandlers(terminal);

    return () => {
      terminal.dispose();
    };
  }, [currentTheme]);

  // Initialize WebSocket connection
  useEffect(() => {
    if (!xtermRef.current || !token) return;

    const wsManager = new WebSocketManager({
      apiUrl,
      token,
      onConnect: handleWebSocketConnect,
      onDisconnect: handleWebSocketDisconnect,
      onReconnecting: handleWebSocketReconnecting,
      onMessage: handleWebSocketMessage,
      onError: handleWebSocketError
    });

    wsManagerRef.current = wsManager;

    return () => {
      wsManager.disconnect();
    };
  }, [token, apiUrl]);

  // Connect to session
  useEffect(() => {
    if (!wsManagerRef.current || !xtermRef.current) return;

    if (sessionId) {
      // Restore existing session
      restoreSession(sessionId);
    } else {
      // Create new session
      createSession();
    }
  }, [sessionId, vmId]);

  const setupTerminalEventHandlers = useCallback((terminal: XTerm) => {
    // Handle user input
    terminal.onData((data) => {
      if (readOnly) return;
      
      if (wsManagerRef.current && isConnected) {
        wsManagerRef.current.sendTerminalData(data);
      }
    });

    // Handle terminal resize
    terminal.onResize(({ cols, rows }) => {
      if (wsManagerRef.current && isConnected) {
        wsManagerRef.current.sendResize(cols, rows);
      }
    });

    // Handle right-click
    terminal.onRightClick((event) => {
      event.preventDefault();
      setContextMenu({ x: event.clientX, y: event.clientY });
    });

    // Handle selection change
    terminal.onSelectionChange(() => {
      setContextMenu(null);
    });

    // Handle bell
    terminal.onBell(() => {
      // Visual bell or notification
      document.title = document.title.startsWith('🔔') ? document.title : `🔔 ${document.title}`;
      setTimeout(() => {
        document.title = document.title.replace('🔔 ', '');
      }, 1000);
    });
  }, [readOnly, isConnected]);

  const handleWebSocketConnect = useCallback(async () => {
    setIsConnected(true);
    setIsReconnecting(false);
    
    // Join session
    if (currentSession?.id && wsManagerRef.current) {
      await wsManagerRef.current.joinSession(currentSession.id);
    }
  }, [currentSession]);

  const handleWebSocketDisconnect = useCallback(() => {
    setIsConnected(false);
    if (xtermRef.current) {
      xtermRef.current.write('\r\n\x1b[31mConnection lost. Reconnecting...\x1b[0m\r\n');
    }
  }, []);

  const handleWebSocketReconnecting = useCallback(() => {
    setIsReconnecting(true);
  }, []);

  const handleWebSocketMessage = useCallback((message: any) => {
    if (!xtermRef.current) return;

    switch (message.type) {
      case 'terminal_data':
        xtermRef.current.write(message.data);
        break;
      
      case 'session_created':
        if (onSessionCreated) {
          onSessionCreated(message.session_id);
        }
        break;
      
      case 'session_recovered':
        // Write recovery data
        if (message.recovery_data?.history) {
          xtermRef.current.clear();
          message.recovery_data.history.forEach((line: string) => {
            xtermRef.current!.write(line);
          });
        }
        break;
      
      case 'session_ended':
        if (onSessionEnded) {
          onSessionEnded();
        }
        break;
      
      case 'error':
        xtermRef.current.write(`\r\n\x1b[31mError: ${message.message}\x1b[0m\r\n`);
        break;
    }
  }, [onSessionCreated, onSessionEnded]);

  const handleWebSocketError = useCallback((error: Error) => {
    console.error('WebSocket error:', error);
    if (xtermRef.current) {
      xtermRef.current.write(`\r\n\x1b[31mConnection error: ${error.message}\x1b[0m\r\n`);
    }
  }, []);

  // Terminal actions
  const handleCopy = useCallback(() => {
    if (xtermRef.current && xtermRef.current.hasSelection()) {
      const selection = xtermRef.current.getSelection();
      navigator.clipboard.writeText(selection).catch(console.error);
    }
  }, []);

  const handlePaste = useCallback(async () => {
    if (readOnly) return;
    
    try {
      const text = await navigator.clipboard.readText();
      if (wsManagerRef.current && isConnected) {
        wsManagerRef.current.sendTerminalData(text);
      }
    } catch (error) {
      console.error('Failed to paste:', error);
    }
  }, [readOnly, isConnected]);

  const handleSearch = useCallback((query: string, options?: { caseSensitive?: boolean; wholeWord?: boolean; regex?: boolean }) => {
    if (searchAddonRef.current) {
      searchAddonRef.current.findNext(query, options);
    }
  }, []);

  const handleClear = useCallback(() => {
    if (xtermRef.current) {
      xtermRef.current.clear();
    }
  }, []);

  const handleFit = useCallback(() => {
    if (fitAddonRef.current) {
      fitAddonRef.current.fit();
    }
  }, []);

  const handleThemeChange = useCallback((newTheme: TerminalTheme) => {
    setCurrentTheme(newTheme);
    if (xtermRef.current) {
      const themeConfig = getTerminalTheme(newTheme);
      Object.keys(themeConfig).forEach(key => {
        (xtermRef.current as any).setOption(key, (themeConfig as any)[key]);
      });
    }
  }, []);

  // Resize handler
  useEffect(() => {
    const handleResize = () => {
      if (fitAddonRef.current) {
        fitAddonRef.current.fit();
      }
    };

    window.addEventListener('resize', handleResize);
    return () => window.removeEventListener('resize', handleResize);
  }, []);

  // Click outside handler for context menu
  useEffect(() => {
    const handleClickOutside = () => setContextMenu(null);
    if (contextMenu) {
      document.addEventListener('click', handleClickOutside);
      return () => document.removeEventListener('click', handleClickOutside);
    }
  }, [contextMenu]);

  return (
    <div className="terminal-container bg-background text-foreground border rounded-lg overflow-hidden" style={{ height, width }}>
      {showToolbar && (
        <TerminalToolbar
          isConnected={isConnected}
          isReconnecting={isReconnecting}
          onCopy={handleCopy}
          onPaste={handlePaste}
          onClear={handleClear}
          onSearch={() => setSearchVisible(!searchVisible)}
          onFit={handleFit}
          onThemeChange={handleThemeChange}
          currentTheme={currentTheme}
        />
      )}
      
      {searchVisible && (
        <TerminalSearch
          onSearch={handleSearch}
          onClose={() => setSearchVisible(false)}
          searchAddon={searchAddonRef.current}
        />
      )}
      
      <div
        ref={terminalRef}
        className="terminal-content"
        style={{
          height: showToolbar ? 'calc(100% - 40px)' : '100%',
          width: '100%'
        }}
      />
      
      {contextMenu && (
        <TerminalContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          onCopy={handleCopy}
          onPaste={handlePaste}
          onSelectAll={() => xtermRef.current?.selectAll()}
          onClose={() => setContextMenu(null)}
          hasSelection={xtermRef.current?.hasSelection() || false}
        />
      )}
    </div>
  );
};