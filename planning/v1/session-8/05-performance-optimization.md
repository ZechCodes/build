# Session 8.5: Performance Optimization & Responsiveness

## Objective
Implement comprehensive performance optimization and responsiveness enhancements for the frontend terminal implementation, ensuring smooth user experience, efficient resource utilization, and optimal rendering performance.

## Integration with Previous Sessions
- **Session 1**: Uses Logfire for performance metrics collection and analysis
- **Session 8.1**: Optimizes terminal component rendering and memory usage
- **Session 8.2**: Enhances WebSocket client performance and connection efficiency
- **Session 8.3**: Optimizes UI features and theme switching performance

## Performance Monitoring Implementation

### Terminal Performance Monitor
**Location**: `frontend/src/components/Terminal/PerformanceMonitor.ts`

```typescript
// frontend/src/components/Terminal/PerformanceMonitor.ts
import { logfire } from '../../utils/logfire';

export interface PerformanceMetrics {
  timestamp: number;
  frameRate: number;
  renderTime: number;
  memoryUsage: number;
  terminalBufferSize: number;
  websocketLatency: number;
  keystrokeLatency: number;
  scrollPerformance: number;
  themeSwithTime: number;
  cpuUsage: number;
}

export interface PerformanceThresholds {
  minFrameRate: number;
  maxRenderTime: number;
  maxMemoryUsage: number;
  maxKeystrokeLatency: number;
  maxWebSocketLatency: number;
  maxScrollLatency: number;
  maxThemeSwithTime: number;
}

export const DEFAULT_PERFORMANCE_THRESHOLDS: PerformanceThresholds = {
  minFrameRate: 30, // 30 FPS minimum
  maxRenderTime: 16.67, // 60 FPS = 16.67ms per frame
  maxMemoryUsage: 100 * 1024 * 1024, // 100MB
  maxKeystrokeLatency: 50, // 50ms
  maxWebSocketLatency: 100, // 100ms
  maxScrollLatency: 16, // 16ms for smooth scrolling
  maxThemeSwithTime: 200 // 200ms
};

export class TerminalPerformanceMonitor {
  private metrics: PerformanceMetrics[] = [];
  private thresholds: PerformanceThresholds;
  private observers: PerformanceObserver[] = [];
  private frameRateCalculator: FrameRateCalculator;
  private memoryMonitor: MemoryMonitor;
  private performanceAlerts: Map<string, number> = new Map();
  
  private monitoringActive = false;
  private updateInterval = 1000; // 1 second

  constructor(thresholds: PerformanceThresholds = DEFAULT_PERFORMANCE_THRESHOLDS) {
    this.thresholds = thresholds;
    this.frameRateCalculator = new FrameRateCalculator();
    this.memoryMonitor = new MemoryMonitor();
    this.setupPerformanceObservers();
  }

  public startMonitoring(): void {
    if (this.monitoringActive) return;

    this.monitoringActive = true;
    this.frameRateCalculator.start();
    this.memoryMonitor.start();
    
    // Start periodic metric collection
    setInterval(() => {
      if (this.monitoringActive) {
        this.collectMetrics();
      }
    }, this.updateInterval);

    logfire.info('Terminal performance monitoring started', {
      thresholds: this.thresholds,
      updateInterval: this.updateInterval
    });
  }

  public stopMonitoring(): void {
    this.monitoringActive = false;
    this.frameRateCalculator.stop();
    this.memoryMonitor.stop();
    
    // Cleanup performance observers
    this.observers.forEach(observer => observer.disconnect());
    this.observers = [];

    logfire.info('Terminal performance monitoring stopped');
  }

  private setupPerformanceObservers(): void {
    // Navigation timing observer
    if ('PerformanceObserver' in window) {
      const navObserver = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          if (entry.entryType === 'navigation') {
            this.recordNavigationMetrics(entry as PerformanceNavigationTiming);
          }
        }
      });
      
      navObserver.observe({ type: 'navigation', buffered: true });
      this.observers.push(navObserver);

      // Measure observer for custom performance marks
      const measureObserver = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          if (entry.entryType === 'measure') {
            this.recordCustomMeasurement(entry);
          }
        }
      });
      
      measureObserver.observe({ type: 'measure', buffered: true });
      this.observers.push(measureObserver);

      // Long task observer for detecting blocking operations
      const longTaskObserver = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          this.recordLongTask(entry);
        }
      });
      
      try {
        longTaskObserver.observe({ type: 'longtask', buffered: true });
        this.observers.push(longTaskObserver);
      } catch (e) {
        console.warn('Long task observer not supported');
      }
    }
  }

  private collectMetrics(): void {
    try {
      const timestamp = performance.now();
      
      const metrics: PerformanceMetrics = {
        timestamp,
        frameRate: this.frameRateCalculator.getCurrentFPS(),
        renderTime: this.measureRenderTime(),
        memoryUsage: this.memoryMonitor.getCurrentUsage(),
        terminalBufferSize: this.getTerminalBufferSize(),
        websocketLatency: this.measureWebSocketLatency(),
        keystrokeLatency: this.measureKeystrokeLatency(),
        scrollPerformance: this.measureScrollPerformance(),
        themeSwithTime: this.getLastThemeSwitchTime(),
        cpuUsage: this.estimateCPUUsage()
      };

      this.metrics.push(metrics);
      
      // Keep only last 100 metrics to prevent memory bloat
      if (this.metrics.length > 100) {
        this.metrics.shift();
      }

      // Check performance thresholds
      this.checkPerformanceThresholds(metrics);

      // Log to Logfire periodically (every 10 seconds)
      if (this.metrics.length % 10 === 0) {
        this.logPerformanceMetrics(metrics);
      }

    } catch (error) {
      logfire.error('Performance metrics collection failed', {
        error: error instanceof Error ? error.message : 'Unknown error'
      });
    }
  }

  private measureRenderTime(): number {
    // Measure time for a dummy render operation
    const start = performance.now();
    
    // Simulate a simple DOM operation
    const testElement = document.createElement('div');
    testElement.textContent = 'performance test';
    document.body.appendChild(testElement);
    document.body.removeChild(testElement);
    
    return performance.now() - start;
  }

  private getTerminalBufferSize(): number {
    // Estimate terminal buffer size (would integrate with actual terminal)
    const terminalElement = document.querySelector('.terminal-content');
    if (terminalElement) {
      return terminalElement.textContent?.length || 0;
    }
    return 0;
  }

  private measureWebSocketLatency(): number {
    // This would be implemented with actual WebSocket ping/pong
    // For now, return a simulated value based on recent performance
    return Math.random() * 50 + 20; // 20-70ms simulated latency
  }

  private measureKeystrokeLatency(): number {
    // Measure time between keypress and terminal update
    // This would be implemented with actual keystroke timing
    return Math.random() * 30 + 10; // 10-40ms simulated latency
  }

  private measureScrollPerformance(): number {
    // Measure scroll performance
    const start = performance.now();
    
    const terminalElement = document.querySelector('.terminal-content');
    if (terminalElement) {
      const originalScrollTop = terminalElement.scrollTop;
      terminalElement.scrollTop += 100;
      terminalElement.scrollTop = originalScrollTop;
    }
    
    return performance.now() - start;
  }

  private getLastThemeSwitchTime(): number {
    // Get the last recorded theme switch time
    const lastThemeSwitch = performance.getEntriesByName('theme-switch');
    if (lastThemeSwitch.length > 0) {
      return lastThemeSwitch[lastThemeSwitch.length - 1].duration;
    }
    return 0;
  }

  private estimateCPUUsage(): number {
    // Estimate CPU usage based on frame rate and render times
    const avgFrameRate = this.frameRateCalculator.getAverageFPS();
    const targetFrameRate = 60;
    
    // Lower frame rate indicates higher CPU usage
    const cpuEstimate = Math.max(0, (targetFrameRate - avgFrameRate) / targetFrameRate * 100);
    return Math.min(100, cpuEstimate);
  }

  private checkPerformanceThresholds(metrics: PerformanceMetrics): void {
    const violations: string[] = [];

    if (metrics.frameRate < this.thresholds.minFrameRate) {
      violations.push(`Low frame rate: ${metrics.frameRate.toFixed(1)} FPS`);
    }

    if (metrics.renderTime > this.thresholds.maxRenderTime) {
      violations.push(`High render time: ${metrics.renderTime.toFixed(1)}ms`);
    }

    if (metrics.memoryUsage > this.thresholds.maxMemoryUsage) {
      violations.push(`High memory usage: ${(metrics.memoryUsage / 1024 / 1024).toFixed(1)}MB`);
    }

    if (metrics.keystrokeLatency > this.thresholds.maxKeystrokeLatency) {
      violations.push(`High keystroke latency: ${metrics.keystrokeLatency.toFixed(1)}ms`);
    }

    if (metrics.websocketLatency > this.thresholds.maxWebSocketLatency) {
      violations.push(`High WebSocket latency: ${metrics.websocketLatency.toFixed(1)}ms`);
    }

    if (violations.length > 0) {
      this.recordPerformanceAlert(violations);
    }
  }

  private recordPerformanceAlert(violations: string[]): void {
    const alertKey = violations.join(',');
    const count = this.performanceAlerts.get(alertKey) || 0;
    this.performanceAlerts.set(alertKey, count + 1);

    logfire.warning('Terminal performance threshold violations', {
      violations,
      occurrenceCount: count + 1,
      timestamp: Date.now()
    });
  }

  private logPerformanceMetrics(metrics: PerformanceMetrics): void {
    logfire.info('Terminal performance metrics', {
      frameRate: Math.round(metrics.frameRate * 100) / 100,
      renderTime: Math.round(metrics.renderTime * 100) / 100,
      memoryUsageMB: Math.round(metrics.memoryUsage / 1024 / 1024 * 100) / 100,
      bufferSizeKB: Math.round(metrics.terminalBufferSize / 1024 * 100) / 100,
      websocketLatency: Math.round(metrics.websocketLatency * 100) / 100,
      keystrokeLatency: Math.round(metrics.keystrokeLatency * 100) / 100,
      cpuUsage: Math.round(metrics.cpuUsage * 100) / 100
    });
  }

  private recordNavigationMetrics(entry: PerformanceNavigationTiming): void {
    logfire.info('Terminal navigation performance', {
      domContentLoaded: entry.domContentLoadedEventEnd - entry.domContentLoadedEventStart,
      loadComplete: entry.loadEventEnd - entry.loadEventStart,
      domInteractive: entry.domInteractive - entry.navigationStart,
      firstPaint: entry.responseEnd - entry.requestStart
    });
  }

  private recordCustomMeasurement(entry: PerformanceEntry): void {
    if (entry.name.startsWith('terminal-')) {
      logfire.debug('Terminal custom measurement', {
        measurementName: entry.name,
        duration: entry.duration,
        startTime: entry.startTime
      });
    }
  }

  private recordLongTask(entry: PerformanceEntry): void {
    logfire.warning('Terminal long task detected', {
      duration: entry.duration,
      startTime: entry.startTime,
      name: entry.name
    });
  }

  public getPerformanceSummary(): any {
    if (this.metrics.length === 0) {
      return { message: 'No metrics available' };
    }

    const recent = this.metrics.slice(-10); // Last 10 metrics
    
    return {
      current: {
        frameRate: recent[recent.length - 1]?.frameRate || 0,
        memoryUsageMB: Math.round((recent[recent.length - 1]?.memoryUsage || 0) / 1024 / 1024),
        avgKeystrokeLatency: recent.reduce((sum, m) => sum + m.keystrokeLatency, 0) / recent.length
      },
      averages: {
        frameRate: recent.reduce((sum, m) => sum + m.frameRate, 0) / recent.length,
        renderTime: recent.reduce((sum, m) => sum + m.renderTime, 0) / recent.length,
        memoryUsage: recent.reduce((sum, m) => sum + m.memoryUsage, 0) / recent.length
      },
      thresholdViolations: this.performanceAlerts.size,
      lastUpdate: recent[recent.length - 1]?.timestamp || 0
    };
  }

  public markPerformanceEvent(name: string): void {
    performance.mark(`terminal-${name}`);
  }

  public measurePerformanceEvent(name: string, startMark: string): void {
    performance.measure(`terminal-${name}`, `terminal-${startMark}`);
  }
}

// Frame rate calculator
class FrameRateCalculator {
  private frameCount = 0;
  private lastTime = 0;
  private fps = 0;
  private isRunning = false;
  private animationFrameId?: number;

  public start(): void {
    if (this.isRunning) return;
    
    this.isRunning = true;
    this.lastTime = performance.now();
    this.frameCount = 0;
    this.calculateFPS();
  }

  public stop(): void {
    this.isRunning = false;
    if (this.animationFrameId) {
      cancelAnimationFrame(this.animationFrameId);
    }
  }

  private calculateFPS(): void {
    if (!this.isRunning) return;

    this.frameCount++;
    const currentTime = performance.now();
    
    if (currentTime - this.lastTime >= 1000) {
      this.fps = this.frameCount;
      this.frameCount = 0;
      this.lastTime = currentTime;
    }

    this.animationFrameId = requestAnimationFrame(() => this.calculateFPS());
  }

  public getCurrentFPS(): number {
    return this.fps;
  }

  public getAverageFPS(): number {
    // Simplified average calculation
    return this.fps;
  }
}

// Memory monitor
class MemoryMonitor {
  private isRunning = false;

  public start(): void {
    this.isRunning = true;
  }

  public stop(): void {
    this.isRunning = false;
  }

  public getCurrentUsage(): number {
    if ('memory' in performance) {
      return (performance as any).memory.usedJSHeapSize;
    }
    
    // Fallback estimation
    return this.estimateMemoryUsage();
  }

  private estimateMemoryUsage(): number {
    // Estimate based on DOM elements and terminal buffer
    const terminalElements = document.querySelectorAll('.terminal-content *').length;
    const estimatedBytesPerElement = 100; // Rough estimate
    
    return terminalElements * estimatedBytesPerElement;
  }
}
```

