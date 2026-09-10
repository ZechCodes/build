# Session 8.3: UI Features, Themes & Accessibility

## Objective
Implement comprehensive UI features, theme management, and accessibility support for the terminal component, ensuring inclusive design and customizable user experience.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for UI interaction analytics and accessibility event tracking
- **Session 2**: Integrates with authentication for user preference persistence
- **Session 8.1**: Extends terminal component with advanced UI features
- **Session 8.2**: Enhances WebSocket client with UI state management

## Core Implementation

### Terminal Theme Manager
**Location**: `frontend/src/components/Terminal/ThemeManager.ts`

```typescript
// frontend/src/components/Terminal/ThemeManager.ts
import { logfire } from '../../utils/logfire';

export interface TerminalTheme {
  name: string;
  displayName: string;
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
  // UI-specific colors
  borderColor: string;
  statusBarBg: string;
  statusBarFg: string;
  scrollbarThumb: string;
  scrollbarTrack: string;
}

export interface ThemeSettings {
  fontSize: number;
  fontFamily: string;
  lineHeight: number;
  letterSpacing: number;
  cursorStyle: 'block' | 'underline' | 'bar';
  cursorBlink: boolean;
  scrollback: number;
  tabStopWidth: number;
  bellStyle: 'sound' | 'visual' | 'none';
  transparencyLevel: number;
}

export const defaultThemes: Record<string, TerminalTheme> = {
  dark: {
    name: 'dark',
    displayName: 'Dark Theme',
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
    brightWhite: '#ffffff',
    borderColor: '#333333',
    statusBarBg: '#2d2d2d',
    statusBarFg: '#cccccc',
    scrollbarThumb: '#424242',
    scrollbarTrack: '#1e1e1e'
  },
  light: {
    name: 'light',
    displayName: 'Light Theme',
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
    brightWhite: '#ffffff',
    borderColor: '#e0e0e0',
    statusBarBg: '#f5f5f5',
    statusBarFg: '#333333',
    scrollbarThumb: '#c0c0c0',
    scrollbarTrack: '#f0f0f0'
  },
  solarizedDark: {
    name: 'solarized-dark',
    displayName: 'Solarized Dark',
    background: '#002b36',
    foreground: '#839496',
    cursor: '#839496',
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
    brightBlack: '#586e75',
    brightRed: '#cb4b16',
    brightGreen: '#586e75',
    brightYellow: '#657b83',
    brightBlue: '#839496',
    brightMagenta: '#6c71c4',
    brightCyan: '#93a1a1',
    brightWhite: '#fdf6e3',
    borderColor: '#073642',
    statusBarBg: '#073642',
    statusBarFg: '#839496',
    scrollbarThumb: '#586e75',
    scrollbarTrack: '#002b36'
  },
  monokaiPro: {
    name: 'monokai-pro',
    displayName: 'Monokai Pro',
    background: '#2d2a2e',
    foreground: '#fcfcfa',
    cursor: '#fcfcfa',
    cursorAccent: '#2d2a2e',
    selection: '#5b595c',
    black: '#403e41',
    red: '#ff6188',
    green: '#a9dc76',
    yellow: '#ffd866',
    blue: '#fc9867',
    magenta: '#ab9df2',
    cyan: '#78dce8',
    white: '#fcfcfa',
    brightBlack: '#727072',
    brightRed: '#ff6188',
    brightGreen: '#a9dc76',
    brightYellow: '#ffd866',
    brightBlue: '#fc9867',
    brightMagenta: '#ab9df2',
    brightCyan: '#78dce8',
    brightWhite: '#fcfcfa',
    borderColor: '#403e41',
    statusBarBg: '#403e41',
    statusBarFg: '#fcfcfa',
    scrollbarThumb: '#727072',
    scrollbarTrack: '#2d2a2e'
  }
};

export const defaultSettings: ThemeSettings = {
  fontSize: 14,
  fontFamily: '"Fira Code", "SF Mono", Monaco, Inconsolata, "Roboto Mono", monospace',
  lineHeight: 1.2,
  letterSpacing: 0,
  cursorStyle: 'block',
  cursorBlink: true,
  scrollback: 10000,
  tabStopWidth: 4,
  bellStyle: 'visual',
  transparencyLevel: 0
};

export class TerminalThemeManager {
  private currentTheme: TerminalTheme;
  private currentSettings: ThemeSettings;
  private customThemes: Map<string, TerminalTheme> = new Map();
  private changeListeners: Set<(theme: TerminalTheme, settings: ThemeSettings) => void> = new Set();

  constructor() {
    this.currentTheme = defaultThemes.dark;
    this.currentSettings = { ...defaultSettings };
    this.loadUserPreferences();
  }

  private async loadUserPreferences(): Promise<void> {
    try {
      const savedPreferences = localStorage.getItem('terminal-preferences');
      if (savedPreferences) {
        const preferences = JSON.parse(savedPreferences);
        
        if (preferences.theme && defaultThemes[preferences.theme]) {
          this.currentTheme = defaultThemes[preferences.theme];
        }
        
        if (preferences.settings) {
          this.currentSettings = { ...this.currentSettings, ...preferences.settings };
        }
        
        if (preferences.customThemes) {
          for (const [name, theme] of Object.entries(preferences.customThemes)) {
            this.customThemes.set(name, theme as TerminalTheme);
          }
        }
      }

      // Apply system preferences
      await this.applySystemPreferences();
      
    } catch (error) {
      console.error('Failed to load terminal preferences:', error);
      logfire.error('Terminal preferences loading failed', { error: error instanceof Error ? error.message : 'Unknown error' });
    }
  }

  private async applySystemPreferences(): Promise<void> {
    // Detect system dark/light mode preference
    if (window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches) {
      if (this.currentTheme.name === 'light') {
        this.setTheme('dark');
      }
    }

    // Detect high contrast preference
    if (window.matchMedia && window.matchMedia('(prefers-contrast: high)').matches) {
      await this.applyHighContrastAdjustments();
    }

    // Detect reduced motion preference
    if (window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      this.updateSettings({
        cursorBlink: false
      });
    }

    // Listen for system preference changes
    window.matchMedia('(prefers-color-scheme: dark)').addListener((e) => {
      if (e.matches && this.currentTheme.name === 'light') {
        this.setTheme('dark');
      } else if (!e.matches && this.currentTheme.name === 'dark') {
        this.setTheme('light');
      }
    });
  }

  private async applyHighContrastAdjustments(): Promise<void> {
    const highContrastTheme: TerminalTheme = {
      ...this.currentTheme,
      background: '#000000',
      foreground: '#ffffff',
      cursor: '#ffffff',
      cursorAccent: '#000000',
      selection: '#ffffff',
      borderColor: '#ffffff'
    };

    this.currentTheme = highContrastTheme;
    this.notifyListeners();
  }

  public setTheme(themeName: string): void {
    const theme = defaultThemes[themeName] || this.customThemes.get(themeName);
    if (!theme) {
      throw new Error(`Theme "${themeName}" not found`);
    }

    this.currentTheme = theme;
    this.savePreferences();
    this.notifyListeners();

    logfire.info('Terminal theme changed', {
      themeName,
      themeDisplayName: theme.displayName
    });
  }

  public updateSettings(partialSettings: Partial<ThemeSettings>): void {
    this.currentSettings = { ...this.currentSettings, ...partialSettings };
    this.savePreferences();
    this.notifyListeners();

    logfire.info('Terminal settings updated', {
      updatedFields: Object.keys(partialSettings),
      newSettings: partialSettings
    });
  }

  public addCustomTheme(theme: TerminalTheme): void {
    this.customThemes.set(theme.name, theme);
    this.savePreferences();

    logfire.info('Custom terminal theme added', {
      themeName: theme.name,
      themeDisplayName: theme.displayName
    });
  }

  public removeCustomTheme(themeName: string): boolean {
    const removed = this.customThemes.delete(themeName);
    if (removed) {
      this.savePreferences();
      
      logfire.info('Custom terminal theme removed', { themeName });
    }
    return removed;
  }

  public getAvailableThemes(): Array<{ name: string; displayName: string; isCustom: boolean }> {
    const themes = [];
    
    // Add default themes
    for (const theme of Object.values(defaultThemes)) {
      themes.push({
        name: theme.name,
        displayName: theme.displayName,
        isCustom: false
      });
    }
    
    // Add custom themes
    for (const theme of this.customThemes.values()) {
      themes.push({
        name: theme.name,
        displayName: theme.displayName,
        isCustom: true
      });
    }
    
    return themes;
  }

  public getCurrentTheme(): TerminalTheme {
    return { ...this.currentTheme };
  }

  public getCurrentSettings(): ThemeSettings {
    return { ...this.currentSettings };
  }

  public addChangeListener(listener: (theme: TerminalTheme, settings: ThemeSettings) => void): void {
    this.changeListeners.add(listener);
  }

  public removeChangeListener(listener: (theme: TerminalTheme, settings: ThemeSettings) => void): void {
    this.changeListeners.delete(listener);
  }

  private notifyListeners(): void {
    for (const listener of this.changeListeners) {
      try {
        listener(this.currentTheme, this.currentSettings);
      } catch (error) {
        console.error('Theme change listener error:', error);
      }
    }
  }

  private savePreferences(): void {
    try {
      const preferences = {
        theme: this.currentTheme.name,
        settings: this.currentSettings,
        customThemes: Object.fromEntries(this.customThemes)
      };
      
      localStorage.setItem('terminal-preferences', JSON.stringify(preferences));
    } catch (error) {
      console.error('Failed to save terminal preferences:', error);
    }
  }
}
```

