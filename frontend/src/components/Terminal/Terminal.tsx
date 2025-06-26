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
import { 
  sanitizeTerminalText, 
  validateWebSocketMessage, 
  sanitizeClipboardContent,
  detectSensitiveData,
  maskSensitiveData,
  logSecurityEvent 
} from '../../utils/security';
import { 
  usePerformanceMonitoring, 
  useDataBatcher, 
  useThrottledCallback,
  useWSLatencyMonitoring,
  useMemoryOptimization 
} from '../../hooks/usePerformance';
import { performanceMonitor } from '../../utils/performance';

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

  // Performance monitoring
  const { startRender, endRender } = usePerformanceMonitoring('Terminal');
  const { startPing, endPing } = useWSLatencyMonitoring();
  const { addCleanup } = useMemoryOptimization();

  // Data batching for terminal output
  const { addData: addTerminalData, flush: flushTerminalData } = useDataBatcher(
    useCallback((data: string) => {
      if (xtermRef.current) {
        performanceMonitor.startProfiling('terminal-write');
        xtermRef.current.write(data);
        performanceMonitor.endProfiling('terminal-write');
      }
    }, [])
  );

  // Throttled resize handler
  const throttledResize = useThrottledCallback(
    useCallback(() => {
      if (fitAddonRef.current) {
        performanceMonitor.startProfiling('terminal-fit');
        fitAddonRef.current.fit();
        performanceMonitor.endProfiling('terminal-fit');
      }
    }, []),
    100 // Throttle resize to max 10 times per second
  );

  // Initialize terminal
  useEffect(() => {
    if (!terminalRef.current) return;

    startRender();
    performanceMonitor.startProfiling('terminal-init');

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

    // Add cleanup for memory optimization
    addCleanup(() => {
      terminal.dispose();
    });

    performanceMonitor.endProfiling('terminal-init');
    endRender();

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
    
    // Join session and measure latency
    if (currentSession?.id && wsManagerRef.current) {
      startPing();
      await wsManagerRef.current.joinSession(currentSession.id);
      endPing();
    }
  }, [currentSession, startPing, endPing]);

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

    // Validate message structure for security
    if (!validateWebSocketMessage(message)) {
      logSecurityEvent('invalid_websocket_message', {
        messageType: message?.type,
        hasData: !!message?.data
      }, 'medium');
      return;
    }

    switch (message.type) {
      case 'terminal_data':
        // Sanitize terminal data to prevent XSS
        const sanitizedData = sanitizeTerminalText(message.data);
        
        // Record data throughput
        performanceMonitor.recordDataThroughput(sanitizedData.length);
        
        // Check for sensitive data
        const sensitiveCheck = detectSensitiveData(sanitizedData);
        if (sensitiveCheck.hasSensitiveData) {
          logSecurityEvent('sensitive_data_detected', {
            patterns: sensitiveCheck.patterns,
            dataLength: sanitizedData.length
          }, 'high');
          
          // Option to mask sensitive data (configurable)
          const maskedData = maskSensitiveData(sanitizedData);
          addTerminalData(maskedData);
        } else {
          addTerminalData(sanitizedData);
        }
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
      
      // Sanitize clipboard content for security
      const sanitizedText = sanitizeClipboardContent(text);
      
      // Check for potentially dangerous content
      if (text.length !== sanitizedText.length) {
        logSecurityEvent('clipboard_content_sanitized', {
          originalLength: text.length,
          sanitizedLength: sanitizedText.length
        }, 'low');
      }
      
      if (wsManagerRef.current && isConnected) {
        wsManagerRef.current.sendTerminalData(sanitizedText);
      }
    } catch (error) {
      console.error('Failed to paste:', error);
      logSecurityEvent('clipboard_access_failed', {
        error: error instanceof Error ? error.message : 'Unknown error'
      }, 'low');
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
    window.addEventListener('resize', throttledResize);
    return () => window.removeEventListener('resize', throttledResize);
  }, [throttledResize]);

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