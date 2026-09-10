# Session 8: Frontend Terminal Implementation

## Objective
Implement a comprehensive browser-based terminal interface using xterm.js that provides a seamless, feature-rich terminal experience with WebSocket connectivity, session management, and advanced terminal features.

## Overview
This session creates the user-facing terminal interface that brings together all backend services from previous sessions. It implements xterm.js integration, WebSocket client functionality, terminal controls, clipboard operations, search functionality, and recording controls. The interface provides a native terminal experience in the browser with proper integration to the session management and VM systems.

## Prerequisites
- Session 1 (Core Infrastructure) completed successfully
- Session 2 (Authentication) completed successfully
- Session 3 (VM Management) completed successfully
- Session 4 (PTY Layer) completed successfully
- Session 5 (WebSocket Layer) completed successfully
- Session 6 (Session Management) completed successfully
- Session 7 (VM Snapshot System) completed successfully
- Frontend framework operational (React/TypeScript)
- WebSocket API endpoints available

## Components to Implement

### 1. Terminal Component Core
**Location**: `frontend/src/components/Terminal/`

#### Main Terminal Component
```typescript
// frontend/src/components/Terminal/Terminal.tsx
import React, { useEffect, useRef, useState, useCallback } from 'react';
import { Terminal as XTerm } from 'xterm';
import { FitAddon } from 'xterm-addon-fit';
import { SearchAddon } from 'xterm-addon-search';
import { WebLinksAddon } from 'xterm-addon-web-links';
import { WebglAddon } from 'xterm-addon-webgl';
import { CanvasAddon } from 'xterm-addon-canvas';
import 'xterm/css/xterm.css';

import { WebSocketManager } from './WebSocketManager';
import { TerminalToolbar } from './TerminalToolbar';
import { TerminalContextMenu } from './TerminalContextMenu';
import { useTerminalSession } from '../../hooks/useTerminalSession';
import { useAuth } from '../../hooks/useAuth';
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
  showToolbar = true
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
  
  const { user } = useAuth();
  const {
    currentSession,
    createSession,
    restoreSession,
    endSession,
    sessionHistory
  } = useTerminalSession(vmId);

  // Initialize terminal
  useEffect(() => {
    if (!terminalRef.current || !user) return;

    const terminal = new XTerm({
      ...getTerminalTheme(theme),
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
  }, [theme, user]);

  // Initialize WebSocket connection
  useEffect(() => {
    if (!xtermRef.current || !user) return;

    const wsManager = new WebSocketManager({
      apiUrl: process.env.REACT_APP_WS_API_URL || 'ws://localhost:8000',
      token: user.token,
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
  }, [user]);

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
    <div className="terminal-container" style={{ height, width }}>
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
          currentTheme={theme}
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
```

### 2. WebSocket Manager
**Location**: `frontend/src/components/Terminal/`

