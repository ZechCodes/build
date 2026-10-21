import { describe, it, expect, vi } from 'vitest';
import { createWriteOperationService } from '../src/core/writeOperations.js';

function memoryCache() {
  const rows = new Map();
  return {
    rows, activateProjection() {}, deactivateProjection() {}, subscribe: () => () => {},
    list: async () => [...rows.values()], get: async id => rows.get(id),
    put: vi.fn(async row => { rows.set(row.operationId, structuredClone(row)); return row; }),
    update: async (id, update) => { const row = update(rows.get(id)); if (row) rows.set(id, row); return rows.get(id); },
    accept: async (id, fields) => { const row = rows.get(id); Object.assign(row, fields, {acknowledged:true,state:'accepted'}); return row; },
    confirm: async (id, fields) => { const row = rows.get(id); Object.assign(row, fields, {acknowledged:true,state:fields.confirmation.state}); return row; },
    acquireLease: async () => true, renewLease: async () => true, releaseLease: async () => true,
  };
}
const greeting = { bridge_epoch:'epoch-A',journal_incarnation:'journal-A',capabilities:['bridge.epoch'],ack_confirm_methods:['tasks.update','tasks.comment'] };
const ack = id => ({ ok:true,bridge_epoch:'epoch-A',write:{phase:'ack',operation_id:id,revision:1,state:'accepted',accepted_epoch:'epoch-A'},result:{saved:id} });
const confirmation = (id, extra = {}) => ({type:'writes.confirmed',bridge_epoch:'epoch-A',operation_id:id,revision:2,state:'succeeded',result:{saved:id},...extra});
function setup(call, extra={}) {
  const cache = memoryCache();
  const session = {callRaw:vi.fn(async (method,...args)=>{
    const result=await call(method,...args);
    return method.startsWith('writes.')?{journal_incarnation:'journal-A',...result}:result;
  }),onWriteFrame:() => () => {}};
  const service = createWriteOperationService({cache,randomUUID:()=> 'op-A',setTimeout:()=> 1,clearTimeout:()=>{},random:()=>.5,...extra});
  return {cache,session,service,bind:()=>service.bind({session,greeting})};
}
const flush = async () => { for (let n=0;n<20;n++) await Promise.resolve(); };

