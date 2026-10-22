import { describe, it, expect, vi } from 'vitest';
import { createWriteOperationService } from '../src/core/writeOperations.js';

function memoryCache() {
  const rows = new Map();
  return {
    rows, activateProjection() {}, deactivateProjection() {}, subscribe: () => () => {},
    list: async () => [...rows.values()], get: async id => rows.get(id),
    put: vi.fn(async row => { rows.set(row.operationId, structuredClone(row)); return row; }),
    update: async (id, update, options={}) => { if(options.accept && !options.accept()) return rows.get(id); const row = update(rows.get(id)); if (row) rows.set(id, row); return rows.get(id); },
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
    for (let n=0;n<260;n++) await cache.put({operationId:`id-${n}`,request:{method:'tasks.update',params:{}},revision:1,state:'accepted',acknowledged:true,journalIncarnation:'journal-A',localOrder:n});
    await bind();
    const status=session.callRaw.mock.calls.filter(([method])=>method==='writes.status');
    expect(status.map(([,params])=>params.operation_ids.length)).toEqual([128,128,128,128,4,4]);
    expect(status[1][1].cursor).toBe('page-2');
    const subscriptions=session.callRaw.mock.calls.filter(([method])=>method==='writes.subscribe');
    expect(subscriptions).toHaveLength(1);
    expect(subscriptions[0][1].operation_ids).toEqual(Array.from({length:128},(_,n)=>`id-${n}`));
    await service.handleFrame(confirmation('id-0'));
    await service.reconcile();
    const refilled=session.callRaw.mock.calls.filter(([method])=>method==='writes.subscribe').at(-1);
    expect(refilled[1].operation_ids).toEqual(Array.from({length:128},(_,n)=>`id-${n+1}`));
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
  expect(session.callRaw.mock.calls.map(([method])=>method)).toEqual(['writes.status']);
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

it('checks an uncertain result with a fresh named read after an earlier refresh completed',async()=>{
  const refresh=vi.fn().mockResolvedValue(undefined);
  const {service,cache,bind}=setup(async()=>({operations:[]}),{onRefresh:refresh});
  const frame=confirmation('uncertain',{state:'failed',effect:'partial',refresh:[{kind:'task',id:'T'}]});
  await cache.put({operationId:'uncertain',request:{method:'tasks.update',params:{task_id:'T'}},state:'failed',revision:2,acknowledged:true,overlayActive:false,overlay:[],unresolved:true,refreshPending:false,confirmation:frame,journalIncarnation:'journal-A'});
  await bind();expect(refresh).not.toHaveBeenCalled();
  await service.checkAgain('uncertain');await flush();
  expect(refresh).toHaveBeenCalledTimes(1);
  expect(cache.rows.get('uncertain')).toMatchObject({state:'failed',unresolved:true,refreshPending:false});
  await service.close();
});

it('fetches a truncated replacement subscription through status using the same complete ID set',async()=>{
  const {service,session,cache,bind}=setup(async(method,params)=>{
    if(method==='writes.subscribe') {
      if(params.cursor) throw new Error('SubscribeParams rejects cursor');
      return {operations:[confirmation('first')],cursor:'88',next_cursor:'subscription-page-2'};
    }
    if(params.cursor==='subscription-page-2') return {operations:[confirmation('second',{state:'failed',effect:'unapplied',error:{message:'refused'}})]};
    return {operations:[]};
  });
  for(const operationId of ['first','second']) await cache.put({operationId,request:{method:'tasks.update',params:{}},state:'accepted',revision:1,acknowledged:true,journalIncarnation:'journal-A'});
  await bind();
  expect(cache.rows.get('first').state).toBe('succeeded');
  expect(cache.rows.get('second').state).toBe('failed');
  const subscribes=session.callRaw.mock.calls.filter(([method])=>method==='writes.subscribe');
  expect(subscribes).toHaveLength(1);
  expect(subscribes[0][1]).toEqual({operation_ids:['first','second']});
  expect(session.callRaw).toHaveBeenCalledWith('writes.status',{operation_ids:['first','second'],cursor:'subscription-page-2'},expect.anything());
  expect(session.callRaw.mock.calls.some(([,params])=>params.cursor==='88')).toBe(false);
  await service.close();
});

it('records an executing summary revision while retaining the original accepted ack',async()=>{
  const savedAck=ack('running');
  let terminal=false;
  const {service,cache,bind}=setup(async(method)=>{
    if(method==='writes.status' && terminal) return {operations:[{...confirmation('running',{revision:3}),ack:savedAck.write}]};
    return {operations:[{operation_id:'running',revision:2,state:'executing',ack:savedAck.write,result:savedAck.result}]};
  });
  await cache.put({operationId:'running',request:{method:'tasks.update',params:{}},state:'queued',revision:0,acknowledged:false,journalIncarnation:'journal-A'});
  await bind();
  expect(cache.rows.get('running')).toMatchObject({state:'executing',revision:2,ack:savedAck});
  terminal=true;await service.reconcile();
  expect(cache.rows.get('running')).toMatchObject({state:'succeeded',revision:3,ack:savedAck});
  await service.handleFrame(savedAck);
  expect(cache.rows.get('running')).toMatchObject({state:'succeeded',revision:3});
  await service.close();
});

it('rejects unknown summary states without treating an attached ack as success',async()=>{
  const {service,cache,bind}=setup(async()=>({operations:[{operation_id:'unknown-state',revision:7,state:'invented',ack:ack('unknown-state').write,result:{saved:true}}]}));
  await cache.put({operationId:'unknown-state',request:{method:'tasks.update',params:{}},state:'queued',revision:0,acknowledged:false,journalIncarnation:'journal-A'});
  await bind();
  expect(cache.rows.get('unknown-state')).toMatchObject({state:'queued',revision:0,acknowledged:false});
  await service.close();
});

it('subscribes the originator explicitly and recovers a confirmation push delivered only to subscribers',async()=>{
  for(const losePush of [false,true]) {
    const subscribed=new Set();
    let listener;
    let summary={operation_id:'op-A',revision:1,state:'accepted',ack:ack('op-A').write,result:ack('op-A').result};
    const {service,session,cache,bind}=setup(async(method,params)=>{
      if(method==='tasks.update') return ack('op-A');
      if(method==='writes.subscribe') {
        subscribed.clear();params.operation_ids.forEach(id=>subscribed.add(id));
      }
      return {operations:params.operation_ids.map(()=>summary),cursor:'100'};
    });
    session.onWriteFrame=fn=>{listener=fn;return()=>{listener=null;};};
    await bind();
    await expect(service.submit({method:'tasks.update',params:{task_id:'T'}})).resolves.toEqual({saved:'op-A'});
    await service.reconcile();
    expect(subscribed.has('op-A')).toBe(true);
    summary={...confirmation('op-A'),ack:ack('op-A').write};
    if(!losePush && subscribed.has('op-A')) listener(confirmation('op-A'));
    await flush();
    if(losePush) {
      expect(cache.rows.get('op-A').state).toBe('accepted');
      await service.reconcile();
    }
    expect(cache.rows.get('op-A').state).toBe('succeeded');
    expect(session.callRaw.mock.calls.filter(([method])=>method==='tasks.update')).toHaveLength(1);
    await service.close();
  }
});

it('selects the same oldest replacement set when persisted submission clocks tie',async()=>{
  const {service,session,cache,bind}=setup(async()=>({operations:[],cursor:'1'}));
  for(let n=129;n>=0;n--) await cache.put({operationId:`id-${String(n).padStart(3,'0')}`,request:{method:'tasks.update',params:{}},state:'accepted',revision:1,acknowledged:true,journalIncarnation:'journal-A',localOrder:5,createdAt:10});
  await bind();
  const subscription=session.callRaw.mock.calls.find(([method])=>method==='writes.subscribe');
  expect(subscription[1].operation_ids).toEqual(Array.from({length:128},(_,n)=>`id-${String(n).padStart(3,'0')}`));
  await service.close();
});

it('fences delayed quarantine behind a newer matching-journal acknowledgement',async()=>{
  let statusCalls=0;
  const {service,session,cache,bind}=setup(async method=>{
    if(method==='writes.status' && ++statusCalls===1) return {journal_incarnation:'unexpected-journal',operations:[]};
    return {operations:[{operation_id:'retained',state:'accepted',revision:1,ack:ack('retained').write,result:ack('retained').result}]};
  });
  await cache.put({operationId:'retained',request:{method:'tasks.update',params:{}},state:'queued',revision:0,acknowledged:false,journalIncarnation:'journal-A'});
  const update=cache.update;
  let finishQuarantine;
  let first=true;
  cache.update=(id,updater,options)=>{
    if(first) {first=false;return new Promise(resolve=>{finishQuarantine=()=>resolve(update(id,updater,options));});}
    return update(id,updater,options);
  };
  const old=bind();await flush();
  expect(finishQuarantine).toBeTypeOf('function');
  await service.bind({session,greeting});
  expect(cache.rows.get('retained')).toMatchObject({state:'accepted',revision:1,acknowledged:true});
  finishQuarantine();await old;
  expect(cache.rows.get('retained')).toMatchObject({state:'accepted',revision:1,acknowledged:true});
  expect(cache.rows.get('retained').unresolved).not.toBe(true);
  await service.close();
});

it('keeps a direct confirmation result reference unresolved before the ack arrives',async()=>{
  const {service,session,cache,bind}=setup((method,params)=>method==='writes.status'?{operations:params.operation_ids.map(operation_id=>({operation_id,state:'not_found'}))}:method==='writes.subscribe'?{operations:[]}:new Promise(()=>{}));
  await bind();
  const outcome=service.submit({method:'tasks.update',params:{task_id:'T'}}).then(value=>({value}),error=>({error}));
  await flush();
  await service.handleFrame(confirmation('op-A',{result:undefined,result_ref:{operation_id:'op-A',revision:2}}));
  expect((await outcome).error).toMatchObject({uncertain:true,error_code:'outcome_unknown'});
  expect(cache.rows.get('op-A')).toMatchObject({state:'outcome_unknown',revision:2,unresolved:true,unresolvedReason:'result_reference'});
  await service.handleFrame(ack('op-A'));
  await service.checkAgain('op-A');
  expect(cache.rows.get('op-A')).toMatchObject({state:'outcome_unknown',revision:2,unresolved:true});
  expect(session.callRaw.mock.calls.filter(([method])=>method==='tasks.update')).toHaveLength(1);
  await service.close();
});

it('fences a delayed expired summary behind a newer current-opening ack',async()=>{
  let statusCalls=0;
  const {service,session,cache,bind}=setup(async method=>{
    if(method==='writes.status' && ++statusCalls===1) return {operations:[{operation_id:'retained',state:'result_expired',revision:0}]};
    return {operations:[{operation_id:'retained',state:'accepted',revision:1,ack:ack('retained').write,result:ack('retained').result}]};
  });
  await cache.put({operationId:'retained',request:{method:'tasks.update',params:{}},state:'queued',revision:0,acknowledged:false,journalIncarnation:'journal-A'});
  const update=cache.update;
  let finishExpired;
  let first=true;
  cache.update=(id,updater,options)=>{
    if(first) {first=false;return new Promise(resolve=>{finishExpired=()=>resolve(update(id,updater,options));});}
    return update(id,updater,options);
  };
  const old=bind();await flush();
  expect(finishExpired).toBeTypeOf('function');
  await service.bind({session,greeting});
  finishExpired();await old;
  expect(cache.rows.get('retained')).toMatchObject({state:'accepted',revision:1,acknowledged:true});
  expect(cache.rows.get('retained').unresolved).not.toBe(true);
  await service.close();
});

it('ignores already obsolete expiry and result-reference summaries after a newer ack',async()=>{
  for(const stale of [{state:'result_expired',revision:2},{state:'succeeded',revision:2,result_ref:{revision:2}}]) {
    const {service,cache,bind}=setup(async()=>({operations:[{operation_id:'newer',...stale}]}));
    const latestAck=ack('newer');latestAck.write.revision=3;
    await cache.put({operationId:'newer',request:{method:'tasks.update',params:{}},state:'accepted',revision:3,acknowledged:true,ack:latestAck,journalIncarnation:'journal-A'});
    await bind();
    expect(cache.rows.get('newer')).toMatchObject({state:'accepted',revision:3,acknowledged:true});
    expect(cache.rows.get('newer').unresolved).not.toBe(true);
    await service.close();
  }
});

it('fences an ACK result reference at its nested revision and refuses equal-revision healing',async()=>{
  const {service,cache,bind}=setup(method=>method.startsWith('writes.')?{operations:[]}:new Promise(()=>{}));
  await bind();
  const outcome=service.submit({method:'tasks.update',params:{task_id:'T'}}).catch(error=>error);await flush();
  await service.handleFrame({...ack('op-A'),result:undefined,result_ref:{revision:1}});
  expect(await outcome).toMatchObject({uncertain:true,error_code:'outcome_unknown'});
  expect(cache.rows.get('op-A')).toMatchObject({state:'outcome_unknown',revision:1,unresolved:true});
  await service.handleFrame(ack('op-A'));
  expect(cache.rows.get('op-A')).toMatchObject({state:'outcome_unknown',revision:1,unresolved:true});
  await service.close();
});

it('reconciles every known summary variant across bounded status pages',async()=>{
  const accepted=id=>({operation_id:id,state:'accepted',revision:1,ack:ack(id).write,result:ack(id).result});
  const pages=[
    [accepted('accepted'),{...accepted('executing'),state:'executing',revision:2}],
    [{...confirmation('succeeded'),ack:ack('succeeded').write},{...confirmation('failed',{state:'failed',effect:'unapplied',error:{message:'refused'}}),ack:ack('failed').write}],
    [{operation_id:'missing',state:'not_found'},{operation_id:'expired',state:'result_expired',revision:4}],
  ];
  const {service,session,cache,bind}=setup(async(method,params)=>{
    if(method==='writes.subscribe') return {operations:[],cursor:'909'};
    const page=params.cursor?Number(params.cursor.slice(5)):0;
    return {operations:pages[page],...(page<2?{next_cursor:`page-${page+1}`}:{})};
  });
  const ids=['accepted','executing','succeeded','failed','missing','expired'];
  for(const [localOrder,operationId] of ids.entries()) await cache.put({operationId,request:{method:'tasks.update',params:{}},state:'queued',revision:0,acknowledged:operationId==='missing',journalIncarnation:'journal-A',localOrder});
  await bind();
  expect(ids.map(id=>cache.rows.get(id).state)).toEqual(['accepted','executing','succeeded','failed','outcome_unknown','result_expired']);
  expect(cache.rows.get('executing')).toMatchObject({revision:2,ack:ack('executing')});
  expect(cache.rows.get('expired')).toMatchObject({revision:4,unresolved:true});
  const statuses=session.callRaw.mock.calls.filter(([method])=>method==='writes.status');
  expect(statuses.map(([,params])=>params.cursor)).toEqual([undefined,'page-1','page-2']);
  expect(statuses.every(([,params])=>JSON.stringify(params.operation_ids)===JSON.stringify(ids))).toBe(true);
  const subscriptions=session.callRaw.mock.calls.filter(([method])=>method==='writes.subscribe');
  expect(subscriptions).toHaveLength(1);
  expect(subscriptions[0][1].operation_ids).toEqual(['accepted','executing']);
  await service.close();
});
