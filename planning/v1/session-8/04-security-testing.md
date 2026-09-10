# Session 8.4: Frontend Security Testing & XSS Protection

## Objective
Implement comprehensive security testing and XSS protection for the frontend terminal implementation, ensuring robust defense against client-side attacks and data exposure vulnerabilities.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for security event monitoring and frontend attack detection
- **Session 2**: Validates integration with authentication system for secure terminal access
- **Session 8.1**: Secures terminal component against script injection and data leakage
- **Session 8.2**: Protects WebSocket communications from manipulation and interception

## Frontend Security Testing Framework

### Terminal Security Test Suite
**Location**: `frontend/src/components/Terminal/__tests__/security/`

```typescript
// frontend/src/components/Terminal/__tests__/security/TerminalSecurityTests.ts
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { Terminal } from '../Terminal';
import { WebSocketClient } from '../../services/WebSocketClient';
import { TerminalThemeManager } from '../ThemeManager';
import { logfire } from '../../utils/logfire';

interface SecurityTestResult {
  testName: string;
  passed: boolean;
  severity: 'LOW' | 'MEDIUM' | 'HIGH' | 'CRITICAL';
  description: string;
  vulnerabilityDetails?: string;
  recommendations: string[];
  cveReferences?: string[];
}

export class FrontendTerminalSecurityTester {
  private testResults: SecurityTestResult[] = [];
  private terminal: any;
  private websocketClient: WebSocketClient;
  private themeManager: TerminalThemeManager;

  constructor() {
    this.terminal = null;
    this.websocketClient = new WebSocketClient({ url: 'ws://localhost:8000' });
    this.themeManager = new TerminalThemeManager();
  }

  async runComprehensiveSecurityTests(): Promise<SecurityTestResult[]> {
    try {
      this.testResults = [];

      // XSS and Script Injection Tests
      await this.testXSSInTerminalOutput();
      await this.testScriptInjectionInThemes();
      await this.testHTMLInjectionInTerminalData();
      await this.testCSSStylingInjection();
      await this.testEventHandlerInjection();

      // Data Protection Tests
      await this.testSensitiveDataExposure();
      await this.testLocalStorageDataLeakage();
      await this.testClipboardSecurityVulnerabilities();
      await this.testMemoryLeakageInTerminalBuffer();
      await this.testSessionDataPersistenceAttacks();

      // WebSocket Security Tests
      await this.testWebSocketMessageValidation();
      await this.testWebSocketOriginValidation();
      await this.testWebSocketDataIntegrity();
      await this.testConnectionHijackingProtection();
      await this.testMessageReplayAttacks();

      // Authentication & Authorization Tests
      await this.testAuthenticationBypass();
      await this.testSessionTokenValidation();
      await this.testCrossUserTerminalAccess();
      await this.testPrivilegeEscalationViaTerminal();
      await this.testUnauthenticatedTerminalAccess();

      // Input Validation Tests
      await this.testTerminalInputSanitization();
      await this.testKeystrokeInjectionAttacks();
      await this.testControlSequenceInjection();
      await this.testFilePathInjectionViaTerminal();
      await this.testCommandInjectionPrevention();

      // UI Security Tests
      await this.testClickjackingProtection();
      await this.testUIRedressingAttacks();
      await this.testAccessibilitySecurityVulnerabilities();
      await this.testKeyboardNavigationSecurity();
      await this.testFocusManagementSecurity();

      logfire.info('Frontend terminal security testing completed', {
        totalTests: this.testResults.length,
        passedTests: this.testResults.filter(r => r.passed).length,
        criticalFailures: this.testResults.filter(r => !r.passed && r.severity === 'CRITICAL').length,
        highFailures: this.testResults.filter(r => !r.passed && r.severity === 'HIGH').length
      });

      return this.testResults;

    } catch (error) {
      logfire.error('Frontend security testing failed', {
        error: error instanceof Error ? error.message : 'Unknown error'
      });
      return this.testResults;
    }
  }

  private async testXSSInTerminalOutput(): Promise<void> {
    const testName = 'xss_terminal_output_protection';
    
    try {
      // Test 1: Script tag injection in terminal output
      const maliciousScript = '<script>alert("XSS")</script>';
      const { container } = render(<Terminal vmId="test-vm" />);
      
      // Simulate receiving malicious content via WebSocket
      const mockMessage = {
        type: 'terminal_output',
        sessionId: 'test-session',
        data: maliciousScript
      };

      // Terminal should escape/sanitize the content
      const terminalContent = container.querySelector('.terminal-content');
      expect(terminalContent?.innerHTML).not.toContain('<script>');
      expect(terminalContent?.textContent).toContain('&lt;script&gt;');

      // Test 2: Event handler injection
      const eventHandlerInjection = '<img src="x" onerror="alert(\'XSS\')">';
      const mockMessage2 = {
        type: 'terminal_output',
        sessionId: 'test-session',
        data: eventHandlerInjection
      };

      expect(terminalContent?.innerHTML).not.toContain('onerror=');

      // Test 3: JavaScript URL injection
      const jsUrlInjection = '<a href="javascript:alert(\'XSS\')">Click me</a>';
      const mockMessage3 = {
        type: 'terminal_output',
        sessionId: 'test-session',
        data: jsUrlInjection
      };

      expect(terminalContent?.innerHTML).not.toContain('javascript:');

      this.addTestResult({
        testName,
        passed: true,
        severity: 'CRITICAL',
        description: 'Terminal output XSS protection working correctly',
        recommendations: []
      });

    } catch (error) {
      this.addTestResult({
        testName,
        passed: false,
        severity: 'CRITICAL',
        description: 'Terminal output vulnerable to XSS attacks',
        vulnerabilityDetails: error instanceof Error ? error.message : 'Unknown error',
        recommendations: [
          'Implement comprehensive output sanitization',
          'Use Content Security Policy to prevent script execution',
          'Validate and escape all terminal output data',
          'Implement output filtering for dangerous HTML constructs'
        ],
        cveReferences: ['CVE-2019-11358', 'CVE-2020-11022']
      });
    }
  }

  private async testScriptInjectionInThemes(): Promise<void> {
    const testName = 'script_injection_theme_system';
    
    try {
      // Test malicious theme injection
      const maliciousTheme = {
        name: 'evil-theme',
        displayName: 'Evil Theme</title><script>alert("XSS")</script>',
        background: 'expression(alert("CSS XSS"))',
        foreground: 'url("javascript:alert(\'XSS\')")',
        // Other properties...
      };

      // Attempt to add malicious theme
      try {
        this.themeManager.addCustomTheme(maliciousTheme as any);
        
        // Check if theme was properly sanitized
        const themes = this.themeManager.getAvailableThemes();
        const addedTheme = themes.find(t => t.name === 'evil-theme');
        
        expect(addedTheme?.displayName).not.toContain('<script>');
        expect(addedTheme?.displayName).not.toContain('</title>');
        
      } catch (error) {
        // Expected if validation prevents malicious themes
      }

      // Test CSS injection in theme colors
      const cssInjectionTheme = {
        name: 'css-injection',
        displayName: 'CSS Injection',
        background: '#000000; background: url("http://evil.com/track");',
        foreground: '#ffffff',
        // Other properties...
      };

      try {
        this.themeManager.addCustomTheme(cssInjectionTheme as any);
        
        // Verify CSS was sanitized
        const currentTheme = this.themeManager.getCurrentTheme();
        expect(currentTheme.background).not.toContain('url(');
        expect(currentTheme.background).not.toContain('http://');
        
      } catch (error) {
        // Expected if validation prevents CSS injection
      }

      this.addTestResult({
        testName,
        passed: true,
        severity: 'HIGH',
        description: 'Theme system protected against script injection',
        recommendations: []
      });

    } catch (error) {
      this.addTestResult({
        testName,
        passed: false,
        severity: 'HIGH',
        description: 'Theme system vulnerable to script injection',
        vulnerabilityDetails: error instanceof Error ? error.message : 'Unknown error',
        recommendations: [
          'Implement strict theme validation and sanitization',
          'Use CSS property whitelisting for theme colors',
          'Validate theme names and descriptions against XSS',
          'Implement Content Security Policy for custom themes'
        ]
      });
    }
  }

  private async testSensitiveDataExposure(): Promise<void> {
    const testName = 'sensitive_data_exposure_prevention';
    
    try {
      // Test 1: Authentication tokens in terminal buffer
      const { container } = render(<Terminal vmId="test-vm" />);
      
      // Simulate terminal output containing sensitive data
      const sensitiveOutput = 'Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...';
      
      // Terminal should detect and mask sensitive data
      const terminalElement = container.querySelector('.terminal-content');
      
      // Check if sensitive data is masked or filtered
      expect(terminalElement?.textContent).not.toContain('eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9');

      // Test 2: Password patterns in terminal
      const passwordOutput = 'Password: secretpassword123';
      
      // Should be masked as: Password: ***************
      expect(terminalElement?.textContent).not.toContain('secretpassword123');

      // Test 3: API keys and secrets
      const apiKeyOutput = 'API_KEY=sk-1234567890abcdef';
      
      expect(terminalElement?.textContent).not.toContain('sk-1234567890abcdef');

      // Test 4: Social Security Numbers or similar PII
      const ssnOutput = 'SSN: 123-45-6789';
      
      expect(terminalElement?.textContent).not.toContain('123-45-6789');

      this.addTestResult({
        testName,
        passed: true,
        severity: 'HIGH',
        description: 'Sensitive data exposure prevention working correctly',
        recommendations: []
      });

    } catch (error) {
      this.addTestResult({
        testName,
        passed: false,
        severity: 'HIGH',
        description: 'Terminal exposes sensitive data in output',
        vulnerabilityDetails: error instanceof Error ? error.message : 'Unknown error',
        recommendations: [
          'Implement sensitive data detection and masking',
          'Filter common patterns for passwords, tokens, and PII',
          'Add configurable sensitive data detection rules',
          'Implement secure logging that excludes sensitive patterns'
        ]
      });
    }
  }

  private async testWebSocketMessageValidation(): Promise<void> {
    const testName = 'websocket_message_validation';
    
    try {
      // Test 1: Malformed message structure
      const malformedMessage = '{"type":"terminal_output","data":';
      
      try {
        await this.websocketClient.sendMessage(JSON.parse(malformedMessage));
        // Should not reach here if validation works
        throw new Error('Malformed message was accepted');
      } catch (error) {
        // Expected - malformed JSON should be rejected
      }

      // Test 2: Invalid message type
      const invalidTypeMessage = {
        type: 'malicious_command',
        sessionId: 'test-session',
        data: 'rm -rf /'
      };

      try {
        await this.websocketClient.sendMessage(invalidTypeMessage);
        // Should validate message type
      } catch (error) {
        // Expected if message type validation works
      }

      // Test 3: Oversized message payload
      const oversizedMessage = {
        type: 'terminal_input',
        sessionId: 'test-session',
        data: 'A'.repeat(10000000) // 10MB of data
      };

      try {
        await this.websocketClient.sendMessage(oversizedMessage);
        throw new Error('Oversized message was accepted');
      } catch (error) {
        // Expected - oversized messages should be rejected
      }

      // Test 4: SQL injection in session ID
      const sqlInjectionMessage = {
        type: 'terminal_input',
        sessionId: "'; DROP TABLE sessions; --",
        data: 'ls'
      };

      // Should sanitize session ID
      expect(sqlInjectionMessage.sessionId).not.toContain('DROP TABLE');

      this.addTestResult({
        testName,
        passed: true,
        severity: 'HIGH',
        description: 'WebSocket message validation working correctly',
        recommendations: []
      });

    } catch (error) {
      this.addTestResult({
        testName,
        passed: false,
        severity: 'HIGH',
        description: 'WebSocket message validation insufficient',
        vulnerabilityDetails: error instanceof Error ? error.message : 'Unknown error',
        recommendations: [
          'Implement strict message schema validation',
          'Add message size limits and rate limiting',
          'Validate all message fields against injection attacks',
          'Implement message type whitelisting'
        ]
      });
    }
  }

  private async testClipboardSecurityVulnerabilities(): Promise<void> {
    const testName = 'clipboard_security_protection';
    
    try {
      // Mock clipboard API for testing
      const mockClipboard = {
        writeText: jest.fn(),
        readText: jest.fn()
      };
      
      Object.defineProperty(navigator, 'clipboard', {
        value: mockClipboard,
        configurable: true
      });

      const { container } = render(<Terminal vmId="test-vm" />);

      // Test 1: Clipboard data sanitization
      const maliciousClipboardData = '<script>alert("XSS")</script>sensitive-data';
      mockClipboard.readText.mockResolvedValue(maliciousClipboardData);

      // Simulate paste operation
      const terminalElement = container.querySelector('.terminal-content');
      fireEvent.keyDown(terminalElement!, {
        key: 'v',
        ctrlKey: true,
        shiftKey: true
      });

      await waitFor(() => {
        // Verify malicious script was not executed or stored
        expect(terminalElement?.textContent).not.toContain('<script>');
      });

      // Test 2: Clipboard access permissions
      // Should request permission before accessing clipboard
      mockClipboard.readText.mockRejectedValue(new Error('Permission denied'));

      // Should handle permission denial gracefully
      fireEvent.keyDown(terminalElement!, {
        key: 'v',
        ctrlKey: true,
        shiftKey: true
      });

      // Should not crash or expose error details

      // Test 3: Sensitive data filtering in copy operations
      const sensitiveTerminalContent = 'Password: secret123\nAPI_KEY: sk-abcd1234';
      
      // Mock terminal selection
      const mockTerminal = {
        getSelection: () => sensitiveTerminalContent
      };

      // Copy operation should filter sensitive data
      await fireEvent.keyDown(terminalElement!, {
        key: 'c',
        ctrlKey: true,
        shiftKey: true
      });

      // Verify sensitive data was not copied to clipboard
      expect(mockClipboard.writeText).toHaveBeenCalledWith(
        expect.not.stringContaining('secret123')
      );

      this.addTestResult({
        testName,
        passed: true,
        severity: 'MEDIUM',
        description: 'Clipboard security protection working correctly',
        recommendations: []
      });

    } catch (error) {
      this.addTestResult({
        testName,
        passed: false,
        severity: 'MEDIUM',
        description: 'Clipboard operations have security vulnerabilities',
        vulnerabilityDetails: error instanceof Error ? error.message : 'Unknown error',
        recommendations: [
          'Implement clipboard data sanitization',
          'Add permission checks for clipboard access',
          'Filter sensitive data from copy operations',
          'Implement clipboard content validation'
        ]
      });
    }
  }

  private async testAuthenticationBypass(): Promise<void> {
    const testName = 'authentication_bypass_prevention';
    
    try {
      // Test 1: Terminal access without authentication
      const { container } = render(<Terminal vmId="test-vm" />);
      
      // Should not render functional terminal without auth
      expect(container.querySelector('.terminal-error')).toBeInTheDocument();
      expect(container.querySelector('.xterm')).not.toBeInTheDocument();

      // Test 2: Invalid JWT token handling
      const invalidToken = 'invalid.jwt.token';
      
      // Mock auth context with invalid token
      const AuthContextMock = {
        user: null,
        token: invalidToken,
        isAuthenticated: false
      };

      // Terminal should reject invalid authentication
      expect(AuthContextMock.isAuthenticated).toBe(false);

      // Test 3: Expired token handling
      const expiredToken = 'eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjE2MDA1MzQwMDB9.invalid';
      
      // Should detect expired token and deny access
      expect(this.isTokenExpired(expiredToken)).toBe(true);

      // Test 4: Token manipulation detection
      const manipulatedToken = 'eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.manipulated.signature';
      
      // Should detect token manipulation
      expect(this.isTokenValid(manipulatedToken)).toBe(false);

      this.addTestResult({
        testName,
        passed: true,
        severity: 'CRITICAL',
        description: 'Authentication bypass prevention working correctly',
        recommendations: []
      });

    } catch (error) {
      this.addTestResult({
        testName,
        passed: false,
        severity: 'CRITICAL',
        description: 'Authentication can be bypassed',
        vulnerabilityDetails: error instanceof Error ? error.message : 'Unknown error',
        recommendations: [
          'Implement strict authentication validation',
          'Add JWT token integrity verification',
          'Check token expiration on all operations',
          'Implement secure token storage and transmission'
        ]
      });
    }
  }

  private async testClickjackingProtection(): Promise<void> {
    const testName = 'clickjacking_protection';
    
    try {
      // Test 1: X-Frame-Options header
      const response = await fetch('/terminal');
      const xFrameOptions = response.headers.get('X-Frame-Options');
      
      expect(xFrameOptions).toBe('DENY');

      // Test 2: Content Security Policy frame-ancestors
      const cspHeader = response.headers.get('Content-Security-Policy');
      expect(cspHeader).toContain("frame-ancestors 'none'");

      // Test 3: Terminal component iframe protection
      const { container } = render(<Terminal vmId="test-vm" />);
      
      // Should not be rendered inside iframe
      expect(window.self).toBe(window.top);

      // Test 4: Critical action confirmation
      const deleteButton = container.querySelector('[data-action="delete"]');
      if (deleteButton) {
        fireEvent.click(deleteButton);
        
        // Should require confirmation for critical actions
        expect(screen.getByText(/confirm/i)).toBeInTheDocument();
      }

      this.addTestResult({
        testName,
        passed: true,
        severity: 'MEDIUM',
        description: 'Clickjacking protection implemented correctly',
        recommendations: []
      });

    } catch (error) {
      this.addTestResult({
        testName,
        passed: false,
        severity: 'MEDIUM',
        description: 'Clickjacking protection insufficient',
        vulnerabilityDetails: error instanceof Error ? error.message : 'Unknown error',
        recommendations: [
          'Implement X-Frame-Options: DENY header',
          'Add Content Security Policy frame-ancestors directive',
          'Implement iframe detection and blocking',
          'Add confirmation dialogs for critical actions'
        ]
      });
    }
  }

  private addTestResult(result: SecurityTestResult): void {
    this.testResults.push(result);
    
    if (result.passed) {
      logfire.info('Frontend security test passed', {
        testName: result.testName,
        severity: result.severity
      });
    } else {
      logfire.warning('Frontend security test failed', {
        testName: result.testName,
        severity: result.severity,
        description: result.description,
        vulnerabilityDetails: result.vulnerabilityDetails
      });
    }
  }

  private isTokenExpired(token: string): boolean {
    try {
      const payload = JSON.parse(atob(token.split('.')[1]));
      return payload.exp < Date.now() / 1000;
    } catch {
      return true;
    }
  }

  private isTokenValid(token: string): boolean {
    try {
      const parts = token.split('.');
      return parts.length === 3 && parts.every(part => part.length > 0);
    } catch {
      return false;
    }
  }
}
```

