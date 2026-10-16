'use strict';
/* CraftHub web UI. Plain JavaScript, no dependencies, nothing loaded from other sites.
   English strings that also exist in the launcher's catalogs (i18n/*.tsv) go through t(), so they are
   translated the same way; strings new to the server fall back to English. */

const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

/* ---------------------------------------------------------------- settings (per browser) */
const SKEY = 'crafthub.settings';
const S = Object.assign({ theme: 'system', language: 'auto', view: 'grid', filter: 'all', sort: 'suite', desc: false, hidden: [], dismissed: [] }, (() => {
  try { return JSON.parse(localStorage.getItem(SKEY) || '{}'); } catch { return {}; }
})());
const save = () => { try { localStorage.setItem(SKEY, JSON.stringify(S)); } catch { /* private mode */ } };

const mq = matchMedia('(prefers-color-scheme: dark)');
function applyTheme() { document.documentElement.dataset.eff = S.theme === 'system' ? (mq.matches ? 'dark' : 'light') : S.theme; }
mq.addEventListener('change', applyTheme);
applyTheme();

/* ---------------------------------------------------------------- translations */
let L = null, LANG = 'en';
const SUPPORTED = ['ru', 'zh-CN', 'de', 'fr', 'pt-BR', 'es', 'cs', 'pl', 'hu', 'ja'];
function langFromTag(tag) {
  const t = String(tag).toLowerCase().replace(/_/g, '-').replace(/[.@].*$/, '');
  const p = t.split('-')[0];
  if (p === 'zh') return (t === 'zh' || /^zh-(cn|sg|hans)/.test(t)) ? 'zh-CN' : null;   // Traditional Chinese is not offered
  if (p === 'pt') return 'pt-BR';
  return SUPPORTED.includes(p) ? p : (p === 'en' ? 'en' : null);
}
function autoLang() {
  const q = new URLSearchParams(location.search).get('lang');
  if (q) { const l = langFromTag(q); if (l) return l; }
  for (const tag of (navigator.languages || [navigator.language || 'en'])) { const l = langFromTag(tag); if (l) return l; }
  return 'en';
}
async function loadLang() {
  LANG = S.language === 'auto' ? autoLang() : S.language;
  L = null;
  if (LANG !== 'en') { try { L = await (await fetch('/api/i18n/' + encodeURIComponent(LANG))).json(); } catch { LANG = 'en'; } }
  document.documentElement.lang = LANG;
}
const fmt = (s, v) => v ? s.replace(/\{(\w+)\}/g, (m, k) => (k in v ? v[k] : m)) : s;
const t = (src, v) => fmt((L && L.plain[src]) || src, v);
const tctx = (ctx, src) => (L && L.ctx[ctx] && L.ctx[ctx][src]) || src;
function pluralIndex(n) {
  switch (LANG) {
    case 'ru': { const a = n % 100; if (a >= 11 && a <= 14) return 2; const b = n % 10; return b === 1 ? 0 : (b >= 2 && b <= 4 ? 1 : 2); }
    case 'pl': return n === 1 ? 0 : (n % 10 >= 2 && n % 10 <= 4 && !(n % 100 >= 12 && n % 100 <= 14) ? 1 : 2);
    case 'cs': return n === 1 ? 0 : (n >= 2 && n <= 4 ? 1 : 2);
    case 'fr': return n > 1 ? 1 : 0;
    case 'zh-CN': case 'ja': return 0;
    default: return n !== 1 ? 1 : 0;
  }
}
function tn(n, one, other) {
  const forms = L && L.plural[one + '|' + other];
  const s = forms ? (forms[pluralIndex(n)] ?? forms[forms.length - 1]) : (n === 1 ? one : other);
  return fmt(s, { n });
}

/* ---------------------------------------------------------------- formatting */
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
function fmtDate(secs) {
  if (!secs) return '';
  const d = new Date(secs * 1000);
  return t('{month} {day}, {year}', { month: tctx('month', MONTHS[d.getUTCMonth()]), day: d.getUTCDate(), year: d.getUTCFullYear() });
}
function ago(secs, now) {
  const d = Math.max(0, now - secs);
  if (d < 60) return t('just now');
  if (d < 3600) return tn(Math.floor(d / 60), '{n} min ago', '{n} min ago');
  if (d < 86400) return tn(Math.floor(d / 3600), '{n} h ago', '{n} h ago');
  if (d < 172800) return t('yesterday');
  if (d < 7 * 86400) return tn(Math.floor(d / 86400), '{n} day ago', '{n} days ago');
  return fmtDate(secs);
}
const fmtSize = n => n > 1048576 ? (n / 1048576).toFixed(n > 104857600 ? 0 : 1) + ' MB' : Math.max(1, Math.round(n / 1024)) + ' KB';

function semver(v) {
  const m = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?/.exec(v || '');
  return m ? { n: [+m[1], +m[2], +m[3]], pre: m[4] ? m[4].split('.') : null } : null;
}
function cmpVer(a, b) {
  const x = semver(a), y = semver(b);
  if (!x || !y) return (x ? 1 : 0) - (y ? 1 : 0);
  for (let i = 0; i < 3; i++) if (x.n[i] !== y.n[i]) return x.n[i] - y.n[i];
  if (!x.pre && !y.pre) return 0;
  if (!x.pre) return 1;
  if (!y.pre) return -1;
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
    const p = x.pre[i], q = y.pre[i];
    if (p === undefined) return -1;
    if (q === undefined) return 1;
    const pn = /^\d+$/.test(p), qn = /^\d+$/.test(q);
    if (pn && qn) { if (+p !== +q) return +p - +q; } else if (pn) return -1; else if (qn) return 1; else if (p !== q) return p < q ? -1 : 1;
  }
  return 0;
}

