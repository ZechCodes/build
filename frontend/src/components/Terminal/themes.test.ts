import { describe, it, expect } from 'vitest';
import { getTerminalTheme, TerminalTheme } from './themes';

describe('Terminal Themes', () => {
  describe('getTerminalTheme', () => {
    it('should return dark theme configuration', () => {
      const theme = getTerminalTheme('dark');
      
      expect(theme).toEqual({
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
      });
    });

    it('should return light theme configuration', () => {
      const theme = getTerminalTheme('light');
      
      expect(theme).toEqual({
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
      });
    });

    it('should return high-contrast theme configuration', () => {
      const theme = getTerminalTheme('high-contrast');
      
      expect(theme).toEqual({
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
      });
    });

    it('should return solarized-dark theme configuration', () => {
      const theme = getTerminalTheme('solarized-dark');
      
      expect(theme).toEqual({
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
      });
    });

    it('should return solarized-light theme configuration', () => {
      const theme = getTerminalTheme('solarized-light');
      
      expect(theme).toEqual({
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
      });
    });

    it('should default to dark theme for unknown theme names', () => {
      const theme = getTerminalTheme('unknown-theme' as TerminalTheme);
      const darkTheme = getTerminalTheme('dark');
      
      expect(theme).toEqual(darkTheme);
    });

    it('should handle undefined theme input gracefully', () => {
      const theme = getTerminalTheme(undefined as any);
      // This may return undefined depending on implementation
      expect(theme).toBeDefined();
    });
  });

  describe('theme properties', () => {
    const allThemes: TerminalTheme[] = ['dark', 'light', 'high-contrast', 'solarized-dark', 'solarized-light'];

    allThemes.forEach(themeName => {
      describe(`${themeName} theme`, () => {
        const theme = getTerminalTheme(themeName);

        it('should have all required color properties', () => {
          const requiredProperties = [
            'background',
            'foreground',
            'cursor',
            'cursorAccent',
            'selection',
            'black',
            'red',
            'green',
            'yellow',
            'blue',
            'magenta',
            'cyan',
            'white',
            'brightBlack',
            'brightRed',
            'brightGreen',
            'brightYellow',
            'brightBlue',
            'brightMagenta',
            'brightCyan',
            'brightWhite'
          ];

          requiredProperties.forEach(prop => {
            expect(theme).toHaveProperty(prop);
            expect(typeof theme[prop as keyof typeof theme]).toBe('string');
          });
        });

        it('should have valid color values', () => {
          Object.values(theme).forEach(color => {
            // Accept hex colors with optional alpha channel
            expect(color).toMatch(/^#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?$/);
          });
        });

        it('should have different background and foreground colors', () => {
          expect(theme.background).not.toBe(theme.foreground);
        });

        it('should have cursor accent different from cursor', () => {
          expect(theme.cursor).not.toBe(theme.cursorAccent);
        });
      });
    });
  });

  describe('theme contrast and accessibility', () => {
    // Helper function to calculate relative luminance
    const getLuminance = (hex: string): number => {
      // Remove alpha channel if present
      const cleanHex = hex.length > 7 ? hex.substring(0, 7) : hex;
      const rgb = cleanHex.substring(1).match(/.{2}/g)?.map(x => parseInt(x, 16)) || [0, 0, 0];
      const [r, g, b] = rgb.map(c => {
        c = c / 255;
        return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
      });
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };

    // Helper function to calculate contrast ratio
    const getContrastRatio = (color1: string, color2: string): number => {
      const lum1 = getLuminance(color1);
      const lum2 = getLuminance(color2);
      const brightest = Math.max(lum1, lum2);
      const darkest = Math.min(lum1, lum2);
      return (brightest + 0.05) / (darkest + 0.05);
    };

    it('should have adequate contrast between background and foreground (dark theme)', () => {
      const theme = getTerminalTheme('dark');
      const contrast = getContrastRatio(theme.background, theme.foreground);
      
      // WCAG AA standard requires at least 4.5:1 for normal text
      expect(contrast).toBeGreaterThan(4.5);
    });

    it('should have adequate contrast between background and foreground (light theme)', () => {
      const theme = getTerminalTheme('light');
      const contrast = getContrastRatio(theme.background, theme.foreground);
      
      // WCAG AA standard requires at least 4.5:1 for normal text
      expect(contrast).toBeGreaterThan(4.5);
    });

    it('should have high contrast between background and foreground (high-contrast theme)', () => {
      const theme = getTerminalTheme('high-contrast');
      const contrast = getContrastRatio(theme.background, theme.foreground);
      
      // High contrast theme should exceed WCAG AAA standard (7:1)
      expect(contrast).toBeGreaterThan(7);
    });

    it('should have good cursor visibility (dark theme)', () => {
      const theme = getTerminalTheme('dark');
      const cursorContrast = getContrastRatio(theme.background, theme.cursor);
      
      expect(cursorContrast).toBeGreaterThan(3); // Minimum for UI elements
    });

    it('should have good cursor visibility (light theme)', () => {
      const theme = getTerminalTheme('light');
      const cursorContrast = getContrastRatio(theme.background, theme.cursor);
      
      expect(cursorContrast).toBeGreaterThan(3); // Minimum for UI elements
    });
  });

  describe('theme color relationships', () => {
    it('should have bright colors that are distinct from normal colors', () => {
      const theme = getTerminalTheme('dark');
      
      // Check that bright colors are different from their normal counterparts
      expect(theme.brightRed).not.toBe(theme.red);
      expect(theme.brightGreen).not.toBe(theme.green);
      expect(theme.brightBlue).not.toBe(theme.blue);
      expect(theme.brightYellow).not.toBe(theme.yellow);
      expect(theme.brightMagenta).not.toBe(theme.magenta);
      expect(theme.brightCyan).not.toBe(theme.cyan);
      expect(theme.brightWhite).not.toBe(theme.white);
      expect(theme.brightBlack).not.toBe(theme.black);
    });

    it('should have consistent color families across themes', () => {
      const darkTheme = getTerminalTheme('dark');
      const lightTheme = getTerminalTheme('light');
      
      // Both themes should have the same structure
      expect(Object.keys(darkTheme).sort()).toEqual(Object.keys(lightTheme).sort());
    });
  });
});