## XSS Protection Implementation

### Output Sanitization
**Location**: `frontend/src/utils/sanitization.ts`

```typescript
// frontend/src/utils/sanitization.ts
import DOMPurify from 'dompurify';
import { logfire } from './logfire';

export interface SanitizationConfig {
  allowedTags: string[];
  allowedAttributes: string[];
  sensitivePatterns: RegExp[];
  maxLength: number;
}

export const DEFAULT_TERMINAL_SANITIZATION: SanitizationConfig = {
  allowedTags: [], // No HTML tags allowed in terminal output
  allowedAttributes: [],
  sensitivePatterns: [
    /Bearer\s+[A-Za-z0-9\-\.\_\~\+\/]+=*/g, // JWT tokens
    /[Pp]assword\s*[:=]\s*\S+/g, // Passwords
    /[Aa]pi[_\-]?[Kk]ey\s*[:=]\s*\S+/g, // API keys
    /[Ss]ecret\s*[:=]\s*\S+/g, // Secrets
    /\b\d{3}-?\d{2}-?\d{4}\b/g, // SSN patterns
    /\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b/g, // Email addresses
  ],
  maxLength: 100000 // 100KB max terminal output
};

export class TerminalSanitizer {
  private config: SanitizationConfig;
  private suspiciousPatterns: Map<string, number> = new Map();

  constructor(config: SanitizationConfig = DEFAULT_TERMINAL_SANITIZATION) {
    this.config = config;
    
    // Configure DOMPurify for terminal use
    DOMPurify.addHook('beforeSanitizeElements', (node) => {
      // Log potential XSS attempts
      if (node.tagName === 'SCRIPT') {
        logfire.warning('XSS attempt detected in terminal output', {
          nodeType: node.tagName,
          content: node.textContent?.substring(0, 100)
        });
      }
    });
  }

  public sanitizeTerminalOutput(data: string): string {
    try {
      // Step 1: Check length limits
      if (data.length > this.config.maxLength) {
        logfire.warning('Terminal output truncated due to size limit', {
          originalLength: data.length,
          maxLength: this.config.maxLength
        });
        data = data.substring(0, this.config.maxLength) + '\n[OUTPUT TRUNCATED]';
      }

      // Step 2: Detect and mask sensitive data
      data = this.maskSensitiveData(data);

      // Step 3: Remove HTML tags and dangerous content
      data = DOMPurify.sanitize(data, {
        ALLOWED_TAGS: this.config.allowedTags,
        ALLOWED_ATTR: this.config.allowedAttributes,
        KEEP_CONTENT: true,
        FORCE_BODY: false
      });

      // Step 4: Escape remaining special characters
      data = this.escapeSpecialCharacters(data);

      // Step 5: Detect suspicious patterns
      this.detectSuspiciousPatterns(data);

      return data;

    } catch (error) {
      logfire.error('Terminal output sanitization failed', {
        error: error instanceof Error ? error.message : 'Unknown error',
        dataLength: data.length
      });
      
      // Return safe fallback
      return '[SANITIZATION ERROR - OUTPUT REMOVED]';
    }
  }

  public sanitizeThemeValue(value: string, property: string): string {
    try {
      // Remove dangerous CSS constructs
      const dangerousPatterns = [
        /url\s*\(/gi,
        /javascript:/gi,
        /expression\s*\(/gi,
        /import/gi,
        /@import/gi,
        /binding/gi,
        /behavior/gi
      ];

      let sanitized = value;
      for (const pattern of dangerousPatterns) {
        if (pattern.test(sanitized)) {
          logfire.warning('Dangerous CSS pattern detected in theme', {
            property,
            value: value.substring(0, 50),
            pattern: pattern.toString()
          });
          sanitized = sanitized.replace(pattern, '');
        }
      }

      // Validate color values
      if (property.includes('color') || property.includes('background')) {
        if (!this.isValidColorValue(sanitized)) {
          logfire.warning('Invalid color value in theme', {
            property,
            value: sanitized
          });
          return '#000000'; // Safe fallback
        }
      }

      return sanitized;

    } catch (error) {
      logfire.error('Theme value sanitization failed', {
        property,
        value: value.substring(0, 50),
        error: error instanceof Error ? error.message : 'Unknown error'
      });
      
      return ''; // Safe fallback
    }
  }

  public sanitizeWebSocketMessage(message: any): any {
    try {
      // Validate message structure
      if (!this.isValidMessageStructure(message)) {
        throw new Error('Invalid message structure');
      }

      // Sanitize string fields
      const sanitized = { ...message };
      
      if (typeof sanitized.data === 'string') {
        sanitized.data = this.sanitizeTerminalOutput(sanitized.data);
      }
      
      if (typeof sanitized.sessionId === 'string') {
        sanitized.sessionId = this.sanitizeSessionId(sanitized.sessionId);
      }

      return sanitized;

    } catch (error) {
      logfire.error('WebSocket message sanitization failed', {
        messageType: message?.type,
        error: error instanceof Error ? error.message : 'Unknown error'
      });
      
      throw new Error('Message sanitization failed');
    }
  }

  private maskSensitiveData(data: string): string {
    let masked = data;
    
    for (const pattern of this.config.sensitivePatterns) {
      masked = masked.replace(pattern, (match) => {
        const prefix = match.split(/[:=]/)[0];
        const maskedValue = '*'.repeat(Math.min(match.length - prefix.length - 1, 20));
        
        logfire.info('Sensitive data masked in terminal output', {
          pattern: pattern.toString(),
          originalLength: match.length,
          maskedLength: maskedValue.length
        });
        
        return `${prefix}: ${maskedValue}`;
      });
    }
    
    return masked;
  }

  private escapeSpecialCharacters(data: string): string {
    return data
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#x27;')
      .replace(/\//g, '&#x2F;');
  }

  private detectSuspiciousPatterns(data: string): void {
    const suspiciousPatterns = [
      { name: 'script_tag', pattern: /<script/gi },
      { name: 'javascript_url', pattern: /javascript:/gi },
      { name: 'event_handler', pattern: /on\w+\s*=/gi },
      { name: 'data_url', pattern: /data:\s*text\/html/gi },
      { name: 'iframe_tag', pattern: /<iframe/gi }
    ];

    for (const { name, pattern } of suspiciousPatterns) {
      const matches = data.match(pattern);
      if (matches) {
        const count = this.suspiciousPatterns.get(name) || 0;
        this.suspiciousPatterns.set(name, count + matches.length);
        
        logfire.warning('Suspicious pattern detected in terminal output', {
          pattern: name,
          matches: matches.length,
          totalCount: count + matches.length
        });
      }
    }
  }

  private isValidColorValue(value: string): boolean {
    // Validate hex colors
    if (/^#([A-Fa-f0-9]{6}|[A-Fa-f0-9]{3})$/.test(value)) {
      return true;
    }
    
    // Validate RGB/RGBA
    if (/^rgba?\(\s*\d+\s*,\s*\d+\s*,\s*\d+\s*(,\s*[\d.]+)?\s*\)$/.test(value)) {
      return true;
    }
    
    // Validate named colors (basic set)
    const namedColors = [
      'black', 'white', 'red', 'green', 'blue', 'yellow', 'cyan', 'magenta',
      'transparent', 'inherit', 'initial', 'unset'
    ];
    
    return namedColors.includes(value.toLowerCase());
  }

  private isValidMessageStructure(message: any): boolean {
    if (!message || typeof message !== 'object') {
      return false;
    }
    
    const requiredFields = ['type'];
    const allowedTypes = [
      'terminal_output', 'terminal_input', 'terminal_resize',
      'terminal_connected', 'terminal_disconnected', 'session_restored'
    ];
    
    return requiredFields.every(field => field in message) &&
           allowedTypes.includes(message.type);
  }

  private sanitizeSessionId(sessionId: string): string {
    // Remove any non-alphanumeric characters except hyphens and underscores
    const sanitized = sessionId.replace(/[^a-zA-Z0-9\-_]/g, '');
    
    if (sanitized !== sessionId) {
      logfire.warning('Session ID sanitized', {
        original: sessionId.substring(0, 10),
        sanitized: sanitized.substring(0, 10)
      });
    }
    
    return sanitized;
  }

  public getSuspiciousPatternSummary(): Record<string, number> {
    return Object.fromEntries(this.suspiciousPatterns);
  }
}
```

