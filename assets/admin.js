'use strict';
(() => {
  const messages = {
    en: {administration:'Administration',signOut:'Sign out',privateAccess:'PRIVATE ACCESS',signIn:'Sign in to manage access.',loginHelp:'Use your administrator account. Device accounts can only synchronize.',username:'Username',password:'Password',signInButton:'Sign in',loginPrivacy:'A short-lived session. No clipboard content is shown here.',title:'Synchronization access',subtitle:'Manage access to your shared clipboard.',createAccount:'Create account',devices:'Devices',accounts:'Accounts',filterAccount:'Filter by account',allAccounts:'All accounts',refresh:'Refresh',device:'Device',account:'Account',status:'Status',lastActivity:'Last activity',actions:'Actions',noDevices:'No devices yet. Pair a client using a sync account.',onlineHelp:'Online means an authenticated request within the last 60 seconds, not guaranteed reachability. Last activity is saved at most once per minute.',permissions:'Permissions',accountHelp:'Device accounts may be shared by several devices. All accounts share one clipboard. Environment accounts are managed in server configuration.',sessionHelp:'HTTPS · 15-minute inactivity timeout · 1-hour session limit',newAccountHelp:'A sync-only account for one or more devices. Save its password before creating it; it cannot be retrieved later.',usernameHelp:'Up to 64 letters, numbers, periods, underscores or hyphens.',generate:'Generate',showPassword:'Show password',hidePassword:'Hide password',cancel:'Cancel',revokeAccess:'Revoke access?',revokePrivacy:'This does not erase clipboard data already stored on a device.',confirmRevoke:'Revoke access',rename:'Edit device label',deviceLabel:'Device label',save:'Save',unnamed:'Unnamed device',online:'Online',offline:'Offline',unknown:'Not recorded',revoke:'Revoke',edit:'Label',admin:'Administrator & sync',legacy:'Existing sync account',deviceAccount:'Sync only',environment:'Server configuration',loading:'Loading…',updated:'Updated',signedIn:'SIGNED IN AS',accountCreated:'Account created. Use its username and password to pair your clients.',revoked:'Access revoked.',renamed:'Device label saved.',sessionExpired:'Your session expired. Sign in again.',networkError:'Unable to reach the server. Refresh to check whether the action completed before retrying.',badCredentials:'Incorrect administrator username or password.',badRequest:'Check the field values and try again.',forbidden:'Request rejected. Reload the page and sign in again.',notFound:'The item no longer exists. Refresh the list.',conflict:'That username is reserved or already exists.',rateLimit:'Too many authentication attempts or another password operation is running. Wait and try again.',serverError:'The server could not complete the request.',deviceRevoke:'This device will lose synchronization access. The account remains valid and can pair again.',accountRevoke:'This account and every device registered to it will lose access. Its password will no longer register devices.',copyId:'Copy device ID',copied:'Device ID copied.',noMatches:'No devices match this account.',close:'Close'},
    zh: {administration:'管理中心',signOut:'退出登录',privateAccess:'私有访问',signIn:'登录，管理设备访问。',loginHelp:'请使用管理员账户。设备账户只能用于同步。',username:'用户名',password:'密码',signInButton:'登录',loginPrivacy:'使用短时会话。此页面不展示剪贴板正文。',title:'同步管理',subtitle:'管理共享剪贴板的访问权限。',createAccount:'创建账户',devices:'设备',accounts:'账户',filterAccount:'按账户筛选',allAccounts:'全部账户',refresh:'刷新',device:'设备',account:'账户',status:'状态',lastActivity:'最后活动',actions:'操作',noDevices:'尚无设备。请在客户端使用同步账户配对。',onlineHelp:'在线表示近 60 秒内有认证请求，不保证设备可达。最后活动时间最多每分钟持久化一次。',permissions:'权限',accountHelp:'一个设备账户可供多台设备使用。所有账户共享同一份剪贴板。环境账户通过服务端配置管理。',sessionHelp:'HTTPS · 闲置 15 分钟退出 · 会话最长 1 小时',newAccountHelp:'仅用于同步的账户，可供多台设备使用。创建前请保存密码；创建后无法再次查看。',usernameHelp:'最多 64 个英文字母、数字、点、下划线或连字符。',generate:'生成密码',showPassword:'显示密码',hidePassword:'隐藏密码',cancel:'取消',revokeAccess:'撤销访问权限？',revokePrivacy:'不会删除设备上已经保存的剪贴板内容。',confirmRevoke:'撤销访问',rename:'修改设备备注',deviceLabel:'设备备注',save:'保存',unnamed:'未命名设备',online:'在线',offline:'离线',unknown:'暂无记录',revoke:'撤销',edit:'备注',admin:'管理员与同步',legacy:'现有同步账户',deviceAccount:'仅同步',environment:'服务端配置',loading:'正在加载…',updated:'更新于',signedIn:'当前账户',accountCreated:'账户已创建。请使用它的用户名和密码配对客户端。',revoked:'访问权限已撤销。',renamed:'设备备注已保存。',sessionExpired:'会话已过期，请重新登录。',networkError:'无法连接服务端。请先刷新核查操作结果，再决定是否重试。',badCredentials:'管理员用户名或密码不正确。',badRequest:'请检查输入内容后重试。',forbidden:'请求被拒绝，请刷新页面并重新登录。',notFound:'目标已不存在，请刷新列表。',conflict:'该用户名已被使用或保留。',rateLimit:'认证请求过多或另一个密码操作正在执行，请稍后重试。',serverError:'服务端未能完成请求。',deviceRevoke:'此设备将失去同步权限。设备账户仍然有效，可以重新配对。',accountRevoke:'此账户及其登记的全部设备将失去访问权限。该账户密码也无法再登记设备。',copyId:'复制设备 ID',copied:'设备 ID 已复制。',noMatches:'此账户下没有设备。',close:'关闭'}
  };
  Object.assign(messages.en,{backfillTitle:'Reconnect backfill',backfillHelp:'Keep only the latest eligible message per device. Global setting; 0 disables and clears offline backfill. Changes apply immediately to new messages.',backfillWindow:'Window in minutes (0–1440)',backfillSaved:'Backfill setting saved.',backfillInvalid:'Invalid server configuration: backfill is safely disabled. Save a valid value to repair it.',backfillChanged:'Last changed by',backfillNoAudit:'No configuration changes yet.'});
  Object.assign(messages.zh,{backfillTitle:'重连补发',backfillHelp:'每台设备只保留最近一条满足条件的消息。全局生效；0 关闭并清除离线补发。保存后立即用于新消息，已存消息的到期时间保持不变。',backfillWindow:'时间窗口（分钟，0–1440）',backfillSaved:'补发设置已保存。',backfillInvalid:'服务端配置异常：已安全关闭补发。保存合法值即可修复。',backfillChanged:'最近修改人',backfillNoAudit:'暂无配置变更记录。'});
  Object.assign(messages.en,{
    settings:'Settings',opensNewTab:' (opens in a new tab)',syncState:'Sync state',
    pushOnly:'Send only',receiveOnly:'Receive only',bidirectional:'Bidirectional',paused:'Paused',
    reportReported:'Accepted by server',reportUnknown:'Direction not reported',reportRevoked:'Direction revoked',reportExpired:'Direction expired',
    syncDetails:'Sync details',reportedAt:'Last report',expiresAt:'Activity expiry',reportSource:'Last report via',directionVersion:'Direction version',dataLease:'Data lease',leaseActive:'Active',leaseInactive:'Inactive',ackRevision:'Last acknowledged revision',
    reportHelp:'Accepted means the server stored the direction; it does not prove the client received the confirmation. Activity expiry is separate from token validity. Acknowledgments include skipped messages and do not prove a clipboard write.',
    onlineHelp:'Online means authenticated activity within 60 seconds. An accepted direction and an active data lease do not prove that a client wrote to its system clipboard. Last activity is saved at most once per minute.',
    backfillHelp:'Keep only the latest eligible message per device. This global setting applies immediately to new messages; existing records retain their expiry. Set 0 to disable and clear offline backfill. Online delivery continues.',
    diagnostics:'Server diagnostics',resetStatistics:'Clear statistics',resetStatisticsTitle:'Clear diagnostic statistics?',resetStatisticsHelp:'This clears routing counters only. Devices, pairing, clipboard content, pending deliveries, settings and audit records are retained. New activity will be counted immediately.',statisticsReset:'Diagnostic statistics cleared.',metricsHelp:'Routing counters since startup or the last reset; restart also resets them. Backfills count selected candidates, not confirmed clipboard writes.',
    metricReports:'Accepted reports',metricReportErrors:'Report errors',metricWsReports:'WS reports',metricHttpReports:'HTTP reports',metricFiltered:'Filtered deliveries',metricUnknown:'Unknown-state checks',metricRevocations:'Direction revocations',metricExpired:'Expired directions',metricBackfills:'Backfill candidates',metricConfigurationAlerts:'Configuration alerts',
    troubleshooting:'When a device is not receiving',checkProtocol:'Use a protocol-3 client and check its connection status.',checkDirection:'On Devices, check receive direction, pause, accepted report and data lease.',checkBackfill:'For reconnects, check the backfill window and expiry. New devices receive no prior history.',checkLocal:'Check client filters and sensitive-content settings. Sensitive text may stay in the inbox without changing the system clipboard.'
  });
  Object.assign(messages.zh,{
    settings:'设置',opensNewTab:'（在新标签页打开）',syncState:'同步状态',
    pushOnly:'仅发送',receiveOnly:'仅接收',bidirectional:'双向同步',paused:'已暂停',
    reportReported:'服务端已接受',reportUnknown:'未上报方向',reportRevoked:'方向已撤销',reportExpired:'方向已过期',
    syncDetails:'同步详情',reportedAt:'最后上报',expiresAt:'活动有效期至',reportSource:'最后上报方式',directionVersion:'方向版本',dataLease:'数据租约',leaseActive:'有效',leaseInactive:'无效',ackRevision:'最后确认修订号',
    reportHelp:'已接受仅表示服务端保存了方向，不保证客户端收到确认。活动有效期与令牌有效性不同。确认包括已跳过的消息，不代表系统剪贴板已写入。',
    onlineHelp:'在线表示近 60 秒内有认证活动。方向已接受、数据租约有效均不代表客户端已写入系统剪贴板。最后活动时间最多每分钟持久化一次。',
    backfillHelp:'每台设备只保留最近一条满足条件的消息。全局设置，保存后立即用于新消息，已存消息保留原到期时间。设为 0 关闭并清除离线补发，在线实时同步继续生效。',
    diagnostics:'服务端诊断',resetStatistics:'清空统计',resetStatisticsTitle:'清空诊断统计？',resetStatisticsHelp:'仅清空分发计数，保留设备、配对、剪贴板内容、待分发消息、设置和审计记录。新的活动会立即重新计数。',statisticsReset:'诊断统计已清空。',metricsHelp:'以下为服务端启动或上次清空后的分发计数，重启也会归零。补发计数表示选中的候选消息，不代表已写入客户端剪贴板。',
    metricReports:'已接受上报',metricReportErrors:'上报错误',metricWsReports:'WS 上报',metricHttpReports:'HTTP 上报',metricFiltered:'过滤下发',metricUnknown:'未知状态检查',metricRevocations:'方向撤销',metricExpired:'方向过期',metricBackfills:'补发候选',metricConfigurationAlerts:'配置告警',
    troubleshooting:'设备无法接收时',checkProtocol:'使用协议 3 客户端，并检查客户端连接状态。',checkDirection:'在设备页检查接收方向、暂停状态、方向上报与数据租约。',checkBackfill:'重连时检查补发窗口与到期时间。新设备不会接收之前的历史。',checkLocal:'检查客户端过滤器与敏感内容设置。敏感文本可能仅保存在收件区，不写入系统剪贴板。'
  });
  Object.assign(messages.en,{about:'About',aboutServer:'About this server',serverVersion:'Server version',protocolVersion:'Client protocol',checkUpdate:'Check for updates',updatePrivacy:'Only when you click: your browser contacts the public GitHub Releases API. No clipboard data, device credentials or administrator session is sent. Containers are not updated automatically.',viewRelease:'View release notes',updateChecking:'Checking GitHub releases…',updateAvailable:'A newer release is available:',updateCurrent:'You are running the latest release:',updateAhead:'This server is newer than the latest published release:',updateFailed:'Unable to check for updates. Try again later or visit the releases page.',updateRateLimit:'GitHub rate limit reached. Wait before checking again.',updateMissing:'No published release was found.',updateInvalid:'The release response could not be verified.'});
  Object.assign(messages.zh,{about:'关于',aboutServer:'关于服务端',serverVersion:'服务端版本',protocolVersion:'客户端协议',checkUpdate:'检查更新',updatePrivacy:'仅在点击后，由浏览器访问公开的 GitHub Releases API，不发送剪贴板内容、设备凭据或管理员会话。不会自动更新容器。',viewRelease:'查看版本发布说明',updateChecking:'正在检查 GitHub 正式版本…',updateAvailable:'有更新的正式版本：',updateCurrent:'当前已是最新正式版本：',updateAhead:'当前服务端版本高于最新已发布版本：',updateFailed:'无法检查更新，请稍后重试或查看版本发布页面。',updateRateLimit:'GitHub 请求已达限额，请稍后再检查。',updateMissing:'尚未找到已发布的正式版本。',updateInvalid:'无法验证版本响应。'});
  const tabs=['devices','accounts','settings','about'];
  const metrics=['reports','reportErrors','wsReports','httpReports','filtered','unknown','revocations','expired','backfills','configurationAlerts'];
  let serverInfo=null,updateCheck=null,updateResult=null,tooltipOwner=null,tooltipTimer=null;
  let metricsRevision=0;
  let syncSettings=null,settingsDirty=false,settingsEdit=0,lastNotice='',savedStatus=false,refreshedAt=null;
  let lang = 'en';
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
    renderUpdate();
    if(lastNotice)$('notice').textContent=t(lastNotice);
    if(savedStatus)$('sync-settings-status').textContent=t('backfillSaved');
    if(refreshedAt)renderUpdated();
    render();
  }
  function renderUpdated() {$('updated').textContent=`${t('updated')} ${new Intl.DateTimeFormat(document.documentElement.lang,{hour:'2-digit',minute:'2-digit',second:'2-digit'}).format(refreshedAt)}`;}
  function showError(id,message) {$(id).textContent=message;$(id).hidden=!message;}
  function notice(key) {lastNotice=key;$('notice').textContent=t(key);$('notice').hidden=false;}
  function message(status) {return t(({400:'badRequest',401:'sessionExpired',403:'forbidden',404:'notFound',409:'conflict',413:'badRequest',429:'rateLimit'})[status]||'serverError');}
  function signedOut(reason='') {
    hideTooltip();updateCheck?.abort();updateCheck=null;updateResult=null;serverInfo=null;$('server-version').textContent='';$('protocol-version').textContent='';$('update-status').textContent='';$('release-link').hidden=true;$('check-update').disabled=false;csrf='';currentUser='';devices=[];accounts=[];syncSettings=null;settingsDirty=false;settingsEdit++;lastNotice='';savedStatus=false;refreshedAt=null;target=null;labelTarget=null;$('backfill-window').value='';$('sync-metrics').replaceChildren();showError('sync-settings-error','');$('notice').textContent='';$('notice').hidden=true;$('updated').textContent='';$('signed-in').textContent='';$('sync-settings-audit').textContent='';$('sync-settings-status').textContent='';clearInterval(timer);timer=null;
    document.querySelectorAll('dialog[open]').forEach(d=>d.close());
    $('account-form').reset();$('rename-form').reset();$('revoke-target').textContent='';$('revoke-description').textContent='';$('filter').replaceChildren(new Option(t('allAccounts'),''));showError('page-error','');$('password').value='';$('workspace').hidden=true;$('logout').hidden=true;$('login-panel').hidden=false;
    const expanded=new Set([...$('device-rows').querySelectorAll('details[open]')].map(e=>e.dataset.deviceId));
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
    const session=csrf,revision=metricsRevision;
    if(!session || loadingSession===session)return;loadingSession=session;$('refresh').disabled=true;showError('page-error','');
    try {const [nextDevices,nextAccounts,nextSettings,nextInfo]=await Promise.all([api('devices'),api('accounts'),api('sync-settings'),api('about')]);if(csrf!==session)return;devices=nextDevices;accounts=nextAccounts;if(revision!==metricsRevision&&syncSettings)nextSettings.metrics=syncSettings.metrics;syncSettings=nextSettings;serverInfo=nextInfo;render();refreshedAt=new Date();renderUpdated();}
    catch(error){if(csrf===session && !(error instanceof StaleSession))showError('page-error',error.message);}finally{if(loadingSession===session){loadingSession='';$('refresh').disabled=false;}}
  }
  async function signedIn(info) {
    csrf=info.csrfToken;currentUser=info.username;lastTouch=Date.now();$('password').value='';showError('login-error','');
    $('login-panel').hidden=true;$('workspace').hidden=false;$('logout').hidden=false;$('signed-in').textContent=`${t('signedIn')} ${currentUser}`;
    await load();if(csrf!==info.csrfToken)return;clearInterval(timer);timer=setInterval(()=>{if(!document.hidden)load();},15000);
  }
  function cell(row,label,text,className) {const td=element('td',text,className);td.dataset.label=t(label);row.append(td);return td;}
  function button(text,handler,className='quiet') {const b=element('button',text,className);b.type='button';b.addEventListener('click',handler);return b;}
  function date(value) {return value?new Intl.DateTimeFormat(document.documentElement.lang,{dateStyle:'medium',timeStyle:'short'}).format(new Date(value*1000)):t('unknown');}
  function syncStatus(cell,device,expanded) {
    const sync=device.sync||{reportState:'unknown'};
    const accepted=sync.reportState==='reported';
    const direction=t(({PUSH_ONLY:'pushOnly',RECEIVE_ONLY:'receiveOnly',BIDIRECTIONAL:'bidirectional'})[sync.direction]||'unknown');
    const reportKey=({reported:'reportReported',unknown:'reportUnknown',revoked:'reportRevoked',expired:'reportExpired'})[sync.reportState]||'reportUnknown';
    cell.append(element('p',accepted?(sync.paused?`${t('paused')} · ${direction}`:direction):t(reportKey),'sync-direction'));
    if(accepted)cell.append(element('p',t(reportKey),'small muted sync-report'));
    const details=element('details',undefined,'sync-details');details.dataset.deviceId=device.deviceId;details.open=expanded;
    const summary=element('summary',t('syncDetails'));summary.setAttribute('aria-describedby','sync-report-help');
    summary.addEventListener('pointerenter',event=>{if(event.pointerType!=='touch')showTooltip(summary);});
    summary.addEventListener('pointerleave',scheduleTooltipHide);
    summary.addEventListener('focus',()=>{if(summary.matches(':focus-visible'))showTooltip(summary);});
    summary.addEventListener('blur',scheduleTooltipHide);details.append(summary);
    const list=element('dl');
    for(const [key,value] of [['reportedAt',date(sync.reportedAt)],['reportSource',sync.lastReportSource?.toUpperCase()||t('unknown')],['directionVersion',sync.version??t('unknown')],['expiresAt',date(sync.expiresAt)],['dataLease',t(sync.dataLeaseActive?'leaseActive':'leaseInactive')],['ackRevision',device.ackRevision]])list.append(element('dt',t(key)),element('dd',String(value)));
    details.append(list,element('p',t('reportHelp'),'small muted touch-help'));cell.append(details);
  }
  function hideTooltip() {clearTimeout(tooltipTimer);tooltipOwner=null;$('sync-report-help').hidden=true;}
  function showTooltip(owner) {
    clearTimeout(tooltipTimer);tooltipOwner=owner;const tip=$('sync-report-help');tip.textContent=t('reportHelp');tip.hidden=false;
    const rect=owner.getBoundingClientRect(),box=tip.getBoundingClientRect();
    tip.style.left=`${Math.max(12,Math.min(rect.left,innerWidth-box.width-12))}px`;
    tip.style.top=`${Math.max(12,rect.bottom+8+box.height<innerHeight-12?rect.bottom+8:rect.top-box.height-8)}px`;
  }
  function scheduleTooltipHide() {clearTimeout(tooltipTimer);tooltipTimer=setTimeout(()=>{if(!$('sync-report-help').matches(':hover')&&!(document.activeElement===tooltipOwner&&tooltipOwner?.matches(':focus-visible')))hideTooltip();},100);}
  function renderUpdate() {
    $('update-status').textContent=updateResult?`${t(updateResult.key)}${updateResult.version?' '+updateResult.version:''}`:'';
    $('release-link').hidden=!updateResult||updateResult.key==='updateChecking';
  }
  function version(value) {if(typeof value!=='string'||!/^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(value)||value.length>64)return null;const parts=value.replace(/^v/,'').split('.').map(Number);return parts.every(Number.isSafeInteger)?parts:null;}
  async function checkUpdate() {
    if(!csrf||updateCheck||!serverInfo)return;
    const session=csrf,controller=new AbortController();updateCheck=controller;const timeout=setTimeout(()=>controller.abort(),10000);
    $('check-update').disabled=true;updateResult={key:'updateChecking'};renderUpdate();
    try {
      const response=await fetch('https://api.github.com/repos/Infinifar/rustclipsync-server/releases/latest',{headers:{Accept:'application/vnd.github+json'},credentials:'omit',referrerPolicy:'no-referrer',redirect:'error',cache:'no-store',signal:controller.signal});
      if(!response.ok)throw new Error(response.status===403||response.status===429?'updateRateLimit':response.status===404?'updateMissing':'updateFailed');
      const reader=response.body?.getReader();if(!reader)throw new Error('updateInvalid');
      let bytes=0,body='';const decoder=new TextDecoder('utf-8',{fatal:true});
      try{while(true){const {done,value}=await reader.read();if(done)break;bytes+=value.byteLength;if(bytes>262144){await reader.cancel();throw new Error('updateInvalid');}body+=decoder.decode(value,{stream:true});}body+=decoder.decode();}finally{reader.releaseLock();}
      const release=JSON.parse(body),latest=version(release.tag_name),current=version(serverInfo.version);
      if(!latest||!current||release.draft!==false||release.prerelease!==false)throw new Error('updateInvalid');
      let comparison=0;for(let i=0;i<3;i++){if(latest[i]!==current[i]){comparison=latest[i]>current[i]?1:-1;break;}}
      if(csrf===session&&updateCheck===controller)updateResult={key:comparison>0?'updateAvailable':comparison<0?'updateAhead':'updateCurrent',version:release.tag_name};
    }catch(error){if(csrf===session&&updateCheck===controller)updateResult={key:['updateRateLimit','updateMissing','updateInvalid'].includes(error.message)?error.message:'updateFailed'};}
    finally{clearTimeout(timeout);if(updateCheck===controller){updateCheck=null;$('check-update').disabled=false;renderUpdate();}}
  }
  function render() {
    hideTooltip();
    if(serverInfo){$('server-version').textContent=serverInfo.version;$('protocol-version').textContent=serverInfo.protocolVersion;}

    if(syncSettings){$('sync-metrics').replaceChildren();for(const key of metrics){const row=element('div');row.append(element('dt',t('metric'+key[0].toUpperCase()+key.slice(1))),element('dd',String(syncSettings.metrics?.[key]??'—')));$('sync-metrics').append(row);}if(!settingsDirty)$('backfill-window').value=syncSettings.backfillWindowMinutes;showError('sync-settings-error',syncSettings.configurationValid?'':t('backfillInvalid'));const last=syncSettings.audit[0];$('sync-settings-audit').textContent=last?`${t('backfillChanged')} ${last.actor} · ${new Intl.DateTimeFormat(document.documentElement.lang,{dateStyle:'medium',timeStyle:'short'}).format(new Date(last.changedAt*1000))} · ${last.oldValue??'—'} → ${last.newValue}`:t('backfillNoAudit');}
    const selected=$('filter').value;$('filter').replaceChildren(new Option(t('allAccounts'),''));
    accounts.forEach(a=>$('filter').add(new Option(a.username,a.id)));if([...$('filter').options].some(o=>o.value===selected))$('filter').value=selected;
    const filtered=devices.filter(d=>!$('filter').value || (d.accountId||'@legacy')===$('filter').value);
    const expanded=new Set([...$('device-rows').querySelectorAll('details[open]')].map(e=>e.dataset.deviceId));
    $('device-rows').replaceChildren();
    for(const d of filtered) {
      const row=element('tr');const name=cell(row,'device');name.append(element('p',d.label||t('unnamed'),'device-name'),element('span',d.deviceId,'device-id'));
      cell(row,'account',d.accountName);syncStatus(cell(row,'syncState',undefined,'sync-state'),d,expanded.has(d.deviceId));
      const last=cell(row,'lastActivity',undefined,'last-activity');last.append(element('span',t(d.online?'online':'offline'),`status ${d.online?'online':''}`),element('p',date(d.lastSeen),'small muted activity-time'));
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
    for(const name of tabs)$(`${name}-panel`).hidden=tab!==name;$('filter').hidden=tab!=='devices';$('create-account').hidden=tab==='settings'||tab==='about';
  }
  function setTab(value) {tab=value;for(const name of tabs){const b=$(`tab-${name}`);b.setAttribute('aria-selected',String(value===name));b.tabIndex=value===name?0:-1;}render();}
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
  for(const value of tabs){$(`tab-${value}`).addEventListener('click',()=>setTab(value));$(`tab-${value}`).addEventListener('keydown',event=>{if(['ArrowLeft','ArrowRight','Home','End'].includes(event.key)){event.preventDefault();const next=event.key==='Home'?tabs[0]:event.key==='End'?tabs.at(-1):tabs[(tabs.indexOf(value)+(event.key==='ArrowRight'?1:tabs.length-1))%tabs.length];setTab(next);$(`tab-${next}`).focus();}});}
  $('backfill-window').addEventListener('input',()=>{settingsDirty=true;settingsEdit++;savedStatus=false;$('sync-settings-status').textContent='';});
  $('sync-settings-form').addEventListener('submit',event=>{event.preventDefault();submit(event.currentTarget,'sync-settings-error',async()=>{const edit=settingsEdit;const value=Number($('backfill-window').value);if(!Number.isInteger(value)||value<0||value>1440)throw new Error(t('badRequest'));await api('sync-settings','PUT',{backfillWindowMinutes:value});if(settingsEdit===edit){settingsDirty=false;savedStatus=true;$('sync-settings-status').textContent=t('backfillSaved');}await load();});});
  $('reset-statistics').addEventListener('click',()=>{if(!csrf)return;showError('reset-statistics-error','');$('reset-statistics-dialog').showModal();});
  $('reset-statistics-form').addEventListener('submit',event=>{event.preventDefault();submit(event.currentTarget,'reset-statistics-error',async()=>{
    $('reset-statistics').disabled=true;
    try {const result=await api('diagnostics/reset','POST',{confirm:true});metricsRevision++;if(syncSettings)syncSettings.metrics=result.metrics;render();$('reset-statistics-dialog').close();notice('statisticsReset');}
    finally {$('reset-statistics').disabled=false;}
  });});
  $('check-update').addEventListener('click',checkUpdate);
  $('sync-report-help').addEventListener('pointerenter',()=>clearTimeout(tooltipTimer));$('sync-report-help').addEventListener('pointerleave',scheduleTooltipHide);
  document.addEventListener('keydown',event=>{if(event.key==='Escape')hideTooltip();});window.addEventListener('resize',hideTooltip);window.addEventListener('scroll',hideTooltip,{passive:true});
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
  window.addEventListener('pagehide',()=>signedOut());
  window.addEventListener('pageshow',event=>{if(event.persisted){signedOut();fetch('/api/admin/session',{credentials:'same-origin',cache:'no-store'}).then(r=>r.ok?r.json():null).then(info=>{if(info)signedIn(info);}).catch(()=>showError('login-error',t('networkError')));}});
  localize();fetch('/api/admin/session',{credentials:'same-origin',cache:'no-store'}).then(r=>r.ok?r.json():null).then(info=>{if(info)signedIn(info);}).catch(()=>showError('login-error',t('networkError')));
})();