describe('durable write operation service', () => {
  it('persists exact normalized intent before transport and resolves only on durable ack', async () => {
    let reply;
    const {service,session,cache,bind}=setup((method) => method.startsWith('writes.') ? {operations:[]} : new Promise(resolve => {reply=resolve;}));
    await bind();
    const promise=service.submit({method:'tasks.update',params:{task_id:'T',body:'edited'},draft:{body:'edited'},owner:{kind:'task',id:'T'},generation:3,baseRevision:4});
    await flush();
    expect(cache.rows.get('op-A')).toMatchObject({request:{method:'tasks.update',params:{task_id:'T',body:'edited'}},draft:{body:'edited'},generation:3,baseRevision:4,state:'queued'});
    expect(session.callRaw).toHaveBeenCalledWith('tasks.update',{task_id:'T',body:'edited'},expect.objectContaining({write:{operation_id:'op-A',after:undefined}}));
    reply(ack('op-A'));
    await expect(promise).resolves.toEqual({saved:'op-A'});
    expect(cache.rows.get('op-A').acknowledged).toBe(true);
    await service.close();
  });
  it('does not send if persistence fails', async () => {
    const {service,session,cache,bind}=setup(async()=>({operations:[]})); await bind();
    cache.put.mockRejectedValueOnce(new Error('disk full'));
    await expect(service.submit({method:'tasks.update',params:{}})).rejects.toThrow('disk full');
    expect(session.callRaw).not.toHaveBeenCalledWith('tasks.update',expect.anything(),expect.anything());
    await service.close();
  });
  it('recovers confirmation arriving before ack and rejects stale epoch frames', async () => {
    const {service,cache,bind}=setup((method)=> method.startsWith('writes.') ? {operations:[]} : new Promise(()=>{})); await bind();
    const pending=service.submit({method:'tasks.update',params:{}}); await flush();
    await service.handleFrame(confirmation('op-A',{bridge_epoch:'old'}));
    expect(cache.rows.get('op-A').state).toBe('queued');
    await service.handleFrame(confirmation('op-A'));
    await expect(pending).resolves.toEqual({saved:'op-A'});
    await service.handleFrame(ack('op-A'));
    expect(cache.rows.get('op-A').state).toBe('succeeded');
    await service.close();
  });
  it('derives dependencies from reservations and persists them on retry', async()=>{
    const {service,cache,bind}=setup(async method=>method.startsWith('writes.')?{operations:[]}:ack('op-A')); await bind();
    await cache.put({operationId:'creator',revision:1,state:'accepted',acknowledged:true,reservedIds:{task_id:'reserved'},request:{method:'tasks.update',params:{}}});
    await service.submit({method:'tasks.comment',params:{task_id:'reserved',body:'hi'}});
    expect(cache.rows.get('op-A').after).toEqual(['creator']);
    await service.close();
  });
  it('queries before same-ID retransmission, and never replays acknowledged missing records', async()=>{
    const {service,session,cache,bind}=setup(async(method,params)=>method==='writes.status'?{operations:params.operation_ids.map(operation_id=>({operation_id,state:'not_found'}))}:method==='writes.subscribe'?{operations:[],cursor:'c'}:ack('unsent'));
    await cache.put({operationId:'unsent',revision:0,state:'queued',acknowledged:false,journalIncarnation:'journal-A',request:{method:'tasks.update',params:{}},after:[]});
    await cache.put({operationId:'missing',revision:1,state:'accepted',acknowledged:true,journalIncarnation:'journal-A',request:{method:'tasks.update',params:{}},after:[]});
    await bind();
    expect(session.callRaw.mock.calls.findIndex(([m])=>m==='writes.status')).toBeLessThan(session.callRaw.mock.calls.findIndex(([m])=>m==='tasks.update'));
    expect(cache.rows.get('missing')).toMatchObject({state:'outcome_unknown',unresolved:true});
    expect(session.callRaw.mock.calls.filter(([m])=>m==='tasks.update')).toHaveLength(1);
    await service.close();
  });
});