#### WebSocket Communication Manager
```typescript
// frontend/src/components/Terminal/WebSocketManager.ts
import { EventEmitter } from 'events';

interface WebSocketConfig {
  apiUrl: string;
  token: string;
  onConnect?: () => void;
  onDisconnect?: () => void;
  onReconnecting?: () => void;
  onMessage?: (message: any) => void;
  onError?: (error: Error) => void;
}

export class WebSocketManager extends EventEmitter {
  private ws: WebSocket | null = null;
  private config: WebSocketConfig;
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 10;
  private reconnectDelay = 1000;
  private maxReconnectDelay = 30000;
  private heartbeatInterval: NodeJS.Timeout | null = null;
  private isReconnecting = false;
  private sessionId: string | null = null;

  constructor(config: WebSocketConfig) {
    super();
    this.config = config;
    this.connect();
  }

  private connect(): void {
    try {
      const wsUrl = new URL('/ws/terminal', this.config.apiUrl);
      wsUrl.searchParams.set('token', this.config.token);
      
      this.ws = new WebSocket(wsUrl.toString());
      
      this.ws.onopen = this.handleOpen.bind(this);
      this.ws.onmessage = this.handleMessage.bind(this);
      this.ws.onclose = this.handleClose.bind(this);
      this.ws.onerror = this.handleError.bind(this);
      
    } catch (error) {
      console.error('Failed to create WebSocket connection:', error);
      this.scheduleReconnect();
    }
  }

  private handleOpen(): void {
    console.log('WebSocket connected');
    this.reconnectAttempts = 0;
    this.isReconnecting = false;
    this.startHeartbeat();
    
    if (this.config.onConnect) {
      this.config.onConnect();
    }
    
    this.emit('connect');
  }

  private handleMessage(event: MessageEvent): void {
    try {
      const message = JSON.parse(event.data);
      
      switch (message.type) {
        case 'heartbeat':
          // Respond to heartbeat
          this.send({ type: 'heartbeat_response', timestamp: Date.now() });
          break;
        
        default:
          if (this.config.onMessage) {
            this.config.onMessage(message);
          }
          this.emit('message', message);
          break;
      }
    } catch (error) {
      console.error('Failed to parse WebSocket message:', error);
    }
  }

  private handleClose(event: CloseEvent): void {
    console.log('WebSocket disconnected:', event.code, event.reason);
    this.stopHeartbeat();
    
    if (this.config.onDisconnect) {
      this.config.onDisconnect();
    }
    
    this.emit('disconnect', event);
    
    // Attempt to reconnect unless explicitly closed
    if (event.code !== 1000 && event.code !== 1001) {
      this.scheduleReconnect();
    }
  }

  private handleError(event: Event): void {
    console.error('WebSocket error:', event);
    
    const error = new Error('WebSocket connection error');
    if (this.config.onError) {
      this.config.onError(error);
    }
    
    this.emit('error', error);
  }

  private scheduleReconnect(): void {
    if (this.isReconnecting || this.reconnectAttempts >= this.maxReconnectAttempts) {
      return;
    }
    
    this.isReconnecting = true;
    
    if (this.config.onReconnecting) {
      this.config.onReconnecting();
    }
    
    const delay = Math.min(
      this.reconnectDelay * Math.pow(2, this.reconnectAttempts),
      this.maxReconnectDelay
    );
    
    console.log(`Reconnecting in ${delay}ms (attempt ${this.reconnectAttempts + 1})`);
    
    setTimeout(() => {
      this.reconnectAttempts++;
      this.connect();
    }, delay);
  }

  private startHeartbeat(): void {
    this.heartbeatInterval = setInterval(() => {
      if (this.ws?.readyState === WebSocket.OPEN) {
        this.send({ type: 'heartbeat', timestamp: Date.now() });
      }
    }, 30000); // 30 seconds
  }

  private stopHeartbeat(): void {
    if (this.heartbeatInterval) {
      clearInterval(this.heartbeatInterval);
      this.heartbeatInterval = null;
    }
  }

  private send(message: any): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(message));
    }
  }

  public sendTerminalData(data: string): void {
    this.send({
      type: 'terminal_data',
      data,
      session_id: this.sessionId
    });
  }

  public sendResize(cols: number, rows: number): void {
    this.send({
      type: 'terminal_resize',
      data: { cols, rows },
      session_id: this.sessionId
    });
  }

  public async joinSession(sessionId: string): Promise<void> {
    this.sessionId = sessionId;
    this.send({
      type: 'session_join',
      session_id: sessionId
    });
  }

  public async createSession(vmId: string): Promise<void> {
    this.send({
      type: 'session_create',
      vm_id: vmId
    });
  }

  public async endSession(): Promise<void> {
    if (this.sessionId) {
      this.send({
        type: 'session_end',
        session_id: this.sessionId
      });
      this.sessionId = null;
    }
  }

  public disconnect(): void {
    this.stopHeartbeat();
    if (this.ws) {
      this.ws.close(1000, 'Client disconnect');
      this.ws = null;
    }
  }

  public isConnected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }
}
```

### 3. Terminal Toolbar Component
**Location**: `frontend/src/components/Terminal/`