/* Safe Markdown for release notes: everything is escaped first, only http(s) links survive. */
function md(src) {
  const inline = s => esc(s)
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, '<b>$1</b>')
    .replace(/(^|[\s(])\*([^*\s][^*]*)\*/g, '$1<i>$2</i>')
    .replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, '<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>');
  const out = []; let list = null, code = false;
  const close = () => { if (list) { out.push(`</${list}>`); list = null; } };
  for (const raw of String(src || '').replace(/\r/g, '').split('\n')) {
    if (/^```/.test(raw)) { close(); out.push(code ? '</pre>' : '<pre>'); code = !code; continue; }
    if (code) { out.push(esc(raw) + '\n'); continue; }
    let m;
    if ((m = /^(#{1,6})\s+(.*)$/.exec(raw))) { close(); out.push(`<h4>${inline(m[2])}</h4>`); }
    else if ((m = /^\s*[-*+]\s+(.*)$/.exec(raw))) { if (list !== 'ul') { close(); out.push('<ul>'); list = 'ul'; } out.push(`<li>${inline(m[1])}</li>`); }
    else if ((m = /^\s*\d+[.)]\s+(.*)$/.exec(raw))) { if (list !== 'ol') { close(); out.push('<ol>'); list = 'ol'; } out.push(`<li>${inline(m[1])}</li>`); }
    else if (!raw.trim()) close();
    else { close(); out.push(`<p>${inline(raw)}</p>`); }
  }
  close(); if (code) out.push('</pre>');
  return `<div class="md">${out.join('')}</div>`;
}

/* ---------------------------------------------------------------- platform of the visitor */
const detectOS = () => {
  const u = navigator.userAgent;
  if (/Windows/i.test(u)) return 'windows';
  if (/Android|iPhone|iPad/i.test(u)) return 'other';
  if (/Mac OS X|Macintosh/i.test(u)) return 'macos';
  if (/FreeBSD/i.test(u)) return 'freebsd';
  if (/Linux|X11/i.test(u)) return 'linux';
  return 'other';
};
let PLAT = (() => { const o = detectOS(); return o === 'macos' ? 'macos' : o === 'other' ? '' : o + '-' + (/aarch64|arm64/i.test(navigator.userAgent) ? 'arm64' : 'x64'); })();
const PLAT_LABELS = { 'windows-x64': 'Windows x64', 'windows-arm64': 'Windows ARM64', 'windows-x86': 'Windows x86', macos: 'macOS', 'linux-x64': 'Linux x64', 'linux-arm64': 'Linux ARM64', 'linux-riscv64': 'Linux RISC-V', 'freebsd-x64': 'FreeBSD x64' };
const OS_LABEL = { windows: 'Windows', macos: 'macOS', linux: 'Linux', freebsd: 'FreeBSD' };
if (navigator.userAgentData && navigator.userAgentData.getHighEntropyValues) {
  navigator.userAgentData.getHighEntropyValues(['architecture', 'bitness']).then(h => {
    const os = detectOS(), arm = /arm/i.test(h.architecture || '');
    if (os === 'windows') PLAT = 'windows-' + (arm ? 'arm64' : h.bitness === '32' ? 'x86' : 'x64');
    else if (os === 'linux') PLAT = 'linux-' + (arm ? 'arm64' : 'x64');
    if (typeof render === 'function' && APPS.length) render();
  }).catch(() => {});
}

/* ---------------------------------------------------------------- state & API */
let ME = { admin: false, brand: {} }, APPS = [], META = {}, BUSY = false, MENU = null;
let drawerId = null, menuOwner = null;

function toast(msg, ms = 4500) {
  const el = $('#toast'); el.textContent = msg; el.style.display = 'block';
  clearTimeout(toast.h); toast.h = setTimeout(() => { el.style.display = 'none'; }, ms);
}
async function api(path, opts = {}) {
  const o = { method: 'GET', headers: {}, ...opts };
  if (o.method !== 'GET') o.headers['X-Requested-With'] = (ME.brand && ME.brand.requested_with) || 'crafthub';
  if (o.body && typeof o.body !== 'string') { o.body = JSON.stringify(o.body); o.headers['Content-Type'] = 'application/json'; }
  const r = await fetch(path, o);
  if (r.status === 401) { location.href = '/auth/login?next=' + encodeURIComponent(location.pathname); throw new Error('signed out'); }
  const text = await r.text();
  let data = null; try { data = text ? JSON.parse(text) : null; } catch { /* not JSON */ }
  if (!r.ok) throw new Error((data && data.error) || r.statusText || 'request failed');
  return data;
}
const byId = id => APPS.find(a => a.id === id);
const jobOn = a => a.job && ['queued', 'downloading', 'verifying', 'extracting'].includes(a.job.state) ? a.job : null;
const isHidden = a => S.hidden.includes(a.id);

/* ---------------------------------------------------------------- icons (inline, tinted by the theme) */
const ICON = {
  refresh: '<svg viewBox="0 0 24 24"><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/></svg>',
  gear: '<svg viewBox="0 0 24 24"><path d="M10.08 4.85 L10.44 2.12 L13.56 2.12 L13.92 4.85 A7.4 7.4 0 0 1 15.70 5.59 L17.88 3.91 L20.09 6.12 L18.41 8.30 A7.4 7.4 0 0 1 19.15 10.08 L21.88 10.44 L21.88 13.56 L19.15 13.92 A7.4 7.4 0 0 1 18.41 15.70 L20.09 17.88 L17.88 20.09 L15.70 18.41 A7.4 7.4 0 0 1 13.92 19.15 L13.56 21.88 L10.44 21.88 L10.08 19.15 A7.4 7.4 0 0 1 8.30 18.41 L6.12 20.09 L3.91 17.88 L5.59 15.70 A7.4 7.4 0 0 1 4.85 13.92 L2.12 13.56 L2.12 10.44 L4.85 10.08 A7.4 7.4 0 0 1 5.59 8.30 L3.91 6.12 L6.12 3.91 L8.30 5.59 A7.4 7.4 0 0 1 10.08 4.85 Z"/><circle cx="12" cy="12" r="3"/></svg>',
};

/* ---------------------------------------------------------------- header, strip, footer */
function drawChrome() {
  const b = ME.brand || {};
  document.title = b.name || 'CraftHub';
  $('#wm').textContent = b.wordmark || 'CRAFTHUB';
  $('#tag').textContent = t(b.tag || 'Server');
  $('#suite').textContent = t('Craft App Suite');
  const su = ME.suite || {};
  $('#hright').innerHTML = `
    <button class="nav ${S.view === 'grid' ? 'on' : ''}" aria-pressed="${S.view === 'grid'}" data-act="view" data-v="grid" title="${esc(t('Show apps as cards'))}"><span>${esc(t('Grid'))}</span></button>
    <button class="nav ${S.view === 'table' ? 'on' : ''}" aria-pressed="${S.view === 'table'}" data-act="view" data-v="table" title="${esc(t('Show apps as a table'))}"><span>${esc(t('Table'))}</span></button>
    ${ME.can_sign_out ? `<span class="rule"></span><a class="nav" href="/auth/logout" title="${esc(ME.name || '')}">${esc(ME.name || 'Sign out')} ⎋</a>` : ''}
    <span class="rule"></span>
    <button class="nav" data-act="check" ${BUSY ? 'aria-busy="true"' : ''} title="${esc(t('Check for updates'))}" aria-label="${esc(t('Check for updates'))}">${esc(BUSY ? t('Checking…') : t('Refresh'))}</button>
    <button class="nav" data-act="settings" title="${esc(t('Settings'))}" aria-label="${esc(t('Settings'))}">${esc(t('Settings'))}</button>`;
  const sortOpts = [['suite', 'Suite order'], ['name', 'Name'], ['status', 'Status'], ['installed', 'Installed version'], ['latest', 'Latest version'], ['released', 'Newest release']];
  if (ME.show_inspired_by) sortOpts.splice(3, 0, ['inspired', 'Inspired by']);
  $('#sortSel').innerHTML = sortOpts.map(([k, l]) => `<option value="${k}" ${S.sort === k ? 'selected' : ''}>${esc(t(l))}</option>`).join('');
  $('#sortLbl').textContent = t('Sort');
  $('#q').placeholder = t('Search apps');
  $('#q').setAttribute('aria-label', t('Search apps'));
  $('#addApp').textContent = t('Add app');
  $('#addApp').hidden = !ME.admin;
  $('#fieldSort').hidden = S.view !== 'grid';
}

function filterOf(a, f) {
  switch (f) {
    case 'installed': return !!a.active;
    case 'updates': return a.status === 'update_available';
    case 'notinstalled': return !a.active;
    default: return true;
  }
}
function visible() {
  const q = ($('#q').value || '').trim().toLowerCase();
  let list = APPS.filter(a => S.filter === 'hidden' ? isHidden(a) : !isHidden(a));
  if (S.filter !== 'hidden') list = list.filter(a => filterOf(a, S.filter));
  if (q) list = list.filter(a => [a.name, a.tagline, a.id, a.inspired_by || ''].some(x => String(x).toLowerCase().includes(q)));
  const grp = { classic: 0, new: 0, community: 1 }; // classic and new apps are one numbered suite
  const rank = a => ({ update_available: 0, available: 1, up_to_date: 2, installed: 2, unavailable: 3, failed: 3, no_release: 4, checking: 5 }[a.status] ?? 5);
  const cmp = {
    suite: (x, y) => grp[x.group] - grp[y.group] || (x.number || 999) - (y.number || 999) || (x.custom - y.custom) || x.name.localeCompare(y.name),
    name: (x, y) => x.name.localeCompare(y.name),
    inspired: (x, y) => (x.inspired_by || '~').localeCompare(y.inspired_by || '~'),
    status: (x, y) => rank(x) - rank(y),
    installed: (x, y) => cmpVer(x.active, y.active),
    latest: (x, y) => cmpVer(x.latest && x.latest.version, y.latest && y.latest.version),
    released: (x, y) => ((x.latest && x.latest.published_at) || 0) - ((y.latest && y.latest.published_at) || 0),
  }[S.sort] || (() => 0);
  list.sort((x, y) => (S.desc ? -1 : 1) * cmp(x, y) || x.name.localeCompare(y.name));
  return list;
}

function drawChips() {
  const vis = APPS.filter(a => !isHidden(a));
  const n = f => vis.filter(a => filterOf(a, f)).length;
  const chips = [['all', 'All', vis.length], ['installed', 'Installed', n('installed')], ['updates', 'Updates', n('updates')], ['notinstalled', 'Not installed', n('notinstalled')]];
  if (S.hidden.length) chips.push(['hidden', 'Hidden Apps', APPS.filter(isHidden).length]);
  if (S.filter === 'hidden' && !S.hidden.length) S.filter = 'all';
  $('#chips').innerHTML = chips.map(([k, l, c]) => `<button class="chip ${S.filter === k ? 'on' : ''}" aria-pressed="${S.filter === k}" data-act="filter" data-v="${k}">${esc(t(l))} <span class="n ${k === 'updates' && c ? 'warn' : ''}">${c}</span></button>`).join('');
  const upd = n('updates');
  const ub = $('#updAll');
  ub.hidden = !(ME.admin && upd > 0);
  ub.textContent = upd === 1 ? t('Update 1 app') : t('Update all ({count})', { count: upd });
}

function drawNotices() {
  const out = [];
  if (ME.auth_mode === 'none') {
    out.push(`<div class="notice warn"><div class="body">${esc('Authentication is off: anyone who can reach this page is an administrator.')}${ME.admin ? ` <button class="link" data-act="settings">${esc('Set up SSO in Settings')}</button>` : ''}</div></div>`);
  }
  for (const n of (META.announcements || []).filter(n => !S.dismissed.includes(n.id))) {
    out.push(`<div class="notice"><div class="body"><b>${esc(n.title)}</b> ${esc(n.body)} ${n.link ? `<a href="${esc(n.link)}" target="_blank" rel="noopener noreferrer">${esc(t('Learn more'))}</a>` : ''}</div><button class="x" data-act="dismiss" data-id="${esc(n.id)}" title="${esc(t('Dismiss'))}">×</button></div>`);
  }
  $('#notices').innerHTML = out.join('');
  const alert = (META.alerts || []).find(n => !S.dismissed.includes(n.id));
  const ad = $('#alertDlg');
  if (alert && !ad.open) {
    $('#alertBody').innerHTML = `<div class="dh"><h3>${esc(alert.title)}</h3></div><div class="db"><p>${esc(alert.body)}</p>${alert.link ? `<p><a href="${esc(alert.link)}" target="_blank" rel="noopener noreferrer">${esc(t('Open link'))}</a></p>` : ''}<div class="row"><button class="btn primary" data-act="ack" data-id="${esc(alert.id)}">${esc(t('OK'))}</button></div></div>`;
    ad.showModal();
  }
}

function drawFooter() {
  const su = ME.suite || {};
  let status = t('Not checked yet');
  if (META.last_check) {
    const names = (META.sources || []).map(s => s === 'feed' ? 'ArtCraft' : 'GitHub');
    const src = names.length === 2 ? t('{a} and {b}', { a: names[0], b: names[1] }) : names[0];
    status = t('Checked {ago}', { ago: ago(META.last_check, META.now) });
    if (src) status = t('Checked {ago} · using {sources}', { ago: ago(META.last_check, META.now), sources: src });
  }
  if (META.rate_limited_until) status = t('GitHub limit hit; back in {min} min', { min: Math.max(1, Math.ceil((META.rate_limited_until - META.now) / 60)) });
  $('#foot').innerHTML = `
    <span class="hud grow" title="${esc(status)}">${esc(status)}</span>
    <a class="hud opt" href="${esc((ME.brand && ME.brand.repo) || 'https://github.com/MegaTheLEGEND/CraftHub')}" target="_blank" rel="noopener">${esc(t('GitHub'))} ↗</a>`;
}

/* ---------------------------------------------------------------- app cards & rows */
const luminance = hex => { const n = parseInt(hex.slice(1), 16); const f = c => { c /= 255; return c <= .03928 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4; }; return .2126 * f(n >> 16) + .7152 * f((n >> 8) & 255) + .0722 * f(n & 255); };
const onColor = hex => luminance(hex) > .3 ? '#0b0b0c' : '#ffffff';
const verOf = v => v ? String(v).replace(/^v/, '') : '';

function nameHTML(a) {
  const m = /^(.*?)(Craft)$/.exec(a.name);
  return m && m[1] ? `${esc(m[1])}<b>${esc(m[2])}</b>` : esc(a.name);
}
function appStyle(a) {
  const s = [];
  if (a.color) { s.push(`--app:${a.color}`, `--on-app:${onColor(a.color)}`); }
  if (a.ink_light) s.push(`--ink-l:${a.ink_light}`);
  if (a.ink_dark) s.push(`--ink-d:${a.ink_dark}`);
  return s.join(';');
}
function badgeOf(a) {
  const j = jobOn(a);
  if (j) return [{ queued: 'Waiting for other installs…', downloading: 'Downloading…', verifying: 'Verifying…', extracting: 'Installing…' }[j.state], 'accent'];
  if (a.job && a.job.state === 'error') return ['Install failed', 'bad'];
  const m = {
    update_available: ['Update available', 'warn'], available: ['Available to install', 'accent'], up_to_date: ['Installed · up to date', 'good'],
    installed: ['Installed', 'good'], unavailable: ['Not available here', ''], no_release: ['Coming soon', ''], checking: ['Checking…', ''], failed: ['Offline', 'bad'],
  };
  return m[a.status] || ['', ''];
}
function versionLine(a) {
  const date = a.latest && a.latest.published_at ? ' · ' + fmtDate(a.latest.published_at) : '';
  const act = verOf(a.active), lat = a.latest ? verOf(a.latest.version) : '';
  switch (a.status) {
    case 'update_available': return t('{version} installed → {latest} released', { version: act, latest: lat }) + date;
    case 'up_to_date': return t('Version {version}', { version: act }) + (act === lat ? date : '');
    case 'installed': return t('Version {version} installed', { version: act });
    case 'available': return t('Version {latest}', { latest: lat }) + date;
    case 'no_release': return t('No release published yet');
    default: return lat ? t('Version {latest}', { latest: lat }) + date : '';
  }
}
function primary(a) {
  const adm = ME.admin, openHref = `/apps/${encodeURIComponent(a.id)}/`;
  const open = { label: t('Open'), cls: '', href: openHref };
  if (a.job && a.job.state === 'error' && adm && a.latest) return { label: t('Retry install'), cls: 'primary', act: 'install', tag: a.job.tag || a.latest.tag };
  switch (a.status) {
    case 'update_available': return adm ? { label: t('Update to {v}', { v: verOf(a.latest.version) }), cls: 'primary', act: 'install', tag: a.latest.tag } : open;
    case 'available': return adm ? { label: t('Install'), cls: 'primary', act: 'install', tag: a.latest.tag } : { label: t('Install'), cls: '', disabled: true, title: 'An administrator installs apps on this server' };
    case 'up_to_date': case 'installed': return a.active ? open : { label: t('Not available'), disabled: true };
    case 'checking': return { label: t('Checking…'), disabled: true };
    case 'no_release': return { label: t('Coming soon'), disabled: true };
    case 'failed': return { label: t('Offline'), disabled: true };
    default: return { label: t('Not available'), disabled: true };
  }
}
function progressHTML(a, j) {
  const pct = j.total ? Math.min(100, Math.round(100 * j.done / j.total)) : 0;
  const txt = j.state === 'queued' ? t('Waiting for other installs…')
    : j.state === 'verifying' ? t('Verifying checksum…')
    : j.state === 'extracting' ? t('Installing…')
    : j.total ? t('Downloading {done} of {total}', { done: fmtSize(j.done), total: fmtSize(j.total) }) + ` · ${pct}%` : t('Downloading…');
  return `<div class="prog"><div class="txt">${esc(txt)}</div><div class="bar4 ${j.total && j.state === 'downloading' ? '' : 'indet'}" role="progressbar" aria-label="${esc(txt)}" aria-valuemin="0" aria-valuemax="100" ${j.total && j.state === 'downloading' ? `aria-valuenow="${pct}"` : ''}><i style="width:${pct}%"></i></div></div>` +
    (ME.admin ? `<button class="btn sq" data-act="cancel" data-id="${esc(a.id)}" title="${esc(t('Cancel download'))}">✕</button>` : '');
}
function actionsHTML(a, compact) {
  const j = jobOn(a);
  if (j) return progressHTML(a, j);
  const p = primary(a);
  const cls = `btn ${compact ? 'main ' : 'wide '}${p.cls || ''} ${p.disabled ? 'disabled' : ''}`;
  const main = p.href ? `<a class="${cls}" href="${esc(p.href)}" target="_blank" rel="noopener">${esc(p.label)}</a>`
    : `<button class="${cls}" ${p.disabled ? 'disabled' : ''} ${p.title ? `title="${esc(p.title)}"` : ''} ${p.act ? `data-act="${p.act}" data-id="${esc(a.id)}" data-tag="${esc(p.tag || '')}"` : ''}>${esc(p.label)}</button>`;
  return main + `<button class="btn sq" data-act="menu" data-id="${esc(a.id)}" aria-haspopup="true" aria-expanded="${MENU === a.id}" aria-label="More actions" title="More actions">…</button>`;
}
function menuHTML(a) {
  const it = [];
  const dl = a.downloads && a.downloads[PLAT];
  if (a.active) it.push(`<a href="/apps/${encodeURIComponent(a.id)}/" target="_blank" rel="noopener">${esc(t('Open'))}</a>`);
  if (a.has_details) it.push(`<button data-act="details" data-id="${esc(a.id)}">${esc(t('App details'))}</button>`);
  if (a.latest) it.push(`<button data-act="notes" data-id="${esc(a.id)}">${esc(t('Release notes'))}</button>`);
  if (dl) it.push(`<a href="${esc(dl.url)}" rel="noopener">${esc(t('Download for {os}', { os: PLAT_LABELS[PLAT] }))}</a>`);
  if (a.native.length) it.push(`<button data-act="downloads" data-id="${esc(a.id)}">${esc(t('Download'))}…</button>`);
  it.push(`<a href="https://github.com/${esc(a.repo)}/releases" target="_blank" rel="noopener">${esc(t('All releases'))} ↗</a>`);
  if (ME.admin) {
    it.push('<div class="sep"></div>');
    it.push(`<button data-act="versions" data-id="${esc(a.id)}">Versions…</button>`);
    it.push(`<button data-act="refreshApp" data-id="${esc(a.id)}">${esc(t('Check for updates'))}</button>`);
    if (a.active && !jobOn(a)) it.push(`<button class="danger" data-act="uninstall" data-id="${esc(a.id)}">${esc(t('Uninstall…'))}</button>`);
  }
  it.push('<div class="sep"></div>');
  it.push(`<button data-act="${isHidden(a) ? 'unhide' : 'hide'}" data-id="${esc(a.id)}">${esc(isHidden(a) ? t('Unhide App') : t('Hide App'))}</button>`);
  return `<div class="menu" data-menu="${esc(a.id)}">${it.join('')}</div>`;
}
function iconHTML(a) {
  return `<div class="icon"><span>${esc(a.name[0])}</span><img alt="" src="${esc(a.icon)}" onerror="this.remove()"></div>`;
}
function badgesHTML(a) {
  const [txt, tone] = badgeOf(a);
  let h = txt ? `<span class="badge ${tone}">${esc(t(txt))}</span>` : '';
  if (a.pinned) h += `<span class="badge" title="An administrator rolled back to an older version">Rolled back</span>`;
  if (a.custom) h += `<span class="badge" title="Added from a GitHub repository">Custom</span>`;
  return h;
}
function errHTML(a) {
  if (a.job && a.job.state === 'error') return `<div class="err">${esc(a.job.message)}</div>`;
  if (a.job && a.job.state === 'cancelled') return `<div class="small faint">${esc(t('Install cancelled'))}</div>`;
  if (a.last_error && !a.latest && !a.active) return `<div class="err">${esc(a.last_error)}</div>`;
  return '';
}

function cardHTML(a) {
  const links = [];
  if (a.website) links.push(`<a href="${esc(a.website)}" target="_blank" rel="noopener">${esc(t('Website'))} ↗</a>`);
  if (/^[\w.-]+\/[\w.-]+$/.test(a.repo)) links.push(`<a href="https://github.com/${esc(a.repo)}" target="_blank" rel="noopener" title="${esc(t('GitHub repository'))}">${esc(t('GitHub'))} ↗</a>`);
  const num = a.number ? String(a.number).padStart(2, '0') : '··';
  const tagline = a.group !== 'community' ? tctx(a.id, a.tagline) : a.tagline;
  return `<article class="card" data-card="${esc(a.id)}" style="${appStyle(a)}">
    <div class="top"><span class="num">${num}</span><span class="links">${links.join('')}</span></div>
    <div class="inner">
      <div class="ident">${iconHTML(a)}<div class="t"><h2 class="name">${nameHTML(a)}</h2><div class="tagline">${esc(tagline)}</div>${a.inspired_by ? `<div class="like">${esc(t('Like {name}', { name: a.inspired_by }))}®</div>` : ''}</div></div>
      <div class="badges">${badgesHTML(a)}</div>
      <div class="verline">${esc(versionLine(a))}</div>
      ${errHTML(a)}
      <div class="actions">${actionsHTML(a)}</div>
    </div>${MENU === a.id ? menuHTML(a) : ''}</article>`;
}

const sectionOf = a => a.group === 'community' ? 'community' : 'suite';
function groupTitle(g, i) { return `${String(i + 1).padStart(2, '0')} / ${{ suite: t('Craft App Suite'), community: 'Community apps' }[g].toUpperCase()}`; }

function tableHTML(list) {
  const cols = [['name', 'App', 'column'], ...(ME.show_inspired_by ? [['inspired', 'Inspired by']] : []), ['status', 'Status'], ['installed', 'Installed'], ['latest', 'Latest'], ['released', 'Released']];
  const th = cols.map(([k, l, c]) => `<th class="${S.sort === k ? 'on' : ''}" tabindex="0" aria-sort="${S.sort === k ? (S.desc ? 'descending' : 'ascending') : 'none'}" data-act="sort" data-v="${k}" title="${esc(t(S.sort === k ? 'Click to reverse the order' : 'Click to sort by this column'))}">${esc(c ? tctx('column', l) : t(l))}${S.sort === k ? (S.desc ? ' ↓' : ' ↑') : ''}</th>`).join('');
  const rows = list.map(a => {
    const [txt, tone] = badgeOf(a);
    const tagline = a.group !== 'community' ? tctx(a.id, a.tagline) : a.tagline;
    return `<tr class="${jobOn(a) ? 'job' : ''}" style="${appStyle(a)}" data-card="${esc(a.id)}">
      <td><div class="appcell">${iconHTML(a)}<div><div class="name">${nameHTML(a)}</div><div class="tagline">${esc(tagline)}</div></div></div></td>
      ${ME.show_inspired_by ? `<td class="muted">${a.inspired_by ? esc(a.inspired_by) + '®' : '<span class="faint">—</span>'}</td>` : ''}
      <td><span class="badge ${tone}">${esc(t(txt))}</span>${a.pinned ? ' <span class="badge">Rolled back</span>' : ''}</td>
      <td class="num2">${a.active ? esc(verOf(a.active)) : '<span class="faint">—</span>'}</td>
      <td class="num2" style="${a.status === 'update_available' ? 'color:var(--warn)' : ''}">${a.latest ? esc(verOf(a.latest.version)) : '<span class="faint">—</span>'}</td>
      <td class="muted">${a.latest && a.latest.published_at ? esc(fmtDate(a.latest.published_at)) : '<span class="faint">—</span>'}</td>
      <td><div class="rowact">${actionsHTML(a, true)}${MENU === a.id ? menuHTML(a) : ''}</div></td></tr>`;
  }).join('');
  return `<div class="tablewrap"><table class="apps"><thead><tr>${th}<th></th></tr></thead><tbody>${rows}</tbody></table></div>`;
}

function focusSig(el) {
  if (!el || !el.dataset || !el.dataset.act || !el.closest('#hright,#chips,#content')) return null;
  return ['act', 'id', 'v', 'i'].filter(k => el.dataset[k] !== undefined).map(k => `[data-${k}="${CSS.escape(el.dataset[k])}"]`).join('');
}
function render() {
  const sig = document.activeElement !== document.body ? focusSig(document.activeElement) : null;
  renderAll();
  if (sig && document.activeElement === document.body) {
    const next = $(`#hright ${sig}`) || $(`#chips ${sig}`) || $(`#content ${sig}`);
    if (next) next.focus();
  }
}
function focusMenu() { const m = $('.menu'); const f = m && $('a, button', m); if (f) f.focus(); }
function renderAll() {
  drawChrome(); drawChips(); drawNotices(); drawFooter();
  const list = visible(), box = $('#content');
  if (!APPS.length) { box.innerHTML = `<div class="empty">${esc(t('Loading ArtCraft Launcher…').replace('ArtCraft Launcher', ME.brand.name || 'CraftHub'))}</div>`; return; }
  if (!list.length) { box.innerHTML = `<div class="empty">${esc(t('No apps match'))}</div>`; return; }
  if (S.view === 'table') { box.innerHTML = tableHTML(list); return; }
  if (S.sort === 'suite' && !S.desc) {
    const groups = [...new Set(list.map(sectionOf))];
    box.innerHTML = groups.map((g, i) => {
      const items = list.filter(a => sectionOf(a) === g);
      return `<div class="group"><span class="hud">${esc(groupTitle(g, i))}</span><span class="hud">${esc(tn(items.length, '{n} app', '{n} apps').toUpperCase())}</span></div><div class="grid">${items.map(cardHTML).join('')}</div>`;
    }).join('');
  } else box.innerHTML = `<div class="grid">${list.map(cardHTML).join('')}</div>`;
}