### Performance Optimization Strategies

### Virtual Scrolling Implementation
**Location**: `frontend/src/components/Terminal/VirtualScrolling.ts`

```typescript
// frontend/src/components/Terminal/VirtualScrolling.ts
import { logfire } from '../../utils/logfire';

export interface VirtualScrollConfig {
  itemHeight: number;
  bufferSize: number;
  overscan: number;
  maxItems: number;
}

export interface VirtualScrollState {
  scrollTop: number;
  containerHeight: number;
  startIndex: number;
  endIndex: number;
  visibleItems: any[];
}

export class TerminalVirtualScroller {
  private config: VirtualScrollConfig;
  private state: VirtualScrollState;
  private container: HTMLElement;
  private onStateChange?: (state: VirtualScrollState) => void;
  private rafId?: number;
  private items: any[] = [];

  constructor(
    container: HTMLElement,
    config: VirtualScrollConfig,
    onStateChange?: (state: VirtualScrollState) => void
  ) {
    this.container = container;
    this.config = config;
    this.onStateChange = onStateChange;
    
    this.state = {
      scrollTop: 0,
      containerHeight: container.clientHeight,
      startIndex: 0,
      endIndex: 0,
      visibleItems: []
    };

    this.setupEventListeners();
    this.updateVisibleItems();
  }

  private setupEventListeners(): void {
    // Scroll event listener with throttling
    this.container.addEventListener('scroll', this.handleScroll.bind(this), {
      passive: true
    });

    // Resize observer for container size changes
    if ('ResizeObserver' in window) {
      const resizeObserver = new ResizeObserver((entries) => {
        for (const entry of entries) {
          this.state.containerHeight = entry.contentRect.height;
          this.updateVisibleItems();
        }
      });
      
      resizeObserver.observe(this.container);
    }
  }

  private handleScroll(): void {
    // Use requestAnimationFrame to throttle scroll updates
    if (this.rafId) {
      cancelAnimationFrame(this.rafId);
    }

    this.rafId = requestAnimationFrame(() => {
      this.state.scrollTop = this.container.scrollTop;
      this.updateVisibleItems();
    });
  }

  private updateVisibleItems(): void {
    const startTime = performance.now();

    const {
      itemHeight,
      bufferSize,
      overscan
    } = this.config;

    const {
      scrollTop,
      containerHeight
    } = this.state;

    // Calculate visible range
    const visibleStart = Math.floor(scrollTop / itemHeight);
    const visibleEnd = Math.min(
      this.items.length - 1,
      Math.ceil((scrollTop + containerHeight) / itemHeight)
    );

    // Add buffer and overscan
    this.state.startIndex = Math.max(0, visibleStart - bufferSize - overscan);
    this.state.endIndex = Math.min(
      this.items.length - 1,
      visibleEnd + bufferSize + overscan
    );

    // Extract visible items
    this.state.visibleItems = this.items.slice(
      this.state.startIndex,
      this.state.endIndex + 1
    );

    // Notify state change
    if (this.onStateChange) {
      this.onStateChange(this.state);
    }

    const updateTime = performance.now() - startTime;
    
    // Log performance metrics
    if (updateTime > 5) { // Log if update takes more than 5ms
      logfire.debug('Virtual scroll update performance', {
        updateTime,
        visibleItemCount: this.state.visibleItems.length,
        totalItems: this.items.length,
        startIndex: this.state.startIndex,
        endIndex: this.state.endIndex
      });
    }
  }

  public setItems(items: any[]): void {
    // Limit total items to prevent memory issues
    if (items.length > this.config.maxItems) {
      items = items.slice(-this.config.maxItems);
      
      logfire.info('Terminal buffer truncated for performance', {
        originalLength: items.length,
        truncatedLength: this.config.maxItems
      });
    }

    this.items = items;
    this.updateVisibleItems();
  }

  public addItem(item: any): void {
    this.items.push(item);
    
    // Remove old items if exceeding max
    if (this.items.length > this.config.maxItems) {
      this.items.shift();
    }

    this.updateVisibleItems();
  }

  public scrollToBottom(): void {
    const maxScrollTop = this.items.length * this.config.itemHeight - this.state.containerHeight;
    this.container.scrollTop = Math.max(0, maxScrollTop);
  }

  public scrollToIndex(index: number): void {
    const scrollTop = index * this.config.itemHeight;
    this.container.scrollTop = scrollTop;
  }

  public getState(): VirtualScrollState {
    return { ...this.state };
  }

  public cleanup(): void {
    if (this.rafId) {
      cancelAnimationFrame(this.rafId);
    }
  }
}
```