#### Terminal Controls and Actions
```typescript
// frontend/src/components/Terminal/TerminalToolbar.tsx
import React from 'react';
import {
  Copy,
  Clipboard,
  Search,
  RotateCcw,
  Maximize2,
  Settings,
  Circle,
  Wifi,
  WifiOff
} from 'lucide-react';

import { Button } from '../ui/Button';
import { Dropdown } from '../ui/Dropdown';
import { TerminalTheme } from './themes';

interface TerminalToolbarProps {
  isConnected: boolean;
  isReconnecting: boolean;
  onCopy: () => void;
  onPaste: () => void;
  onClear: () => void;
  onSearch: () => void;
  onFit: () => void;
  onThemeChange: (theme: TerminalTheme) => void;
  currentTheme: TerminalTheme;
}

export const TerminalToolbar: React.FC<TerminalToolbarProps> = ({
  isConnected,
  isReconnecting,
  onCopy,
  onPaste,
  onClear,
  onSearch,
  onFit,
  onThemeChange,
  currentTheme
}) => {
  const connectionStatus = isReconnecting 
    ? 'reconnecting' 
    : isConnected 
      ? 'connected' 
      : 'disconnected';

  const statusIcon = {
    connected: <Wifi className="w-4 h-4 text-green-500" />,
    disconnected: <WifiOff className="w-4 h-4 text-red-500" />,
    reconnecting: <Circle className="w-4 h-4 text-yellow-500 animate-pulse" />
  }[connectionStatus];

  const themeOptions = [
    { value: 'dark', label: 'Dark' },
    { value: 'light', label: 'Light' },
    { value: 'high-contrast', label: 'High Contrast' },
    { value: 'solarized-dark', label: 'Solarized Dark' },
    { value: 'solarized-light', label: 'Solarized Light' }
  ];

  return (
    <div className="terminal-toolbar flex items-center justify-between px-3 py-2 bg-gray-100 dark:bg-gray-800 border-b border-gray-200 dark:border-gray-700">
      <div className="flex items-center space-x-2">
        <div className="flex items-center space-x-1">
          {statusIcon}
          <span className="text-sm font-medium capitalize text-gray-700 dark:text-gray-300">
            {connectionStatus}
          </span>
        </div>
      </div>
      
      <div className="flex items-center space-x-1">
        <Button
          variant="ghost"
          size="sm"
          onClick={onCopy}
          title="Copy selection (Ctrl+C)"
        >
          <Copy className="w-4 h-4" />
        </Button>
        
        <Button
          variant="ghost"
          size="sm"
          onClick={onPaste}
          title="Paste (Ctrl+V)"
        >
          <Clipboard className="w-4 h-4" />
        </Button>
        
        <div className="w-px h-6 bg-gray-300 dark:bg-gray-600 mx-1" />
        
        <Button
          variant="ghost"
          size="sm"
          onClick={onSearch}
          title="Search (Ctrl+F)"
        >
          <Search className="w-4 h-4" />
        </Button>
        
        <Button
          variant="ghost"
          size="sm"
          onClick={onClear}
          title="Clear terminal"
        >
          <RotateCcw className="w-4 h-4" />
        </Button>
        
        <Button
          variant="ghost"
          size="sm"
          onClick={onFit}
          title="Fit to window"
        >
          <Maximize2 className="w-4 h-4" />
        </Button>
        
        <div className="w-px h-6 bg-gray-300 dark:bg-gray-600 mx-1" />
        
        <Dropdown
          trigger={
            <Button variant="ghost" size="sm" title="Terminal settings">
              <Settings className="w-4 h-4" />
            </Button>
          }
        >
          <div className="p-2 min-w-[150px]">
            <label className="block text-sm font-medium mb-2">Theme</label>
            <select
              value={currentTheme}
              onChange={(e) => onThemeChange(e.target.value as TerminalTheme)}
              className="w-full p-1 border rounded text-sm"
            >
              {themeOptions.map(option => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          </div>
        </Dropdown>
      </div>
    </div>
  );
};
```

### 4. Terminal Themes
**Location**: `frontend/src/components/Terminal/`

#### Theme Configuration
```typescript
// frontend/src/components/Terminal/themes.ts
export type TerminalTheme = 'dark' | 'light' | 'high-contrast' | 'solarized-dark' | 'solarized-light';

interface ITerminalThemeConfig {
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

const themes: Record<TerminalTheme, ITerminalThemeConfig> = {
  dark: {
    background: '#1e1e1e',
    foreground: '#ffffff',
    cursor: '#ffffff',
    cursorAccent: '#000000',
    selection: '#ffffff40',
    black: '#000000',
    red: '#cd3131',
    green: '#0dbc79',
    yellow: '#e5e510',
    blue: '#2472c8',
    magenta: '#bc3fbc',
    cyan: '#11a8cd',
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
    foreground: '#000000',
    cursor: '#000000',
    cursorAccent: '#ffffff',
    selection: '#00000040',
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
    brightGreen: '#14ce14',
    brightYellow: '#b5ba00',
    brightBlue: '#0451a5',
    brightMagenta: '#bc05bc',
    brightCyan: '#0598bc',
    brightWhite: '#a5a5a5'
  },
  
  'high-contrast': {
    background: '#000000',
    foreground: '#ffffff',
    cursor: '#ffffff',
    cursorAccent: '#000000',
    selection: '#ffffff80',
    black: '#000000',
    red: '#ff0000',
    green: '#00ff00',
    yellow: '#ffff00',
    blue: '#0000ff',
    magenta: '#ff00ff',
    cyan: '#00ffff',
    white: '#ffffff',
    brightBlack: '#808080',
    brightRed: '#ff8080',
    brightGreen: '#80ff80',
    brightYellow: '#ffff80',
    brightBlue: '#8080ff',
    brightMagenta: '#ff80ff',
    brightCyan: '#80ffff',
    brightWhite: '#ffffff'
  },
  
  'solarized-dark': {
    background: '#002b36',
    foreground: '#839496',
    cursor: '#93a1a1',
    cursorAccent: '#002b36',
    selection: '#073642',
    black: '#073642',
    red: '#dc322f',
    green: '#859900',
    yellow: '#b58900',
    blue: '#268bd2',
    magenta: '#d33682',
    cyan: '#2aa198',
    white: '#eee8d5',
    brightBlack: '#002b36',
    brightRed: '#cb4b16',
    brightGreen: '#586e75',
    brightYellow: '#657b83',
    brightBlue: '#839496',
    brightMagenta: '#6c71c4',
    brightCyan: '#93a1a1',
    brightWhite: '#fdf6e3'
  },
  
  'solarized-light': {
    background: '#fdf6e3',
    foreground: '#657b83',
    cursor: '#586e75',
    cursorAccent: '#fdf6e3',
    selection: '#eee8d5',
    black: '#073642',
    red: '#dc322f',
    green: '#859900',
    yellow: '#b58900',
    blue: '#268bd2',
    magenta: '#d33682',
    cyan: '#2aa198',
    white: '#eee8d5',
    brightBlack: '#002b36',
    brightRed: '#cb4b16',
    brightGreen: '#586e75',
    brightYellow: '#657b83',
    brightBlue: '#839496',
    brightMagenta: '#6c71c4',
    brightCyan: '#93a1a1',
    brightWhite: '#fdf6e3'
  }
};

export function getTerminalTheme(theme: TerminalTheme): ITerminalThemeConfig {
  return themes[theme];
}
```