/* ---------------------------------------------------------------- polling */
async function refresh() {
  try {
    const r = await api('/api/apps');
    APPS = r.apps; META = r.meta;
    render();
    if (drawerId) drawDrawerActions();
    if (versionsOpen) versionsOpen();
  } catch (e) { if (e.message !== 'signed out') toast(e.message); }
}
async function pollLoop() {
  const busy = APPS.some(jobOn);
  await new Promise(r => setTimeout(r, busy ? 1200 : 15000));
  if (!document.hidden && (busy || !$('#dlg').open)) await refresh();
  pollLoop();
}

/* ---------------------------------------------------------------- dialogs */
let versionsOpen = null;
function openDlg(title, html) {
  $('#dlgTitle').textContent = title; $('#dlgBody').innerHTML = html;
  if (!$('#dlg').open) $('#dlg').showModal();
}
function closeDlg() { versionsOpen = null; window._settingsDraw = null; if ($('#dlg').open) $('#dlg').close(); }

async function showNotes(id) {
  const a = byId(id); if (!a) return;
  openDlg(t('{name} {version}', { name: a.name, version: a.latest ? verOf(a.latest.version) : '' }), '<p class="muted">…</p>');
  try {
    const rels = await api(`/api/apps/${encodeURIComponent(id)}/releases`);
    const r = rels.find(x => a.latest && x.tag === a.latest.tag) || rels[0];
    $('#dlgBody').innerHTML = (r && r.notes ? md(r.notes) : `<p class="muted">${esc(t('No notes were published with this release.'))}</p>`) +
      (r ? `<div class="row small muted">${r.published_at ? esc(t('Released {date}', { date: fmtDate(r.published_at) })) + ' · ' : ''}<a href="${esc(r.url)}" target="_blank" rel="noopener">${esc(t('Open on GitHub'))} ↗</a></div>` : '');
  } catch (e) { $('#dlgBody').innerHTML = `<div class="err">${esc(e.message)}</div>`; }
}