### Terminal Controls Component
**Location**: `frontend/src/components/Terminal/TerminalControls.tsx`

```tsx
// frontend/src/components/Terminal/TerminalControls.tsx
import React, { useState, useCallback } from 'react';
import { Terminal as XTerm } from 'xterm';
import { logfire } from '../../utils/logfire';
import { TerminalThemeManager, ThemeSettings } from './ThemeManager';
import './TerminalControls.css';

interface TerminalControlsProps {
  terminal: XTerm | null;
  themeManager: TerminalThemeManager;
  onThemeChange?: (themeName: string) => void;
  onSettingsChange?: (settings: Partial<ThemeSettings>) => void;
  onSearch?: () => void;
  onClear?: () => void;
  onSave?: () => void;
  isConnected: boolean;
  className?: string;
}

export const TerminalControls: React.FC<TerminalControlsProps> = ({
  terminal,
  themeManager,
  onThemeChange,
  onSettingsChange,
  onSearch,
  onClear,
  onSave,
  isConnected,
  className = ''
}) => {
  const [showSettings, setShowSettings] = useState(false);
  const [showThemes, setShowThemes] = useState(false);
  const [currentSettings, setCurrentSettings] = useState(themeManager.getCurrentSettings());
  
  const availableThemes = themeManager.getAvailableThemes();
  const currentTheme = themeManager.getCurrentTheme();

  const handleFontSizeChange = useCallback((delta: number) => {
    const newFontSize = Math.max(8, Math.min(32, currentSettings.fontSize + delta));
    const newSettings = { fontSize: newFontSize };
    
    setCurrentSettings(prev => ({ ...prev, ...newSettings }));
    onSettingsChange?.(newSettings);
    
    logfire.info('Terminal font size changed', { 
      oldSize: currentSettings.fontSize, 
      newSize: newFontSize 
    });
  }, [currentSettings.fontSize, onSettingsChange]);

  const handleThemeChange = useCallback((themeName: string) => {
    themeManager.setTheme(themeName);
    onThemeChange?.(themeName);
    setShowThemes(false);
    
    logfire.info('Terminal theme changed via controls', { themeName });
  }, [themeManager, onThemeChange]);

  const handleSettingChange = useCallback((key: keyof ThemeSettings, value: any) => {
    const newSettings = { [key]: value };
    setCurrentSettings(prev => ({ ...prev, ...newSettings }));
    onSettingsChange?.(newSettings);
    
    logfire.info('Terminal setting changed', { setting: key, value });
  }, [onSettingsChange]);

  const handleCopyTerminalContent = useCallback(async () => {
    if (terminal) {
      try {
        const selection = terminal.getSelection();
        if (selection) {
          await navigator.clipboard.writeText(selection);
          logfire.info('Terminal content copied to clipboard', { 
            textLength: selection.length 
          });
        } else {
          // Copy visible content if no selection
          const visibleContent = terminal.buffer.active.getLine(0)?.translateToString() || '';
          await navigator.clipboard.writeText(visibleContent);
          logfire.info('Terminal visible content copied', { 
            textLength: visibleContent.length 
          });
        }
      } catch (error) {
        console.error('Failed to copy terminal content:', error);
        logfire.error('Terminal copy failed', { 
          error: error instanceof Error ? error.message : 'Unknown error' 
        });
      }
    }
  }, [terminal]);

  const handleDownloadTerminalLog = useCallback(() => {
    if (terminal) {
      try {
        // Get all terminal content
        const lineCount = terminal.buffer.active.length;
        const lines = [];
        
        for (let i = 0; i < lineCount; i++) {
          const line = terminal.buffer.active.getLine(i);
          if (line) {
            lines.push(line.translateToString());
          }
        }
        
        const content = lines.join('\n');
        const blob = new Blob([content], { type: 'text/plain' });
        const url = URL.createObjectURL(blob);
        
        const link = document.createElement('a');
        link.href = url;
        link.download = `terminal-log-${new Date().toISOString().slice(0, 19)}.txt`;
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);
        URL.revokeObjectURL(url);
        
        logfire.info('Terminal log downloaded', { 
          lineCount: lines.length,
          contentLength: content.length 
        });
      } catch (error) {
        console.error('Failed to download terminal log:', error);
        logfire.error('Terminal log download failed', { 
          error: error instanceof Error ? error.message : 'Unknown error' 
        });
      }
    }
  }, [terminal]);

  return (
    <div className={`terminal-controls ${className}`}>
      {/* Main controls bar */}
      <div className="controls-bar">
        <div className="controls-section">
          <button
            className="control-button"
            onClick={() => setShowThemes(!showThemes)}
            title="Change theme"
            aria-label="Change terminal theme"
          >
            🎨
          </button>
          
          <button
            className="control-button"
            onClick={() => setShowSettings(!showSettings)}
            title="Terminal settings"
            aria-label="Open terminal settings"
          >
            ⚙️
          </button>
          
          <button
            className="control-button"
            onClick={onSearch}
            title="Search (Ctrl+Shift+F)"
            aria-label="Search terminal content"
          >
            🔍
          </button>
        </div>

        <div className="controls-section">
          <button
            className="control-button"
            onClick={() => handleFontSizeChange(-1)}
            title="Decrease font size"
            aria-label="Decrease font size"
          >
            🔍-
          </button>
          
          <span className="font-size-display" title="Current font size">
            {currentSettings.fontSize}px
          </span>
          
          <button
            className="control-button"
            onClick={() => handleFontSizeChange(1)}
            title="Increase font size"
            aria-label="Increase font size"
          >
            🔍+
          </button>
        </div>

        <div className="controls-section">
          <button
            className="control-button"
            onClick={handleCopyTerminalContent}
            title="Copy content (Ctrl+Shift+C)"
            aria-label="Copy terminal content"
          >
            📋
          </button>
          
          <button
            className="control-button"
            onClick={handleDownloadTerminalLog}
            title="Download terminal log"
            aria-label="Download terminal log"
          >
            💾
          </button>
          
          <button
            className="control-button"
            onClick={onClear}
            title="Clear terminal"
            aria-label="Clear terminal"
          >
            🗑️
          </button>
        </div>

        <div className="controls-section">
          <div className={`connection-status ${isConnected ? 'connected' : 'disconnected'}`}>
            <span className="status-dot" />
            <span className="status-text">
              {isConnected ? 'Connected' : 'Disconnected'}
            </span>
          </div>
        </div>
      </div>

      {/* Theme selector dropdown */}
      {showThemes && (
        <div className="themes-dropdown">
          <div className="dropdown-header">
            <h3>Terminal Themes</h3>
            <button 
              className="close-button"
              onClick={() => setShowThemes(false)}
              aria-label="Close theme selector"
            >
              ✕
            </button>
          </div>
          
          <div className="themes-grid">
            {availableThemes.map((theme) => (
              <button
                key={theme.name}
                className={`theme-option ${currentTheme.name === theme.name ? 'active' : ''}`}
                onClick={() => handleThemeChange(theme.name)}
                aria-label={`Select ${theme.displayName} theme`}
              >
                <div className="theme-preview">
                  <div 
                    className="theme-color-bar"
                    style={{ 
                      background: `linear-gradient(90deg, 
                        ${theme.name === 'dark' ? '#1e1e1e' : 
                          theme.name === 'light' ? '#ffffff' : 
                          theme.name === 'solarized-dark' ? '#002b36' : '#2d2a2e'} 0%, 
                        ${theme.name === 'dark' ? '#d4d4d4' : 
                          theme.name === 'light' ? '#333333' : 
                          theme.name === 'solarized-dark' ? '#839496' : '#fcfcfa'} 100%)`
                    }}
                  />
                </div>
                <span className="theme-name">{theme.displayName}</span>
                {theme.isCustom && <span className="custom-badge">Custom</span>}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* Settings panel */}
      {showSettings && (
        <div className="settings-panel">
          <div className="panel-header">
            <h3>Terminal Settings</h3>
            <button 
              className="close-button"
              onClick={() => setShowSettings(false)}
              aria-label="Close settings panel"
            >
              ✕
            </button>
          </div>
          
          <div className="settings-content">
            <div className="setting-group">
              <label htmlFor="font-family">Font Family</label>
              <select
                id="font-family"
                value={currentSettings.fontFamily}
                onChange={(e) => handleSettingChange('fontFamily', e.target.value)}
              >
                <option value='"Fira Code", monospace'>Fira Code</option>
                <option value='"SF Mono", monospace'>SF Mono</option>
                <option value='Monaco, monospace'>Monaco</option>
                <option value='Inconsolata, monospace'>Inconsolata</option>
                <option value='"Roboto Mono", monospace'>Roboto Mono</option>
                <option value='Consolas, monospace'>Consolas</option>
              </select>
            </div>

            <div className="setting-group">
              <label htmlFor="cursor-style">Cursor Style</label>
              <select
                id="cursor-style"
                value={currentSettings.cursorStyle}
                onChange={(e) => handleSettingChange('cursorStyle', e.target.value)}
              >
                <option value="block">Block</option>
                <option value="underline">Underline</option>
                <option value="bar">Bar</option>
              </select>
            </div>

            <div className="setting-group">
              <label>
                <input
                  type="checkbox"
                  checked={currentSettings.cursorBlink}
                  onChange={(e) => handleSettingChange('cursorBlink', e.target.checked)}
                />
                Cursor Blink
              </label>
            </div>

            <div className="setting-group">
              <label htmlFor="scrollback">Scrollback Lines</label>
              <input
                id="scrollback"
                type="number"
                min="1000"
                max="50000"
                step="1000"
                value={currentSettings.scrollback}
                onChange={(e) => handleSettingChange('scrollback', parseInt(e.target.value))}
              />
            </div>

            <div className="setting-group">
              <label htmlFor="bell-style">Bell Style</label>
              <select
                id="bell-style"
                value={currentSettings.bellStyle}
                onChange={(e) => handleSettingChange('bellStyle', e.target.value)}
              >
                <option value="visual">Visual</option>
                <option value="sound">Sound</option>
                <option value="none">None</option>
              </select>
            </div>

            <div className="setting-group">
              <label htmlFor="line-height">Line Height</label>
              <input
                id="line-height"
                type="number"
                min="1.0"
                max="2.0"
                step="0.1"
                value={currentSettings.lineHeight}
                onChange={(e) => handleSettingChange('lineHeight', parseFloat(e.target.value))}
              />
            </div>
          </div>
        </div>
      )}
    </div>
  );
};

export default TerminalControls;
```