// Wire-shaped summaries deliberately differ from full transport ack envelopes.
describe('write recovery protocol', () => {
  it('restores the checked-in status fixture ack and exact reserved IDs', async () => {
    const {default:fixture} = await import('../../fixtures/api/v1/writes/status.json');
    const {default:hello} = await import('../../fixtures/api/v1/writes/epoch-greeting.json');
    const {service,cache,session} = setup(async()=>fixture.result);
    const operationId=fixture.result.operations[0].operation_id;
    await cache.put({operationId,request:{method:'tasks.comment',params:{}},revision:0,state:'queued',acknowledged:false,journalIncarnation:hello.result.journal_incarnation});
    await service.bind({session,greeting:{...hello.result,ack_confirm_methods:['tasks.comment']}});
    expect(cache.rows.get(operationId).ack).toMatchObject({write:fixture.result.operations[0].ack,result:fixture.result.operations[0].result});
    expect(cache.rows.get(operationId).state).toBe('accepted');
    await service.close();
  });
  it('continues status reconciliation after methods are disabled for new writes', async()=>{
    const {service,session,cache}=setup(async(method)=>method==='writes.status'?{operations:[confirmation('saved')]}:{operations:[]});
    await cache.put({operationId:'saved',request:{method:'tasks.update',params:{}},revision:1,state:'accepted',acknowledged:true,journalIncarnation:'journal-A'});
    await service.bind({session,greeting:{...greeting,ack_confirm_methods:[]}});
    expect(cache.rows.get('saved').state).toBe('succeeded');
    await expect(service.submit({method:'tasks.update',params:{}})).rejects.toMatchObject({error_code:'write_unsupported'});
    expect(session.callRaw.mock.calls.some(([method])=>method==='tasks.update')).toBe(false);
    await service.close();
  });
  it('quarantines another journal without sending its intent', async()=>{
    const {service,cache,session,bind}=setup(async()=>({operations:[]}));
    await cache.put({operationId:'old',request:{method:'tasks.update',params:{}},revision:0,state:'queued',acknowledged:false,journalIncarnation:'old-journal'});
    await bind();
    expect(cache.rows.get('old')).toMatchObject({state:'outcome_unknown',unresolvedReason:'journal_changed'});
    expect(session.callRaw).not.toHaveBeenCalled();
    await service.close();
  });
  it('does not send from a tab which does not own the lease', async()=>{
    const {service,cache,session,bind}=setup(async()=>({operations:[]}));
    cache.acquireLease=async()=>false;
    await bind();
    const pending=service.submit({method:'tasks.update',params:{}}).catch(()=>{}); await flush();
    expect(cache.rows.has('op-A')).toBe(true);
    expect(session.callRaw).not.toHaveBeenCalled();
    await service.close();await pending;
  });
  it('bounds status batches to 128 and follows cursor pages before subscribing', async()=>{
    const {service,session,cache,bind}=setup(async(method,params)=> method==='writes.status' && !params.cursor ? {operations:[],next_cursor:'page-2'} : {operations:[],cursor:'42'});
    for (let n=0;n<260;n++) await cache.put({operationId:`id-${n}`,request:{method:'tasks.update',params:{}},revision:1,state:'accepted',acknowledged:true,journalIncarnation:'journal-A'});
    await bind();
    const status=session.callRaw.mock.calls.filter(([method])=>method==='writes.status');
    expect(status.map(([,params])=>params.operation_ids.length)).toEqual([128,128,128,128,4,4]);
    expect(status[1][1].cursor).toBe('page-2');
    expect(session.callRaw.mock.calls.filter(([method])=>method==='writes.subscribe')).toHaveLength(3);
    await service.close();
  });
  it('settles durable ack without waiting for canonical refresh', async()=>{
    const {service,cache,session,bind}=setup(async(method)=>method.startsWith('writes.')?{operations:[]}:ack('op-A'));
    await bind();
    await expect(service.submit({method:'tasks.update',params:{}})).resolves.toEqual({saved:'op-A'});
    await service.handleFrame(confirmation('op-A'));
    expect(cache.rows.get('op-A').state).toBe('succeeded');
    expect(session.callRaw).toHaveBeenCalled();
    await service.close();
  });
  it('fences callbacks after the owning context is retired', async()=>{
    let active=true;
    const {service,session,cache}=setup(async(method)=>method.startsWith('writes.')?{operations:[]}:new Promise(()=>{}));
    await service.bind({session,greeting,active:()=>active});
    const pending=service.submit({method:'tasks.update',params:{}}).catch(()=>{}); await flush();
    active=false;
    await service.handleFrame(ack('op-A'));
    expect(cache.rows.get('op-A').state).toBe('queued');
    await service.close();await pending;
  });
});

it('cancels one caller wait without cancelling execution or another same-ID caller', async()=>{
  const {service,cache,bind}=setup(method=>method.startsWith('writes.')?{operations:[]}:new Promise(()=>{}));
  await bind();
  const controller=new AbortController();
  const first=service.submit({method:'tasks.update',params:{},options:{signal:controller.signal}});
  const firstOutcome=first.catch(error=>error);
  await flush();
  const second=service.submit({method:'tasks.update',params:{},operationId:'op-A'});
  controller.abort();
  await flush();
  const error=await Promise.race([firstOutcome,Promise.resolve('still waiting')]);
  expect(error).toMatchObject({name:'AbortError',error_code:'local_wait_ended'});
  expect(cache.rows.get('op-A').state).toBe('queued');
  await service.handleFrame(ack('op-A'));
  await expect(second).resolves.toEqual({saved:'op-A'});
  expect(cache.rows.get('op-A').acknowledged).toBe(true);
  await service.close();
});