function showDownloads(id) {
  const a = byId(id); if (!a) return;
  const groups = {};
  a.native.forEach(n => (groups[n.os] ||= []).push(n));
  const mine = PLAT.split('-')[0];
  const order = Object.keys(groups).sort((x, y) => (y === mine) - (x === mine) || x.localeCompare(y));
  openDlg(`${a.name} · ${t('Download')}`, order.map(os => `<h4>${esc(OS_LABEL[os] || os)}${os === mine ? ' · ' + esc(PLAT_LABELS[PLAT] || '') : ''}</h4><table>` +
    groups[os].map(n => `<tr><td><a href="${esc(n.url)}" rel="noopener">${esc(n.name)}</a></td><td class="small muted">${esc(n.arch)}</td><td class="small muted">${esc(t(n.label))}</td><td class="r small muted">${n.size ? fmtSize(n.size) : ''}</td></tr>`).join('') + '</table>').join('') +
    `<p class="small muted">Downloads come straight from GitHub releases.</p>`);
}

async function showVersions(id) {
  const draw = async () => {
    const a = byId(id); if (!a) return;
    let rels = [];
    try { rels = await api(`/api/apps/${encodeURIComponent(id)}/releases`); } catch (e) { $('#dlgBody').innerHTML = `<div class="err">${esc(e.message)}</div>`; return; }
    const inst = Object.fromEntries(a.installed.map(i => [i.version, i]));
    const rows = rels.map(r => {
      const i = inst[r.version];
      const status = a.active === r.version ? '<span class="badge good">active</span>' : i ? '<span class="badge">installed</span>' : r.web ? '' : '<span class="small muted">no web build</span>';
      const unv = i && !i.verified ? ' <span class="badge warn" title="No published checksum was available">unverified</span>' : '';
      let btns = '';
      if (a.active === r.version) btns = `<a class="btn sm" href="/apps/${encodeURIComponent(id)}/" target="_blank">${esc(t('Open'))}</a>`;
      else if (i) btns = `<button class="btn sm" data-v-act="activate" data-v="${esc(r.version)}">Activate</button> <a class="btn sm" target="_blank" href="/versions/${encodeURIComponent(id)}/${encodeURIComponent(r.version)}/">Preview</a> <button class="btn sm danger" data-v-act="remove" data-v="${esc(r.version)}">Remove</button>`;
      else if (r.web) btns = `<button class="btn sm" data-v-act="install" data-tag="${esc(r.tag)}">${esc(t('Install'))}</button>`;
      return `<tr><td><a href="${esc(r.url)}" rel="noopener" target="_blank">${esc(r.tag)}</a> ${r.prerelease ? '<span class="badge">pre</span>' : ''}</td><td class="small muted">${r.published_at ? esc(fmtDate(r.published_at)) : ''}</td><td>${status}${unv}</td><td class="r">${btns}</td></tr>`;
    }).join('');
    $('#dlgBody').innerHTML = `
      <div class="row">
        <label class="sw"><input type="checkbox" id="optAuto" ${a.auto_update ? 'checked' : ''}> ${esc(t('Automatically download and install updates'))}</label>
        <label class="sw"><input type="checkbox" id="optPre" ${a.include_prerelease ? 'checked' : ''}> ${esc(t('Include pre-releases (betas and release candidates)'))}</label>
        <button class="btn sm" id="optRefresh">${esc(t('Check for updates'))}</button>
        <span class="small muted">${esc(a.last_check ? t('Checked {ago}', { ago: ago(a.last_check, META.now) }) : t('Not checked yet'))}</span>
      </div>
      <table><tr><th>Release</th><th>${esc(t('Released'))}</th><th>${esc(t('Status'))}</th><th></th></tr>${rows || '<tr><td colspan="4" class="small muted">No releases found.</td></tr>'}</table>
      ${a.custom ? `<p style="margin-top:18px"><button class="btn danger" id="rmApp">Remove this app</button> <span class="small muted">Takes it out of the catalog and deletes its installed versions.</span></p>` : ''}`;
    $('#optAuto').onchange = e => setting(id, { auto_update: e.target.checked });
    $('#optPre').onchange = e => setting(id, { include_prerelease: e.target.checked });
    $('#optRefresh').onclick = async () => { try { await api(`/api/apps/${encodeURIComponent(id)}/refresh`, { method: 'POST' }); } catch (e) { toast(e.message); } refresh(); };
    const rm = $('#rmApp');
    if (rm) rm.onclick = async () => {
      if (!confirm(`Remove ${a.name} and all of its installed versions?`)) return;
      try { await api(`/api/catalog/${encodeURIComponent(id)}`, { method: 'DELETE' }); toast('Removed ' + a.name); closeDlg(); } catch (e) { toast(e.message); }
      refresh();
    };
    $$('button[data-v-act]', $('#dlgBody')).forEach(b => b.onclick = async () => {
      const v = b.dataset.v, act = b.dataset.vAct;
      try {
        if (act === 'install') await doInstall(id, b.dataset.tag);
        if (act === 'activate') { await api(`/api/apps/${encodeURIComponent(id)}/activate`, { method: 'POST', body: { version: v } }); toast('Activated v' + v); }
        if (act === 'remove') { if (!confirm(`Remove v${v}?`)) return; await api(`/api/apps/${encodeURIComponent(id)}/versions/${encodeURIComponent(v)}`, { method: 'DELETE' }); }
      } catch (e) { toast(e.message); }
      refresh();
    });
  };
  const a = byId(id);
  versionsOpen = draw;
  openDlg(`${a.name} · Versions`, '<p class="muted">…</p>');
  await draw();
}
async function setting(id, body) { try { await api(`/api/apps/${encodeURIComponent(id)}/settings`, { method: 'POST', body }); } catch (e) { toast(e.message); } refresh(); }

