import React from 'react';

/**
 * Performance monitoring and optimization utilities for terminal components
 */

interface PerformanceMetrics {
  renderTime: number;
  memoryUsage: number;
  frameRate: number;
  terminalDataThroughput: number;
  wsLatency: number;
}

interface PerformanceConfig {
  maxBufferSize: number;
  throttleInterval: number;
  maxFrameRate: number;
  enableMetrics: boolean;
  enableProfiling: boolean;
}

class PerformanceMonitor {
  private metrics: PerformanceMetrics[] = [];
  private config: PerformanceConfig;
  private frameCount = 0;
  private lastFrameTime = 0;
  private renderObserver?: PerformanceObserver;
  private memoryMonitorInterval?: number;

  constructor(config: Partial<PerformanceConfig> = {}) {
    this.config = {
      maxBufferSize: 10000,
      throttleInterval: 16, // ~60fps
      maxFrameRate: 60,
      enableMetrics: true,
      enableProfiling: false,
      ...config
    };

    if (this.config.enableMetrics) {
      this.startMonitoring();
    }
  }

  private startMonitoring() {
    // Monitor render performance
    if ('PerformanceObserver' in window) {
      this.renderObserver = new PerformanceObserver((list) => {
        const entries = list.getEntries();
        entries.forEach((entry) => {
          if (entry.entryType === 'measure' && entry.name.includes('terminal')) {
            this.recordRenderTime(entry.duration);
          }
        });
      });

      this.renderObserver.observe({ entryTypes: ['measure'] });
    }

    // Monitor memory usage
    this.memoryMonitorInterval = window.setInterval(() => {
      this.recordMemoryUsage();
    }, 5000);

    // Monitor frame rate
    this.startFrameRateMonitoring();
  }

  private startFrameRateMonitoring() {
    const measureFrameRate = (timestamp: number) => {
      if (this.lastFrameTime) {
        const delta = timestamp - this.lastFrameTime;
        const fps = 1000 / delta;
        this.recordFrameRate(fps);
      }
      this.lastFrameTime = timestamp;
      this.frameCount++;

      if (this.config.enableMetrics) {
        requestAnimationFrame(measureFrameRate);
      }
    };

    requestAnimationFrame(measureFrameRate);
  }

  recordRenderTime(duration: number) {
    if (!this.config.enableMetrics) return;

    const currentMetrics = this.getCurrentMetrics();
    currentMetrics.renderTime = duration;
    this.updateMetrics(currentMetrics);
  }

  recordMemoryUsage() {
    if (!this.config.enableMetrics) return;

    const memory = (performance as any).memory;
    if (memory) {
      const memoryUsage = memory.usedJSHeapSize / 1024 / 1024; // MB
      const currentMetrics = this.getCurrentMetrics();
      currentMetrics.memoryUsage = memoryUsage;
      this.updateMetrics(currentMetrics);
    }
  }

  recordFrameRate(fps: number) {
    if (!this.config.enableMetrics) return;

    const currentMetrics = this.getCurrentMetrics();
    currentMetrics.frameRate = fps;
    this.updateMetrics(currentMetrics);
  }

  recordDataThroughput(bytesPerSecond: number) {
    if (!this.config.enableMetrics) return;

    const currentMetrics = this.getCurrentMetrics();
    currentMetrics.terminalDataThroughput = bytesPerSecond;
    this.updateMetrics(currentMetrics);
  }

  recordWSLatency(latency: number) {
    if (!this.config.enableMetrics) return;

    const currentMetrics = this.getCurrentMetrics();
    currentMetrics.wsLatency = latency;
    this.updateMetrics(currentMetrics);
  }

  private getCurrentMetrics(): PerformanceMetrics {
    return this.metrics[this.metrics.length - 1] || {
      renderTime: 0,
      memoryUsage: 0,
      frameRate: 0,
      terminalDataThroughput: 0,
      wsLatency: 0
    };
  }

  private updateMetrics(metrics: PerformanceMetrics) {
    this.metrics.push({ ...metrics });
    
    // Keep buffer size manageable
    if (this.metrics.length > this.config.maxBufferSize) {
      this.metrics = this.metrics.slice(-this.config.maxBufferSize / 2);
    }
  }

  getAverageMetrics(samples = 100): PerformanceMetrics {
    const recentMetrics = this.metrics.slice(-samples);
    if (recentMetrics.length === 0) {
      return {
        renderTime: 0,
        memoryUsage: 0,
        frameRate: 0,
        terminalDataThroughput: 0,
        wsLatency: 0
      };
    }

    const totals = recentMetrics.reduce((acc, metric) => ({
      renderTime: acc.renderTime + metric.renderTime,
      memoryUsage: acc.memoryUsage + metric.memoryUsage,
      frameRate: acc.frameRate + metric.frameRate,
      terminalDataThroughput: acc.terminalDataThroughput + metric.terminalDataThroughput,
      wsLatency: acc.wsLatency + metric.wsLatency
    }), {
      renderTime: 0,
      memoryUsage: 0,
      frameRate: 0,
      terminalDataThroughput: 0,
      wsLatency: 0
    });

    return {
      renderTime: totals.renderTime / recentMetrics.length,
      memoryUsage: totals.memoryUsage / recentMetrics.length,
      frameRate: totals.frameRate / recentMetrics.length,
      terminalDataThroughput: totals.terminalDataThroughput / recentMetrics.length,
      wsLatency: totals.wsLatency / recentMetrics.length
    };
  }

