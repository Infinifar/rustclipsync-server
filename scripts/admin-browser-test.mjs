// Disposable HTTPS server and Chromium profile. Never uses a desktop keyring,
// clipboard, deployed service, or existing browser profile. Node 22+; no npm deps.
import assert from 'node:assert/strict';
import {mkdtemp,readFile,writeFile,mkdir,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {resolve,join} from 'node:path';
import {spawn,spawnSync} from 'node:child_process';
import http from 'node:http';
import https from 'node:https';
import net from 'node:net';
import {once} from 'node:events';

const binary=resolve(process.argv[2]||'target/debug/rustclipsync-server');
const screenshots=resolve(process.argv[3]||'target/admin-ui');
const password='disposable-admin-password',devicePassword='disposable-device-password';
const temp=await mkdtemp(join(tmpdir(),'rcs-admin-ui-'));
let server,chrome,proxy,ws;
let holdNextDeviceRequest=false,heldDeviceResponse;
const delay=ms=>new Promise(r=>setTimeout(r,ms));
async function port(){const socket=net.createServer();socket.listen(0,'127.0.0.1');await once(socket,'listening');const p=socket.address().port;await new Promise(r=>socket.close(r));return p;}
async function until(fn,description){for(let i=0;i<150;i++){if(await fn())return;await delay(100);}throw new Error(`Timed out: ${description}`);}
try {
  await mkdir(screenshots,{recursive:true});
  const backend=await port();
  const cert=join(temp,'cert.pem'),key=join(temp,'key.pem');
  const generated=spawnSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-days','1','-subj','/CN=127.0.0.1','-keyout',key,'-out',cert],{stdio:'ignore'});
  assert.equal(generated.status,0,'test TLS certificate generation');
  proxy=https.createServer({key:await readFile(key),cert:await readFile(cert)},(request,response)=>{
    if(holdNextDeviceRequest && request.url==='/api/admin/devices'){
      holdNextDeviceRequest=false;heldDeviceResponse=response;request.resume();return;
    }
    const upstream=http.request({hostname:'127.0.0.1',port:backend,path:request.url,method:request.method,headers:request.headers},result=>{response.writeHead(result.statusCode,result.headers);result.pipe(response);});
    upstream.on('error',()=>{response.writeHead(502);response.end();});request.pipe(upstream);
  });
  proxy.listen(0,'127.0.0.1');await once(proxy,'listening');
  const origin=`https://127.0.0.1:${proxy.address().port}`;
  const env={...process.env,RCS_LISTEN:`127.0.0.1:${backend}`,RCS_DATA_DIR:join(temp,'data'),RCS_DB_KEY:'3c'.repeat(32),RCS_USER:'fixture',RCS_PASSWORD:'disposable-legacy-password',RCS_ADMIN_USER:'administrator',RCS_ADMIN_PASSWORD:password,RCS_WEB_ENABLED:'true',RCS_WEB_ORIGIN:origin};
  delete env.RCS_PASSWORD_FILE;delete env.RCS_ADMIN_PASSWORD_FILE;
  server=spawn(binary,[],{env,stdio:['ignore','ignore','pipe']});
  let serverErrors='';server.stderr.on('data',b=>{serverErrors=(serverErrors+b.toString()).slice(-5000);});
  await until(async()=>{if(server.exitCode!==null)throw new Error(`fixture exited: ${serverErrors}`);try{return(await fetch(`http://127.0.0.1:${backend}/healthz`)).ok;}catch{return false;}},'fixture startup');
  const executable=process.env.CHROMIUM_BIN||['chromium','google-chrome','chromium-browser'].find(p=>spawnSync('which',[p],{stdio:'ignore'}).status===0);
  assert.ok(executable,'Chromium or CHROMIUM_BIN required');
  const profile=join(temp,'browser');
  const args=['--headless=new',`--user-data-dir=${profile}`,'--password-store=basic','--no-first-run','--no-default-browser-check','--disable-sync','--disable-background-networking','--disable-extensions','--disable-component-update','--disable-gpu','--ignore-certificate-errors','--remote-debugging-port=0','--window-size=1505,1045','--force-device-scale-factor=1','about:blank'];
  if(process.getuid?.()===0)args.push('--no-sandbox');
  chrome=spawn(executable,args,{stdio:'ignore'});
  let devtools;
  await until(async()=>{try{devtools=(await readFile(join(profile,'DevToolsActivePort'),'utf8')).trim().split('\n');return devtools.length===2;}catch{return false;}},'disposable browser');
  ws=new WebSocket(`ws://127.0.0.1:${devtools[0]}${devtools[1]}`);await once(ws,'open');
  let next=0;const pending=new Map();const exceptions=[],remoteRequests=[];
  ws.addEventListener('message',event=>{const m=JSON.parse(event.data);if(m.id){const p=pending.get(m.id);if(p){pending.delete(m.id);clearTimeout(p.timer);m.error?p.reject(new Error(JSON.stringify(m.error))):p.resolve(m.result);}}else if(m.method==='Runtime.exceptionThrown'){exceptions.push(m.params.exceptionDetails.text);}else if(m.method==='Network.requestWillBeSent' && !m.params.request.url.startsWith(origin) && !m.params.request.url.startsWith('about:'))remoteRequests.push(m.params.request.url);});
  const cdp=(method,params={},sessionId)=>new Promise((resolve,reject)=>{const id=++next;const timer=setTimeout(()=>{pending.delete(id);reject(new Error(`CDP timeout: ${method}`));},15000);pending.set(id,{resolve,reject,timer});ws.send(JSON.stringify({id,method,params,...(sessionId?{sessionId}:{})}));});
  const {targetId}=await cdp('Target.createTarget',{url:'about:blank'});
  const {sessionId}=await cdp('Target.attachToTarget',{targetId,flatten:true});
  const command=(method,params)=>cdp(method,params,sessionId);
  await command('Page.enable');await command('Runtime.enable');await command('Network.enable');await command('Accessibility.enable');
  const evaluate=async expression=>{const result=await command('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(result.exceptionDetails)throw new Error(result.exceptionDetails.text);return result.result.value;};
  const click=selector=>evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
  const fill=(id,value)=>evaluate(`document.getElementById(${JSON.stringify(id)}).value=${JSON.stringify(value)}`);
  const submit=id=>evaluate(`document.getElementById(${JSON.stringify(id)}).requestSubmit()`);
  const wait=(expression,description=expression)=>until(()=>evaluate(expression),description);
  async function capture(name,width,height,mobile=false){await command('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile});await delay(150);const {data}=await command('Page.captureScreenshot',{format:'png',captureBeyondViewport:false});await writeFile(join(screenshots,name+'.png'),Buffer.from(data,'base64'));}
  async function rest(path,{method='GET',headers={},body}={}){return new Promise((resolve,reject)=>{const req=https.request(origin+path,{method,rejectUnauthorized:false,headers:{'X-RCS-Protocol':'3',...(headers.Authorization?.startsWith('Bearer ')?{'X-RCS-Sync-Session':headers.Authorization.slice(7).split(':')[0]}:{}),...headers,...(body!==undefined?{'Content-Type':'application/json'}:{})}},res=>{const chunks=[];res.on('data',c=>chunks.push(c));res.on('end',()=>{const text=Buffer.concat(chunks).toString();let value;try{value=JSON.parse(text);}catch{value=text;}resolve({status:res.statusCode,value});});});req.on('error',reject);req.end(body===undefined?undefined:JSON.stringify(body));});}
  const auth=(user,pass)=>'Basic '+Buffer.from(`${user}:${pass}`).toString('base64');
  const ids=[1,2,3,4].map(n=>`00000000-0000-4000-8000-${String(n).padStart(12,'0')}`);
  const token='ab'.repeat(32);const bearer=id=>`Bearer ${id}:${token}`;
  await command('Page.navigate',{url:origin+'/admin/'});await wait(`!!document.getElementById('login-form')`);
  await click('#language'); // Browser's fixture locale is en-US; select deterministic English below.
  if(await evaluate(`document.documentElement.lang!=='en'`))await click('#language');
  await capture('login',1505,1045);
  await fill('username','administrator');await fill('password','wrong');await submit('login-form');
  await wait(`document.getElementById('login-error').textContent.includes('Incorrect')`);
  assert.equal(await evaluate(`document.getElementById('password').value`),'');
  await fill('password',password);await submit('login-form');await wait(`!document.getElementById('workspace').hidden && document.getElementById('account-rows').children.length===2`);
  await click('#create-account');await fill('account-username','shared-device');await fill('account-password',devicePassword);await submit('account-form');await wait(`!document.getElementById('account-dialog').open && document.getElementById('account-rows').children.length===3`);
  assert.equal(await evaluate(`document.getElementById('account-password').value`),'');
  for(let i=0;i<4;i++){const user=i<2?'shared-device':i===2?'fixture':'administrator';const pass=i<2?devicePassword:i===2?'disposable-legacy-password':password;assert.equal((await rest('/api/devices',{method:'POST',headers:{Authorization:auth(user,pass)},body:{deviceId:ids[i],deviceToken:token}})).status,200);assert.equal((await rest(`/api/devices/${ids[i]}/sync-direction`,{method:'PUT',headers:{Authorization:bearer(ids[i])},body:{sessionId:ids[i],version:1,direction:'BIDIRECTIONAL',paused:false,connect:true}})).status,200);}
  assert.equal((await rest('/api/state',{headers:{Authorization:bearer(ids[0])}})).status,200);
  await click('#refresh');await wait(`document.getElementById('device-rows').children.length===4`);
  for(let i=0;i<4;i++) {await click(`#device-rows tr:nth-child(${i+1}) button`);await fill('device-label',['Workstation','Phone','Laptop','Tablet'][i]);await submit('rename-form');await wait(`!document.getElementById('rename-dialog').open && document.querySelector('#device-rows tr:nth-child(${i+1}) .device-name').textContent===${JSON.stringify(['Workstation','Phone','Laptop','Tablet'][i])}`);}
  await click('#device-rows tr:nth-child(4) button');await fill('device-label','<img src=x onerror="window.testXss=1">');await submit('rename-form');await wait(`!document.getElementById('rename-dialog').open && document.querySelector('#device-rows tr:nth-child(4) .device-name').textContent.startsWith('<img')`);
  assert.equal(await evaluate(`document.querySelectorAll('#device-rows img').length`),0);assert.equal(await evaluate(`window.testXss||0`),0);
  await click('#device-rows tr:nth-child(4) button');await fill('device-label','Tablet');await submit('rename-form');await wait(`!document.getElementById('rename-dialog').open && document.querySelector('#device-rows tr:nth-child(4) .device-name').textContent==='Tablet'`);
  assert.equal(await evaluate(`document.querySelectorAll('#device-rows .online').length`),4);
  await capture('devices-desktop',1505,1045);
  await capture('devices-mobile',390,844,true);
  assert.ok(await evaluate(`document.documentElement.scrollWidth<=window.innerWidth`),'no mobile horizontal overflow');
  await command('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:'dark'}]});await capture('devices-dark',1505,1045);await command('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:'light'}]});
  assert.equal(await evaluate(`document.getElementById('backfill-window').value`),'5');
  await fill('backfill-window','0');await submit('sync-settings-form');await wait(`document.getElementById('sync-settings-status').textContent.includes('saved') && document.getElementById('sync-settings-audit').textContent.includes('administrator')`);
  assert.equal(await evaluate(`fetch('/api/admin/sync-settings').then(r=>r.json()).then(v=>v.backfillWindowMinutes)`),0);
  await evaluate(`document.getElementById('sync-settings-title').scrollIntoView({block:'center'})`);await capture('settings-desktop',1505,1045);
  await command('Emulation.setDeviceMetricsOverride',{width:390,height:844,deviceScaleFactor:1,mobile:true});await evaluate(`document.getElementById('sync-settings-title').scrollIntoView({block:'center'})`);await capture('settings-mobile',390,844,true);
  await evaluate(`window.scrollTo(0,0)`);
  assert.equal(await evaluate(`fetch('/api/admin/sync-settings',{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({backfillWindowMinutes:5})}).then(r=>r.status)`),403);
  const ax=await command('Accessibility.getFullAXTree');assert.equal(ax.nodes.filter(n=>n.role?.value==='button'&&!n.name?.value).length,0,'buttons need accessible names');
  const cookies=await command('Network.getCookies',{urls:[origin]});const cookie=cookies.cookies.find(c=>c.name==='__Host-rcs-admin');assert.ok(cookie?.httpOnly&&cookie.secure&&cookie.sameSite==='Strict');
  assert.equal(await evaluate('localStorage.length+sessionStorage.length'),0,'no credentials in web storage');
  assert.equal(await evaluate(`fetch('/api/admin/devices/${ids[0]}',{method:'DELETE'}).then(r=>r.status)`),403,'missing CSRF must fail');
  await click('#tab-accounts');await wait(`!document.getElementById('accounts-panel').hidden`);await capture('accounts-desktop',1505,1045);
  await click('#account-rows tr:first-child button');await wait(`document.getElementById('revoke-dialog').open`);
  await command('Input.dispatchKeyEvent',{type:'keyDown',key:'Tab',code:'Tab',windowsVirtualKeyCode:9});await command('Input.dispatchKeyEvent',{type:'keyUp',key:'Tab',code:'Tab',windowsVirtualKeyCode:9});
  assert.ok(await evaluate(`document.activeElement.closest('dialog')?.id==='revoke-dialog'`),'dialog traps focus');
  await command('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});await command('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});await wait(`!document.getElementById('revoke-dialog').open`);
  assert.equal((await rest('/api/state',{headers:{Authorization:bearer(ids[1])}})).status,200,'cancel does not revoke');
  await click('#account-rows tr:first-child button');await capture('revoke-dialog',1505,1045);await submit('revoke-form');await wait(`!document.getElementById('revoke-dialog').open && document.getElementById('account-rows').children.length===2`);
  for(const id of ids.slice(0,2))assert.equal((await rest('/api/state',{headers:{Authorization:bearer(id)}})).status,401);
  for(const id of ids.slice(2))assert.equal((await rest('/api/state',{headers:{Authorization:bearer(id)}})).status,200);
  assert.equal((await rest('/api/devices',{method:'POST',headers:{Authorization:auth('shared-device',devicePassword)},body:{deviceId:ids[0],deviceToken:token}})).status,401);
  await click('#logout');await wait(`!document.getElementById('login-panel').hidden && document.getElementById('workspace').hidden`);
  assert.equal(await evaluate(`fetch('/api/admin/devices').then(r=>r.status)`),401);
  assert.equal(await evaluate(`document.getElementById('device-rows').children.length`),0);
  // An old request's delayed 401 must not sign out a newer valid session.
  await fill('password',password);await submit('login-form');await wait(`!document.getElementById('workspace').hidden && document.getElementById('device-rows').children.length===2`);
  holdNextDeviceRequest=true;await click('#refresh');await until(()=>!!heldDeviceResponse,'old-session request held by fixture');
  await click('#logout');await wait(`!document.getElementById('login-panel').hidden`);
  await fill('password',password);await submit('login-form');await wait(`!document.getElementById('workspace').hidden`);
  heldDeviceResponse.writeHead(401);heldDeviceResponse.end();await delay(250);
  assert.equal(await evaluate(`document.getElementById('workspace').hidden`),false,'delayed old-session 401 must preserve the new session');
  await wait(`document.getElementById('device-rows').children.length===2`);
  assert.equal(await evaluate(`fetch('/api/admin/session').then(r=>r.status)`),200);
  await click('#logout');await wait(`!document.getElementById('login-panel').hidden`);
  assert.deepEqual(exceptions,[],'no browser runtime exceptions');assert.deepEqual(remoteRequests,[],'no external asset requests');
  console.log('PASS: HTTPS admin login, multi-device account, presence, labels/XSS, CSRF, cookie flags, keyboard dialogs, account revocation, client isolation, logout and session replacement race');
  console.log('Screenshots:',screenshots);
} finally {
  if(ws)ws.close();
  for(const child of [chrome,server])if(child&&child.exitCode===null){child.kill('SIGTERM');await Promise.race([once(child,'exit'),delay(3000)]);if(child.exitCode===null)child.kill('SIGKILL');}
  if(proxy){proxy.closeAllConnections();await new Promise(r=>proxy.close(r));}
  await rm(temp,{recursive:true,force:true});
}