function showAddApp() {
  openDlg('Add app from GitHub', `
    <p class="small muted">Add any GitHub repository that publishes releases in the Craft layout: a zip asset such as <code>name-web-1.2.3.zip</code> with an <code>index.html</code> at its root, and optionally a <code>SHA256SUMS.txt</code>. It is listed right away but nothing is downloaded until you press Install.</p>
    <p class="small muted">Hosted apps run on the same origin as this server with your session, so only add repositories you trust.</p>
    <div class="fld"><label>Repository (owner/repo or a GitHub URL)</label><input id="addRepo" placeholder="tangency-labs/solidcraft" autocomplete="off" spellcheck="false"></div>
    <details><summary class="small muted">Options</summary>
      <div class="fld"><label>Display name (default: the repository name)</label><input id="addName"></div>
      <div class="fld"><label>Description (default: the GitHub description)</label><input id="addDesc"></div>
      <div class="fld"><label>Web build asset name contains</label><input id="addMarker" value="-web-"></div>
    </details>
    <div class="row"><button class="btn primary" id="addGo">Add</button></div>
    <div class="err" id="addErr" hidden></div>`);
  const go = async () => {
    const repo = $('#addRepo').value.trim(); if (!repo) return;
    const btn = $('#addGo'); btn.disabled = true; $('#addErr').hidden = true;
    try {
      const a = await api('/api/catalog', { method: 'POST', body: { repo, name: $('#addName').value, description: $('#addDesc').value, web_asset_contains: $('#addMarker').value } });
      toast(`Added ${a.name}. Press Install to download its web build.`, 7000); closeDlg(); refresh();
    } catch (e) { const el = $('#addErr'); el.textContent = e.message; el.hidden = false; btn.disabled = false; }
  };
  $('#addGo').onclick = go; $('#addRepo').onkeydown = e => { if (e.key === 'Enter') go(); };
  $('#addRepo').focus();
}