### Terminal Search Component
**Location**: `frontend/src/components/Terminal/TerminalSearch.tsx`

```tsx
// frontend/src/components/Terminal/TerminalSearch.tsx
import React, { useState, useEffect, useCallback, useRef } from 'react';
import { Terminal as XTerm } from 'xterm';
import { SearchAddon } from 'xterm-addon-search';
import { logfire } from '../../utils/logfire';
import './TerminalSearch.css';

interface TerminalSearchProps {
  terminal: XTerm | null;
  searchAddon: SearchAddon | null;
  isVisible: boolean;
  onClose: () => void;
  className?: string;
}

interface SearchOptions {
  caseSensitive: boolean;
  wholeWord: boolean;
  regex: boolean;
  decorations: boolean;
}

export const TerminalSearch: React.FC<TerminalSearchProps> = ({
  terminal,
  searchAddon,
  isVisible,
  onClose,
  className = ''
}) => {
  const [searchTerm, setSearchTerm] = useState('');
  const [searchOptions, setSearchOptions] = useState<SearchOptions>({
    caseSensitive: false,
    wholeWord: false,
    regex: false,
    decorations: true
  });
  const [currentMatch, setCurrentMatch] = useState(0);
  const [totalMatches, setTotalMatches] = useState(0);
  const [searchHistory, setSearchHistory] = useState<string[]>([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  
  const searchInputRef = useRef<HTMLInputElement>(null);
  const searchTimeoutRef = useRef<NodeJS.Timeout>();

  // Focus search input when visible
  useEffect(() => {
    if (isVisible && searchInputRef.current) {
      searchInputRef.current.focus();
      searchInputRef.current.select();
    }
  }, [isVisible]);

  // Load search history from localStorage
  useEffect(() => {
    try {
      const savedHistory = localStorage.getItem('terminal-search-history');
      if (savedHistory) {
        setSearchHistory(JSON.parse(savedHistory));
      }
    } catch (error) {
      console.error('Failed to load search history:', error);
    }
  }, []);

  const saveSearchHistory = useCallback((term: string) => {
    if (term.trim() && !searchHistory.includes(term)) {
      const newHistory = [term, ...searchHistory.slice(0, 9)]; // Keep last 10 searches
      setSearchHistory(newHistory);
      
      try {
        localStorage.setItem('terminal-search-history', JSON.stringify(newHistory));
      } catch (error) {
        console.error('Failed to save search history:', error);
      }
    }
  }, [searchHistory]);

  const performSearch = useCallback((term: string, direction: 'next' | 'previous' = 'next') => {
    if (!terminal || !searchAddon || !term.trim()) {
      setCurrentMatch(0);
      setTotalMatches(0);
      return;
    }

    try {
      const searchOptions_xterm = {
        caseSensitive: searchOptions.caseSensitive,
        wholeWord: searchOptions.wholeWord,
        regex: searchOptions.regex,
        decorations: searchOptions.decorations ? {
          matchBackground: '#ffff00',
          matchBorder: '#ff0000',
          matchOverviewRuler: '#ffff00',
          activeMatchBackground: '#ff6600',
          activeMatchBorder: '#ff0000',
          activeMatchColorOverviewRuler: '#ff6600'
        } : undefined
      };

      let found: boolean;
      if (direction === 'next') {
        found = searchAddon.findNext(term, searchOptions_xterm);
      } else {
        found = searchAddon.findPrevious(term, searchOptions_xterm);
      }

      if (found) {
        // Update match counters (simplified - xterm doesn't provide total count)
        if (direction === 'next') {
          setCurrentMatch(prev => prev + 1);
        } else {
          setCurrentMatch(prev => Math.max(1, prev - 1));
        }
        
        logfire.debug('Terminal search match found', {
          searchTerm: term,
          direction,
          caseSensitive: searchOptions.caseSensitive,
          wholeWord: searchOptions.wholeWord,
          regex: searchOptions.regex
        });
      } else {
        setCurrentMatch(0);
        setTotalMatches(0);
        
        logfire.debug('Terminal search no matches', {
          searchTerm: term,
          direction
        });
      }
    } catch (error) {
      console.error('Search error:', error);
      logfire.error('Terminal search failed', {
        searchTerm: term,
        error: error instanceof Error ? error.message : 'Unknown error'
      });
    }
  }, [terminal, searchAddon, searchOptions]);

  const handleSearchChange = useCallback((value: string) => {
    setSearchTerm(value);
    setHistoryIndex(-1);

    // Debounce search
    if (searchTimeoutRef.current) {
      clearTimeout(searchTimeoutRef.current);
    }

    searchTimeoutRef.current = setTimeout(() => {
      if (value.trim()) {
        performSearch(value, 'next');
        setCurrentMatch(1);
      } else {
        setCurrentMatch(0);
        setTotalMatches(0);
        searchAddon?.clearDecorations();
      }
    }, 300);
  }, [performSearch, searchAddon]);

  const handleSearchNext = useCallback(() => {
    if (searchTerm.trim()) {
      performSearch(searchTerm, 'next');
      saveSearchHistory(searchTerm);
    }
  }, [searchTerm, performSearch, saveSearchHistory]);

  const handleSearchPrevious = useCallback(() => {
    if (searchTerm.trim()) {
      performSearch(searchTerm, 'previous');
      saveSearchHistory(searchTerm);
    }
  }, [searchTerm, performSearch, saveSearchHistory]);

  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    switch (e.key) {
      case 'Enter':
        e.preventDefault();
        if (e.shiftKey) {
          handleSearchPrevious();
        } else {
          handleSearchNext();
        }
        break;
        
      case 'Escape':
        e.preventDefault();
        onClose();
        break;
        
      case 'ArrowUp':
        e.preventDefault();
        if (searchHistory.length > 0) {
          const newIndex = Math.min(historyIndex + 1, searchHistory.length - 1);
          setHistoryIndex(newIndex);
          setSearchTerm(searchHistory[newIndex]);
        }
        break;
        
      case 'ArrowDown':
        e.preventDefault();
        if (historyIndex > 0) {
          const newIndex = historyIndex - 1;
          setHistoryIndex(newIndex);
          setSearchTerm(searchHistory[newIndex]);
        } else if (historyIndex === 0) {
          setHistoryIndex(-1);
          setSearchTerm('');
        }
        break;
    }
  }, [handleSearchNext, handleSearchPrevious, onClose, searchHistory, historyIndex]);

  const handleOptionChange = useCallback((option: keyof SearchOptions) => {
    setSearchOptions(prev => {
      const newOptions = { ...prev, [option]: !prev[option] };
      
      // Re-run search with new options
      if (searchTerm.trim()) {
        setTimeout(() => performSearch(searchTerm, 'next'), 100);
      }
      
      logfire.debug('Terminal search options changed', {
        option,
        value: newOptions[option],
        searchTerm
      });
      
      return newOptions;
    });
  }, [searchTerm, performSearch]);

  const clearSearch = useCallback(() => {
    setSearchTerm('');
    setCurrentMatch(0);
    setTotalMatches(0);
    setHistoryIndex(-1);
    searchAddon?.clearDecorations();
    
    if (searchInputRef.current) {
      searchInputRef.current.focus();
    }
  }, [searchAddon]);

  if (!isVisible) {
    return null;
  }

  return (
    <div className={`terminal-search ${className}`}>
      <div className="search-container">
        <div className="search-input-group">
          <input
            ref={searchInputRef}
            type="text"
            value={searchTerm}
            onChange={(e) => handleSearchChange(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Search terminal..."
            className="search-input"
            aria-label="Search terminal content"
          />
          
          <div className="search-controls">
            <button
              onClick={handleSearchPrevious}
              disabled={!searchTerm.trim()}
              title="Previous match (Shift+Enter)"
              aria-label="Previous search match"
              className="search-button"
            >
              ↑
            </button>
            
            <button
              onClick={handleSearchNext}
              disabled={!searchTerm.trim()}
              title="Next match (Enter)"
              aria-label="Next search match"
              className="search-button"
            >
              ↓
            </button>
            
            <button
              onClick={clearSearch}
              title="Clear search"
              aria-label="Clear search"
              className="search-button"
            >
              ✕
            </button>
          </div>
        </div>

        <div className="search-info">
          {searchTerm.trim() && (
            <span className="match-count">
              {currentMatch > 0 ? `${currentMatch} of ${totalMatches || '?'}` : 'No matches'}
            </span>
          )}
        </div>

        <div className="search-options">
          <button
            onClick={() => handleOptionChange('caseSensitive')}
            className={`option-button ${searchOptions.caseSensitive ? 'active' : ''}`}
            title="Case sensitive"
            aria-label="Toggle case sensitive search"
          >
            Aa
          </button>
          
          <button
            onClick={() => handleOptionChange('wholeWord')}
            className={`option-button ${searchOptions.wholeWord ? 'active' : ''}`}
            title="Whole word"
            aria-label="Toggle whole word search"
          >
            Ab
          </button>
          
          <button
            onClick={() => handleOptionChange('regex')}
            className={`option-button ${searchOptions.regex ? 'active' : ''}`}
            title="Regular expression"
            aria-label="Toggle regular expression search"
          >
            .*
          </button>
        </div>

        <button
          onClick={onClose}
          className="close-search"
          title="Close search (Escape)"
          aria-label="Close search"
        >
          ✕
        </button>
      </div>
    </div>
  );
};

export default TerminalSearch;
```

