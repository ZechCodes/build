#!/usr/bin/env node

/**
 * Simple test runner to execute our TDD tests
 */

const { spawn } = require('child_process');
const path = require('path');

// Change to frontend directory
process.chdir(path.join(__dirname));

console.log('🧪 Running TDD Terminal Tests...');
console.log('Current directory:', process.cwd());

// Run vitest for Terminal component tests
const testProcess = spawn('npx', ['vitest', 'run', 'Terminal.test.tsx', '--reporter=verbose'], {
  stdio: 'inherit',
  shell: false
});

testProcess.on('close', (code) => {
  if (code === 0) {
    console.log('✅ Tests completed successfully');
  } else {
    console.log(`❌ Tests failed with exit code ${code}`);
  }
  process.exit(code);
});

testProcess.on('error', (err) => {
  console.error('Failed to run tests:', err);
  process.exit(1);
});