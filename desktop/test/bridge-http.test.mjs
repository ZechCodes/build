import assert from 'node:assert/strict';
import test from 'node:test';
import { createBridgeHttp } from '../src/bridge-http.mjs';

const mint = overrides => ({issuer:'https://getbuild.ing',account_id:'account',device_id:'device',origin:'https://bridge.test',endpoint_epoch:'epoch',family_id:'family',access_token:'fixture-device-token',expires_at:Math.floor(Date.now()/1000)+300,scope:'bridge:read bridge:write',name:'Bridge',fingerprint:'fingerprint',...overrides});
const opening = () => ({session_id:'session',opening_generation:'generation',grant_expires_at:new Date(Date.now()+30000).toISOString(),greeting:{capabilities:[]}});
const json = (value,status=200) => new Response(JSON.stringify(value),{status,headers:{'content-type':'application/json'}});
function harness(t, options={}) {
  const requests=[]; const output=[];
  const owner={generation:1,alive:true};
  const auth={generation:1,accessToken:'fixture-hosted-token'};
  const fetch=async(url,opts)=>{
    requests.push({url,opts});
    if (options.fetch) {const result=await options.fetch(url,opts); if (result) return result;}
    if(url.endsWith('/challenges')) return json({challenge_id:'challenge',nonce:'n'.repeat(32),issuer:'https://getbuild.ing',expires_at:Math.floor(Date.now()/1000)+60});
    if(url.endsWith('/.well-known/build-bridge')) return json({proof:'fixture-proof'});
    if(url.endsWith('/complete') || url.endsWith('/http-credentials')) return json(mint(options.mint));
    if(url.endsWith('/v1/sessions')) return json(opening(),201);
    if(url.endsWith('/v1/subscriptions')) return json({subscription_id:'subscription',durable:false},201);
    if(opts.method==='DELETE') return new Response(null,{status:204});
    return json({result:{tasks:[]}});
  };
  const bridge=createBridgeHttp({fetch,getAuth:()=>auth,isOwnerCurrent:o=>o.alive,send:(o,event)=>output.push({o,event}),manifest:{version:1,operations:[{method:'tasks.list',operationId:'listTasks',verb:'POST',path:'/v1/projects/{project_id}/task-queries',pathParams:['project_id'],queryParams:[],body:'remaining',responseMaxBytes:1024,scope:'bridge:read'}]}});
  t.after(()=>bridge.clear());
  return {bridge,owner,auth,requests,output};
}

test('discovery sends hosted OAuth only to hosted routes; device token remains private',async t=>{
  const h=harness(t);const identity=await h.bridge.discover(h.owner,'https://bridge.test');
  assert.deepEqual(Object.keys(identity).sort(),['deviceId','endpointEpoch','familyId','handle','origin']);
  assert.equal(JSON.stringify(identity).includes('token'),false);
  assert.equal(h.requests[0].opts.headers.Authorization,'Bearer fixture-hosted-token');
  assert.equal(h.requests[1].opts.headers.Authorization,undefined);
  assert.equal(h.requests[2].opts.headers.Authorization,'Bearer fixture-hosted-token');
  await h.bridge.open(h.owner,identity.handle,'1');
  assert.equal(h.requests[3].opts.headers.Authorization,'Bearer fixture-device-token');
});

test('substituted endpoint and account renewal bindings are refused before bridge credentials',async t=>{
  const h=harness(t,{mint:{origin:'https://other.test'}});
  await assert.rejects(h.bridge.discover(h.owner,'https://bridge.test'),{code:'binding_mismatch'});
  assert.equal(h.requests.some(r=>r.opts.headers.Authorization==='Bearer fixture-device-token'),false);
  await assert.rejects(h.bridge.resume(h.owner,{deviceId:'other',origin:'https://bridge.test',familyId:'family',endpointEpoch:'epoch'}),{code:'binding_mismatch'});
});

test('handles belong to exact owner and credential generation; late discovery cannot resurrect',async t=>{
  const h=harness(t); const {handle}=await h.bridge.discover(h.owner,'https://bridge.test');
  await assert.rejects(h.bridge.open({...h.owner},handle,'1'),{code:'invalid_handle'});
  h.auth.generation++;
  await assert.rejects(h.bridge.open(h.owner,handle,'1'),{code:'stale_owner'});
  let release; const wait=new Promise(resolve=>release=resolve);
  const late=harness(t,{fetch:async url=>{if(url.endsWith('/complete')){await wait;return json(mint());}}});
  const pending=late.bridge.discover(late.owner,'https://bridge.test');
  await new Promise(setImmediate);late.bridge.clear();release();
  await assert.rejects(pending,{code:'stale_owner'});
});

test('request resolver forbids proxy overrides, validates identifiers and preserves typed params',async t=>{
  const h=harness(t);const {handle}=await h.bridge.discover(h.owner,'https://bridge.test');await h.bridge.open(h.owner,handle,'1');
  await assert.rejects(h.bridge.request(h.owner,handle,'https://evil.test',{}),{code:'unsupported_operation'});
  await assert.rejects(h.bridge.request(h.owner,handle,'listTasks',{project_id:'p'},{headers:{Authorization:'bad'}}),{code:'invalid_options'});
  await assert.rejects(h.bridge.request(h.owner,handle,'listTasks',{project_id:'..'}),{code:'invalid_params'});
  assert.deepEqual(await h.bridge.request(h.owner,handle,'listTasks',{project_id:'p/a',filters:['open']}),{result:{tasks:[]}});
  const last=h.requests.at(-1);assert.equal(last.url,'https://bridge.test/v1/projects/p%2Fa/task-queries');
  assert.deepEqual(JSON.parse(last.opts.body),{filters:['open']});
  assert.equal(last.opts.headers['X-Build-Session'],'session');
});

test('cancel and signout abort outstanding work and drop late replies',async t=>{
  let release;
  const h=harness(t,{fetch:async url=>{if(url.includes('task-queries'))return new Promise(resolve=>release=resolve);}});
  const {handle}=await h.bridge.discover(h.owner,'https://bridge.test');await h.bridge.open(h.owner,handle,'1');
  const pending=h.bridge.request(h.owner,handle,'listTasks',{project_id:'p'},{requestId:'request'});
  await new Promise(setImmediate);h.bridge.cancel(h.owner,'request');
  assert.equal(h.requests.at(-1).opts.signal.aborted,true);release(json({result:'late'}));
  await assert.rejects(pending,{code:'canceled'});
  h.bridge.clear();
  await assert.rejects(h.bridge.request(h.owner,handle,'listTasks',{project_id:'p'}),{code:'invalid_handle'});
});