it('recovers a lost ack on the same opening with exactly the persisted ID and payload', async()=>{
  let sends=0;
  const {service,session,cache,bind}=setup(async(method,params)=>{
    if(method==='writes.status') return {operations:params.operation_ids.map(operation_id=>({operation_id,state:'not_found'}))};
    if(method==='writes.subscribe') return {operations:[],cursor:'8'};
    if(++sends===1) throw Object.assign(new Error('lost ack'),{timedOut:true,uncertain:true});
    return ack('op-A');
  });
  await bind();
  const result=service.submit({method:'tasks.update',params:{task_id:'T',body:'B1'},draft:{body:'B1'}});
  await flush();
  await service.reconcile();
  await expect(result).resolves.toEqual({saved:'op-A'});
  const requests=session.callRaw.mock.calls.filter(([method])=>method==='tasks.update');
  expect(requests).toHaveLength(2);
  expect(requests[0][1]).toEqual(requests[1][1]);
  expect(requests[0][2].write.operation_id).toBe(requests[1][2].write.operation_id);
  expect(cache.rows.size).toBe(1);
  await service.close();
});

it('checks an unresolved acknowledged operation by ID without replaying it',async()=>{
  const {service,session,cache,bind}=setup(async(method,params)=>method==='writes.status'?{operations:params.operation_ids.map(operation_id=>confirmation(operation_id))}:{operations:[]});
  await cache.put({operationId:'unknown',request:{method:'tasks.update',params:{}},revision:1,state:'outcome_unknown',acknowledged:true,unresolved:true,journalIncarnation:'journal-A'});
  await bind();
  expect(session.callRaw).not.toHaveBeenCalled();
  await service.checkAgain('unknown');
  expect(cache.rows.get('unknown').state).toBe('succeeded');
  expect(session.callRaw.mock.calls.map(([method])=>method)).toEqual(['writes.status','writes.subscribe']);
  await service.close();
});

it('restores publication refresh from a persisted successful operation after reload',async()=>{
  const refresh=vi.fn().mockRejectedValueOnce(new Error('refresh lost')).mockResolvedValue(undefined);
  const {service,session,cache}=setup(async()=>({operations:[]}),{onRefresh:refresh});
  const frame=confirmation('saved',{refresh:[{kind:'task',id:'T'}]});
  await cache.put({operationId:'saved',request:{method:'tasks.update',params:{task_id:'T'}},state:'succeeded',revision:2,acknowledged:true,overlayActive:true,overlay:[{address:{entityId:'T'}}],confirmation:frame,journalIncarnation:'journal-A'});
  await service.bind({session,greeting});await flush();
  expect(refresh).toHaveBeenCalledTimes(1);
  await service.reconcile();await flush();
  expect(refresh).toHaveBeenCalledTimes(2);
  expect(refresh.mock.calls[1][2].overlay).toEqual([{address:{entityId:'T'}}]);
  expect(refresh.mock.calls[1][3].active()).toBe(true);
  await service.close();
});

it('persists raw durable ack and resolves success when effect normalization fails',async()=>{
  const {service,cache,bind}=setup(async(method)=>method.startsWith('writes.')?{operations:[]}:ack('op-A'),{
    normalizeAckEffects:async()=>{throw new Error('temporary cache read failed');},
  });
  await bind();
  await expect(service.submit({method:'tasks.update',params:{}})).resolves.toEqual({saved:'op-A'});
  expect(cache.rows.get('op-A').ack).toEqual(ack('op-A'));
  await service.close();
});

