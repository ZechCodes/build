import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  sanitizeHtml,
  sanitizeTerminalText,
  validateWebSocketMessage,
  sanitizeClipboardContent,
  detectSensitiveData,
  maskSensitiveData,
  sanitizeSearchQuery,
  rateLimiter,
  logSecurityEvent,
  enforceCSP,
  initializeSecurity
} from './security';

describe('Security Utils', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Mock console methods
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    vi.spyOn(console, 'log').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe('sanitizeHtml', () => {
    it('should remove dangerous script tags', () => {
      const input = '<div>Safe content</div><script>alert("xss")</script>';
      const result = sanitizeHtml(input);
      expect(result).toBe('<div>Safe content</div>');
      expect(result).not.toContain('script');
    });

    it('should preserve allowed HTML tags', () => {
      const input = '<b>Bold</b> <i>italic</i> <a href="https://example.com">link</a>';
      const result = sanitizeHtml(input);
      expect(result).toContain('<b>Bold</b>');
      expect(result).toContain('<i>italic</i>');
      expect(result).toContain('<a href="https://example.com">link</a>');
    });

    it('should remove javascript: URLs', () => {
      const input = '<a href="javascript:alert(1)">Bad link</a>';
      const result = sanitizeHtml(input);
      expect(result).not.toContain('javascript:');
    });
  });

  describe('sanitizeTerminalText', () => {
    it('should preserve ANSI escape sequences', () => {
      const input = '\x1b[31mRed text\x1b[0m';
      const result = sanitizeTerminalText(input);
      expect(result).toBe('\x1b[31mRed text\x1b[0m');
    });

    it('should remove dangerous control characters', () => {
      const input = 'Normal text\x00\x08\x0C\x7F';
      const result = sanitizeTerminalText(input);
      expect(result).toBe('Normal text');
    });

    it('should remove script tags', () => {
      const input = 'Text <script>alert("xss")</script> more text';
      const result = sanitizeTerminalText(input);
      expect(result).toBe('Text  more text');
    });

    it('should remove javascript: URLs', () => {
      const input = 'Click javascript:alert(1) here';
      const result = sanitizeTerminalText(input);
      expect(result).toBe('Click alert(1) here');
    });

    it('should remove event handlers', () => {
      const input = 'Text onclick=alert(1) more text';
      const result = sanitizeTerminalText(input);
      expect(result).toBe('Text alert(1) more text');
    });

    it('should preserve ANSI while removing dangerous content', () => {
      const input = '\x1b[31mRed javascript:alert(1) text\x1b[0m';
      const result = sanitizeTerminalText(input);
      expect(result).toBe('\x1b[31mRed alert(1) text\x1b[0m');
    });
  });

  describe('validateWebSocketMessage', () => {
    it('should accept valid terminal_data message', () => {
      const message = {
        type: 'terminal_data',
        data: 'Hello world',
        session_id: 'session-123'
      };
      expect(validateWebSocketMessage(message)).toBe(true);
    });

    it('should accept valid terminal_resize message', () => {
      const message = {
        type: 'terminal_resize',
        data: { cols: 80, rows: 24 }
      };
      expect(validateWebSocketMessage(message)).toBe(true);
    });

    it('should reject message without type', () => {
      const message = { data: 'test' };
      expect(validateWebSocketMessage(message)).toBe(false);
    });

    it('should reject message with invalid type', () => {
      const message = { type: 'invalid_type', data: 'test' };
      expect(validateWebSocketMessage(message)).toBe(false);
    });

    it('should reject terminal_data with non-string data', () => {
      const message = { type: 'terminal_data', data: 123 };
      expect(validateWebSocketMessage(message)).toBe(false);
    });

    it('should reject terminal_resize with invalid dimensions', () => {
      const message = { type: 'terminal_resize', data: { cols: -1, rows: 24 } };
      expect(validateWebSocketMessage(message)).toBe(false);
    });

    it('should reject terminal_resize with too large dimensions', () => {
      const message = { type: 'terminal_resize', data: { cols: 2000, rows: 24 } };
      expect(validateWebSocketMessage(message)).toBe(false);
    });

    it('should reject non-object messages', () => {
      expect(validateWebSocketMessage('string')).toBe(false);
      expect(validateWebSocketMessage(null)).toBe(false);
      expect(validateWebSocketMessage(undefined)).toBe(false);
    });
  });

  describe('sanitizeClipboardContent', () => {
    it('should remove control characters', () => {
      const input = 'Text\x00\x08\x0C\x7Fmore text';
      const result = sanitizeClipboardContent(input);
      expect(result).toBe('Textmore text');
    });

    it('should normalize line endings', () => {
      const input = 'Line 1\r\nLine 2\rLine 3\n';
      const result = sanitizeClipboardContent(input);
      expect(result).toBe('Line 1\nLine 2\nLine 3\n');
    });

    it('should limit content length', () => {
      const input = 'a'.repeat(15000);
      const result = sanitizeClipboardContent(input);
      expect(result.length).toBe(10000);
    });

    it('should handle empty content', () => {
      const result = sanitizeClipboardContent('');
      expect(result).toBe('');
    });
  });

  describe('detectSensitiveData', () => {
    it('should detect password patterns', () => {
      const text = 'export PASSWORD=secret123';
      const result = detectSensitiveData(text);
      expect(result.hasSensitiveData).toBe(true);
      expect(result.patterns).toContain('password');
    });

    it('should detect API key patterns', () => {
      const text = 'api_key=sk_test_abcdef123456';
      const result = detectSensitiveData(text);
      expect(result.hasSensitiveData).toBe(true);
      expect(result.patterns).toContain('api_key');
    });

    it('should detect SSH private keys', () => {
      const text = '-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA...';
      const result = detectSensitiveData(text);
      expect(result.hasSensitiveData).toBe(true);
      expect(result.patterns).toContain('ssh_key');
    });

    it('should detect credit card numbers', () => {
      const text = 'Card: 4532 1234 5678 9012';
      const result = detectSensitiveData(text);
      expect(result.hasSensitiveData).toBe(true);
      expect(result.patterns).toContain('credit_card');
    });

    it('should detect email addresses', () => {
      const text = 'Contact: user@example.com';
      const result = detectSensitiveData(text);
      expect(result.hasSensitiveData).toBe(true);
      expect(result.patterns).toContain('email');
    });

    it('should return false for safe content', () => {
      const text = 'This is just normal text content';
      const result = detectSensitiveData(text);
      expect(result.hasSensitiveData).toBe(false);
      expect(result.patterns).toHaveLength(0);
    });

    it('should detect multiple patterns', () => {
      const text = 'password=secret123 and api_key=abc123def456 and user@test.com';
      const result = detectSensitiveData(text);
      expect(result.hasSensitiveData).toBe(true);
      expect(result.patterns).toContain('password');
      expect(result.patterns).toContain('api_key');
      expect(result.patterns).toContain('email');
    });
  });

  describe('maskSensitiveData', () => {
    it('should mask password values', () => {
      const text = 'export PASSWORD=secret123';
      const result = maskSensitiveData(text);
      expect(result).toContain('***MASKED***');
      expect(result).not.toContain('secret123');
    });

    it('should mask API keys', () => {
      const text = 'api_key=sk_test_abcdef123456';
      const result = maskSensitiveData(text);
      expect(result).toContain('***MASKED***');
      expect(result).not.toContain('sk_test_abcdef123456');
    });

    it('should mask SSH private keys', () => {
      const text = '-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA...\n-----END RSA PRIVATE KEY-----';
      const result = maskSensitiveData(text);
      expect(result).toContain('***PRIVATE KEY CONTENT MASKED***');
      expect(result).toContain('-----BEGIN RSA PRIVATE KEY-----');
      expect(result).toContain('-----END RSA PRIVATE KEY-----');
      expect(result).not.toContain('MIIEpAIBAAKCAQEA');
    });

    it('should preserve non-sensitive content', () => {
      const text = 'This is normal text that should not be masked';
      const result = maskSensitiveData(text);
      expect(result).toBe(text);
    });
  });

  describe('sanitizeSearchQuery', () => {
    it('should remove angle brackets', () => {
      const query = 'search<script>alert(1)</script>';
      const result = sanitizeSearchQuery(query);
      expect(result).toBe('searchscriptalert(1)/script');
    });

    it('should remove javascript: URLs', () => {
      const query = 'javascript:alert(1)';
      const result = sanitizeSearchQuery(query);
      expect(result).toBe('alert(1)');
    });

    it('should remove event handlers', () => {
      const query = 'onclick=alert(1)';
      const result = sanitizeSearchQuery(query);
      expect(result).toBe('alert(1)');
    });

    it('should limit query length', () => {
      const query = 'a'.repeat(2000);
      const result = sanitizeSearchQuery(query);
      expect(result.length).toBe(1000);
    });

    it('should preserve normal search queries', () => {
      const query = 'normal search term';
      const result = sanitizeSearchQuery(query);
      expect(result).toBe(query);
    });
  });

  describe('RateLimiter', () => {
    beforeEach(() => {
      // Reset rate limiter state
      rateLimiter.reset('test-key');
    });

    it('should allow requests within limit', () => {
      expect(rateLimiter.isAllowed('test-key', 5, 60000)).toBe(true);
      expect(rateLimiter.isAllowed('test-key', 5, 60000)).toBe(true);
      expect(rateLimiter.isAllowed('test-key', 5, 60000)).toBe(true);
    });

    it('should reject requests over limit', () => {
      // Make 5 requests (max allowed)
      for (let i = 0; i < 5; i++) {
        expect(rateLimiter.isAllowed('test-key', 5, 60000)).toBe(true);
      }
      // 6th request should be rejected
      expect(rateLimiter.isAllowed('test-key', 5, 60000)).toBe(false);
    });

    it('should reset after calling reset method', () => {
      // Max out the rate limit
      for (let i = 0; i < 5; i++) {
        rateLimiter.isAllowed('test-key', 5, 60000);
      }
      expect(rateLimiter.isAllowed('test-key', 5, 60000)).toBe(false);
      
      // Reset and try again
      rateLimiter.reset('test-key');
      expect(rateLimiter.isAllowed('test-key', 5, 60000)).toBe(true);
    });

    it('should handle different keys independently', () => {
      // Max out one key
      for (let i = 0; i < 5; i++) {
        rateLimiter.isAllowed('key1', 5, 60000);
      }
      expect(rateLimiter.isAllowed('key1', 5, 60000)).toBe(false);
      
      // Different key should still work
      expect(rateLimiter.isAllowed('key2', 5, 60000)).toBe(true);
    });
  });

  describe('logSecurityEvent', () => {
    it('should log security events with proper structure', () => {
      const consoleSpy = vi.spyOn(console, 'warn');
      
      logSecurityEvent('test_event', { key: 'value' }, 'medium');
      
      expect(consoleSpy).toHaveBeenCalledWith('Security Event:', expect.objectContaining({
        event: 'test_event',
        severity: 'medium',
        details: expect.objectContaining({
          key: 'value',
          userAgent: expect.any(String),
          url: expect.any(String)
        })
      }));
    });

    it('should include timestamp in log entry', () => {
      const consoleSpy = vi.spyOn(console, 'warn');
      
      logSecurityEvent('test_event', {});
      
      expect(consoleSpy).toHaveBeenCalledWith('Security Event:', expect.objectContaining({
        timestamp: expect.any(String)
      }));
    });

    it('should default to medium severity', () => {
      const consoleSpy = vi.spyOn(console, 'warn');
      
      logSecurityEvent('test_event', {});
      
      expect(consoleSpy).toHaveBeenCalledWith('Security Event:', expect.objectContaining({
        severity: 'medium'
      }));
    });
  });

  describe('enforceCSP', () => {
    beforeEach(() => {
      // Clean up any existing meta tags
      document.querySelectorAll('meta[http-equiv="Content-Security-Policy"]').forEach(el => el.remove());
    });

    it('should log warning when CSP meta tag is missing', () => {
      const logSpy = vi.spyOn(console, 'warn');
      
      enforceCSP();
      
      expect(logSpy).toHaveBeenCalledWith('Security Event:', expect.objectContaining({
        event: 'missing_csp'
      }));
    });

    it('should not log warning when CSP meta tag is present', () => {
      // Add CSP meta tag
      const meta = document.createElement('meta');
      meta.setAttribute('http-equiv', 'Content-Security-Policy');
      meta.setAttribute('content', "default-src 'self'");
      document.head.appendChild(meta);
      
      const logSpy = vi.spyOn(console, 'warn');
      
      enforceCSP();
      
      expect(logSpy).not.toHaveBeenCalledWith('Security Event:', expect.objectContaining({
        event: 'missing_csp'
      }));
    });
  });

  describe('initializeSecurity', () => {
    it('should initialize security measures without errors', () => {
      const consoleSpy = vi.spyOn(console, 'log');
      
      expect(() => initializeSecurity()).not.toThrow();
      
      expect(consoleSpy).toHaveBeenCalledWith('Security measures initialized');
    });

    it('should call enforceCSP during initialization', () => {
      // Clean up any existing CSP meta tags
      document.querySelectorAll('meta[http-equiv="Content-Security-Policy"]').forEach(el => el.remove());
      
      const logSpy = vi.spyOn(console, 'warn');
      
      initializeSecurity();
      
      // Should trigger CSP check (and likely warning since no CSP in test env)
      expect(logSpy).toHaveBeenCalledWith('Security Event:', expect.objectContaining({
        event: 'missing_csp'
      }));
    });
  });
});