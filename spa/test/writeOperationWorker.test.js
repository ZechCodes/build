import { afterEach, describe, expect, it, vi } from 'vitest';
import { createWriteOperationWorker } from '../src/core/writeOperationWorker.js';

afterEach(()=>vi.useRealTimers());
function setup(overrides={}) {
  const cache={acquireLease:vi.fn(async()=>true),renewLease:vi.fn(async()=>true),releaseLease:vi.fn(async()=>true)};
  const run=vi.fn(async()=>{});
  const worker=createWriteOperationWorker({cache,tabId:'tab',run,hasWork:()=>true,random:()=>.5,setTimeout,clearTimeout,...overrides});
  return {worker,cache,run};
}
describe('one durable write status worker per device',()=>{
  it('uses the jittered 1/2/5/10/30 second schedule and resets after an outcome',async()=>{
    vi.useFakeTimers();
    const {worker,run}=setup();
    worker.wake();
    for (const wait of [1000,2000,5000,10000,30000,30000]) {
      const before=run.mock.calls.length;
      await vi.advanceTimersByTimeAsync(wait-1);expect(run).toHaveBeenCalledTimes(before);
      await vi.advanceTimersByTimeAsync(1);expect(run).toHaveBeenCalledTimes(before+1);
    }
    worker.reset();worker.wake();
    await vi.advanceTimersByTimeAsync(1000);expect(run).toHaveBeenCalledTimes(7);
    await worker.close();
  });
  it('allows a new opening to reconcile while the old status call is stalled',async()=>{
    vi.useFakeTimers();
    let release;
    const run=vi.fn().mockImplementationOnce(()=>new Promise(resolve=>{release=resolve;})).mockResolvedValue(undefined);
    const {worker,cache}=setup({run});
    const old=worker.reconcile();await Promise.resolve();await Promise.resolve();
    await worker.restart();await worker.reconcile();
    expect(run).toHaveBeenCalledTimes(2);
    expect(cache.acquireLease.mock.calls.map(([holder])=>holder)).toEqual(['tab:0','tab:1']);
    release();await old;await worker.close();
  });
  it('dispatches fresh intent under its acquired lease without waiting for status',async()=>{
    vi.useFakeTimers();
    const events=[];
    const {worker}=setup({run:async()=>{events.push('status');}});
    worker.dispatch(()=>events.push('send'));
    await Promise.resolve();await Promise.resolve();
    expect(events).toEqual(['send','status']);
    await worker.close();
  });
});

it('continues status passes while an acknowledged RPC response is still missing',async()=>{
  vi.useFakeTimers();
  let finishWrite;
  const {worker,run,cache}=setup();
  worker.dispatch(()=>new Promise(resolve=>{finishWrite=resolve;}));
  await vi.advanceTimersByTimeAsync(0);
  expect(run).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1000);
  expect(run).toHaveBeenCalledTimes(2);
  expect(cache.acquireLease).toHaveBeenCalledTimes(1);
  finishWrite();await vi.advanceTimersByTimeAsync(0);await worker.close();
});

it('invalidates transport authority when another tab takes the lease',async()=>{
  vi.useFakeTimers();
  let authority;
  let finish;
  const {worker,cache}=setup();
  cache.renewLease.mockResolvedValue(false);
  worker.dispatch(active=>{authority=active;return new Promise(resolve=>{finish=resolve;});});
  await vi.advanceTimersByTimeAsync(0);expect(authority()).toBe(true);
  await vi.advanceTimersByTimeAsync(5000);expect(authority()).toBe(false);
  finish();await vi.advanceTimersByTimeAsync(0);await worker.close();
});