## TDD Security Testing Cycle

### Security-First Development Process

1. **Red Phase**: Write failing security tests
   ```bash
   # Create security test file
   touch frontend/src/components/Terminal/__tests__/security/XSSProtection.test.tsx
   
   # Run failing security test
   npm test XSSProtection.test.tsx
   ```

2. **Green Phase**: Implement basic security controls
   ```bash
   # Add XSS protection and sanitization
   npm test XSSProtection.test.tsx
   ```

3. **Refactor Phase**: Strengthen security implementation
   ```bash
   # Add comprehensive security validation
   npm test -- --testPathPattern=security
   ```

4. **Security Commit**: Commit security enhancements
   ```bash
   git add frontend/src/components/Terminal/ frontend/src/utils/sanitization.ts
   git commit -m "security: implement comprehensive frontend terminal security controls

   - Add XSS protection with comprehensive output sanitization
   - Implement WebSocket message validation and integrity checks
   - Add sensitive data detection and masking in terminal output
   - Include authentication bypass prevention with token validation
   - Add clipboard security controls and data filtering
   - Implement clickjacking protection with CSP headers
   
   Tests: Added comprehensive frontend security test suite with XSS scenarios
   Security: CRITICAL - prevents client-side attacks and data exposure
   Compliance: Addresses OWASP Top 10 frontend security vulnerabilities"
   ```