/* ---------------------------------------------------------------- settings dialog */
async function showSettings() {
  const langs = await (await fetch('/api/i18n')).json();
  const draw = async () => {
    let h = `<h4>${esc(t('Appearance'))}</h4>
      <div class="row">
        <label class="field">${esc(t('Appearance'))} <select id="selTheme">${[['system', 'Match system'], ['dark', 'Dark'], ['light', 'Light']].map(([k, l]) => `<option value="${k}" ${S.theme === k ? 'selected' : ''}>${esc(t(l))}</option>`).join('')}</select></label>
        <label class="field">${esc(t('Language'))} <select id="selLang"><option value="auto" ${S.language === 'auto' ? 'selected' : ''}>${esc(t('Auto'))}</option>${langs.map(l => `<option value="${l.code}" ${S.language === l.code ? 'selected' : ''}>${esc(l.name)}</option>`).join('')}</select></label>
      </div>`;
    if (S.hidden.length) {
      h += `<h4>${esc(t('Hidden Apps'))}</h4><div class="small muted">${esc(t('Apps you hid'))}</div><table>${S.hidden.map(id => { const a = byId(id); return `<tr><td>${esc(a ? a.name : id)}</td><td class="r"><button class="btn sm" data-act="unhide" data-id="${esc(id)}" data-keep="1">${esc(t('Unhide App'))}</button></td></tr>`; }).join('')}</table>`;
    }
    if (ME.admin) {
      let s = null; try { s = await api('/api/settings'); } catch { /* ignore */ }
      if (s) {
        const src = { ui: 'set here', env: 'set by the GITHUB_TOKEN environment variable', none: 'not set' }[s.github_token_source];
        h += `<h4>GitHub token</h4>
          <p class="small muted">Optional. Raises GitHub's rate limit (60 requests an hour without a token) and lets you track private repositories. A fine-grained, read-only token is enough. Status: <b>${esc(src)}</b>.</p>
          <div class="row"><input id="tokIn" type="password" autocomplete="off" style="flex:1;min-width:14rem" placeholder="${s.github_token_set ? '•••••••• (enter a new one to replace)' : 'ghp_… or github_pat_…'}">
          <button class="btn primary" id="tokSave">Save</button>${s.github_token_source === 'ui' ? '<button class="btn danger" id="tokClear">Remove</button>' : ''}</div>
          <p class="small muted">Stored in the data volume (file mode 600) and never shown again.</p>`;
      }
      h += `<h4>Updates</h4><p class="small muted">${ME.update_interval_minutes ? `Checks for new releases every ${ME.update_interval_minutes} min and updates installed apps automatically (first installs are manual)` : 'Automatic checks are off'}${ME.feed_enabled ? ', asking ArtCraft’s release feed first and GitHub when it has nothing' : ', asking GitHub'}. Set <code>UPDATE_INTERVAL_MINUTES</code>, <code>AUTO_INSTALL</code> and <code>KEEP_VERSIONS</code> in the container environment.</p>`;
      h += `<div id="authBox"></div>`;
    }
    h += `<h4>About</h4><p class="small muted">${esc(ME.brand.name)} ${esc(ME.version)}${ME.build_time ? ' · built ' + esc(ME.build_time.slice(0, 10)) : ''} · <a href="${esc(ME.brand.repo)}" target="_blank" rel="noopener">GitHub repository</a><br>
      A self-hosted launcher for the Craft App Suite, styled after the ArtCraft Launcher and built on its open-source core (MIT OR Apache-2.0). Independent project, not affiliated with the ArtCraft team.</p>`;
    $('#dlgBody').innerHTML = h;
    $('#selTheme').onchange = e => { S.theme = e.target.value; save(); applyTheme(); };
    $('#selLang').onchange = async e => { S.language = e.target.value; save(); await loadLang(); render(); closeDlg(); showSettings(); };
    if ($('#tokSave')) {
      const sv = async v => { try { await api('/api/settings', { method: 'POST', body: { github_token: v } }); toast(v ? 'Token saved' : 'Token removed'); } catch (e) { toast(e.message); } draw(); };
      $('#tokSave').onclick = () => { const v = $('#tokIn').value.trim(); if (v) sv(v); };
      if ($('#tokClear')) $('#tokClear').onclick = () => sv('');
    }
    if (ME.admin) await drawAuth();
  };
  window._settingsDraw = draw;
  openDlg(t('Settings'), '<p class="muted">…</p>');
  await draw();
}