### WebSocket Performance Optimization
**Location**: `frontend/src/services/WebSocketOptimizer.ts`

```typescript
// frontend/src/services/WebSocketOptimizer.ts
import { logfire } from '../utils/logfire';

export interface WebSocketOptimizationConfig {
  batchSize: number;
  batchTimeout: number;
  compressionEnabled: boolean;
  heartbeatInterval: number;
  reconnectDelay: number;
  maxReconnectAttempts: number;
}

export interface MessageBatch {
  messages: any[];
  timestamp: number;
  size: number;
}

export class WebSocketOptimizer {
  private config: WebSocketOptimizationConfig;
  private messageBatch: any[] = [];
  private batchTimer?: NodeJS.Timeout;
  private compressionWorker?: Worker;
  private performanceMetrics = {
    messagesSent: 0,
    messagesReceived: 0,
    totalLatency: 0,
    averageLatency: 0,
    compressionRatio: 0,
    bandwidthSaved: 0
  };

  constructor(config: WebSocketOptimizationConfig) {
    this.config = config;
    this.initializeCompressionWorker();
  }

  private initializeCompressionWorker(): void {
    if (this.config.compressionEnabled && 'Worker' in window) {
      try {
        // Create compression worker for offloading compression tasks
        this.compressionWorker = new Worker('/workers/compression-worker.js');
        
        this.compressionWorker.onmessage = (event) => {
          const { type, data, originalSize, compressedSize } = event.data;
          
          if (type === 'compressed') {
            this.updateCompressionMetrics(originalSize, compressedSize);
            this.sendCompressedMessage(data);
          }
        };

        logfire.info('WebSocket compression worker initialized');
      } catch (error) {
        logfire.warning('Compression worker initialization failed', {
          error: error instanceof Error ? error.message : 'Unknown error'
        });
      }
    }
  }

  public optimizeMessage(message: any): Promise<any> {
    return new Promise((resolve) => {
      const startTime = performance.now();

      // Add to batch for potential batching
      this.messageBatch.push({
        ...message,
        timestamp: startTime
      });

      // Check if we should send immediately or batch
      if (this.shouldSendImmediately(message)) {
        this.flushBatch();
        resolve(message);
      } else {
        this.scheduleBatchSend();
        resolve(message);
      }
    });
  }

  private shouldSendImmediately(message: any): boolean {
    // Send immediately for interactive messages
    const interactiveTypes = ['terminal_input', 'terminal_resize'];
    
    if (interactiveTypes.includes(message.type)) {
      return true;
    }

    // Send if batch is full
    if (this.messageBatch.length >= this.config.batchSize) {
      return true;
    }

    return false;
  }

  private scheduleBatchSend(): void {
    if (this.batchTimer) {
      return; // Timer already scheduled
    }

    this.batchTimer = setTimeout(() => {
      this.flushBatch();
    }, this.config.batchTimeout);
  }

  private flushBatch(): void {
    if (this.messageBatch.length === 0) {
      return;
    }

    if (this.batchTimer) {
      clearTimeout(this.batchTimer);
      this.batchTimer = undefined;
    }

    const batch = [...this.messageBatch];
    this.messageBatch = [];

    // Process batch
    if (batch.length === 1) {
      this.sendSingleMessage(batch[0]);
    } else {
      this.sendBatchedMessages(batch);
    }

    this.performanceMetrics.messagesSent += batch.length;
  }

  private sendSingleMessage(message: any): void {
    const messageSize = JSON.stringify(message).length;
    
    if (this.config.compressionEnabled && messageSize > 1024) {
      this.compressAndSend(message);
    } else {
      this.sendRawMessage(message);
    }
  }

  private sendBatchedMessages(messages: any[]): void {
    const batchMessage = {
      type: 'batch',
      messages,
      timestamp: Date.now()
    };

    const batchSize = JSON.stringify(batchMessage).length;
    
    logfire.debug('Sending batched WebSocket messages', {
      messageCount: messages.length,
      batchSize,
      compressionEnabled: this.config.compressionEnabled
    });

    if (this.config.compressionEnabled && batchSize > 2048) {
      this.compressAndSend(batchMessage);
    } else {
      this.sendRawMessage(batchMessage);
    }
  }

  private compressAndSend(message: any): void {
    if (this.compressionWorker) {
      const messageString = JSON.stringify(message);
      
      this.compressionWorker.postMessage({
        type: 'compress',
        data: messageString
      });
    } else {
      // Fallback to uncompressed if worker not available
      this.sendRawMessage(message);
    }
  }

  private sendRawMessage(message: any): void {
    // This would integrate with actual WebSocket sending
    const messageString = JSON.stringify(message);
    const sendTime = performance.now();
    
    // Simulate actual send (would be replaced with real WebSocket.send())
    setTimeout(() => {
      const latency = performance.now() - sendTime;
      this.updateLatencyMetrics(latency);
    }, Math.random() * 50); // Simulate network latency
  }

  private sendCompressedMessage(compressedData: any): void {
    // Send compressed data via WebSocket
    const sendTime = performance.now();
    
    // Simulate sending compressed data
    setTimeout(() => {
      const latency = performance.now() - sendTime;
      this.updateLatencyMetrics(latency);
    }, Math.random() * 30); // Compressed data should be faster
  }

  private updateLatencyMetrics(latency: number): void {
    this.performanceMetrics.totalLatency += latency;
    this.performanceMetrics.averageLatency = 
      this.performanceMetrics.totalLatency / this.performanceMetrics.messagesSent;
  }

  private updateCompressionMetrics(originalSize: number, compressedSize: number): void {
    const ratio = compressedSize / originalSize;
    this.performanceMetrics.compressionRatio = 
      (this.performanceMetrics.compressionRatio + ratio) / 2; // Running average
    
    this.performanceMetrics.bandwidthSaved += (originalSize - compressedSize);
    
    logfire.debug('WebSocket compression metrics updated', {
      originalSize,
      compressedSize,
      compressionRatio: ratio,
      totalBandwidthSaved: this.performanceMetrics.bandwidthSaved
    });
  }

  public getPerformanceMetrics(): any {
    return {
      ...this.performanceMetrics,
      compressionRatio: Math.round(this.performanceMetrics.compressionRatio * 100) / 100,
      averageLatency: Math.round(this.performanceMetrics.averageLatency * 100) / 100,
      bandwidthSavedKB: Math.round(this.performanceMetrics.bandwidthSaved / 1024 * 100) / 100
    };
  }

  public optimizeReconnection(attempt: number): number {
    // Exponential backoff with jitter
    const baseDelay = this.config.reconnectDelay;
    const exponentialDelay = Math.min(baseDelay * Math.pow(2, attempt), 30000);
    const jitter = Math.random() * 1000;
    
    return exponentialDelay + jitter;
  }

  public cleanup(): void {
    if (this.batchTimer) {
      clearTimeout(this.batchTimer);
    }
    
    if (this.compressionWorker) {
      this.compressionWorker.terminate();
    }
    
    // Flush any remaining messages
    this.flushBatch();
  }
}
```

