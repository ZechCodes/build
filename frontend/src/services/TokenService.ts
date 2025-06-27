/**
 * Token service for handling authentication tokens
 */

interface TokenResponse {
  access_token: string;
  token_type: string;
  expires_in: number;
  user: {
    sub: string;
    email: string;
    username: string;
    role: string;
  };
}

class TokenService {
  private static instance: TokenService;
  private cachedToken: string | null = null;
  private tokenExpiry: Date | null = null;

  private constructor() {}

  public static getInstance(): TokenService {
    if (!TokenService.instance) {
      TokenService.instance = new TokenService();
    }
    return TokenService.instance;
  }

  /**
   * Fetch a demo JWT token for development
   */
  public async getDemoToken(apiUrl: string = 'http://localhost:8000'): Promise<string> {
    // Return cached token if it's still valid
    if (this.cachedToken && this.tokenExpiry && new Date() < this.tokenExpiry) {
      return this.cachedToken;
    }

    try {
      const response = await fetch(`${apiUrl}/demo/token`, {
        method: 'GET',
        headers: {
          'Content-Type': 'application/json',
        },
      });

      if (!response.ok) {
        throw new Error(`Failed to fetch demo token: ${response.status} ${response.statusText}`);
      }

      const tokenData: TokenResponse = await response.json();
      
      // Cache the token with a 5-minute buffer before expiry
      this.cachedToken = tokenData.access_token;
      this.tokenExpiry = new Date(Date.now() + (tokenData.expires_in - 300) * 1000);

      console.log('Demo token fetched successfully:', {
        user: tokenData.user,
        expires_in: tokenData.expires_in,
      });

      return tokenData.access_token;
    } catch (error) {
      console.error('Failed to fetch demo token:', error);
      
      // Fallback to hardcoded demo token for development
      console.warn('Falling back to hardcoded demo-token');
      return 'demo-token';
    }
  }

  /**
   * Clear cached token (for logout or token refresh)
   */
  public clearToken(): void {
    this.cachedToken = null;
    this.tokenExpiry = null;
  }

  /**
   * Check if we have a valid cached token
   */
  public hasValidToken(): boolean {
    return !!(this.cachedToken && this.tokenExpiry && new Date() < this.tokenExpiry);
  }
}

export const tokenService = TokenService.getInstance();