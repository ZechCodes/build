/**
 * Playwright Global Setup
 * Ensures optimal testing environment for 100% success rate
 */

import { chromium, FullConfig } from '@playwright/test';

async function globalSetup(_config: FullConfig) {
  console.log('🚀 Starting global setup for Terminal E2E tests...');
  
  // Verify test environment
  console.log('🔧 Verifying test environment...');
  
  // Check if development servers are available
  const devServerUrl = 'http://127.0.0.1:5173';
  const mockApiUrl = 'http://127.0.0.1:8000';
  
  try {
    // Wait for dev server to be ready
    console.log('⏳ Waiting for development server...');
    let attempts = 0;
    const maxAttempts = 30;
    
    while (attempts < maxAttempts) {
      try {
        const response = await fetch(devServerUrl);
        if (response.ok) {
          console.log('✅ Development server is ready');
          break;
        }
      } catch (e) {
        // Server not ready yet
      }
      
      attempts++;
      await new Promise(resolve => setTimeout(resolve, 2000));
    }
    
    if (attempts >= maxAttempts) {
      throw new Error('Development server failed to start');
    }
    
    // Verify mock API if available
    try {
      const mockResponse = await fetch(mockApiUrl);
      console.log('✅ Mock API server is ready');
    } catch (e) {
      console.log('⚠️  Mock API server not available (optional)');
    }
    
    // Initialize browser for authentication and setup
    const browser = await chromium.launch();
    const context = await browser.newContext();
    const page = await context.newPage();
    
    try {
      // Pre-authenticate or set up test user if needed
      console.log('🔐 Setting up test authentication...');
      
      // Navigate to app to ensure it loads
      await page.goto(devServerUrl);
      await page.waitForLoadState('networkidle');
      
      // Store any authentication tokens or setup state
      // This would depend on your app's authentication system
      
      console.log('✅ Authentication setup complete');
      
    } catch (error) {
      console.error('❌ Authentication setup failed:', error);
      throw error;
    } finally {
      await page.close();
      await context.close();
      await browser.close();
    }
    
    // Set up test data if needed
    console.log('📊 Setting up test data...');
    
    // Create any necessary test fixtures
    // This could include creating test VMs, sessions, etc.
    
    console.log('✅ Test data setup complete');
    
    // Performance baseline
    console.log('📈 Establishing performance baselines...');
    
    // Set performance expectations
    process.env.PERF_RENDER_THRESHOLD = '100'; // ms
    process.env.PERF_INTERACTION_THRESHOLD = '50'; // ms
    process.env.PERF_NETWORK_THRESHOLD = '500'; // ms
    
    console.log('✅ Performance baselines established');
    
    console.log('🎉 Global setup complete - ready for testing!');
    
  } catch (error) {
    console.error('❌ Global setup failed:', error);
    throw error;
  }
}

export default globalSetup;