## Critical Decisions

### Terminal Library Choice
- **Decision**: Use xterm.js as the primary terminal emulator
- **Rationale**: Industry standard, excellent performance, comprehensive feature set
- **Addons**: Fit, Search, WebLinks, WebGL/Canvas for performance

### WebSocket Message Protocol
- **Decision**: JSON-based messaging with binary data support
- **Rationale**: Easier debugging while supporting efficient binary terminal data
- **Compression**: Client-side message compression for large data transfers

### Session Integration Strategy
- **Decision**: Automatic session recovery on reconnection
- **Rationale**: Seamless user experience during network interruptions
- **Implementation**: Session ID persistence and buffer replay

### Theme System
- **Decision**: Multiple built-in themes with runtime switching
- **Rationale**: User preference support and accessibility requirements
- **Themes**: Dark, Light, High-contrast, Solarized variants

## Security Checklist ✅

### Frontend Security
- [ ] XSS prevention in terminal output rendering
- [ ] Content Security Policy (CSP) headers implemented
- [ ] Secure WebSocket connection (WSS) in production
- [ ] Authentication token secure storage (httpOnly cookies)
- [ ] Input sanitization for terminal data
- [ ] Clipboard access security validation
- [ ] URL validation for web links in terminal
- [ ] Secure handling of pasted content
- [ ] Protection against terminal injection attacks
- [ ] Safe rendering of ANSI escape sequences

### WebSocket Security
- [ ] Authentication required for WebSocket upgrade
- [ ] Token validation on connection and messages
- [ ] Origin validation to prevent CSRF
- [ ] Message size limits enforced
- [ ] Rate limiting on client messages
- [ ] Secure reconnection handling
- [ ] Protection against WebSocket hijacking
- [ ] Encrypted message transmission
- [ ] Session binding validation
- [ ] Audit logging of WebSocket events

### Data Security
- [ ] Terminal data encryption in transmission
- [ ] Secure session token handling
- [ ] No sensitive data in client-side storage
- [ ] Secure clipboard operations
- [ ] Protection against data exfiltration
- [ ] Safe handling of terminal history
- [ ] Secure search functionality
- [ ] Protection against keylogging
- [ ] Safe theme and preference storage
- [ ] Secure error message handling

### Client-Side Security
- [ ] Dependency vulnerability scanning
- [ ] Secure build process
- [ ] Subresource integrity validation
- [ ] Secure service worker implementation
- [ ] Protection against code injection
- [ ] Safe event handler implementation
- [ ] Secure local storage usage
- [ ] Protection against timing attacks
- [ ] Safe DOM manipulation
- [ ] Secure third-party library usage

## Testing Requirements

### Component Testing
- [ ] Terminal component rendering and initialization
- [ ] WebSocket connection management
- [ ] Theme switching functionality
- [ ] Toolbar actions and controls
- [ ] Context menu operations
- [ ] Search functionality
- [ ] Keyboard shortcut handling
- [ ] Copy/paste operations