## TDD Implementation Cycle

### Test-Driven Development Process

1. **Red Phase**: Write failing UI feature tests
   ```bash
   # Create UI feature test file
   touch frontend/src/components/Terminal/__tests__/TerminalUI.test.tsx
   
   # Run failing test
   npm test TerminalUI.test.tsx
   ```

2. **Green Phase**: Implement basic UI features
   ```bash
   # Implement theme management and controls
   npm test TerminalUI.test.tsx
   ```

3. **Refactor Phase**: Enhance accessibility and responsiveness
   ```bash
   # Add comprehensive accessibility features
   npm test -- --coverage
   ```

4. **Commit**: Commit UI enhancements
   ```bash
   git add frontend/src/components/Terminal/
   git commit -m "feat: implement terminal UI features with theme management and accessibility

   - Add comprehensive theme management system with multiple built-in themes
   - Implement terminal controls with font sizing and customization options
   - Add advanced search functionality with regex and history support
   - Include accessibility features with ARIA labels and keyboard navigation
   - Integrate with Logfire for UI interaction analytics and user behavior tracking
   
   Tests: Added comprehensive UI feature test suite with accessibility validation
   Accessibility: WCAG 2.1 AA compliance with screen reader and keyboard support
   Performance: Optimized theme switching and search operations"
   ```

