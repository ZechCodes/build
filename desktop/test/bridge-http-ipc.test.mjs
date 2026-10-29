import assert from 'node:assert/strict';
import {EventEmitter} from 'node:events';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import {runInNewContext} from 'node:vm';
import {installBridgeHttpIpc} from '../src/bridge-http-ipc.mjs';
import {APP_URL} from '../src/security-policy.mjs';

function harness(t) {
  const ipcMain={handle(channel,handler){this.channel=channel;this.handler=handler;}};
  const contents=new EventEmitter();contents.mainFrame={url:APP_URL};contents.id=1;contents.send=(...args)=>calls.sent.push(args);
  const window={webContents:contents};const auth={accessToken:'fixture-account-token',generation:1};const calls={sent:[],fetch:[]};
  const fetch=async(url,options)=>{calls.fetch.push({url,options});return new Response(JSON.stringify({issuer:'https://getbuild.ing',account_id:'a',device_id:'d',origin:'https://bridge.test',endpoint_epoch:'e',family_id:'f',access_token:'fixture-device-token',expires_at:Math.floor(Date.now()/1000)+300,scope:'bridge:read'}));};
  const bridge=installBridgeHttpIpc({ipcMain,getWindow:()=>window,getAuth:()=>auth,fetch,manifest:{version:1,operations:[]}});
  bridge.attach(contents);t.after(()=>bridge.clear());
  const event=()=>({sender:contents,senderFrame:contents.mainFrame});
  const invoke=(method,args=[],source=event())=>ipcMain.handler(source,method,args);
  return {bridge,invoke,contents,auth,calls,event};
}

test('IPC rejects subframes, other windows and nonhosted frames before requests',async t=>{
  const h=harness(t);
  for(const source of [{sender:h.contents,senderFrame:{url:APP_URL}},{sender:{},senderFrame:h.contents.mainFrame}]) {
    assert.equal((await h.invoke('resume',[{deviceId:'d',familyId:'f',endpointEpoch:'e'}],source)).error.code,'untrusted_caller');
  }
  h.contents.mainFrame.url='https://evil.test/app/desktop';
  assert.equal((await h.invoke('resume',[])).error.code,'untrusted_caller');
  assert.equal(h.calls.fetch.length,0);
});

test('IPC only dispatches finite typed operations and sanitizes failures',async t=>{
  const h=harness(t);
  for(const method of ['fetch','__proto__','constructor','toString']) assert.equal((await h.invoke(method,[])).error.code,'unsupported_operation');
  assert.equal((await h.invoke('open',['h','1','extra'])).error.code,'invalid_params');
  const result=await h.invoke('resume',[{deviceId:'d',familyId:'f',endpointEpoch:'e'}]);
  assert.equal(result.ok,true);assert.equal(JSON.stringify(result).includes('fixture-device-token'),false);
});

test('main-frame replacement and auth generation fence handles',async t=>{
  const h=harness(t);const first=await h.invoke('resume',[{deviceId:'d',familyId:'f',endpointEpoch:'e'}]);
  h.contents.emit('did-start-navigation',{},APP_URL,false,true);
  const stale=await h.invoke('open',[first.value.handle,'1']);assert.equal(stale.ok,false);
  const second=await h.invoke('resume',[{deviceId:'d',familyId:'f',endpointEpoch:'e'}]);
  h.auth.generation++;
  assert.equal((await h.invoke('open',[second.value.handle,'1'])).error.code,'stale_owner');
});

test('sandbox preload exposes only typed functions, strips Electron event and removes listeners',async()=>{
  const events=new EventEmitter();const invocations=[];let facade;
  events.invoke=async(...args)=>{invocations.push(args);return {ok:true,value:1};};
  runInNewContext(readFileSync(new URL('../src/preload.cjs',import.meta.url),'utf8'),{
    require:name=>{assert.equal(name,'electron');return {ipcRenderer:events,contextBridge:{exposeInMainWorld(name,value){assert.equal(name,'buildBridgeHttp');facade=value;}}};},
  });
  assert.equal(facade.fetch,undefined);assert.equal(facade.invoke,undefined);
  await facade.request('h','op',{x:1},{requestId:'r'});
  assert.equal(invocations[0][0],'build:bridge-http');assert.equal(invocations[0][1],'request');
  let event;const off=facade.onEvent(value=>{event=value;});
  events.emit('build:bridge-http-event',{secret:'Electron event'},{stream:'s',sequence:1,event:'changes',data:{}});
  assert.equal(event.stream,'s');assert.equal(event.secret,undefined);off();assert.equal(events.listenerCount('build:bridge-http-event'),0);
});
