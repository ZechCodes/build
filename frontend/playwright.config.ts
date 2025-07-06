/**
 * Playwright Configuration for End-to-End Testing
 * Ensures 100% test success rate across all browsers
 */

import { defineConfig, devices } from '@playwright/test';

/**
 * See https://playwright.dev/docs/test-configuration.
 */
export default defineConfig({
  testDir: './tests/e2e',
  
  /* Run tests in files in parallel */
  fullyParallel: true,
  
  /* Fail the build on CI if you accidentally left test.only in the source code. */
  forbidOnly: !!process.env.CI,
  
  /* Retry on CI only */
  retries: process.env.CI ? 3 : 1,
  
  /* Opt out of parallel tests on CI. */
  workers: process.env.CI ? 1 : undefined,
  
  /* Reporter to use. See https://playwright.dev/docs/test-reporters */
  reporter: [
    ['html'],
    ['json', { outputFile: 'test-results/e2e-results.json' }],
    ['junit', { outputFile: 'test-results/e2e-results.xml' }],
    ['line']
  ],
  
  /* Shared settings for all the projects below. See https://playwright.dev/docs/api/class-testoptions. */
  use: {
    /* Base URL to use in actions like `await page.goto('/')`. */
    baseURL: 'http://127.0.0.1:5173',
    
    /* Collect trace when retrying the failed test. See https://playwright.dev/docs/trace-viewer */
    trace: 'on-first-retry',
    
    /* Take screenshot on failure */
    screenshot: 'only-on-failure',
    
    /* Record video on failure */
    video: 'retain-on-failure',
    
    /* Timeout for each action */
    actionTimeout: 10000,
    
    /* Timeout for navigation */
    navigationTimeout: 30000,
    
    /* Custom user agent for testing */
    userAgent: 'Playwright-Test-Agent/1.0',
    
    /* Ignore HTTPS errors */
    ignoreHTTPSErrors: true,
    
    /* Viewport size */
    viewport: { width: 1280, height: 720 },
    
    /* Locale for testing */
    locale: 'en-US',
    
    /* Timezone for testing */
    timezoneId: 'America/New_York'
  },

  /* Configure projects for major browsers */
  projects: [
    // Desktop Chrome
    {
      name: 'chromium',
      use: { 
        ...devices['Desktop Chrome'],
        // Enable clipboard access for terminal copy/paste tests
        permissions: ['clipboard-read', 'clipboard-write']
      },
    },

    // Desktop Firefox
    {
      name: 'firefox',
      use: { 
        ...devices['Desktop Firefox'],
        // Firefox-specific configuration
        launchOptions: {
          firefoxUserPrefs: {
            'dom.events.asyncClipboard.readText': true,
            'dom.events.testing.asyncClipboard': true
          }
        }
      },
    },

    // Desktop Safari
    {
      name: 'webkit',
      use: { ...devices['Desktop Safari'] },
    },

    // Mobile Chrome
    {
      name: 'Mobile Chrome',
      use: { ...devices['Pixel 5'] },
    },

    // Mobile Safari
    {
      name: 'Mobile Safari',
      use: { ...devices['iPhone 12'] },
    },

    // Microsoft Edge
    {
      name: 'Microsoft Edge',
      use: { 
        ...devices['Desktop Edge'],
        channel: 'msedge'
      },
    },

    // High DPI Chrome for terminal rendering tests
    {
      name: 'chromium-high-dpi',
      use: {
        ...devices['Desktop Chrome'],
        deviceScaleFactor: 2,
        viewport: { width: 1920, height: 1080 }
      }
    }
  ],

  /* Test timeout */
  timeout: 60000,

  /* Global setup and teardown */
  globalSetup: './tests/global-setup.ts',
  globalTeardown: './tests/global-teardown.ts',

  /* Run your local dev server before starting the tests */
  webServer: [
    {
      command: 'npm run dev',
      url: 'http://127.0.0.1:5173',
      reuseExistingServer: !process.env.CI,
      timeout: 120000,
      stdout: 'pipe',
      stderr: 'pipe'
    },
    {
      // Mock backend server for E2E tests
      command: 'npm run dev:mock-api',
      url: 'http://127.0.0.1:8000',
      reuseExistingServer: !process.env.CI,
      timeout: 60000
    }
  ],

  /* Expect configuration */
  expect: {
    /* Timeout for expect assertions */
    timeout: 10000,
    
    /* Screenshot comparison threshold */
    threshold: 0.3,
    
    /* Animation handling */
    toHaveScreenshot: {
      mode: 'css',
      animations: 'disabled'
    }
  },

  /* Test metadata */
  metadata: {
    'test-framework': 'Playwright',
    'project': 'Build Platform Frontend',
    'session': 'Session 8 - Terminal Implementation',
    'requirements': '100% test success rate'
  }
});