import { describe, it, expect } from 'vitest';
import { getTerminalTheme, type TerminalTheme } from '../themes';

describe('Terminal Themes', () => {
  const themeNames: TerminalTheme[] = ['dark', 'light', 'high-contrast', 'solarized-dark', 'solarized-light'];

  it('should return valid theme configuration for all supported themes', () => {
    themeNames.forEach(themeName => {
      const theme = getTerminalTheme(themeName);
      
      expect(theme).toBeDefined();
      expect(theme.background).toBeTruthy();
      expect(theme.foreground).toBeTruthy();
      expect(theme.cursor).toBeTruthy();
      expect(theme.selection).toBeTruthy();
    });
  });

  it('should have different colors for different themes', () => {
    const darkTheme = getTerminalTheme('dark');
    const lightTheme = getTerminalTheme('light');
    
    expect(darkTheme.background).not.toBe(lightTheme.background);
    expect(darkTheme.foreground).not.toBe(lightTheme.foreground);
  });

  it('should have proper contrast for high-contrast theme', () => {
    const highContrastTheme = getTerminalTheme('high-contrast');
    
    expect(highContrastTheme.background).toBe('#000000');
    expect(highContrastTheme.foreground).toBe('#ffffff');
  });

  it('should have all required color properties', () => {
    const theme = getTerminalTheme('dark');
    
    const requiredProperties = [
      'background', 'foreground', 'cursor', 'cursorAccent', 'selection',
      'black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white',
      'brightBlack', 'brightRed', 'brightGreen', 'brightYellow', 
      'brightBlue', 'brightMagenta', 'brightCyan', 'brightWhite'
    ];

    requiredProperties.forEach(prop => {
      expect(theme).toHaveProperty(prop);
      expect(typeof theme[prop as keyof typeof theme]).toBe('string');
    });
  });

  it('should have valid hex color format', () => {
    const theme = getTerminalTheme('dark');
    const hexColorRegex = /^#[0-9A-Fa-f]{6}([0-9A-Fa-f]{2})?$/;
    
    Object.values(theme).forEach(color => {
      if (typeof color === 'string' && color.startsWith('#')) {
        expect(color).toMatch(hexColorRegex);
      }
    });
  });
});