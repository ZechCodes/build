import DOMPurify from 'dompurify';

/**
 * Security utilities for XSS protection and input sanitization
 */

/**
 * Sanitize HTML content to prevent XSS attacks
 */
export function sanitizeHtml(dirty: string): string {
  return DOMPurify.sanitize(dirty, {
    ALLOWED_TAGS: ['b', 'i', 'em', 'strong', 'a', 'span', 'div', 'p', 'br'],
    ALLOWED_ATTR: ['href', 'class', 'style'],
    ALLOWED_URI_REGEXP: /^(?:(?:(?:f|ht)tps?|mailto|tel|callto|cid|xmpp):|[^a-z]|[a-z+.\-]+(?:[^a-z+.\-:]|$))/i,
  });
}

/**
 * Sanitize text content for terminal display
 * Removes dangerous characters while preserving ANSI escape sequences
 */
export function sanitizeTerminalText(text: string): string {
  // First preserve ANSI escape sequences by temporarily replacing them
  const ansiSequences: string[] = [];
  let ansiIndex = 0;
  
  // Extract and temporarily replace ANSI sequences
  let processedText = text.replace(/\x1b\[[0-9;]*[a-zA-Z]/g, (match) => {
    ansiSequences.push(match);
    return `__ANSI_${ansiIndex++}__`;
  });
  
  // Remove dangerous content
  processedText = processedText
    .replace(/[\x00-\x08\x0B\x0C\x0E-\x1F\x7F-\x9F]/g, '') // Remove control chars
    .replace(/<script\b[^<]*(?:(?!<\/script>)<[^<]*)*<\/script>/gi, '') // Remove script tags
    .replace(/javascript:/gi, '') // Remove javascript: URLs
    .replace(/on\w+\s*=/gi, ''); // Remove event handlers
  
  // Restore ANSI sequences
  ansiSequences.forEach((seq, index) => {
    processedText = processedText.replace(`__ANSI_${index}__`, seq);
  });
  
  return processedText;
}

/**
 * Validate WebSocket message content
 */
export function validateWebSocketMessage(message: any): boolean {
  if (!message || typeof message !== 'object') {
    return false;
  }

  // Check required fields
  if (!message.type || typeof message.type !== 'string') {
    return false;
  }

  // Validate message type
  const allowedTypes = [
    'terminal_data',
    'terminal_resize',
    'session_join',
    'session_create',
    'session_created',
    'session_end',
    'session_recovery',
    'heartbeat',
    'heartbeat_response',
    'error'
  ];

  if (!allowedTypes.includes(message.type)) {
    return false;
  }

  // Validate data field if present
  if (message.data !== undefined) {
    if (message.type === 'terminal_data' && typeof message.data !== 'string') {
      return false;
    }
    
    if (message.type === 'terminal_resize') {
      if (!message.data || typeof message.data !== 'object') {
        return false;
      }
      if (typeof message.data.cols !== 'number' || typeof message.data.rows !== 'number') {
        return false;
      }
      if (message.data.cols < 1 || message.data.cols > 1000 || 
          message.data.rows < 1 || message.data.rows > 1000) {
        return false;
      }
    }
  }

  // Validate session_id if present
  if (message.session_id !== undefined && typeof message.session_id !== 'string') {
    return false;
  }

  return true;
}

/**
 * Sanitize clipboard content before pasting
 */
export function sanitizeClipboardContent(content: string): string {
  return content
    .replace(/[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]/g, '') // Remove control characters
    .replace(/\r\n/g, '\n') // Normalize line endings
    .replace(/\r/g, '\n')
    .substring(0, 10000); // Limit length to prevent abuse
}

/**
 * Detect potentially sensitive data in terminal output
 */
export function detectSensitiveData(text: string): { 
  hasSensitiveData: boolean; 
  patterns: string[]; 
} {
  const sensitivePatterns = [
    {
      name: 'password',
      regex: /password\s*[=:]\s*['"]?[^\s'"]+['"]?/gi
    },
    {
      name: 'api_key',
      regex: /(?:api_key|apikey|api-key)\s*[=:]\s*['"]?[a-zA-Z0-9_-]{10,}['"]?/gi
    },
    {
      name: 'secret',
      regex: /(?:secret|token)\s*[=:]\s*['"]?[a-zA-Z0-9_-]{10,}['"]?/gi
    },
    {
      name: 'ssh_key',
      regex: /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/gi
    },
    {
      name: 'credit_card',
      regex: /\b(?:\d{4}[-\s]?){3}\d{4}\b/g
    },
    {
      name: 'email',
      regex: /\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b/g
    }
  ];

  const detectedPatterns: string[] = [];
  
  for (const pattern of sensitivePatterns) {
    if (pattern.regex.test(text)) {
      detectedPatterns.push(pattern.name);
    }
  }

  return {
    hasSensitiveData: detectedPatterns.length > 0,
    patterns: detectedPatterns
  };
}

/**
 * Mask sensitive data in terminal output
 */
export function maskSensitiveData(text: string): string {
  const patterns = [
    {
      regex: /password\s*[=:]\s*['"]?([^\s'"]+)['"]?/gi,
      replacement: 'password=***MASKED***'
    },
    {
      regex: /(?:api_key|apikey|api-key)\s*[=:]\s*['"]?[a-zA-Z0-9_-]{10,}['"]?/gi,
      replacement: 'api_key=***MASKED***'
    },
    {
      regex: /(?:secret|token)\s*[=:]\s*['"]?[a-zA-Z0-9_-]{10,}['"]?/gi,
      replacement: 'secret=***MASKED***'
    },
    {
      regex: /(-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----)[\s\S]*?(-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----)/gi,
      replacement: '$1\n***PRIVATE KEY CONTENT MASKED***\n$2'
    }
  ];

  let maskedText = text;
  
  for (const pattern of patterns) {
    maskedText = maskedText.replace(pattern.regex, pattern.replacement);
  }

  return maskedText;
}

/**
 * Validate and sanitize search query
 */
export function sanitizeSearchQuery(query: string): string {
  return query
    .replace(/[<>]/g, '') // Remove angle brackets
    .replace(/javascript:/gi, '') // Remove javascript: URLs
    .replace(/on\w+\s*=/gi, '') // Remove event handlers
    .substring(0, 1000); // Limit length
}

/**
 * Rate limiting for security-sensitive operations
 */
class RateLimiter {
  private attempts: Map<string, number[]> = new Map();
  
  isAllowed(key: string, maxAttempts: number, windowMs: number): boolean {
    const now = Date.now();
    const userAttempts = this.attempts.get(key) || [];
    
    // Remove old attempts outside the window
    const validAttempts = userAttempts.filter(time => now - time < windowMs);
    
    if (validAttempts.length >= maxAttempts) {
      return false;
    }
    
    validAttempts.push(now);
    this.attempts.set(key, validAttempts);
    return true;
  }
  
  reset(key: string): void {
    this.attempts.delete(key);
  }
}

export const rateLimiter = new RateLimiter();

/**
 * Security event logger
 */
export function logSecurityEvent(
  event: string, 
  details: Record<string, any>, 
  severity: 'low' | 'medium' | 'high' = 'medium'
): void {
  const logEntry = {
    timestamp: new Date().toISOString(),
    event,
    severity,
    details: {
      ...details,
      userAgent: navigator.userAgent,
      url: window.location.href
    }
  };
  
  // In a real application, this would send to a security monitoring service
  console.warn('Security Event:', logEntry);
  
  // For high severity events, you might want to immediately alert
  if (severity === 'high') {
    // Send to monitoring service, disable features, etc.
  }
}

/**
 * CSP (Content Security Policy) helper
 */
export function enforceCSP(): void {
  // Validate that CSP headers are properly set
  if (!document.querySelector('meta[http-equiv="Content-Security-Policy"]')) {
    logSecurityEvent('missing_csp', {
      message: 'Content Security Policy not detected'
    }, 'medium');
  }
}

/**
 * Initialize security measures
 */
export function initializeSecurity(): void {
  // Configure DOMPurify
  DOMPurify.setConfig({
    ALLOW_DATA_ATTR: false,
    ALLOW_UNKNOWN_PROTOCOLS: false,
    SANITIZE_DOM: true,
    KEEP_CONTENT: false
  });
  
  // Check CSP
  enforceCSP();
  
  // Log security initialization
  console.log('Security measures initialized');
}