it('does not resolve an expired result or result reference as an acknowledged action',async()=>{
  for (const summary of [{state:'result_expired'},{state:'succeeded',result_ref:{revision:2,cursor:'result-page'}}]) {
    const {service,bind}=setup(async(method,params)=>method==='writes.status'?{operations:params.operation_ids.map(operation_id=>({operation_id,...summary}))}:method==='writes.subscribe'?{operations:[]}:Promise.reject(Object.assign(new Error('ack lost'),{timedOut:true,uncertain:true})));
    await bind();
    await expect(service.submit({method:'tasks.update',params:{}})).rejects.toMatchObject({uncertain:true,error_code:summary.state==='result_expired'?'result_expired':'outcome_unknown'});
    await service.close();
  }
});

it('distinguishes an explicit empty dependency override from an omitted saved dependency set',async()=>{
  const {service,cache,bind}=setup(async()=>({operations:[]}));await bind();
  await cache.put({operationId:'saved',request:{method:'tasks.update',params:{}},state:'accepted',revision:1,acknowledged:true,ack:ack('saved'),journalIncarnation:'journal-A'});
  await expect(service.submit({method:'tasks.update',params:{},operationId:'saved',after:[]})).rejects.toMatchObject({error_code:'operation_conflict'});
  await expect(service.submit({method:'tasks.update',params:{},operationId:'saved'})).resolves.toEqual({saved:'saved'});
  await service.close();
});

it('keeps a status retry leased through delayed encryption and transport handoff',async()=>{
  let handoff;
  const {service,cache,bind}=setup((method,params,options)=>{
    if(method==='writes.status') return {operations:params.operation_ids.map(operation_id=>({operation_id,state:'not_found'}))};
    if(method==='writes.subscribe') return {operations:[],cursor:'12'};
    return new Promise((resolve,reject)=>{handoff=()=>{try {options.assertCurrent();resolve(ack('retry'));} catch(error) {reject(error);}};});
  });
  const released=vi.spyOn(cache,'releaseLease');
  await cache.put({operationId:'retry',request:{method:'tasks.update',params:{task_id:'T'}},revision:0,state:'queued',acknowledged:false,journalIncarnation:'journal-A'});
  const opening=bind();await flush();
  expect(handoff).toBeTypeOf('function');
  await flush();
  expect(released).not.toHaveBeenCalled();
  handoff();await opening;
  expect(cache.rows.get('retry').state).toBe('accepted');
  await service.close();
});

it('persists a refresh obligation for failure before ack without an overlay',async()=>{
  const refresh=vi.fn().mockRejectedValue(new Error('named read unavailable'));
  const {service,cache,bind}=setup(method=>method.startsWith('writes.')?{operations:[]}:new Promise(()=>{}),{onRefresh:refresh});
  await bind();
  const pending=service.submit({method:'tasks.comment',params:{task_id:'T',body:'draft'}}).catch(error=>error);await flush();
  await service.handleFrame(confirmation('op-A',{state:'failed',effect:'unapplied',error:{message:'Comment refused'},refresh:[{kind:'task',id:'T'}]}));
  await pending;await flush();
  expect(cache.rows.get('op-A')).toMatchObject({state:'failed',refreshPending:true});
  expect(cache.rows.get('op-A').overlay).toEqual([]);
  expect(refresh).toHaveBeenCalled();
  await service.close();
});

it('retries persisted refresh obligations after reload even with a retired overlay',async()=>{
  const refresh=vi.fn().mockResolvedValue(undefined);
  const {service,cache,bind}=setup(async()=>({operations:[]}),{onRefresh:refresh});
  const frame=confirmation('failed',{state:'failed',effect:'unknown',refresh:[{kind:'task',id:'T'}]});
  await cache.put({operationId:'failed',request:{method:'tasks.update',params:{task_id:'T'}},state:'failed',revision:2,acknowledged:true,overlayActive:false,overlay:[],unresolved:true,refreshPending:true,confirmation:frame,journalIncarnation:'journal-A'});
  await bind();await flush();
  expect(refresh).toHaveBeenCalledTimes(1);
  expect(cache.rows.get('failed')).toMatchObject({refreshPending:false,unresolved:true,state:'failed'});
  await service.close();
});