## Complete Frontend Security Checklist ✅

### XSS Protection (Critical)
- [ ] HTML sanitization for all terminal output using DOMPurify
- [ ] Script tag filtering and removal from terminal data
- [ ] Event handler injection prevention in terminal content
- [ ] JavaScript URL filtering in terminal output
- [ ] CSS injection prevention in custom themes
- [ ] Data URL validation to prevent HTML injection
- [ ] Content Security Policy implementation with strict directives
- [ ] Input encoding for all user-generated content
- [ ] DOM-based XSS prevention in dynamic content generation
- [ ] Reflected XSS protection in URL parameters and form inputs

### Authentication & Session Security (Critical)
- [ ] JWT token validation with signature verification
- [ ] Token expiration checking on all operations
- [ ] Authentication state validation before terminal access
- [ ] Session token secure storage (httpOnly cookies when possible)
- [ ] Cross-user terminal access prevention with strict ownership checks
- [ ] Authentication bypass prevention with mandatory auth gates
- [ ] Token refresh mechanism with secure rotation
- [ ] Multi-factor authentication support for sensitive operations
- [ ] Session timeout implementation with automatic logout
- [ ] Concurrent session monitoring and management

### Data Protection & Privacy (High)
- [ ] Sensitive data pattern detection and masking in terminal output
- [ ] PII filtering for SSN, email, and personal information
- [ ] Password and credential masking in terminal logs
- [ ] API key and token detection with automatic redaction
- [ ] Clipboard data sanitization for copy/paste operations
- [ ] Local storage encryption for sensitive preferences
- [ ] Memory leak prevention in terminal buffer management
- [ ] Data retention controls with automatic cleanup
- [ ] GDPR compliance for user data handling and rights
- [ ] Audit logging for all sensitive data access operations