  getPerformanceReport(): {
    current: PerformanceMetrics;
    average: PerformanceMetrics;
    issues: string[];
  } {
    const current = this.getCurrentMetrics();
    const average = this.getAverageMetrics();
    const issues: string[] = [];

    // Detect performance issues
    if (average.renderTime > 16) {
      issues.push(`High render time: ${average.renderTime.toFixed(2)}ms (target: <16ms)`);
    }

    if (average.frameRate < 30) {
      issues.push(`Low frame rate: ${average.frameRate.toFixed(1)}fps (target: >30fps)`);
    }

    if (average.memoryUsage > 100) {
      issues.push(`High memory usage: ${average.memoryUsage.toFixed(1)}MB (target: <100MB)`);
    }

    if (average.wsLatency > 100) {
      issues.push(`High WebSocket latency: ${average.wsLatency.toFixed(1)}ms (target: <100ms)`);
    }

    return { current, average, issues };
  }

  startProfiling(label: string) {
    if (this.config.enableProfiling) {
      performance.mark(`${label}-start`);
    }
  }

  endProfiling(label: string) {
    if (this.config.enableProfiling) {
      performance.mark(`${label}-end`);
      performance.measure(label, `${label}-start`, `${label}-end`);
    }
  }

  cleanup() {
    if (this.renderObserver) {
      this.renderObserver.disconnect();
    }

    if (this.memoryMonitorInterval) {
      clearInterval(this.memoryMonitorInterval);
    }

    this.config.enableMetrics = false;
    this.metrics = [];
  }
}

// Performance optimization utilities

/**
 * Throttle function for high-frequency events
 */
export function throttle<T extends (...args: any[]) => any>(
  func: T,
  delay: number
): (...args: Parameters<T>) => void {
  let timeoutId: number | undefined;
  let lastExecTime = 0;

  return (...args: Parameters<T>) => {
    const currentTime = Date.now();

    if (currentTime - lastExecTime > delay) {
      func(...args);
      lastExecTime = currentTime;
    } else {
      clearTimeout(timeoutId);
      timeoutId = window.setTimeout(() => {
        func(...args);
        lastExecTime = Date.now();
      }, delay - (currentTime - lastExecTime));
    }
  };
}

/**
 * Debounce function for user input events
 */
export function debounce<T extends (...args: any[]) => any>(
  func: T,
  delay: number
): (...args: Parameters<T>) => void {
  let timeoutId: number | undefined;

  return (...args: Parameters<T>) => {
    clearTimeout(timeoutId);
    timeoutId = window.setTimeout(() => func(...args), delay);
  };
}

/**
 * Batch processing for terminal data
 */
export class DataBatcher {
  private buffer: string[] = [];
  private flushTimeout?: number;
  private readonly maxBufferSize: number;
  private readonly flushInterval: number;
  private readonly onFlush: (data: string) => void;

  constructor(
    onFlush: (data: string) => void,
    maxBufferSize = 1000,
    flushInterval = 16 // ~60fps
  ) {
    this.onFlush = onFlush;
    this.maxBufferSize = maxBufferSize;
    this.flushInterval = flushInterval;
  }

  add(data: string) {
    this.buffer.push(data);

    // Flush immediately if buffer is full
    if (this.buffer.length >= this.maxBufferSize) {
      this.flush();
    } else {
      // Schedule flush if not already scheduled
      if (!this.flushTimeout) {
        this.flushTimeout = window.setTimeout(() => {
          this.flush();
        }, this.flushInterval);
      }
    }
  }

  flush() {
    if (this.buffer.length === 0) return;

    const data = this.buffer.join('');
    this.buffer = [];

    if (this.flushTimeout) {
      clearTimeout(this.flushTimeout);
      this.flushTimeout = undefined;
    }

    this.onFlush(data);
  }

  clear() {
    this.buffer = [];
    if (this.flushTimeout) {
      clearTimeout(this.flushTimeout);
      this.flushTimeout = undefined;
    }
  }
}

/**
 * Lazy loading utility for heavy components
 */
export function createLazyComponent<T>(
  importFn: () => Promise<{ default: T }>,
  fallback?: React.ComponentType
) {
  return React.lazy(importFn);
}

/**
 * Memory-efficient circular buffer for terminal history
 */
export class CircularBuffer<T> {
  private buffer: T[];
  private head = 0;
  private tail = 0;
  private size = 0;
  private readonly capacity: number;

  constructor(capacity: number) {
    this.capacity = capacity;
    this.buffer = new Array(capacity);
  }

  push(item: T): void {
    this.buffer[this.tail] = item;
    this.tail = (this.tail + 1) % this.capacity;

    if (this.size < this.capacity) {
      this.size++;
    } else {
      this.head = (this.head + 1) % this.capacity;
    }
  }

  get(index: number): T | undefined {
    if (index < 0 || index >= this.size) return undefined;
    return this.buffer[(this.head + index) % this.capacity];
  }

  toArray(): T[] {
    const result: T[] = [];
    for (let i = 0; i < this.size; i++) {
      result.push(this.buffer[(this.head + i) % this.capacity]);
    }
    return result;
  }

  clear(): void {
    this.head = 0;
    this.tail = 0;
    this.size = 0;
  }

  getSize(): number {
    return this.size;
  }

  getCapacity(): number {
    return this.capacity;
  }
}

// Create global performance monitor instance
export const performanceMonitor = new PerformanceMonitor({
  enableMetrics: process.env.NODE_ENV === 'development',
  enableProfiling: process.env.NODE_ENV === 'development'
});

// Cleanup on page unload
if (typeof window !== 'undefined') {
  window.addEventListener('beforeunload', () => {
    performanceMonitor.cleanup();
  });
}

export { PerformanceMonitor, type PerformanceMetrics, type PerformanceConfig };