/// <reference types="vitest" />
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import path from 'path'

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    
    // Exclude Playwright tests from Vitest
    exclude: ['tests/**', 'tests/e2e/**', '**/e2e/**', '**/node_modules/**', '**/dist/**'],
    
    // Mock CSS imports
    css: {
      modules: {
        classNameStrategy: 'non-scoped'
      }
    },
    
    // Enhanced test configuration for 100% success rate
    testTimeout: 30000,
    hookTimeout: 10000,
    teardownTimeout: 10000,
    
    // Retry configuration for flaky test prevention
    retry: 2,
    
    // Test isolation for reliability
    isolate: true,
    
    // Fail fast on errors
    bail: 1,
    
    // Reporter configuration
    reporter: ['verbose'],
    
    // Output configuration removed - using basic reporter
    
    // Coverage configuration for 100% visibility
    coverage: {
      provider: 'v8',
      reporter: ['text', 'json', 'html', 'lcov'],
      reportsDirectory: 'coverage',
      
      // Coverage thresholds to ensure quality
      thresholds: {
        lines: 90,
        functions: 90,
        branches: 85,
        statements: 90
      },
      
      // Include patterns
      include: [
        'src/**/*.{ts,tsx}',
        '!src/test/**',
        '!src/**/*.d.ts'
      ],
      
      exclude: [
        'node_modules/',
        'src/test/',
        '**/*.d.ts',
        '**/*.config.*',
        'dist/',
        'src/main.tsx',
        'src/vite-env.d.ts',
        '**/__tests__/**',
        '**/*.test.{ts,tsx}',
        '**/*.spec.{ts,tsx}'
      ],
      
      // All files reporting
      all: true,
      
      // Skip full coverage for specific files
      skipFull: false
    },
    
    // Test environment configuration
    env: {
      NODE_ENV: 'test',
      VITE_APP_ENV: 'test',
      VITE_API_URL: 'http://localhost:8000',
      VITE_WS_URL: 'ws://localhost:8000'
    },
    
    // Mock configuration
    server: {
      deps: {
        external: ['@xterm/xterm', '@xterm/addon-*']
      }
    },
    
    // Mock CSS and other non-JS imports
    assetsInclude: [],
    
    // Pool configuration for stability
    pool: 'threads',
    poolOptions: {
      threads: {
        singleThread: true
      }
    },
    
    // Watch configuration
    watch: {
      ignored: ['**/node_modules/**', '**/dist/**', '**/coverage/**']
    }
  }
})