### Integration Testing
- [ ] WebSocket message handling
- [ ] Session creation and recovery
- [ ] Terminal data transmission
- [ ] Resize event handling
- [ ] Connection failure scenarios
- [ ] Reconnection behavior
- [ ] Authentication integration
- [ ] Error handling and display

### Cross-Browser Testing
- [ ] Chrome/Chromium compatibility
- [ ] Firefox compatibility
- [ ] Safari compatibility
- [ ] Edge compatibility
- [ ] Mobile browser testing
- [ ] WebSocket support validation
- [ ] Clipboard API availability
- [ ] Performance across browsers

### Accessibility Testing
- [ ] Screen reader compatibility
- [ ] Keyboard navigation support
- [ ] High contrast theme functionality
- [ ] Font size scaling
- [ ] Color accessibility validation
- [ ] Focus management
- [ ] ARIA labels and roles
- [ ] Voice control compatibility

## Performance Targets

### Rendering Performance
- Terminal initialization < 500ms
- Character rendering latency < 16ms (60fps)
- Large output handling (>10k lines) smoothly
- Theme switching < 100ms
- Search operations < 200ms for 10k lines
- Memory usage < 100MB for active session

### Network Performance
- WebSocket connection establishment < 2 seconds
- Message round-trip latency < 50ms
- Reconnection time < 5 seconds
- Large paste operations handled efficiently
- Binary data transmission optimized
- Connection stability under poor network conditions

### User Experience
- Responsive UI interactions < 100ms
- Smooth scrolling at all buffer sizes
- Efficient clipboard operations
- Fast search with highlighting
- Smooth resize operations
- No blocking UI operations

## Monitoring & Alerting

### Performance Metrics
- Terminal rendering frame rates
- WebSocket connection success rates
- Message transmission latency
- Memory usage per session
- CPU usage during active sessions
- Network bandwidth utilization

### User Experience Metrics
- Session creation success rates
- Connection failure frequencies
- User interaction response times
- Error rates by operation type
- Feature usage analytics
- Browser compatibility metrics

### Error Tracking
- WebSocket connection errors
- Terminal rendering errors
- Authentication failures
- Clipboard operation failures
- Search functionality errors
- Theme switching issues

### Alert Conditions
- WebSocket connection failure rate > 5%
- Terminal rendering errors > 1%
- Session creation failure rate > 2%
- Average connection latency > 200ms
- Memory usage > 200MB per session
- Browser compatibility issues

## Documentation Deliverables

### Technical Documentation
- [ ] Terminal component API documentation
- [ ] WebSocket protocol specification
- [ ] Theme customization guide
- [ ] Integration guide for embedding
- [ ] Performance optimization guide
- [ ] Accessibility implementation guide

### User Documentation
- [ ] Terminal usage guide
- [ ] Keyboard shortcuts reference
- [ ] Theme selection guide
- [ ] Troubleshooting common issues
- [ ] Browser compatibility information
- [ ] Mobile usage guidelines

## Next Steps

Upon successful completion of Session 8:
1. Frontend terminal interface fully operational with xterm.js
2. WebSocket client providing reliable communication
3. Session management integration working seamlessly
4. Multiple themes available with runtime switching
5. Comprehensive terminal features implemented
6. Performance targets met across all supported browsers
7. Security measures fully implemented and tested
8. Proceed to Session 9: Git Integration with Soft-serve

## Risk Mitigation

### Technical Risks
1. **Browser compatibility**: Extensive testing, graceful degradation
2. **Performance issues**: Profiling, optimization, lazy loading
3. **WebSocket instability**: Robust reconnection, error handling
4. **Memory leaks**: Proper cleanup, monitoring, limits
5. **Rendering issues**: Fallback renderers, error boundaries

### Security Risks
1. **XSS attacks**: Output sanitization, CSP headers
2. **Data leakage**: Secure transmission, input validation
3. **Session hijacking**: Token validation, secure cookies
4. **Clipboard abuse**: Permission validation, user consent
5. **Terminal injection**: Input filtering, escape sequence validation

---

**Session 8 Success Criteria:**
- Frontend terminal interface fully functional with professional UX
- xterm.js integration providing native terminal experience
- WebSocket client enabling reliable real-time communication
- Session management integration allowing seamless recovery
- Multiple theme support with accessibility compliance
- Security checklist 100% complete with comprehensive protection
- Performance targets achieved across all supported browsers
- Cross-browser compatibility validated and documented
- Integration with Sessions 1-7 working seamlessly
- All tests passing with >80% coverage including security tests
- Documentation complete with user and technical guides
- Ready for Session 9 Git integration implementation