import { createBridgeHttp } from './bridge-http.mjs';
import { transportError, MAX_JSON_BYTES } from './bridge-http-network.mjs';
import { APP_URL } from './security-policy.mjs';

const CHANNEL='build:bridge-http';
const EVENT='build:bridge-http-event';
const ARITY=new Map([['discover',[1,1]],['resume',[1,1]],['open',[2,2]],['request',[3,4]],['subscribe',[2,2]],['ack',[2,2]],['unsubscribe',[1,1]],['cancel',[1,1]],['close',[1,1]]]);
const {origin,pathname}=new URL(APP_URL);
function trustedUrl(value) {
  try {const url=new URL(value);return url.origin===origin && url.pathname===pathname;}
  catch {return false;}
}

export function installBridgeHttpIpc({ipcMain,getWindow,getAuth,fetch,manifest}) {
  const owners=new WeakMap();let nextId=0;
  const isOwnerCurrent=owner=>owners.get(owner.contents)===owner && getWindow()?.webContents===owner.contents;
  const bridge=createBridgeHttp({fetch,manifest,getAuth,isOwnerCurrent,send:(owner,event)=>{
    if(isOwnerCurrent(owner)) owner.contents.send(EVENT,event);
  }});
  function retire(contents) {
    const owner=owners.get(contents);
    if(owner) {owners.delete(contents);bridge.clear(owner);}
  }
  function attach(contents) {
    contents.on('did-start-navigation',(_event,_url,inPlace,isMainFrame)=>{if(isMainFrame && !inPlace) retire(contents);});
    contents.on('render-process-gone',()=>retire(contents));
    contents.on('destroyed',()=>retire(contents));
  }
  function caller(event) {
    const contents=getWindow()?.webContents;
    if(!contents || event.sender!==contents || event.senderFrame!==contents.mainFrame || !trustedUrl(event.senderFrame?.url)) throw transportError('untrusted_caller');
    let owner=owners.get(contents);
    if(!owner) {owner={contents,id:++nextId};owners.set(contents,owner);}
    return owner;
  }
  ipcMain.handle(CHANNEL,async(event,method,args)=>{
    try {
      const owner=caller(event);const limits=ARITY.get(method);
      if(!limits) throw transportError('unsupported_operation');
      if(!Array.isArray(args) || args.length<limits[0] || args.length>limits[1] || Buffer.byteLength(JSON.stringify(args))>8*MAX_JSON_BYTES) throw transportError('invalid_params');
      const value=await bridge[method](owner,...args);
      if(!isOwnerCurrent(owner)) throw transportError('stale_owner');
      return {ok:true,value};
    } catch(error) {
      const safe=error.message?.startsWith('Bridge HTTP ') ? error : transportError('request_failed');
      return {ok:false,error:{code:safe.code,message:safe.message,...(error.uncertain ? {uncertain:true} : {})}};
    }
  });
  return {attach,retire,clear:()=>bridge.clear(),revoke:()=>bridge.revoke?.()};
}
