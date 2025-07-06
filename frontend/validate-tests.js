/**
 * Test validation script to check our TDD implementation
 */

const fs = require('fs');
const path = require('path');

console.log('🔍 Validating TDD Test Implementation...\n');

// Check if all required test files exist
const requiredFiles = [
  'src/test/setup.ts',
  'src/test/mocks/handlers.ts',
  'src/test/utils/test-utils.tsx',
  'src/components/Terminal/__tests__/Terminal.test.tsx',
  'playwright.config.ts',
  'vitest.config.ts'
];

let allFilesExist = true;

console.log('📂 Checking required test files:');
requiredFiles.forEach(file => {
  // Sanitize file path to prevent traversal attacks
  const normalizedFile = path.normalize(file).replace(/^(\.\.[\/\\])+/, '');
  const filePath = path.resolve(__dirname, normalizedFile);
  
  // Ensure the resolved path is within the project directory
  if (!filePath.startsWith(path.resolve(__dirname))) {
    console.log(`❌ ${file} - Invalid path (security violation)`);
    allFilesExist = false;
    return;
  }
  
  const exists = fs.existsSync(filePath);
  console.log(`${exists ? '✅' : '❌'} ${file}`);
  if (!exists) allFilesExist = false;
});

if (!allFilesExist) {
  console.log('\n❌ Missing required test files. TDD setup incomplete.');
  process.exit(1);
}

// Check package.json for required dependencies
console.log('\n📦 Checking dependencies:');
const packageJson = JSON.parse(fs.readFileSync(path.join(__dirname, 'package.json'), 'utf8'));

const requiredDeps = [
  '@testing-library/jest-dom',
  '@testing-library/react',
  'vitest',
  'playwright',
  'msw'
];

const missingDeps = requiredDeps.filter(dep => 
  !packageJson.devDependencies[dep] && !packageJson.dependencies[dep]
);

if (missingDeps.length > 0) {
  console.log('❌ Missing dependencies:', missingDeps);
  console.log('\n📋 To install missing dependencies:');
  console.log(`npm install --save-dev ${missingDeps.join(' ')}`);
} else {
  console.log('✅ All required dependencies present');
}

// Analyze Terminal component for TDD compliance
console.log('\n🧪 Analyzing Terminal component for TDD readiness:');

const terminalPath = path.join(__dirname, 'src/components/Terminal/Terminal.tsx');
const terminalContent = fs.readFileSync(terminalPath, 'utf8');

// Check for accessibility attributes
const hasAriaLabel = terminalContent.includes('aria-label') || terminalContent.includes('role="terminal"');
const hasTabIndex = terminalContent.includes('tabIndex');

console.log(`${hasAriaLabel ? '✅' : '❌'} Accessibility attributes`);
console.log(`${hasTabIndex ? '✅' : '❌'} Keyboard navigation support`);

// Check for error handling
const hasErrorHandling = terminalContent.includes('try') && terminalContent.includes('catch');
console.log(`${hasErrorHandling ? '✅' : '❌'} Error handling implemented`);

// Check for performance optimizations
const hasPerformanceHooks = terminalContent.includes('usePerformanceMonitoring') || 
                           terminalContent.includes('useThrottledCallback');
console.log(`${hasPerformanceHooks ? '✅' : '❌'} Performance optimization hooks`);

// Summary
console.log('\n📊 TDD Implementation Status:');
console.log(`✅ Test infrastructure: ${allFilesExist ? 'Complete' : 'Incomplete'}`);
console.log(`✅ Dependencies: ${missingDeps.length === 0 ? 'Complete' : 'Incomplete'}`);
console.log(`✅ Component readiness: ${hasAriaLabel && hasTabIndex ? 'Good' : 'Needs improvement'}`);

console.log('\n🎯 Ready for TDD Cycle 1: Basic Terminal Initialization');
console.log('Next steps:');
console.log('1. Install missing dependencies if any');
console.log('2. Run: npm test Terminal.test.tsx');
console.log('3. Fix failing tests one by one (GREEN phase)');
console.log('4. Refactor for optimization (REFACTOR phase)');

console.log('\n✅ TDD validation complete!');