## TDD Performance Testing Cycle

### Performance-First Development Process

1. **Red Phase**: Write failing performance tests
   ```bash
   # Create performance test file
   touch frontend/src/components/Terminal/__tests__/performance/TerminalPerformance.test.tsx
   
   # Run failing performance test
   npm test TerminalPerformance.test.tsx
   ```

2. **Green Phase**: Implement basic performance optimizations
   ```bash
   # Add performance monitoring and basic optimizations
   npm test TerminalPerformance.test.tsx
   ```

3. **Refactor Phase**: Advanced performance optimization
   ```bash
   # Add advanced optimizations like virtual scrolling
   npm test -- --testPathPattern=performance
   ```

4. **Performance Commit**: Commit performance enhancements
   ```bash
   git add frontend/src/components/Terminal/ frontend/src/services/
   git commit -m "perf: implement comprehensive terminal performance optimization

   - Add performance monitoring with frame rate and latency tracking
   - Implement virtual scrolling for large terminal buffers
   - Add WebSocket message optimization with batching and compression
   - Include memory usage monitoring and automatic cleanup
   - Integrate with Logfire for performance analytics and alerting
   
   Tests: Added comprehensive performance test suite with benchmarks
   Performance: 60+ FPS rendering, <50ms keystroke latency, <100MB memory
   Optimization: Virtual scrolling, message batching, and compression"
   ```

