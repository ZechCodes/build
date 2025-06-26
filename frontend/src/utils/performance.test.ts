import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { PerformanceMonitor, throttle, debounce, DataBatcher, CircularBuffer } from './performance';

// Mock performance API
const mockPerformance = {
  now: () => Date.now(),
  mark: vi.fn(),
  measure: vi.fn(),
  memory: {
    usedJSHeapSize: 50 * 1024 * 1024 // 50MB
  }
};

(global as any).performance = mockPerformance;

// Mock PerformanceObserver
class MockPerformanceObserver {
  private callback: (list: any) => void;
  
  constructor(callback: (list: any) => void) {
    this.callback = callback;
  }

  observe() {}
  disconnect() {}
}

(global as any).PerformanceObserver = MockPerformanceObserver;

describe('Performance Utils', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  describe('PerformanceMonitor', () => {
    let monitor: PerformanceMonitor;

    beforeEach(() => {
      monitor = new PerformanceMonitor({
        enableMetrics: true,
        enableProfiling: true
      });
    });

    afterEach(() => {
      monitor.cleanup();
    });

    it('should create a performance monitor', () => {
      expect(monitor).toBeDefined();
    });

    it('should record render time', () => {
      monitor.recordRenderTime(16.5);
      
      const metrics = monitor.getAverageMetrics(1);
      expect(metrics.renderTime).toBe(16.5);
    });

    it('should record memory usage', () => {
      monitor.recordMemoryUsage();
      
      const metrics = monitor.getAverageMetrics(1);
      expect(metrics.memoryUsage).toBeGreaterThan(0);
    });

    it('should record frame rate', () => {
      monitor.recordFrameRate(60);
      
      const metrics = monitor.getAverageMetrics(1);
      expect(metrics.frameRate).toBe(60);
    });

    it('should record data throughput', () => {
      monitor.recordDataThroughput(1024);
      
      const metrics = monitor.getAverageMetrics(1);
      expect(metrics.terminalDataThroughput).toBe(1024);
    });

    it('should record WebSocket latency', () => {
      monitor.recordWSLatency(50);
      
      const metrics = monitor.getAverageMetrics(1);
      expect(metrics.wsLatency).toBe(50);
    });

    it('should calculate average metrics', () => {
      // Create a fresh monitor for this test
      const freshMonitor = new PerformanceMonitor({ enableMetrics: true });
      
      // Record render times
      freshMonitor.recordRenderTime(10);
      freshMonitor.recordRenderTime(20);
      freshMonitor.recordRenderTime(30);
      
      const averageMetrics = freshMonitor.getAverageMetrics(3);
      // The average should be influenced by the recorded values
      expect(averageMetrics.renderTime).toBeGreaterThan(0);
      expect(averageMetrics.renderTime).toBeLessThan(50);
      
      freshMonitor.cleanup();
    });

    it('should detect performance issues', () => {
      // Create a fresh monitor for this test
      const freshMonitor = new PerformanceMonitor({ enableMetrics: true });
      
      // Record multiple samples to establish a pattern
      for (let i = 0; i < 5; i++) {
        freshMonitor.recordRenderTime(25); // High render time
        freshMonitor.recordFrameRate(20); // Low frame rate
      }
      
      const report = freshMonitor.getPerformanceReport();
      expect(report.issues.length).toBeGreaterThan(0);
      expect(report.issues.some(issue => issue.includes('render time'))).toBe(true);
      expect(report.issues.some(issue => issue.includes('frame rate'))).toBe(true);
      
      freshMonitor.cleanup();
    });

    it('should start and end profiling', () => {
      monitor.startProfiling('test-operation');
      monitor.endProfiling('test-operation');
      
      expect(mockPerformance.mark).toHaveBeenCalledWith('test-operation-start');
      expect(mockPerformance.mark).toHaveBeenCalledWith('test-operation-end');
      expect(mockPerformance.measure).toHaveBeenCalledWith('test-operation', 'test-operation-start', 'test-operation-end');
    });

    it('should cleanup properly', () => {
      const clearIntervalSpy = vi.spyOn(global, 'clearInterval');
      
      monitor.cleanup();
      
      expect(clearIntervalSpy).toHaveBeenCalled();
    });
  });

  describe('throttle', () => {
    it('should throttle function calls', () => {
      const fn = vi.fn();
      const throttledFn = throttle(fn, 100);
      
      throttledFn();
      throttledFn();
      throttledFn();
      
      expect(fn).toHaveBeenCalledTimes(1);
      
      vi.advanceTimersByTime(100);
      throttledFn();
      
      expect(fn).toHaveBeenCalledTimes(2);
    });

    it('should call function immediately on first call', () => {
      const fn = vi.fn();
      const throttledFn = throttle(fn, 100);
      
      throttledFn();
      
      expect(fn).toHaveBeenCalledTimes(1);
    });

    it('should preserve function arguments', () => {
      const fn = vi.fn();
      const throttledFn = throttle(fn, 100);
      
      throttledFn('test', 123);
      
      expect(fn).toHaveBeenCalledWith('test', 123);
    });
  });

  describe('debounce', () => {
    it('should debounce function calls', () => {
      const fn = vi.fn();
      const debouncedFn = debounce(fn, 100);
      
      debouncedFn();
      debouncedFn();
      debouncedFn();
      
      expect(fn).not.toHaveBeenCalled();
      
      vi.advanceTimersByTime(100);
      
      expect(fn).toHaveBeenCalledTimes(1);
    });

    it('should reset timer on each call', () => {
      const fn = vi.fn();
      const debouncedFn = debounce(fn, 100);
      
      debouncedFn();
      vi.advanceTimersByTime(50);
      
      debouncedFn();
      vi.advanceTimersByTime(50);
      
      expect(fn).not.toHaveBeenCalled();
      
      vi.advanceTimersByTime(50);
      
      expect(fn).toHaveBeenCalledTimes(1);
    });

    it('should preserve function arguments', () => {
      const fn = vi.fn();
      const debouncedFn = debounce(fn, 100);
      
      debouncedFn('test', 123);
      vi.advanceTimersByTime(100);
      
      expect(fn).toHaveBeenCalledWith('test', 123);
    });
  });

  describe('DataBatcher', () => {
    let onFlush: ReturnType<typeof vi.fn>;
    let batcher: DataBatcher;

    beforeEach(() => {
      onFlush = vi.fn();
      batcher = new DataBatcher(onFlush, 3, 50);
    });

    it('should batch data and flush after interval', () => {
      batcher.add('data1');
      batcher.add('data2');
      
      expect(onFlush).not.toHaveBeenCalled();
      
      vi.advanceTimersByTime(50);
      
      expect(onFlush).toHaveBeenCalledWith('data1data2');
    });

    it('should flush immediately when buffer is full', () => {
      batcher.add('data1');
      batcher.add('data2');
      batcher.add('data3'); // This should trigger immediate flush
      
      expect(onFlush).toHaveBeenCalledWith('data1data2data3');
    });

    it('should clear buffer properly', () => {
      batcher.add('data1');
      batcher.clear();
      
      vi.advanceTimersByTime(50);
      
      expect(onFlush).not.toHaveBeenCalled();
    });

    it('should flush manually', () => {
      batcher.add('data1');
      batcher.flush();
      
      expect(onFlush).toHaveBeenCalledWith('data1');
    });
  });

  describe('CircularBuffer', () => {
    let buffer: CircularBuffer<string>;

    beforeEach(() => {
      buffer = new CircularBuffer<string>(3);
    });

    it('should add items to buffer', () => {
      buffer.push('item1');
      buffer.push('item2');
      
      expect(buffer.getSize()).toBe(2);
      expect(buffer.get(0)).toBe('item1');
      expect(buffer.get(1)).toBe('item2');
    });

    it('should overwrite oldest items when full', () => {
      buffer.push('item1');
      buffer.push('item2');
      buffer.push('item3');
      buffer.push('item4'); // Should overwrite item1
      
      expect(buffer.getSize()).toBe(3);
      expect(buffer.get(0)).toBe('item2');
      expect(buffer.get(1)).toBe('item3');
      expect(buffer.get(2)).toBe('item4');
    });

    it('should return undefined for invalid indices', () => {
      buffer.push('item1');
      
      expect(buffer.get(-1)).toBeUndefined();
      expect(buffer.get(5)).toBeUndefined();
    });

    it('should convert to array', () => {
      buffer.push('item1');
      buffer.push('item2');
      
      const array = buffer.toArray();
      expect(array).toEqual(['item1', 'item2']);
    });

    it('should clear buffer', () => {
      buffer.push('item1');
      buffer.push('item2');
      buffer.clear();
      
      expect(buffer.getSize()).toBe(0);
      expect(buffer.toArray()).toEqual([]);
    });

    it('should return correct capacity', () => {
      expect(buffer.getCapacity()).toBe(3);
    });
  });
});