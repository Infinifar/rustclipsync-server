'use strict';
(() => {
  const messages = {
    en: {administration:'Administration',signOut:'Sign out',privateAccess:'PRIVATE ACCESS',signIn:'Sign in to manage access.',loginHelp:'Use your administrator account. Device accounts can only synchronize.',username:'Username',password:'Password',signInButton:'Sign in',loginPrivacy:'A short-lived session. No clipboard content is shown here.',title:'Devices & accounts',subtitle:'Manage access to your shared clipboard.',createAccount:'Create account',devices:'Devices',accounts:'Accounts',filterAccount:'Filter by account',allAccounts:'All accounts',refresh:'Refresh',device:'Device',account:'Account',status:'Status',lastActivity:'Last activity',actions:'Actions',noDevices:'No devices yet. Pair a client using a sync account.',onlineHelp:'Online means an authenticated request within the last 60 seconds, not guaranteed reachability. Last activity is saved at most once per minute.',permissions:'Permissions',accountHelp:'Device accounts may be shared by several devices. All accounts share one clipboard. Environment accounts are managed in server configuration.',sessionHelp:'HTTPS · 15-minute inactivity timeout · 1-hour session limit',newAccountHelp:'A sync-only account for one or more devices. Save its password before creating it; it cannot be retrieved later.',usernameHelp:'Up to 64 letters, numbers, periods, underscores or hyphens.',generate:'Generate',showPassword:'Show password',hidePassword:'Hide password',cancel:'Cancel',revokeAccess:'Revoke access?',revokePrivacy:'This does not erase clipboard data already stored on a device.',confirmRevoke:'Revoke access',rename:'Edit device label',deviceLabel:'Device label',save:'Save',unnamed:'Unnamed device',online:'Online',offline:'Offline',unknown:'Not recorded',revoke:'Revoke',edit:'Label',admin:'Administrator & sync',legacy:'Existing sync account',deviceAccount:'Sync only',environment:'Server configuration',loading:'Loading…',updated:'Updated',signedIn:'SIGNED IN AS',accountCreated:'Account created. Use its username and password to pair your clients.',revoked:'Access revoked.',renamed:'Device label saved.',sessionExpired:'Your session expired. Sign in again.',networkError:'Unable to reach the server. Refresh to check whether the action completed before retrying.',badCredentials:'Incorrect administrator username or password.',badRequest:'Check the field values and try again.',forbidden:'Request rejected. Reload the page and sign in again.',notFound:'The item no longer exists. Refresh the list.',conflict:'That username is reserved or already exists.',rateLimit:'Too many authentication attempts or another password operation is running. Wait and try again.',serverError:'The server could not complete the request.',deviceRevoke:'This device will lose synchronization access. The account remains valid and can pair again.',accountRevoke:'This account and every device registered to it will lose access. Its password will no longer register devices.',copyId:'Copy device ID',copied:'Device ID copied.',noMatches:'No devices match this account.',close:'Close'},
    zh: {administration:'管理中心',signOut:'退出登录',privateAccess:'私有访问',signIn:'登录，管理设备访问。',loginHelp:'请使用管理员账户。设备账户只能用于同步。',username:'用户名',password:'密码',signInButton:'登录',loginPrivacy:'使用短时会话。此页面不展示剪贴板正文。',title:'设备与账户',subtitle:'管理共享剪贴板的访问权限。',createAccount:'创建账户',devices:'设备',accounts:'账户',filterAccount:'按账户筛选',allAccounts:'全部账户',refresh:'刷新',device:'设备',account:'账户',status:'状态',lastActivity:'最后活动',actions:'操作',noDevices:'尚无设备。请在客户端使用同步账户配对。',onlineHelp:'在线表示近 60 秒内有认证请求，不保证设备可达。最后活动时间最多每分钟持久化一次。',permissions:'权限',accountHelp:'一个设备账户可供多台设备使用。所有账户共享同一份剪贴板。环境账户通过服务端配置管理。',sessionHelp:'HTTPS · 闲置 15 分钟退出 · 会话最长 1 小时',newAccountHelp:'仅用于同步的账户，可供多台设备使用。创建前请保存密码；创建后无法再次查看。',usernameHelp:'最多 64 个英文字母、数字、点、下划线或连字符。',generate:'生成密码',showPassword:'显示密码',hidePassword:'隐藏密码',cancel:'取消',revokeAccess:'撤销访问权限？',revokePrivacy:'不会删除设备上已经保存的剪贴板内容。',confirmRevoke:'撤销访问',rename:'修改设备备注',deviceLabel:'设备备注',save:'保存',unnamed:'未命名设备',online:'在线',offline:'离线',unknown:'暂无记录',revoke:'撤销',edit:'备注',admin:'管理员与同步',legacy:'现有同步账户',deviceAccount:'仅同步',environment:'服务端配置',loading:'正在加载…',updated:'更新于',signedIn:'当前账户',accountCreated:'账户已创建。请使用它的用户名和密码配对客户端。',revoked:'访问权限已撤销。',renamed:'设备备注已保存。',sessionExpired:'会话已过期，请重新登录。',networkError:'无法连接服务端。请先刷新核查操作结果，再决定是否重试。',badCredentials:'管理员用户名或密码不正确。',badRequest:'请检查输入内容后重试。',forbidden:'请求被拒绝，请刷新页面并重新登录。',notFound:'目标已不存在，请刷新列表。',conflict:'该用户名已被使用或保留。',rateLimit:'认证请求过多或另一个密码操作正在执行，请稍后重试。',serverError:'服务端未能完成请求。',deviceRevoke:'此设备将失去同步权限。设备账户仍然有效，可以重新配对。',accountRevoke:'此账户及其登记的全部设备将失去访问权限。该账户密码也无法再登记设备。',copyId:'复制设备 ID',copied:'设备 ID 已复制。',noMatches:'此账户下没有设备。',close:'关闭'}
  };
  let lang = navigator.language.toLowerCase().startsWith('zh') ? 'zh' : 'en';
  let csrf = '', currentUser = '', tab = 'devices', devices = [], accounts = [], target = null, labelTarget = null, timer = null, lastTouch = 0, loadingSession = '';
  class StaleSession extends Error {}
  const $ = id => document.getElementById(id);
  const t = key => messages[lang][key] || key;
  const element = (tag, text, className) => {const e=document.createElement(tag);if(text!==undefined)e.textContent=text;if(className)e.className=className;return e;};
  function localize() {
    document.documentElement.lang=lang==='zh'?'zh-CN':'en';
    document.querySelectorAll('[data-i18n]').forEach(e=>e.textContent=t(e.dataset.i18n));
    document.querySelectorAll('.dialog-heading .dialog-close').forEach(e=>e.setAttribute('aria-label',t('close')));
    $('language').textContent=lang==='zh'?'EN':'中文';
    if(currentUser)$('signed-in').textContent=`${t('signedIn')} ${currentUser}`;
    render();
  }
  function showError(id,message) {$(id).textContent=message;$(id).hidden=!message;}
  function notice(key) {$('notice').textContent=t(key);$('notice').hidden=false;}
  function message(status) {return t(({400:'badRequest',401:'sessionExpired',403:'forbidden',404:'notFound',409:'conflict',413:'badRequest',429:'rateLimit'})[status]||'serverError');}
  function signedOut(reason='') {
    csrf='';currentUser='';devices=[];accounts=[];clearInterval(timer);timer=null;
    document.querySelectorAll('dialog[open]').forEach(d=>d.close());
    $('account-form').reset();$('password').value='';$('workspace').hidden=true;$('logout').hidden=true;$('login-panel').hidden=false;
    $('device-rows').replaceChildren();$('account-rows').replaceChildren();showError('login-error',reason);$('username').focus();
  }
  async function api(path, method='GET', body) {
    const session=csrf;
    const headers={};if(method!=='GET'){headers['X-CSRF-Token']=session;if(body!==undefined)headers['Content-Type']='application/json';}
    let response;
    try {response=await fetch(`/api/admin/${path}`,{method,headers,credentials:'same-origin',cache:'no-store',body:body===undefined?undefined:JSON.stringify(body)});} catch {if(csrf!==session)throw new StaleSession();throw new Error(t('networkError'));}
    if(csrf!==session)throw new StaleSession();
    if(response.status===401){signedOut(t('sessionExpired'));throw new Error(t('sessionExpired'));}
    if(!response.ok)throw new Error(message(response.status));
    const value=response.status===204?null:await response.json();
    if(csrf!==session)throw new StaleSession();
    return value;
  }
  async function load() {
    const session=csrf;
    if(!session || loadingSession===session)return;loadingSession=session;$('refresh').disabled=true;showError('page-error','');
    try {const [nextDevices,nextAccounts]=await Promise.all([api('devices'),api('accounts')]);if(csrf!==session)return;devices=nextDevices;accounts=nextAccounts;render();$('updated').textContent=`${t('updated')} ${new Intl.DateTimeFormat(document.documentElement.lang,{hour:'2-digit',minute:'2-digit',second:'2-digit'}).format(new Date())}`;}
    catch(error){if(csrf===session && !(error instanceof StaleSession))showError('page-error',error.message);}finally{if(loadingSession===session){loadingSession='';$('refresh').disabled=false;}}
  }
  async function signedIn(info) {
    csrf=info.csrfToken;currentUser=info.username;lastTouch=Date.now();$('password').value='';showError('login-error','');
    $('login-panel').hidden=true;$('workspace').hidden=false;$('logout').hidden=false;$('signed-in').textContent=`${t('signedIn')} ${currentUser}`;
    await load();if(csrf!==info.csrfToken)return;clearInterval(timer);timer=setInterval(()=>{if(!document.hidden)load();},15000);
  }
  function cell(row,label,text,className) {const td=element('td',text,className);td.dataset.label=t(label);row.append(td);return td;}
  function button(text,handler,className='quiet') {const b=element('button',text,className);b.type='button';b.addEventListener('click',handler);return b;}
  function render() {
    const selected=$('filter').value;$('filter').replaceChildren(new Option(t('allAccounts'),''));
    accounts.forEach(a=>$('filter').add(new Option(a.username,a.id)));if([...$('filter').options].some(o=>o.value===selected))$('filter').value=selected;
    const filtered=devices.filter(d=>!$('filter').value || (d.accountId||'@legacy')===$('filter').value);
    $('device-rows').replaceChildren();
    for(const d of filtered) {
      const row=element('tr');const name=cell(row,'device');name.append(element('p',d.label||t('unnamed'),'device-name'),element('span',d.deviceId,'device-id'));
      cell(row,'account',d.accountName);const status=cell(row,'status');status.append(element('span',t(d.online?'online':'offline'),`status ${d.online?'online':''}`));
      const last=cell(row,'lastActivity',d.lastSeen?new Intl.DateTimeFormat(document.documentElement.lang,{dateStyle:'medium',timeStyle:'short'}).format(new Date(d.lastSeen*1000)):t('unknown'),'last-activity');
      if(d.lastSeen)last.title=new Date(d.lastSeen*1000).toISOString();
      const actions=cell(row,'actions');const group=element('div',undefined,'row-actions');
      group.append(button(t('edit'),()=>{labelTarget=d;$('device-label').value=d.label;showError('rename-error','');$('rename-dialog').showModal();}),button(t('revoke'),()=>openRevoke('device',d),'danger'));actions.append(group);$('device-rows').append(row);
    }
    $('device-empty').textContent=t(devices.length?'noMatches':'noDevices');$('device-empty').hidden=filtered.length>0;
    $('account-rows').replaceChildren();
    for(const a of accounts) {
      const row=element('tr');cell(row,'account',a.username);cell(row,'permissions',t(a.kind==='admin'?'admin':a.kind==='legacy'?'legacy':'deviceAccount'));cell(row,'devices',String(a.devices));
      const actions=cell(row,'actions');if(a.kind==='device')actions.append(button(t('revoke'),()=>openRevoke('account',a),'danger'));else actions.append(element('span',t('environment'),'small muted'));$('account-rows').append(row);
    }
    $('devices-panel').hidden=tab!=='devices';$('accounts-panel').hidden=tab!=='accounts';$('filter').hidden=tab!=='devices';
  }
  function setTab(value) {tab=value;for(const name of ['devices','accounts']){const b=$(`tab-${name}`);b.setAttribute('aria-selected',String(value===name));b.tabIndex=value===name?0:-1;}render();}
  function openRevoke(kind,value) {target={kind,value};$('revoke-description').textContent=t(kind==='device'?'deviceRevoke':'accountRevoke');$('revoke-target').textContent=kind==='device'?`${value.label||t('unnamed')}\n${value.deviceId}`:`${value.username}\n${t('devices')}: ${value.devices}`;showError('revoke-error','');$('revoke-dialog').showModal();$('revoke-dialog').querySelector('.dialog-actions .dialog-close').focus();}
  async function submit(form, errorId, operation) {
    const buttons=[...form.querySelectorAll('button')];buttons.forEach(b=>b.disabled=true);showError(errorId,'');
    try{await operation();}catch(error){if(!(error instanceof StaleSession))showError(errorId,error.message);}finally{buttons.forEach(b=>b.disabled=false);}
  }
  $('login-form').addEventListener('submit',event=>{event.preventDefault();submit(event.currentTarget,'login-error',async()=>{
    let response;try{response=await fetch('/api/admin/login',{method:'POST',headers:{'Content-Type':'application/json','X-RCS-Admin':'1'},credentials:'same-origin',cache:'no-store',body:JSON.stringify({username:$('username').value,password:$('password').value})});}catch{throw new Error(t('networkError'));}finally{$('password').value='';}
    if(!response.ok)throw new Error(response.status===401?t('badCredentials'):message(response.status));await signedIn(await response.json());
  });});
  $('logout').addEventListener('click',async()=>{try{await api('logout','POST');signedOut();}catch(error){if(csrf && !(error instanceof StaleSession))showError('page-error',error.message);}});
  $('language').addEventListener('click',()=>{lang=lang==='zh'?'en':'zh';localize();});
  for(const value of ['devices','accounts']){$(`tab-${value}`).addEventListener('click',()=>setTab(value));$(`tab-${value}`).addEventListener('keydown',event=>{if(['ArrowLeft','ArrowRight','Home','End'].includes(event.key)){event.preventDefault();const next=event.key==='Home'?'devices':event.key==='End'?'accounts':tab==='devices'?'accounts':'devices';setTab(next);$(`tab-${next}`).focus();}});}
  $('filter').addEventListener('change',render);$('refresh').addEventListener('click',load);
  $('create-account').addEventListener('click',()=>{$('account-form').reset();$('account-password').type='password';$('reveal').textContent=t('showPassword');showError('account-error','');$('account-dialog').showModal();});
  $('generate').addEventListener('click',()=>{const bytes=crypto.getRandomValues(new Uint8Array(24));$('account-password').value=Array.from(bytes,b=>b.toString(16).padStart(2,'0')).join('');$('account-password').type='text';$('reveal').textContent=t('hidePassword');});
  $('reveal').addEventListener('click',()=>{const show=$('account-password').type==='password';$('account-password').type=show?'text':'password';$('reveal').textContent=t(show?'hidePassword':'showPassword');});
  $('account-form').addEventListener('submit',event=>{event.preventDefault();submit(event.currentTarget,'account-error',async()=>{await api('accounts','POST',{username:$('account-username').value,password:$('account-password').value});$('account-dialog').close();notice('accountCreated');await load();});});
  $('revoke-form').addEventListener('submit',event=>{event.preventDefault();submit(event.currentTarget,'revoke-error',async()=>{if(!target)return;await api(`${target.kind==='device'?'devices':'accounts'}/${encodeURIComponent(target.kind==='device'?target.value.deviceId:target.value.id)}`,'DELETE');$('revoke-dialog').close();target=null;notice('revoked');await load();});});
  $('rename-form').addEventListener('submit',event=>{event.preventDefault();submit(event.currentTarget,'rename-error',async()=>{if(!labelTarget)return;await api(`devices/${encodeURIComponent(labelTarget.deviceId)}`,'PATCH',{label:$('device-label').value});$('rename-dialog').close();notice('renamed');await load();});});
  document.querySelectorAll('.dialog-close').forEach(b=>b.addEventListener('click',()=>b.closest('dialog').close()));
  $('account-dialog').addEventListener('close',()=>{$('account-password').value='';$('account-password').type='password';});
  function touch(){if(!csrf||Date.now()-lastTouch<60000)return;lastTouch=Date.now();api('session','POST').catch(error=>{if(csrf && !(error instanceof StaleSession))showError('page-error',error.message);});}
  document.addEventListener('pointerdown',touch,{passive:true});document.addEventListener('keydown',touch);document.addEventListener('visibilitychange',()=>{if(!document.hidden&&csrf)load();});
  window.addEventListener('pagehide',()=>{clearInterval(timer);$('workspace').hidden=true;$('device-rows').replaceChildren();$('account-rows').replaceChildren();});
  window.addEventListener('pageshow',event=>{if(event.persisted){signedOut();fetch('/api/admin/session',{credentials:'same-origin',cache:'no-store'}).then(r=>r.ok?r.json():null).then(info=>{if(info)signedIn(info);}).catch(()=>showError('login-error',t('networkError')));}});
  localize();fetch('/api/admin/session',{credentials:'same-origin',cache:'no-store'}).then(r=>r.ok?r.json():null).then(info=>{if(info)signedIn(info);}).catch(()=>showError('login-error',t('networkError')));
})();
