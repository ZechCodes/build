#!/usr/bin/env node

/**
 * Comprehensive browser testing for the Build Platform Frontend
 * Uses Playwright to check for JavaScript errors and functionality
 */

import { chromium } from 'playwright';

async function testInBrowser() {
  console.log('🚀 Starting browser testing...\n');
  
  const browser = await chromium.launch({ 
    headless: true,
    args: ['--disable-web-security', '--allow-running-insecure-content']
  });
  
  const context = await browser.newContext({
    viewport: { width: 1280, height: 720 },
    ignoreHTTPSErrors: true
  });
  
  const page = await context.newPage();
  
  // Collect console logs and errors
  const consoleLogs = [];
  const errors = [];
  
  page.on('console', (msg) => {
    const type = msg.type();
    const text = msg.text();
    consoleLogs.push({ type, text });
    
    if (type === 'error') {
      console.log(`❌ Console Error: ${text}`);
    } else if (type === 'warning') {
      console.log(`⚠️ Console Warning: ${text}`);
    } else if (type === 'log' && text.includes('Security')) {
      console.log(`🔒 Security: ${text}`);
    } else if (type === 'log' && text.includes('Performance')) {
      console.log(`⚡ Performance: ${text}`);
    }
  });
  
  page.on('pageerror', (error) => {
    errors.push(error.message);
    console.log(`🚨 JavaScript Error: ${error.message}`);
  });
  
  try {
    console.log('📄 Testing Home Page...');
    await page.goto('http://localhost:3002/', { waitUntil: 'networkidle', timeout: 10000 });
    
    // Wait for React to load
    await page.waitForSelector('#root', { timeout: 5000 });
    console.log('✅ React app root element found');
    
    // Check if the page has loaded content
    const hasContent = await page.locator('body').textContent();
    if (hasContent && hasContent.trim().length > 0) {
      console.log('✅ Page has content loaded');
    } else {
      console.log('❌ Page appears to be empty');
    }
    
    console.log('\n📊 Testing Dashboard Page...');
    await page.goto('http://localhost:3002/dashboard', { waitUntil: 'networkidle', timeout: 10000 });
    
    // Wait for dashboard to load
    await page.waitForTimeout(2000);
    
    // Check for dashboard elements
    try {
      await page.waitForSelector('h1:has-text("Dashboard")', { timeout: 5000 });
      console.log('✅ Dashboard title found');
    } catch (e) {
      console.log('❌ Dashboard title not found');
    }
    
    // Check for terminal component
    try {
      const terminalContainer = page.locator('.terminal-container');
      if (await terminalContainer.count() > 0) {
        console.log('✅ Terminal component found');
        
        // Check for xterm elements
        const xtermElements = page.locator('.xterm');
        if (await xtermElements.count() > 0) {
          console.log('✅ XTerm terminal elements loaded');
        } else {
          console.log('⚠️ XTerm elements not yet loaded (may be initializing)');
        }
        
        // Check for terminal toolbar
        const toolbar = page.locator('.terminal-toolbar');
        if (await toolbar.count() > 0) {
          console.log('✅ Terminal toolbar found');
        } else {
          console.log('❌ Terminal toolbar not found');
        }
        
      } else {
        console.log('❌ Terminal component not found');
      }
    } catch (e) {
      console.log('❌ Error checking terminal:', e.message);
    }
    
    // Test theme switching
    console.log('\n🎨 Testing Terminal Features...');
    try {
      // Look for theme selector or buttons
      const themeButtons = page.locator('button[title*="theme"], button[title*="Theme"]');
      const buttonCount = await themeButtons.count();
      if (buttonCount > 0) {
        console.log(`✅ Found ${buttonCount} theme-related buttons`);
      } else {
        console.log('⚠️ No theme buttons found');
      }
      
      // Look for search functionality
      const searchButtons = page.locator('button[title*="search"], button[title*="Search"]');
      const searchCount = await searchButtons.count();
      if (searchCount > 0) {
        console.log(`✅ Found ${searchCount} search-related buttons`);
      } else {
        console.log('⚠️ No search buttons found');
      }
      
    } catch (e) {
      console.log('⚠️ Error testing features:', e.message);
    }
    
    // Wait a bit more for any async operations
    await page.waitForTimeout(3000);
    
    console.log('\n📊 Analysis Summary:');
    
    // Analyze console logs
    const errorLogs = consoleLogs.filter(log => log.type === 'error');
    const warningLogs = consoleLogs.filter(log => log.type === 'warning');
    const infoLogs = consoleLogs.filter(log => log.type === 'log' || log.type === 'info');
    
    console.log(`\n📋 Console Log Summary:`);
    console.log(`- Errors: ${errorLogs.length}`);
    console.log(`- Warnings: ${warningLogs.length}`);
    console.log(`- Info/Logs: ${infoLogs.length}`);
    
    if (errorLogs.length > 0) {
      console.log('\n🚨 Critical Errors Found:');
      errorLogs.forEach((log, i) => {
        console.log(`${i + 1}. ${log.text}`);
      });
    }
    
    if (warningLogs.length > 0) {
      console.log('\n⚠️ Warnings:');
      warningLogs.slice(0, 5).forEach((log, i) => {
        console.log(`${i + 1}. ${log.text}`);
      });
      if (warningLogs.length > 5) {
        console.log(`... and ${warningLogs.length - 5} more warnings`);
      }
    }
    
    // Show some successful logs
    const successLogs = infoLogs.filter(log => 
      log.text.includes('Security') || 
      log.text.includes('Performance') ||
      log.text.includes('initialized')
    );
    
    if (successLogs.length > 0) {
      console.log('\n✅ Successful Initializations:');
      successLogs.slice(0, 3).forEach((log, i) => {
        console.log(`${i + 1}. ${log.text}`);
      });
    }
    
    console.log('\n🎯 Overall Assessment:');
    if (errors.length === 0 && errorLogs.length === 0) {
      console.log('✅ No critical JavaScript errors found');
    } else {
      console.log(`❌ Found ${errors.length + errorLogs.length} critical errors that need fixing`);
    }
    
    if (warningLogs.length < 10) {
      console.log('✅ Warning count is manageable');
    } else {
      console.log('⚠️ High number of warnings - may need attention');
    }
    
    console.log('\n💡 Next Steps:');
    if (errors.length > 0 || errorLogs.length > 0) {
      console.log('- Fix the critical errors listed above');
      console.log('- Re-run this test to verify fixes');
    } else {
      console.log('- Frontend is ready for manual testing!');
      console.log('- Terminal functionality looks good for UI testing');
      console.log('- WebSocket errors are expected without backend');
    }
    
  } catch (error) {
    console.log(`🚨 Test failed: ${error.message}`);
  } finally {
    await browser.close();
  }
}

// Run the test
testInBrowser().catch(console.error);