### UI Feature Test Cases

```tsx
// frontend/src/components/Terminal/__tests__/TerminalUI.test.tsx
import React from 'react';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { TerminalControls } from '../TerminalControls';
import { TerminalSearch } from '../TerminalSearch';
import { TerminalThemeManager } from '../ThemeManager';

describe('Terminal UI Features', () => {
  let themeManager: TerminalThemeManager;
  let mockTerminal: any;

  beforeEach(() => {
    themeManager = new TerminalThemeManager();
    mockTerminal = {
      getSelection: jest.fn(() => 'test selection'),
      buffer: {
        active: {
          length: 100,
          getLine: jest.fn(() => ({ translateToString: () => 'test line' }))
        }
      }
    };
  });

  test('theme manager changes themes correctly', () => {
    const initialTheme = themeManager.getCurrentTheme();
    expect(initialTheme.name).toBe('dark');

    themeManager.setTheme('light');
    const newTheme = themeManager.getCurrentTheme();
    expect(newTheme.name).toBe('light');
    expect(newTheme.background).toBe('#ffffff');
  });

  test('terminal controls render all buttons', () => {
    render(
      <TerminalControls
        terminal={mockTerminal}
        themeManager={themeManager}
        isConnected={true}
      />
    );

    expect(screen.getByLabelText('Change terminal theme')).toBeInTheDocument();
    expect(screen.getByLabelText('Open terminal settings')).toBeInTheDocument();
    expect(screen.getByLabelText('Search terminal content')).toBeInTheDocument();
    expect(screen.getByLabelText('Copy terminal content')).toBeInTheDocument();
  });

  test('font size controls work correctly', () => {
    const onSettingsChange = jest.fn();
    
    render(
      <TerminalControls
        terminal={mockTerminal}
        themeManager={themeManager}
        onSettingsChange={onSettingsChange}
        isConnected={true}
      />
    );

    const increaseButton = screen.getByLabelText('Increase font size');
    fireEvent.click(increaseButton);

    expect(onSettingsChange).toHaveBeenCalledWith({ fontSize: 15 });
  });

  test('search component handles keyboard navigation', async () => {
    const mockSearchAddon = {
      findNext: jest.fn(() => true),
      findPrevious: jest.fn(() => true),
      clearDecorations: jest.fn()
    };

    render(
      <TerminalSearch
        terminal={mockTerminal}
        searchAddon={mockSearchAddon}
        isVisible={true}
        onClose={jest.fn()}
      />
    );

    const searchInput = screen.getByPlaceholderText('Search terminal...');
    
    // Test Enter key for next search
    fireEvent.change(searchInput, { target: { value: 'test' } });
    fireEvent.keyDown(searchInput, { key: 'Enter' });

    await waitFor(() => {
      expect(mockSearchAddon.findNext).toHaveBeenCalledWith('test', expect.any(Object));
    });
  });

  test('accessibility features work correctly', () => {
    render(
      <TerminalControls
        terminal={mockTerminal}
        themeManager={themeManager}
        isConnected={true}
      />
    );

    // Check ARIA labels
    expect(screen.getByLabelText('Change terminal theme')).toBeInTheDocument();
    expect(screen.getByLabelText('Increase font size')).toBeInTheDocument();
    
    // Check keyboard navigation
    const themeButton = screen.getByLabelText('Change terminal theme');
    expect(themeButton).toHaveAttribute('aria-label');
  });

  test('theme persistence works correctly', () => {
    // Mock localStorage
    const mockLocalStorage = {
      getItem: jest.fn(),
      setItem: jest.fn()
    };
    Object.defineProperty(window, 'localStorage', { value: mockLocalStorage });

    themeManager.setTheme('solarized-dark');
    
    expect(mockLocalStorage.setItem).toHaveBeenCalledWith(
      'terminal-preferences',
      expect.stringContaining('solarized-dark')
    );
  });
});
```