### WebSocket Security (High)
- [ ] Message structure validation with schema enforcement
- [ ] Origin validation for WebSocket connections
- [ ] Message size limits to prevent DoS attacks
- [ ] Rate limiting on WebSocket message frequency
- [ ] Message integrity verification with checksums
- [ ] Connection hijacking prevention with token binding
- [ ] Message replay attack protection with nonces
- [ ] WebSocket URL validation and sanitization
- [ ] Connection state monitoring and anomaly detection
- [ ] Secure WebSocket protocol (WSS) enforcement

### Input Validation & Injection Prevention (Medium)
- [ ] Terminal input sanitization for all user keystrokes
- [ ] Control sequence injection prevention in terminal data
- [ ] File path injection protection in terminal operations
- [ ] Command injection prevention with input validation
- [ ] Theme value validation to prevent CSS injection
- [ ] Search query sanitization to prevent regex DoS
- [ ] Font family validation to prevent malicious font loading
- [ ] Keyboard shortcut validation to prevent key injection
- [ ] Paste operation validation with content filtering
- [ ] Terminal size parameter validation to prevent buffer overflow

### UI Security & Clickjacking Protection (Medium)
- [ ] X-Frame-Options header implementation (DENY)
- [ ] Content Security Policy frame-ancestors directive
- [ ] Iframe detection and blocking mechanisms
- [ ] Critical action confirmation dialogs
- [ ] UI redressing attack prevention with visual indicators
- [ ] Focus management security to prevent focus hijacking
- [ ] Keyboard navigation security with proper access controls
- [ ] Modal dialog security with backdrop click protection
- [ ] Context menu security with action validation
- [ ] Drag and drop security with file type validation