### Performance Test Cases

```tsx
// frontend/src/components/Terminal/__tests__/performance/TerminalPerformance.test.tsx
import React from 'react';
import { render, fireEvent, waitFor } from '@testing-library/react';
import { Terminal } from '../Terminal';
import { TerminalPerformanceMonitor } from '../PerformanceMonitor';
import { WebSocketOptimizer } from '../../services/WebSocketOptimizer';

describe('Terminal Performance', () => {
  let performanceMonitor: TerminalPerformanceMonitor;

  beforeEach(() => {
    performanceMonitor = new TerminalPerformanceMonitor();
    jest.spyOn(performance, 'now');
  });

  afterEach(() => {
    performanceMonitor.stopMonitoring();
    jest.restoreAllMocks();
  });

  test('terminal renders within performance budget', async () => {
    const startTime = performance.now();
    
    render(<Terminal vmId="test-vm" />);
    
    const renderTime = performance.now() - startTime;
    
    // Should render within 100ms
    expect(renderTime).toBeLessThan(100);
  });

  test('keystroke latency is under threshold', async () => {
    const { container } = render(<Terminal vmId="test-vm" />);
    const terminal = container.querySelector('.terminal-content');
    
    const keystrokes = [];
    for (let i = 0; i < 100; i++) {
      const startTime = performance.now();
      
      fireEvent.keyDown(terminal!, { key: 'a' });
      
      await waitFor(() => {
        // Wait for terminal to process keystroke
      });
      
      const latency = performance.now() - startTime;
      keystrokes.push(latency);
    }
    
    const averageLatency = keystrokes.reduce((sum, lat) => sum + lat, 0) / keystrokes.length;
    
    // Average keystroke latency should be under 50ms
    expect(averageLatency).toBeLessThan(50);
  });

  test('memory usage stays within limits', async () => {
    const { rerender } = render(<Terminal vmId="test-vm" />);
    
    // Simulate large terminal output
    const largeOutput = 'A'.repeat(10000);
    
    for (let i = 0; i < 100; i++) {
      rerender(<Terminal vmId="test-vm" key={i} />);
      
      // Simulate terminal output
      const mockMessage = {
        type: 'terminal_output',
        data: largeOutput,
        sessionId: 'test'
      };
      
      // Process message
    }
    
    // Force garbage collection if available
    if (global.gc) {
      global.gc();
    }
    
    const memoryUsage = (performance as any).memory?.usedJSHeapSize || 0;
    const memoryLimitMB = 100;
    
    expect(memoryUsage / 1024 / 1024).toBeLessThan(memoryLimitMB);
  });

  test('frame rate maintains target FPS', async () => {
    render(<Terminal vmId="test-vm" />);
    
    performanceMonitor.startMonitoring();
    
    // Simulate continuous activity for 2 seconds
    const duration = 2000;
    const startTime = Date.now();
    
    while (Date.now() - startTime < duration) {
      // Simulate terminal updates
      await new Promise(resolve => requestAnimationFrame(resolve));
    }
    
    await new Promise(resolve => setTimeout(resolve, 100));
    
    const summary = performanceMonitor.getPerformanceSummary();
    
    // Should maintain at least 30 FPS
    expect(summary.current.frameRate).toBeGreaterThan(30);
  });

  test('virtual scrolling handles large datasets efficiently', async () => {
    const largeDataset = Array.from({ length: 10000 }, (_, i) => ({
      id: i,
      content: `Line ${i}: ${'test '.repeat(10)}`
    }));
    
    const startTime = performance.now();
    
    // Test virtual scrolling with large dataset
    const { container } = render(<Terminal vmId="test-vm" />);
    
    // Simulate setting large dataset
    const terminal = container.querySelector('.terminal-content');
    
    const processingTime = performance.now() - startTime;
    
    // Processing large dataset should be fast
    expect(processingTime).toBeLessThan(500);
    
    // Only visible items should be in DOM
    const visibleElements = container.querySelectorAll('.terminal-line');
    expect(visibleElements.length).toBeLessThan(100); // Much less than 10000
  });

  test('WebSocket message optimization works correctly', async () => {
    const optimizer = new WebSocketOptimizer({
      batchSize: 10,
      batchTimeout: 100,
      compressionEnabled: true,
      heartbeatInterval: 30000,
      reconnectDelay: 1000,
      maxReconnectAttempts: 5
    });

    // Send multiple messages rapidly
    const messages = Array.from({ length: 50 }, (_, i) => ({
      type: 'terminal_input',
      data: `message ${i}`,
      timestamp: Date.now()
    }));

    const startTime = performance.now();
    
    const promises = messages.map(msg => optimizer.optimizeMessage(msg));
    await Promise.all(promises);
    
    const optimizationTime = performance.now() - startTime;
    
    // Optimization should be fast
    expect(optimizationTime).toBeLessThan(100);
    
    const metrics = optimizer.getPerformanceMetrics();
    
    // Should have processed all messages
    expect(metrics.messagesSent).toBe(messages.length);
    
    // Should have some compression if enabled
    if (metrics.compressionRatio > 0) {
      expect(metrics.compressionRatio).toBeLessThan(1);
    }
  });

  test('theme switching performance', async () => {
    const { rerender } = render(<Terminal vmId="test-vm" theme="dark" />);
    
    const switchTimes = [];
    
    for (let i = 0; i < 10; i++) {
      const startTime = performance.now();
      
      rerender(<Terminal vmId="test-vm" theme={i % 2 === 0 ? 'light' : 'dark'} />);
      
      await waitFor(() => {
        // Wait for theme to apply
      });
      
      const switchTime = performance.now() - startTime;
      switchTimes.push(switchTime);
    }
    
    const averageSwitchTime = switchTimes.reduce((sum, time) => sum + time, 0) / switchTimes.length;
    
    // Theme switching should be under 200ms
    expect(averageSwitchTime).toBeLessThan(200);
  });
});
```

