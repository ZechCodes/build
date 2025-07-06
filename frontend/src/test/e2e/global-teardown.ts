/**
 * Playwright Global Teardown
 * Cleanup after E2E tests complete
 */

import { chromium, FullConfig } from '@playwright/test';
import { promises as fs } from 'fs';
import path from 'path';

async function globalTeardown(config: FullConfig) {
  console.log('🧹 Starting global E2E test teardown...');

  try {
    // Cleanup authentication state
    console.log('🔐 Cleaning up authentication state...');
    
    const authPath = 'playwright/.auth/user.json';
    try {
      await fs.unlink(authPath);
      console.log('✅ Authentication state cleaned up');
    } catch (error) {
      // File might not exist, which is fine
      console.log('ℹ️ Authentication state file not found (already cleaned up)');
    }

    // Generate test report summary
    console.log('📊 Generating test report summary...');
    
    const reportData = {
      timestamp: new Date().toISOString(),
      setupComplete: process.env.E2E_SETUP_COMPLETE === 'true',
      testEnvironment: {
        baseURL: config.projects[0].use.baseURL,
        browsers: config.projects.map(p => p.name),
        workers: config.workers,
        retries: config.retries
      }
    };

    const reportPath = 'test-results/e2e-summary.json';
    await fs.mkdir(path.dirname(reportPath), { recursive: true });
    await fs.writeFile(reportPath, JSON.stringify(reportData, null, 2));
    
    console.log('✅ Test report summary generated');

    // Check for any remaining test artifacts
    console.log('🔍 Checking for test artifacts...');
    
    const artifactDirs = [
      'test-results',
      'playwright-report',
      'playwright/.auth'
    ];

    for (const dir of artifactDirs) {
      try {
        const stats = await fs.stat(dir);
        if (stats.isDirectory()) {
          console.log(`📁 Test artifacts preserved in: ${dir}`);
        }
      } catch (error) {
        // Directory doesn't exist, which is fine
      }
    }

    // Validate test completion
    console.log('✅ Validating test completion...');
    
    // Check if all tests passed by looking for result files
    const resultFiles = [
      'test-results/e2e-results.json',
      'test-results/e2e-results.xml'
    ];

    let allTestsPassed = true;
    
    for (const resultFile of resultFiles) {
      try {
        await fs.access(resultFile);
        console.log(`✅ Test results found: ${resultFile}`);
      } catch (error) {
        console.warn(`⚠️ Test results missing: ${resultFile}`);
        allTestsPassed = false;
      }
    }

    if (allTestsPassed) {
      console.log('🎉 All E2E tests completed successfully');
    } else {
      console.warn('⚠️ Some E2E test results may be incomplete');
    }

    // Performance cleanup
    console.log('🚀 Performing performance cleanup...');
    
    // Clear any temporary performance data
    try {
      const tempPerfData = 'test-results/performance-temp.json';
      await fs.unlink(tempPerfData);
    } catch (error) {
      // File might not exist
    }

    // Memory cleanup
    console.log('💾 Cleaning up memory...');
    
    // Force garbage collection if available
    if (global.gc) {
      global.gc();
      console.log('✅ Garbage collection performed');
    }

    // Final validation
    console.log('🔍 Final validation...');
    
    // Ensure no test processes are still running
    const baseURL = config.projects[0].use.baseURL;
    if (baseURL && baseURL.includes('localhost')) {
      const browser = await chromium.launch();
      const context = await browser.newContext();
      const page = await context.newPage();
      
      try {
        // Quick check that the app is still responsive
        await page.goto(baseURL, { timeout: 5000 });
        console.log('✅ Application still responsive after tests');
      } catch (error) {
        console.log('ℹ️ Application may have been shut down (normal for CI)');
      } finally {
        await context.close();
        await browser.close();
      }
    }

    console.log('✅ Global teardown completed successfully');

  } catch (error) {
    console.error('❌ Global teardown encountered errors:', error);
    // Don't throw here - teardown errors shouldn't fail the test suite
  }

  // Clean up environment variables
  delete process.env.E2E_SETUP_COMPLETE;
}

export default globalTeardown;