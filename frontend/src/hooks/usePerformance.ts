import { useEffect, useRef, useCallback, useState } from 'react';
import { performanceMonitor, throttle, debounce, DataBatcher, type PerformanceMetrics } from '../utils/performance';

/**
 * Hook for monitoring component performance
 */
export function usePerformanceMonitoring(componentName: string) {
  const renderCount = useRef(0);
  const startTime = useRef<number>(0);

  useEffect(() => {
    performanceMonitor.startProfiling(`${componentName}-mount`);
    
    return () => {
      performanceMonitor.endProfiling(`${componentName}-mount`);
    };
  }, [componentName]);

  const startRender = useCallback(() => {
    renderCount.current++;
    startTime.current = performance.now();
    performanceMonitor.startProfiling(`${componentName}-render-${renderCount.current}`);
  }, [componentName]);

  const endRender = useCallback(() => {
    const duration = performance.now() - startTime.current;
    performanceMonitor.endProfiling(`${componentName}-render-${renderCount.current}`);
    performanceMonitor.recordRenderTime(duration);
  }, [componentName]);

  return { startRender, endRender, renderCount: renderCount.current };
}

/**
 * Hook for optimized data processing
 */
export function useDataBatcher(
  onFlush: (data: string) => void,
  maxBufferSize = 1000,
  flushInterval = 16
) {
  const batcherRef = useRef<DataBatcher | null>(null);

  useEffect(() => {
    batcherRef.current = new DataBatcher(onFlush, maxBufferSize, flushInterval);
    
    return () => {
      batcherRef.current?.clear();
    };
  }, [onFlush, maxBufferSize, flushInterval]);

  const addData = useCallback((data: string) => {
    batcherRef.current?.add(data);
  }, []);

  const flush = useCallback(() => {
    batcherRef.current?.flush();
  }, []);

  return { addData, flush };
}

/**
 * Hook for throttled callbacks
 */
export function useThrottledCallback<T extends (...args: any[]) => any>(
  callback: T,
  delay: number
): T {
  const throttledCallback = useRef<T>();

  useEffect(() => {
    throttledCallback.current = throttle(callback, delay) as T;
  }, [callback, delay]);

  return throttledCallback.current || callback;
}

/**
 * Hook for debounced callbacks
 */
export function useDebouncedCallback<T extends (...args: any[]) => any>(
  callback: T,
  delay: number
): T {
  const debouncedCallback = useRef<T>();

  useEffect(() => {
    debouncedCallback.current = debounce(callback, delay) as T;
  }, [callback, delay]);

  return debouncedCallback.current || callback;
}

/**
 * Hook for performance metrics monitoring
 */
export function usePerformanceMetrics(updateInterval = 1000) {
  const [metrics, setMetrics] = useState<PerformanceMetrics | null>(null);
  const [issues, setIssues] = useState<string[]>([]);

  useEffect(() => {
    const updateMetrics = () => {
      const report = performanceMonitor.getPerformanceReport();
      setMetrics(report.average);
      setIssues(report.issues);
    };

    updateMetrics(); // Initial update
    const interval = setInterval(updateMetrics, updateInterval);

    return () => clearInterval(interval);
  }, [updateInterval]);

  return { metrics, issues };
}

/**
 * Hook for WebSocket latency monitoring
 */
export function useWSLatencyMonitoring() {
  const pingTimeRef = useRef<number>(0);

  const startPing = useCallback(() => {
    pingTimeRef.current = performance.now();
  }, []);

  const endPing = useCallback(() => {
    if (pingTimeRef.current > 0) {
      const latency = performance.now() - pingTimeRef.current;
      performanceMonitor.recordWSLatency(latency);
      pingTimeRef.current = 0;
    }
  }, []);

  return { startPing, endPing };
}

/**
 * Hook for memory usage optimization
 */
export function useMemoryOptimization() {
  const cleanupFunctions = useRef<(() => void)[]>([]);

  const addCleanup = useCallback((cleanup: () => void) => {
    cleanupFunctions.current.push(cleanup);
  }, []);

  const runCleanup = useCallback(() => {
    cleanupFunctions.current.forEach(cleanup => {
      try {
        cleanup();
      } catch (error) {
        console.warn('Cleanup function failed:', error);
      }
    });
    cleanupFunctions.current = [];
  }, []);

  useEffect(() => {
    return runCleanup;
  }, [runCleanup]);

  // Monitor memory usage and trigger cleanup if needed
  useEffect(() => {
    const checkMemory = () => {
      const memory = (performance as any).memory;
      if (memory && typeof memory.usedJSHeapSize === 'number' && memory.usedJSHeapSize > 50 * 1024 * 1024) { // 50MB threshold
        console.warn('High memory usage detected, running cleanup');
        runCleanup();
        
        // Force garbage collection if available (Chrome DevTools)
        if ('gc' in window && typeof (window as any).gc === 'function') {
          (window as any).gc();
        }
      }
    };

    const interval = setInterval(checkMemory, 10000); // Check every 10 seconds
    return () => clearInterval(interval);
  }, [runCleanup]);

  return { addCleanup, runCleanup };
}

/**
 * Hook for render optimization with memoization
 */
export function useRenderOptimization<T>(
  value: T,
  deps: React.DependencyList
): T {
  const memoizedValue = useRef<T>(value);
  const lastDeps = useRef<React.DependencyList>(deps);

  // Check if dependencies have changed
  const depsChanged = deps.length !== lastDeps.current.length || 
    deps.some((dep, index) => dep !== lastDeps.current[index]);

  if (depsChanged) {
    memoizedValue.current = value;
    lastDeps.current = deps;
  }

  return memoizedValue.current;
}

/**
 * Hook for frame rate optimization
 */
export function useFrameRateOptimization(targetFPS = 60) {
  const lastFrameTime = useRef<number>(0);
  const frameInterval = 1000 / targetFPS;

  const shouldUpdate = useCallback(() => {
    const now = performance.now();
    if (now - lastFrameTime.current >= frameInterval) {
      lastFrameTime.current = now;
      return true;
    }
    return false;
  }, [frameInterval]);

  return shouldUpdate;
}