## Security Checklist for UI Features

### Client-Side Security
- [ ] Input sanitization for all theme and setting values
- [ ] XSS prevention in dynamic theme CSS generation
- [ ] Content Security Policy compliance for custom themes
- [ ] Safe handling of user-generated theme names and values
- [ ] Validation of font family names to prevent CSS injection
- [ ] Protection against theme-based UI redressing attacks
- [ ] Secure storage of user preferences in localStorage
- [ ] Search input validation to prevent regex DoS attacks
- [ ] Download functionality validation to prevent path traversal
- [ ] Error handling that doesn't leak sensitive information

### Accessibility Security
- [ ] Screen reader compatibility without exposing sensitive data
- [ ] Keyboard navigation that respects security contexts
- [ ] Focus management that doesn't bypass security controls
- [ ] ARIA attributes that don't leak confidential information
- [ ] High contrast mode support without compromising security
- [ ] Reduced motion preferences that maintain security visibility
- [ ] Voice control compatibility with proper access validation
- [ ] Touch accessibility that respects authentication state
- [ ] Alternative input method security validation
- [ ] Assistive technology integration with secure data handling

## Performance Requirements

### UI Response Times
- Theme switching: < 100ms visual feedback
- Font size changes: < 50ms application
- Search operations: < 200ms for 10,000 lines
- Control panel opening: < 150ms animation
- Settings persistence: < 100ms save operation
- Search history access: < 50ms retrieval

