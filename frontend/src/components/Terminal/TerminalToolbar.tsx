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
  ] as const;

  return (
    <div className="terminal-toolbar flex items-center justify-between px-3 py-2 bg-muted/50 border-b border-border">
      <div className="flex items-center space-x-2">
        <div className="flex items-center space-x-1">
          {statusIcon}
          <span className="text-sm font-medium capitalize text-muted-foreground">
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
        
        <div className="w-px h-6 bg-border mx-1" />
        
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
        
        <div className="w-px h-6 bg-border mx-1" />
        
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
              className="w-full p-1 border rounded text-sm bg-background"
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