async function drawAuth() {
  let a; try { a = await api('/api/auth'); } catch { return; }
  const f = a.form, tt = a.tested;
  const modeTxt = a.mode === 'oidc' ? 'ON (OpenID Connect)' : a.mode === 'headers' ? 'ON (proxy headers, set by environment)' : 'OFF';
  const src = { ui: 'set in this UI', env: 'from environment variables', recovery: 'recovery mode (AUTH_RECOVERY)' }[a.source];
  const v = x => esc(x ?? '');
  $('#authBox').innerHTML = `
    <h4>Authentication (SSO)</h4>
    <p class="small muted">Status: <b>${modeTxt}</b> · ${src}. Works with Authentik (OAuth2/OpenID provider) and any OIDC provider. Nothing is switched on until a test sign-in with these settings succeeds as an administrator.</p>
    ${a.mode === 'oidc' ? `<p class="small muted"><b>Live now:</b> admin groups = ${v((a.active.admin_groups || []).join(', ') || 'none (every user is an admin)')}; allowed groups = ${v((a.active.allowed_groups || []).join(', ') || 'any signed-in user')}.${a.has_draft ? ' <b>You have saved changes that are not live yet: run Test sign-in, then Enable SSO to apply them.</b>' : ''}</p>` : ''}
    <p class="small muted">Register this redirect URI at the provider: <code>${v(a.redirect_uri)}</code></p>
    <div class="fld"><label>Public URL of this site</label><input id="aPub" value="${v(f.public_url || location.origin)}"></div>
    <div class="fld"><label>Issuer (Authentik: Applications → Providers → your provider → OpenID Configuration Issuer, e.g. https://auth.example.com/application/o/crafthub/)</label><input id="aIss" value="${v(f.oidc_issuer)}" placeholder="https://auth.example.com/application/o/crafthub/"></div>
    <div class="fld"><label>Client ID</label><input id="aCid" value="${v(f.oidc_client_id)}"></div>
    <div class="fld"><label>Client secret</label><input id="aSec" type="password" autocomplete="off" placeholder="${f.secret_set ? '•••••••• (leave blank to keep)' : ''}"></div>
    <div class="fld"><label>Admin groups (comma separated; empty = every user is an admin)</label><input id="aAdm" value="${v((f.admin_groups || []).join(', '))}" placeholder="crafthub-admins"></div>
    <div class="fld"><label>Allowed groups (comma separated; empty = any authenticated user)</label><input id="aAll" value="${v((f.allowed_groups || []).join(', '))}" placeholder="crafthub-users"></div>
    <div class="fld"><label>Scopes · Groups claim</label><div class="row" style="margin:0"><input id="aScp" value="${v(f.oidc_scopes || 'openid profile email')}" style="flex:1"><input id="aGrp" value="${v(f.groups_claim || 'groups')}" style="flex:1"></div></div>
    <div class="row">
      <button class="btn" id="aSave">1. Save</button>
      <button class="btn" id="aTest" ${a.has_draft ? '' : 'disabled'}>2. Test sign-in</button>
      <button class="btn primary" id="aEnable" ${tt && tt.ok ? '' : 'disabled'}>3. Enable SSO</button>
      ${a.mode !== 'none' ? '<button class="btn danger" id="aOff">Turn authentication off</button>' : ''}
      <button class="btn" id="aRefresh">Refresh status</button>
    </div>
    <p class="small muted">${tt ? (tt.ok ? `Test passed: ${v(tt.name)} (admin).` : tt.stale ? 'Last test is outdated; test again.' : `Test signed in as ${v(tt.name)} but not as an admin (groups: ${v((tt.groups || []).join(', ') || 'none')}).`) : 'No test sign-in yet.'}</p>`;
  const lst = x => x.split(',').map(s => s.trim()).filter(Boolean);
  const call = async (path, body, msg) => { try { await api(path, { method: 'POST', body }); if (msg) toast(msg); } catch (e) { toast(e.message, 8000); } if (window._settingsDraw) window._settingsDraw(); };
  $('#aSave').onclick = () => call('/api/auth/draft', { public_url: $('#aPub').value, oidc_issuer: $('#aIss').value, oidc_client_id: $('#aCid').value, oidc_client_secret: $('#aSec').value, oidc_scopes: $('#aScp').value, groups_claim: $('#aGrp').value, admin_groups: lst($('#aAdm').value), allowed_groups: lst($('#aAll').value) }, 'Saved. Now test the sign-in.');
  $('#aTest').onclick = () => { window.open('/auth/test', '_blank'); toast('Finish signing in in the new tab, then press Refresh status.', 8000); };
  $('#aRefresh').onclick = () => window._settingsDraw();
  $('#aEnable').onclick = async () => { try { await api('/api/auth/enable', { method: 'POST' }); toast('SSO enabled. Signing you in…'); setTimeout(() => location.reload(), 800); } catch (e) { toast(e.message, 8000); window._settingsDraw(); } };
  const off = $('#aOff'); if (off) off.onclick = () => { if (confirm('Turn authentication off? Anyone who can reach this site will be an administrator.')) call('/api/auth/disable', {}, 'Authentication is off'); };
}

/* ---------------------------------------------------------------- app details drawer */
let shotIndex = 0, DET = null, drawerOpener = null, lbOpener = null;
function drawDrawerActions() { /* header stays; actions live in the body and are rebuilt on open */ }
async function openDetails(id) {
  const a = byId(id); if (!a) return;
  drawerId = id; shotIndex = 0;
  const dr = $('#drawer');
  if (!dr.classList.contains('on')) drawerOpener = document.activeElement;
  dr.innerHTML = `<div class="dh"><h3>${esc(t('App details'))}</h3><button class="x" data-act="closeDrawer" title="${esc(t('Close (Esc)'))}">✕</button></div><div class="db"><p class="muted">${esc(t('Loading pictures…'))}</p></div>`;
  $('#scrim').classList.add('on'); dr.classList.add('on');
  const dx = $('.x', dr); if (dx) dx.focus();
  try { DET = await api(`/api/apps/${encodeURIComponent(id)}/details`); } catch (e) { $('.db', dr).innerHTML = `<div class="err">${esc(e.message)}</div>`; return; }
  const p = primary(a);
  const lat = a.latest;
  $('.db', dr).innerHTML = `
    <div class="hero" style="${appStyle(a)}">${iconHTML(a)}<div><h2 class="name">${nameHTML(a)}</h2><div class="headline">${esc(DET.headline)}</div></div></div>
    <div class="row">${p.href ? `<a class="btn ${p.cls || ''}" href="${esc(p.href)}" target="_blank" rel="noopener">${esc(p.label)}</a>` : `<button class="btn ${p.cls || ''}" ${p.disabled ? 'disabled' : ''} ${p.act ? `data-act="${p.act}" data-id="${esc(a.id)}" data-tag="${esc(p.tag || '')}"` : ''}>${esc(p.label)}</button>`}
      ${a.website ? `<a class="btn" href="${esc(a.website)}" target="_blank" rel="noopener">${esc(t('Learn more'))} ↗</a>` : ''}</div>
    ${DET.paragraphs.map(x => `<p>${esc(x)}</p>`).join('')}
    <div class="shots" id="shots"></div>
    <h4>${esc(t('Features'))}</h4>
    <ul class="feat" style="${appStyle(a)}">${DET.features.map(f => `<li><b>${esc(f.title)}</b><span>${esc(f.text)}</span></li>`).join('')}</ul>
    <h4>${esc(t('Latest release'))}</h4>
    <dl class="facts">
      <dt>${esc(t('Version'))}</dt><dd>${lat ? esc(verOf(lat.version)) : '—'}</dd>
      <dt>${esc(t('Released'))}</dt><dd>${lat && lat.published_at ? esc(fmtDate(lat.published_at)) : '—'}</dd>
      <dt>${esc(t('Platforms'))}</dt><dd>${esc(DET.platforms)}</dd>
      <dt>${esc(t('Releases'))}</dt><dd><a href="https://github.com/${esc(a.repo)}/releases" target="_blank" rel="noopener">${esc(t('All releases'))} ↗</a></dd>
    </dl>`;
  drawShots();
}
function drawShots() {
  const box = $('#shots'); if (!box || !DET || !DET.shots.length) return;
  const s = DET.shots[shotIndex];
  box.innerHTML = `<div class="lead" tabindex="0" role="button" aria-label="${esc(t('Click to enlarge'))}" data-act="lightbox" title="${esc(t('Click to enlarge'))}"><img alt="${esc(s.caption)}" src="${esc(s.url)}" onerror="this.replaceWith(document.createTextNode('${esc(t("Pictures couldn't be loaded. Reopen this page to try again."))}'))"></div>
    <div class="cap">${esc(s.caption)}</div>
    <div class="thumbs">${DET.shots.map((x, i) => `<button class="${i === shotIndex ? 'on' : ''}" data-act="shot" data-i="${i}" title="${esc(x.caption)}"><img alt="" loading="lazy" src="${esc(x.url)}"></button>`).join('')}</div>`;
}
function closeDrawer() {
  const was = $('#drawer').classList.contains('on');
  drawerId = null; $('#drawer').classList.remove('on'); $('#scrim').classList.remove('on'); closeLightbox();
  if (was && drawerOpener && document.contains(drawerOpener)) drawerOpener.focus();
  drawerOpener = null;
}
function openLightbox() {
  const s = DET.shots[shotIndex];
  let lb = $('.lightbox'); if (!lb) { lb = document.createElement('div'); lb.className = 'lightbox'; lb.setAttribute('role', 'dialog'); lb.setAttribute('aria-modal', 'true'); lb.setAttribute('aria-label', s.caption); document.body.appendChild(lb); lbOpener = document.activeElement; }
  lb.innerHTML = `<img alt="" src="${esc(s.url)}"><div class="cap">${esc(s.caption)}</div><button class="prev" data-act="lbprev" title="${esc(t('Previous picture (←)'))}">←</button><button class="next" data-act="lbnext" title="${esc(t('Next picture (→)'))}">→</button><button class="close" data-act="lbclose" title="${esc(t('Close (Esc)'))}" aria-label="${esc(t('Close (Esc)'))}">✕</button>`;
  const c = $('.close', lb); if (c) c.focus();
}
function closeLightbox() {
  const lb = $('.lightbox'); if (!lb) return;
  lb.remove();
  const o = lbOpener && document.contains(lbOpener) ? lbOpener : $('#shots .lead');
  lbOpener = null; if (o) o.focus();
}
function stepShot(d) { shotIndex = (shotIndex + d + DET.shots.length) % DET.shots.length; drawShots(); if ($('.lightbox')) openLightbox(); }