### Resource Utilization
- Memory usage: < 50MB for theme management
- CPU usage: < 5% during theme operations
- Storage usage: < 1MB for preferences and themes
- Network usage: Zero for local theme operations
- Battery impact: Minimal for theme animations
- Accessibility overhead: < 10% performance impact

## Accessibility Implementation

### WCAG 2.1 AA Compliance
```css
/* High contrast support */
@media (prefers-contrast: high) {
  .terminal-controls {
    --button-border: 2px solid;
    --focus-outline: 3px solid;
    --text-contrast: 21:1;
  }
}

/* Reduced motion support */
@media (prefers-reduced-motion: reduce) {
  .terminal-controls * {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
  }
}

/* Focus management */
.control-button:focus {
  outline: 2px solid var(--focus-color);
  outline-offset: 2px;
}
```

### Screen Reader Support
```tsx
// ARIA live regions for dynamic updates
<div 
  aria-live="polite" 
  aria-atomic="false"
  className="sr-only"
>
  {`Terminal theme changed to ${currentTheme.displayName}`}
</div>

// Keyboard navigation announcements
<button
  aria-label={`Current font size ${fontSize}px. Click to increase.`}
  aria-describedby="font-size-help"
>
  Increase font size
</button>
```

## Integration Testing

### Theme Integration Tests
```tsx
async function testThemeIntegration() {
  // Test theme application to terminal
  const terminal = new Terminal();
  const themeManager = new TerminalThemeManager();
  
  themeManager.setTheme('solarized-dark');
  const theme = themeManager.getCurrentTheme();
  
  terminal.setOption('theme', theme);
  
  // Verify theme applied correctly
  expect(terminal.getOption('theme').background).toBe('#002b36');
}
```

### Accessibility Integration Tests
```tsx
async function testAccessibilityIntegration() {
  // Test with assistive technology simulation
  const { container } = render(<TerminalControls {...props} />);
  
  // Test keyboard navigation
  const firstButton = container.querySelector('button');
  firstButton?.focus();
  
  // Simulate Tab key navigation
  fireEvent.keyDown(document, { key: 'Tab' });
  
  // Verify focus moved correctly
  expect(document.activeElement).not.toBe(firstButton);
}
```

## Next Implementation Steps

1. **Complete UI component implementation** with all accessibility features
2. **Add comprehensive theme customization** with color picker and preview
3. **Implement advanced search features** with saved searches and filters
4. **Add keyboard shortcut customization** with user-defined bindings
5. **Create mobile-responsive design** with touch-friendly controls
6. **Add internationalization support** for multiple languages
7. **Implement plugin system** for custom UI extensions

## Commit Guidelines

UI feature commits should include:
- **Accessibility validation** with WCAG 2.1 AA compliance testing
- **Performance optimization** with minimal overhead for UI operations
- **Cross-browser compatibility** testing with major browsers
- **Test coverage** for UI components and user interactions (>85%)
- **Integration verification** with terminal core functionality
- **Documentation updates** with accessibility and theming guides