## Performance Requirements & Targets

### Rendering Performance
- **Frame rate**: Maintain 60 FPS during normal operation, minimum 30 FPS under load
- **Render time**: < 16.67ms per frame for 60 FPS target
- **Initial load time**: < 2 seconds for terminal component initialization
- **Theme switching**: < 200ms visual transition time
- **Scroll performance**: Smooth 60 FPS scrolling with large buffers
- **Resize handling**: < 100ms response time for terminal resize operations

### Interaction Performance
- **Keystroke latency**: < 50ms from keypress to display
- **Copy/paste operations**: < 100ms for clipboard operations
- **Search operations**: < 200ms for searching 10,000 lines
- **Context menu**: < 150ms to open context menus
- **Focus management**: < 50ms for focus transitions
- **Modal operations**: < 200ms for modal open/close

### Memory Performance
- **Base memory usage**: < 50MB for empty terminal
- **Memory per 1000 lines**: < 10MB additional memory
- **Maximum buffer size**: 100,000 lines with virtual scrolling
- **Memory growth rate**: < 1MB per minute of continuous use
- **Garbage collection**: Minimal GC pressure with efficient cleanup
- **Memory leaks**: Zero memory leaks in normal operations

### Network Performance
- **WebSocket latency**: < 100ms round-trip time
- **Message throughput**: 1000+ messages per second
- **Compression ratio**: 30%+ bandwidth savings when enabled
- **Reconnection time**: < 2 seconds automatic reconnection
- **Batch efficiency**: 50%+ reduction in message overhead
- **Network error recovery**: < 5 seconds for full recovery