/* ---------------------------------------------------------------- actions */
async function doInstall(id, tag) {
  try { await api(`/api/apps/${encodeURIComponent(id)}/install`, { method: 'POST', body: { tag, activate: true } }); toast(t('Installing…')); } catch (e) { toast(e.message); }
  refresh();
}
async function doCheck() {
  BUSY = true; drawChrome();
  try { await api('/api/check', { method: 'POST' }); toast(t('Checking for updates…')); } catch (e) { if (!ME.admin) await refresh(); else toast(e.message); }
  setTimeout(refresh, 2500); setTimeout(refresh, 8000);
  setTimeout(() => { BUSY = false; drawChrome(); }, 3000);
}

document.addEventListener('click', async e => {
  const el = e.target.closest('[data-act]');
  if (!el) { if (MENU && !e.target.closest('.menu')) { MENU = null; render(); } return; }
  const act = el.dataset.act, id = el.dataset.id;
  if (act !== 'menu' && MENU && !el.closest('.menu')) MENU = null;
  switch (act) {
    case 'view': S.view = el.dataset.v; save(); render(); break;
    case 'filter': S.filter = el.dataset.v; save(); render(); break;
    case 'sort': { const k = el.dataset.v; if (S.sort === k) S.desc = !S.desc; else { S.sort = k; S.desc = ['installed', 'latest', 'released'].includes(k); } save(); render(); break; }
    case 'check': doCheck(); break;
    case 'settings': showSettings(); break;
    case 'menu': MENU = MENU === id ? null : id; menuOwner = id; render(); if (MENU) focusMenu(); break;
    case 'install': MENU = null; doInstall(id, el.dataset.tag); break;
    case 'cancel': try { await api(`/api/apps/${encodeURIComponent(id)}/cancel`, { method: 'POST' }); } catch (x) { toast(x.message); } refresh(); break;
    case 'details': MENU = null; render(); openDetails(id); break;
    case 'notes': MENU = null; render(); showNotes(id); break;
    case 'downloads': MENU = null; render(); showDownloads(id); break;
    case 'versions': MENU = null; render(); showVersions(id); break;
    case 'refreshApp': MENU = null; render(); try { await api(`/api/apps/${encodeURIComponent(id)}/refresh`, { method: 'POST' }); toast(t('Checking for updates…')); } catch (x) { toast(x.message); } refresh(); break;
    case 'uninstall': {
      MENU = null; render();
      const a = byId(id);
      if (confirm(`${t('Uninstall {name}?', { name: a.name })}\n\n${t('The app will be removed. Your documents and settings stay.')}`)) {
        try { await api(`/api/apps/${encodeURIComponent(id)}/uninstall`, { method: 'POST' }); toast(t('{name} was removed', { name: a.name })); } catch (x) { toast(x.message); }
        refresh();
      }
      break;
    }
    case 'hide': MENU = null; if (!S.hidden.includes(id)) S.hidden.push(id); save(); render(); break;
    case 'unhide': S.hidden = S.hidden.filter(x => x !== id); save(); render(); if (el.dataset.keep && window._settingsDraw) window._settingsDraw(); break;
    case 'dismiss': S.dismissed.push(id); save(); render(); break;
    case 'ack': S.dismissed.push(id); save(); $('#alertDlg').close(); render(); break;
    case 'closeDrawer': closeDrawer(); break;
    case 'shot': shotIndex = +el.dataset.i; drawShots(); break;
    case 'lightbox': openLightbox(); break;
    case 'lbprev': stepShot(-1); break;
    case 'lbnext': stepShot(1); break;
    case 'lbclose': closeLightbox(); break;
  }
});
document.addEventListener('keydown', e => {
  const trap = $('.lightbox') || (drawerId && $('#drawer'));
  if (e.key === 'Tab' && trap) {
    const f = $$('a[href], button:not([disabled]), [tabindex="0"]', trap).filter(x => x.offsetParent !== null);
    if (f.length) {
      const first = f[0], last = f[f.length - 1];
      if (!trap.contains(document.activeElement)) { e.preventDefault(); first.focus(); }
      else if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
    }
  }
  const el = e.target;
  if ((e.key === 'Enter' || e.key === ' ') && el && el.dataset && el.dataset.act && !/^(BUTTON|A|INPUT|SELECT|TEXTAREA)$/.test(el.tagName)) { e.preventDefault(); el.click(); return; }
  if (MENU && (e.key === 'ArrowDown' || e.key === 'ArrowUp') && el.closest && el.closest('.menu')) {
    const items = $$('a, button', el.closest('.menu')), i = items.indexOf(el);
    e.preventDefault(); items[(i + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length].focus();
  }
  if (e.key === 'Escape') {
    if ($('.lightbox')) closeLightbox(); else if (drawerId) closeDrawer();
    else if (MENU) { const o = menuOwner; MENU = null; render(); const b = $(`[data-act="menu"][data-id="${CSS.escape(o || '')}"]`); if (b) b.focus(); }
  }
  if ($('.lightbox') && DET) { if (e.key === 'ArrowLeft') stepShot(-1); if (e.key === 'ArrowRight') stepShot(1); }
});
$('#scrim').onclick = closeDrawer;
$('#dlgClose').onclick = closeDlg;
$('#dlg').addEventListener('close', () => { versionsOpen = null; window._settingsDraw = null; });
$('#sortSel').onchange = e => { S.sort = e.target.value; S.desc = ['installed', 'latest', 'released'].includes(S.sort); save(); render(); };
$('#q').oninput = render;
$('#addApp').onclick = showAddApp;
$('#updAll').onclick = async () => { try { const r = await api('/api/update-all', { method: 'POST' }); toast(t('Installing…') + ` (${r.queued})`); } catch (e) { toast(e.message); } refresh(); };
document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });

/* ---------------------------------------------------------------- boot */
(async function boot() {
  await loadLang();
  try { ME = await api('/api/me'); } catch { return; }
  drawChrome();
  await refresh();
  const stale = META.last_check ? (META.now - META.last_check) > 300 : true;
  if (ME.admin && stale) { try { await api('/api/check', { method: 'POST' }); setTimeout(refresh, 2500); setTimeout(refresh, 8000); } catch { /* ignore */ } }
  pollLoop();
})();