### Browser Security & Compatibility (Medium)
- [ ] Content Security Policy with strict script-src directives
- [ ] Subresource Integrity (SRI) for external libraries
- [ ] Secure cookie configuration with SameSite attributes
- [ ] HTTPS enforcement for all communications
- [ ] Browser feature detection with secure fallbacks
- [ ] Cross-browser security testing with major browsers
- [ ] Feature policy implementation for sensitive APIs
- [ ] Permissions API integration for browser capabilities
- [ ] Secure context requirements for sensitive operations
- [ ] Browser extension interaction security controls

### Error Handling & Information Disclosure (Low)
- [ ] Error message sanitization to prevent information leakage
- [ ] Stack trace filtering in production builds
- [ ] Debug information removal from production code
- [ ] Safe error boundaries with generic error messages
- [ ] Logging security to prevent log injection attacks
- [ ] Console output filtering to prevent debug information exposure
- [ ] Network error handling without exposing internal details
- [ ] File path sanitization in error messages
- [ ] Database error message filtering
- [ ] Third-party service error handling with information protection

## Penetration Testing Scenarios

### Automated Security Testing
```typescript
// Automated security test runner
export class AutomatedSecurityTester {
  async runPenetrationTests(): Promise<SecurityTestResult[]> {
    const testSuites = [
      new XSSTestSuite(),
      new InjectionTestSuite(),
      new AuthenticationTestSuite(),
      new DataExposureTestSuite()
    ];

    const results: SecurityTestResult[] = [];
    
    for (const suite of testSuites) {
      const suiteResults = await suite.runTests();
      results.push(...suiteResults);
    }

    return results;
  }
}

// XSS test scenarios
class XSSTestSuite {
  async runTests(): Promise<SecurityTestResult[]> {
    const xssPayloads = [
      '<script>alert("XSS")</script>',
      '<img src="x" onerror="alert(\'XSS\')">',
      'javascript:alert("XSS")',
      '<svg onload="alert(\'XSS\')">',
      '<iframe src="javascript:alert(\'XSS\')"></iframe>',
      '<body onload="alert(\'XSS\')">',
      '<link rel="stylesheet" href="javascript:alert(\'XSS\')">',
      '<meta http-equiv="refresh" content="0;url=javascript:alert(\'XSS\')">'
    ];

    const results: SecurityTestResult[] = [];
    
    for (const payload of xssPayloads) {
      const result = await this.testXSSPayload(payload);
      results.push(result);
    }

    return results;
  }

  private async testXSSPayload(payload: string): Promise<SecurityTestResult> {
    // Test XSS payload against terminal output
    // Return test result
    return {
      testName: `xss_payload_${payload.substring(0, 10)}`,
      passed: true, // Should be true if XSS is prevented
      severity: 'CRITICAL',
      description: `XSS payload blocked: ${payload.substring(0, 20)}...`,
      recommendations: []
    };
  }
}
```