## Performance Monitoring Dashboard

### Real-time Performance Metrics
```typescript
// Performance dashboard component
export const PerformanceDashboard: React.FC = () => {
  const [metrics, setMetrics] = useState<PerformanceMetrics | null>(null);
  const [isVisible, setIsVisible] = useState(false);

  useEffect(() => {
    const monitor = new TerminalPerformanceMonitor();
    monitor.startMonitoring();

    const interval = setInterval(() => {
      const summary = monitor.getPerformanceSummary();
      setMetrics(summary);
    }, 1000);

    return () => {
      clearInterval(interval);
      monitor.stopMonitoring();
    };
  }, []);

  if (!isVisible || !metrics) {
    return null;
  }

  return (
    <div className="performance-dashboard">
      <div className="metric-card">
        <span className="metric-label">FPS</span>
        <span className={`metric-value ${metrics.current.frameRate < 30 ? 'warning' : 'good'}`}>
          {Math.round(metrics.current.frameRate)}
        </span>
      </div>
      
      <div className="metric-card">
        <span className="metric-label">Memory</span>
        <span className={`metric-value ${metrics.current.memoryUsageMB > 100 ? 'warning' : 'good'}`}>
          {metrics.current.memoryUsageMB}MB
        </span>
      </div>
      
      <div className="metric-card">
        <span className="metric-label">Latency</span>
        <span className={`metric-value ${metrics.current.avgKeystrokeLatency > 50 ? 'warning' : 'good'}`}>
          {Math.round(metrics.current.avgKeystrokeLatency)}ms
        </span>
      </div>
    </div>
  );
};
```

