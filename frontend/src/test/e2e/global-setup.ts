/**
 * Playwright Global Setup
 * Prepares environment for 100% E2E test success
 */

import { chromium, FullConfig } from '@playwright/test';

async function globalSetup(config: FullConfig) {
  console.log('🚀 Starting global E2E test setup...');
  
  const baseURL = config.projects[0].use.baseURL;
  
  if (!baseURL) {
    throw new Error('Base URL not configured for E2E tests');
  }

  // Launch browser for setup
  const browser = await chromium.launch();
  const context = await browser.newContext();
  const page = await context.newPage();

  try {
    // Wait for development server to be ready
    console.log('⏳ Waiting for development server...');
    await page.goto(baseURL, { waitUntil: 'networkidle' });
    
    // Verify basic app functionality
    console.log('✅ Verifying app loads correctly...');
    await page.waitForSelector('body', { timeout: 30000 });
    
    // Check if app root element exists
    const appRoot = await page.locator('#root').count();
    if (appRoot === 0) {
      throw new Error('App root element not found - frontend may not be working');
    }

    // Setup authentication state for tests
    console.log('🔐 Setting up authentication state...');
    
    // Navigate to login page or setup mock auth
    await page.goto(`${baseURL}/login`);
    
    // Mock authentication for E2E tests
    await page.evaluate(() => {
      localStorage.setItem('auth_token', 'mock-e2e-token');
      localStorage.setItem('user_data', JSON.stringify({
        id: 'e2e-user-123',
        username: 'e2e-test-user',
        email: 'e2e@test.com',
        permissions: ['terminal:access', 'vm:manage']
      }));
    });

    // Save authentication state for all tests
    await context.storageState({ path: 'playwright/.auth/user.json' });
    
    console.log('✅ Authentication state saved');

    // Setup test data
    console.log('📋 Setting up test data...');
    
    // Create mock VM and session data
    await page.evaluate(() => {
      sessionStorage.setItem('test_vm_id', 'e2e-test-vm-456');
      sessionStorage.setItem('test_session_id', 'e2e-test-session-789');
    });

    // Verify WebSocket connectivity
    console.log('🔌 Testing WebSocket connectivity...');
    
    const wsConnected = await page.evaluate(() => {
      return new Promise((resolve) => {
        const ws = new WebSocket('ws://localhost:8000/ws/terminal?token=mock-e2e-token');
        
        ws.onopen = () => {
          ws.close();
          resolve(true);
        };
        
        ws.onerror = () => {
          resolve(false);
        };
        
        // Timeout after 5 seconds
        setTimeout(() => resolve(false), 5000);
      });
    });

    if (!wsConnected) {
      console.warn('⚠️ WebSocket connection failed - some tests may be skipped');
    } else {
      console.log('✅ WebSocket connectivity verified');
    }

    // Performance baseline measurement
    console.log('📊 Measuring performance baseline...');
    
    await page.goto(baseURL);
    
    const performanceMetrics = await page.evaluate(() => {
      return {
        domContentLoaded: performance.timing.domContentLoadedEventEnd - performance.timing.navigationStart,
        loadComplete: performance.timing.loadEventEnd - performance.timing.navigationStart,
        firstPaint: performance.getEntriesByType('paint')[0]?.startTime || 0
      };
    });

    console.log('Performance baseline:', performanceMetrics);

    // Ensure all assets are loaded
    console.log('📦 Verifying asset loading...');
    
    const resourceErrors = await page.evaluate(() => {
      const errors: string[] = [];
      const resources = performance.getEntriesByType('resource');
      
      resources.forEach((resource: any) => {
        if (resource.transferSize === 0 && resource.decodedBodySize === 0) {
          errors.push(`Failed to load: ${resource.name}`);
        }
      });
      
      return errors;
    });

    if (resourceErrors.length > 0) {
      console.warn('⚠️ Some resources failed to load:', resourceErrors);
    } else {
      console.log('✅ All assets loaded successfully');
    }

    // Setup error monitoring
    console.log('🐛 Setting up error monitoring...');
    
    // Monitor for console errors during setup
    page.on('console', msg => {
      if (msg.type() === 'error') {
        console.error('Console error during setup:', msg.text());
      }
    });

    // Monitor for page errors
    page.on('pageerror', error => {
      console.error('Page error during setup:', error.message);
    });

    // Cleanup after successful setup
    console.log('✅ Global setup completed successfully');

  } catch (error) {
    console.error('❌ Global setup failed:', error);
    throw error;
  } finally {
    await context.close();
    await browser.close();
  }

  // Store setup completion flag
  process.env.E2E_SETUP_COMPLETE = 'true';
}

export default globalSetup;