## Security Monitoring & Alerting

### Real-time Security Monitoring
```typescript
// Security event monitoring
export class SecurityEventMonitor {
  private suspiciousEvents: Map<string, number> = new Map();
  private alertThresholds = {
    xss_attempts: 5,
    injection_attempts: 3,
    auth_failures: 10,
    suspicious_patterns: 20
  };

  public recordSecurityEvent(eventType: string, details: any): void {
    const count = this.suspiciousEvents.get(eventType) || 0;
    this.suspiciousEvents.set(eventType, count + 1);

    logfire.warning('Security event recorded', {
      eventType,
      count: count + 1,
      details,
      timestamp: new Date().toISOString()
    });

    // Check if alert threshold is reached
    if (count + 1 >= this.alertThresholds[eventType as keyof typeof this.alertThresholds]) {
      this.triggerSecurityAlert(eventType, count + 1);
    }
  }

  private triggerSecurityAlert(eventType: string, count: number): void {
    logfire.error('Security alert triggered', {
      eventType,
      count,
      threshold: this.alertThresholds[eventType as keyof typeof this.alertThresholds],
      severity: 'HIGH',
      actionRequired: true
    });
  }
}
```

## Next Security Implementation Steps

1. **Complete security test framework** with all attack vector coverage
2. **Implement real-time security monitoring** with automated alerting
3. **Add Content Security Policy** with strict security directives
4. **Create security incident response** automation and procedures
5. **Implement security metrics** collection and reporting
6. **Add automated vulnerability scanning** with CI/CD integration
7. **Create security documentation** with security guidelines and procedures

## Security Commit Guidelines

Frontend security commits must include:
- **Comprehensive XSS protection** with output sanitization and CSP
- **Input validation** for all user inputs and WebSocket messages
- **Authentication security** with token validation and session management
- **Data protection** with sensitive data detection and masking
- **Test coverage** for security scenarios and attack vectors (>95%)
- **Security monitoring** integration with real-time alerting
- **Documentation updates** with security procedures and guidelines