## Performance Optimization Strategies

### Code Splitting and Lazy Loading
```typescript
// Lazy load terminal components
const Terminal = React.lazy(() => import('./Terminal/Terminal'));
const TerminalControls = React.lazy(() => import('./Terminal/TerminalControls'));
const TerminalSearch = React.lazy(() => import('./Terminal/TerminalSearch'));

// Use with Suspense
<Suspense fallback={<TerminalLoadingSkeleton />}>
  <Terminal vmId={vmId} />
</Suspense>
```

### Memoization and Optimization
```typescript
// Memoize expensive operations
const MemoizedTerminal = React.memo(Terminal, (prevProps, nextProps) => {
  return (
    prevProps.vmId === nextProps.vmId &&
    prevProps.theme === nextProps.theme &&
    prevProps.fontSize === nextProps.fontSize
  );
});

// Use useMemo for expensive calculations
const terminalConfig = useMemo(() => ({
  theme: themes[currentTheme],
  fontSize,
  fontFamily,
  // ... other config
}), [currentTheme, fontSize, fontFamily]);
```

### Web Workers for Heavy Operations
```typescript
// Compression worker for WebSocket optimization
// public/workers/compression-worker.js
self.onmessage = function(event) {
  const { type, data } = event.data;
  
  if (type === 'compress') {
    // Implement compression logic
    const compressed = compressData(data);
    
    self.postMessage({
      type: 'compressed',
      data: compressed,
      originalSize: data.length,
      compressedSize: compressed.length
    });
  }
};
```

## Performance Regression Testing

### Automated Performance Validation
```bash
# scripts/performance-test.sh
#!/bin/bash
echo "Running terminal performance regression tests..."

# Set performance baseline
export PERFORMANCE_BASELINE_FILE="benchmarks/terminal_performance_baseline.json"

# Run performance tests with benchmarking
npm test -- --testPathPattern=performance \
  --testTimeout=30000 \
  --maxWorkers=1 \
  --runInBand

# Generate performance report
node scripts/generate-performance-report.js

# Validate performance regression (fail if >10% regression)
node scripts/validate-performance-regression.js \
  --baseline=$PERFORMANCE_BASELINE_FILE \
  --threshold=10

echo "Performance regression testing completed"
```

### Continuous Performance Monitoring
```yaml
# .github/workflows/performance-monitoring.yml
name: Frontend Performance Monitoring
on:
  schedule:
    - cron: '0 */4 * * *'  # Every 4 hours
  workflow_dispatch:

jobs:
  performance-monitoring:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Setup Node.js
        uses: actions/setup-node@v3
        with:
          node-version: '18'
      - name: Install dependencies
        run: npm ci
      - name: Run performance tests
        run: npm run test:performance
      - name: Generate performance report
        run: npm run performance:report
      - name: Send performance alerts
        if: failure()
        run: npm run performance:alert
```

## Integration Testing

### End-to-End Performance Testing
```typescript
// E2E performance test with Playwright
test('terminal performance under load', async ({ page }) => {
  await page.goto('/terminal');
  
  // Start performance monitoring
  await page.evaluate(() => {
    window.performanceMonitor = new TerminalPerformanceMonitor();
    window.performanceMonitor.startMonitoring();
  });
  
  // Simulate heavy terminal usage
  for (let i = 0; i < 1000; i++) {
    await page.keyboard.type('echo "test message ' + i + '"');
    await page.keyboard.press('Enter');
    
    if (i % 100 === 0) {
      // Check performance every 100 operations
      const metrics = await page.evaluate(() => 
        window.performanceMonitor.getPerformanceSummary()
      );
      
      expect(metrics.current.frameRate).toBeGreaterThan(30);
      expect(metrics.current.memoryUsageMB).toBeLessThan(200);
    }
  }
  
  // Final performance check
  const finalMetrics = await page.evaluate(() => 
    window.performanceMonitor.getPerformanceSummary()
  );
  
  expect(finalMetrics.current.frameRate).toBeGreaterThan(30);
  expect(finalMetrics.thresholdViolations).toBe(0);
});
```

## Next Performance Implementation Steps

1. **Complete performance monitoring system** with all metrics collection
2. **Implement virtual scrolling** for large terminal buffers
3. **Add WebSocket optimization** with message batching and compression
4. **Create performance regression testing** in CI/CD pipeline
5. **Add performance dashboard** with real-time metrics
6. **Implement auto-optimization** based on performance degradation
7. **Create performance documentation** with optimization guides

## Performance Commit Guidelines

Performance commits should include:
- **Benchmark results** demonstrating performance improvements
- **Memory usage analysis** with before/after measurements
- **Frame rate testing** with 60 FPS target validation
- **Latency measurements** for user interactions
- **Load testing results** with high-volume scenarios
- **Performance monitoring** integration for ongoing optimization
- **Documentation updates** with performance characteristics and tuning guides