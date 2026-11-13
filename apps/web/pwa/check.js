// The phone check (`?check=1`): what only the device can tell, measured where the app runs and
// sent as `diag` lines to the page's server log (`scripts/web-serve.ts`). Numbers and names only:
// never the server's address, a password or a picture. The page (index.html) imports this before
// the app starts, so it reports a page that never draws too.
const STEPS = ['browser', 'jspi', 'webgpu', 'handover', 'memory', 'frames', 'worker', 'standalone', 'textsize', 'safearea', 'keyboard', 'landscape'];

export function start({ say, ready }) {
  const controlledAtLoad = !!navigator.serviceWorker?.controller;
  const results = new Map(STEPS.map(id => [id, { state: 'none', value: '' }]));
  const code = Array.from(crypto.getRandomValues(new Uint8Array(4)), b => 'ACDEFHJKMNPRTVWXY347'[b % 20]).join('');
  let sent = false;
  const set = (id, state, value) => { results.set(id, { state, value: String(value).slice(0, 240) }); render(); };

  const style = document.createElement('style');
  style.textContent = `
    #check { position: fixed; inset: 0; z-index: 20; overflow-y: auto; background: Canvas; color: CanvasText; font: 17px/24px system-ui, sans-serif;
      padding: max(16px, env(safe-area-inset-top)) max(16px, env(safe-area-inset-right)) max(16px, env(safe-area-inset-bottom)) max(16px, env(safe-area-inset-left)); box-sizing: border-box; touch-action: pan-y; }
    #check[data-hidden] { inset: 0 0 auto 0; height: auto; overflow: hidden; background: color-mix(in srgb, Canvas 92%, transparent); }
    #check[data-hidden] > :not(.measuring) { display: none; }
    #check h1 { font-size: 22px; line-height: 28px; font-weight: 600; margin: 0 0 8px; }
    #check p { margin: 0 0 16px; color: color-mix(in srgb, CanvasText 70%, transparent); }
    #check .step { margin: 16px 0; padding: 12px; border: 1px solid color-mix(in srgb, CanvasText 15%, transparent); }
    #check .step p { margin: 0 0 12px; color: CanvasText; }
    #check input { box-sizing: border-box; width: 100%; height: 48px; padding: 0 12px; font: 16px system-ui, sans-serif; border: 1px solid color-mix(in srgb, CanvasText 25%, transparent); border-radius: 0; background: Canvas; color: CanvasText; }
    #check ul { list-style: none; margin: 0; padding: 0; }
    #check li { display: flex; gap: 12px; padding: 8px 0; border-bottom: 1px solid color-mix(in srgb, CanvasText 10%, transparent); }
    #check li code { flex: 0 0 104px; font: 15px/24px ui-monospace, monospace; }
    #check li span { flex: 1; min-width: 0; overflow-wrap: anywhere; color: color-mix(in srgb, CanvasText 70%, transparent); font-size: 15px; }
    #check li b { flex: 0 0 auto; font-weight: 600; }
    #check .ok { color: #1f9d55; } #check .fail { color: #d93025; } #check .none { color: color-mix(in srgb, CanvasText 50%, transparent); }
    #check button { width: 100%; height: 52px; margin-top: 16px; font: 600 17px system-ui, sans-serif; border: 1px solid #009fff; border-radius: 0; background: #009fff; color: #fff; }
    #check .code { margin: 16px 0; font: 600 22px/30px system-ui, sans-serif; text-align: center; }
    #check .code:empty, #check .error:empty { display: none; }
    #check ul { margin-top: 16px; }`;
  document.head.append(style);
  const panel = document.createElement('section');
  panel.id = 'check';
  panel.setAttribute('aria-labelledby', 'check-title');
  document.body.append(panel);
  let status = '';

  // Built once, so the field keeps its focus (and the keyboard stays up) while results come in;
  // `render` refreshes the words, the list and the button.
  panel.innerHTML = `
    <div class="measuring" hidden></div>
    <h1 id="check-title"></h1>
    <p class="intro"></p>
    <div class="step"><p class="keyboard"></p><input id="check-field" type="text" enterkeyhint="done" autocomplete="off"></div>
    <div class="step"><p class="rotate"></p></div>
    <button type="button" id="check-send"></button>
    <div class="code" role="status"></div>
    <p class="error" role="alert"></p>
    <ul></ul>`;
  const $ = sel => panel.querySelector(sel);
  const field = $('#check-field');
  field.onfocus = () => keyboard('open');
  field.onblur = () => keyboard('closed');
  field.onkeydown = e => { if (e.key === 'Enter') field.blur(); };
  $('#check-send').onclick = send;

  function render() {
    const row = ([id, r]) => `<li><code>${id}</code><span>${escape(r.value)}</span><b class="${r.state}">${escape(say({ ok: 'rune.web.check.works', fail: 'rune.web.check.failed', none: 'rune.web.check.none' }[r.state]))}</b></li>`;
    $('.measuring').textContent = say('rune.web.check.measuring');
    $('.measuring').hidden = !panel.hasAttribute('data-hidden');
    $('#check-title').textContent = say('rune.web.check.title');
    $('.intro').textContent = say('rune.web.check.intro');
    $('.keyboard').textContent = say('rune.web.check.keyboard');
    field.placeholder = say('rune.web.check.field');
    $('.rotate').textContent = say('rune.web.check.rotate');
    $('ul').innerHTML = [...results].map(row).join('');
    $('#check-send').textContent = say(status === 'sending' ? 'rune.web.check.sending' : 'rune.web.check.send');
    $('#check-send').disabled = status === 'sending';
    $('.code').textContent = status === 'sent' ? say('rune.web.check.sent', { code }) : '';
    $('.error').textContent = status === 'error' ? say('rune.web.check.notSent') : '';
  }

  // Each field of a diag line must be a number, a boolean or a short string (`web-serve.ts`).
  async function send() {
    status = 'sending'; render();
    const items = [...results].map(([item, r]) => ({ item, state: r.state, value: r.value }));
    try {
      const response = await fetch('diag', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ check: code, items }), keepalive: true });
      if (!response.ok) throw new Error(String(response.status));
      sent = true; status = 'sent';
    } catch { status = 'error'; }
    render();
    $(status === 'sent' ? '.code' : '.error').scrollIntoView({ block: 'center' });
  }
  // A check left without Send still reports what it measured.
  addEventListener('pagehide', () => {
    if (sent) return;
    const items = [...results].map(([item, r]) => ({ item, state: r.state, value: r.value }));
    navigator.sendBeacon?.('diag', new Blob([JSON.stringify({ check: code, partial: true, items })], { type: 'application/json' }));
  });

  // What the page can say at once.
  const ua = navigator.userAgent;
  const ios = ua.match(/OS (\d+)[_.](\d+)(?:[_.](\d+))? like Mac OS X/);
  const safari = ua.match(/Version\/([\d.]+)/);
  set('browser', 'ok', [ios ? `iOS ${ios.slice(1).filter(Boolean).join('.')}` : navigator.platform, safari ? `Safari ${safari[1]}` : '', `${screen.width}×${screen.height} @${devicePixelRatio}`, ua].filter(Boolean).join(' · '));
  const jspi = 'Suspending' in WebAssembly;
  set('jspi', jspi ? 'ok' : 'fail', jspi ? 'WebAssembly.Suspending' : 'missing: the app cannot start');
  const standalone = matchMedia('(display-mode: standalone)').matches || navigator.standalone === true;
  set('standalone', 'ok', standalone ? 'Home Screen app' : 'browser tab');
  const probe = document.createElement('div');
  probe.style.cssText = 'position:fixed;visibility:hidden;pointer-events:none;font:-apple-system-body;padding:env(safe-area-inset-top) env(safe-area-inset-right) env(safe-area-inset-bottom) env(safe-area-inset-left)';
  document.body.append(probe);
  const px = v => Math.round(parseFloat(v) || 0);
  const insets = () => { const s = getComputedStyle(probe); return `t${px(s.paddingTop)} r${px(s.paddingRight)} b${px(s.paddingBottom)} l${px(s.paddingLeft)}`; };
  const body = parseFloat(getComputedStyle(probe).fontSize);
  // iOS's text size reaches a page through `-apple-system-body` only (17 px at the default size).
  set('textsize', 'ok', `body ${body}px (iOS default 17) · root ${getComputedStyle(document.documentElement).fontSize}`);
  const portrait = () => matchMedia('(orientation: portrait)').matches;
  const view = () => { const v = visualViewport; return v ? `${Math.round(v.width)}×${Math.round(v.height)} top ${Math.round(v.offsetTop)}` : `${innerWidth}×${innerHeight}`; };
  set('safearea', 'ok', `${portrait() ? 'portrait' : 'landscape'} ${insets()} · ${view()}`);

  // Turned sideways and back: the insets and the viewport each way, and where the app's canvas is.
  let wide = null;
  const turned = () => setTimeout(() => {
    if (!portrait()) wide = `landscape ${insets()} · ${view()} · canvas ${canvasBox()}`;
    else if (wide) set('landscape', 'ok', `${wide} | back: portrait ${insets()} · ${view()} · canvas ${canvasBox()}`);
  }, 700);
  addEventListener('orientationchange', turned);
  matchMedia('(orientation: portrait)').addEventListener?.('change', turned);

  // The keyboard: the viewport and the canvas before, with it up, and after.
  const canvas = () => document.querySelector('canvas');
  const canvasBox = () => { const r = canvas()?.getBoundingClientRect(); return r ? `${Math.round(r.top)}+${Math.round(r.height)}` : 'none'; };
  const snap = () => {
    const v = visualViewport, c = canvas()?.getBoundingClientRect();
    return { w: Math.round(v?.width ?? innerWidth), h: Math.round(v?.height ?? innerHeight), top: Math.round(v?.offsetTop ?? 0), inner: innerHeight, canvasTop: c ? Math.round(c.top) : null, canvasH: c ? Math.round(c.height) : null, scroll: Math.round(scrollY) };
  };
  const words = s => `${s.w}×${s.h} top ${s.top} · inner ${s.inner} · canvas ${s.canvasH === null ? 'none' : `${s.canvasTop}+${s.canvasH}`} · scrollY ${s.scroll}`;
  let before = null, up = null;
  function keyboard(phase) {
    if (phase === 'open') before = snap();
    setTimeout(() => {
      if (phase === 'open') { up = snap(); return; }
      const after = snap();
      // Up, the keyboard took the bottom of the screen if the visual viewport shrank; the app's
      // canvas followed it if it ended where the viewport did; closed, nothing should stay moved.
      const shrank = up && before && up.h < before.h - 100;
      const followed = up && up.canvasH !== null && Math.abs(up.canvasTop + up.canvasH - (up.top + up.h)) <= 2;
      const settled = before && after.h >= before.h - 2 && after.scroll === before.scroll && after.top === before.top;
      const notes = [shrank ? `keyboard ${before.h - up.h} pt` : 'the viewport did not shrink', shrank ? (followed ? 'canvas ended at the keyboard' : 'canvas did not follow the keyboard') : '', settled ? 'back in place' : 'did not return to where it was'].filter(Boolean);
      set('keyboard', shrank && followed && settled ? 'ok' : 'fail', `${notes.join(' · ')} | before ${before ? words(before) : '?'} | up ${up ? words(up) : '?'} | after ${words(after)}`);
    }, 800);
  }
  // The GPU: whether the browser gave the page one, and whether the app moved onto it.
  (async () => {
    if (!navigator.gpu) { set('webgpu', 'fail', 'navigator.gpu missing: the app draws on the CPU'); return; }
    try {
      const adapter = await navigator.gpu.requestAdapter();
      const info = adapter?.info ?? {};
      set('webgpu', adapter ? 'ok' : 'fail', adapter ? [info.vendor, info.architecture, info.description].filter(Boolean).join(' ') || 'adapter' : 'requestAdapter() gave none');
    } catch (e) { set('webgpu', 'fail', `requestAdapter threw: ${e?.message ?? e}`); }
  })();
  const mark = name => performance.getEntriesByName(name)[0]?.startTime;
  const marks = () => ['web.gpu.asked', 'web.gpu.ready', 'web.gpu.failed', 'web.gpu.lost', 'web.handover.drawn', 'web.handover.done', 'web.handover.failed', 'web.first.presented']
    .filter(n => mark(n) !== undefined).map(n => `${n.slice(4)} ${Math.round(mark(n))}ms`).join(' · ');

  // After the first frame, or not within 30 s: the hand-over, memory, the worker, then 3 s of scrolling.
  let presented = false;
  document.addEventListener('runepresent', () => { presented = true; setTimeout(afterFirstFrame, 2500); }, { once: true });
  setTimeout(() => {
    if (presented) return;
    const words = document.querySelector('#status')?.textContent?.trim();
    set('handover', 'fail', `no first frame in 30 s${words ? `: ${words}` : ''} · ${marks()}`);
    worker();
  }, 30000);

  async function afterFirstFrame() {
    set('handover', mark('web.handover.done') !== undefined ? 'ok' : mark('web.gpu.asked') === undefined ? 'none' : 'fail', marks() || 'no marks');
    memory();
    worker();
    await frames();
    // A hand-over can land after the scroll.
    set('handover', mark('web.handover.done') !== undefined ? 'ok' : mark('web.gpu.asked') === undefined ? 'none' : 'fail', marks() || 'no marks');
    memory();
  }

  function memory() {
    const wasm = globalThis.__runeWasm?.memory?.buffer?.byteLength;
    const heap = performance.memory?.usedJSHeapSize;
    const parts = [wasm ? `module ${Math.round(wasm / 1048576)} MB` : '', heap ? `JS heap ${Math.round(heap / 1048576)} MB` : 'the browser reports no page memory'];
    set('memory', wasm ? 'ok' : 'none', parts.filter(Boolean).join(' · '));
  }

  // The second open: whether the worker controls this page, and how many of the page's own files
  // came from the device rather than the network.
  function worker() {
    const files = performance.getEntriesByType('resource').filter(e => e.name.startsWith(location.origin + location.pathname.replace(/[^/]*$/, '')) && !/\/diag$/.test(e.name));
    const local = files.filter(e => e.workerStart > 0 || e.deliveryType === 'cache' || (e.transferSize === 0 && e.decodedBodySize > 0)).length;
    const nav = performance.getEntriesByType('navigation')[0];
    const controlled = controlledAtLoad;
    set('worker', controlled ? (local > 0 ? 'ok' : 'fail') : 'none', `${controlled ? 'controlled' : 'not controlled (a first open)'} · ${local} of ${files.length} files from the device · page ${nav?.transferSize === 0 ? 'from the device' : 'from the network'}`);
  }

  // Frame pacing: 3 s of scrolling the app's list, as a wheel would, measured by the page's frames.
  async function frames() {
    const c = canvas();
    if (!c) { set('frames', 'fail', 'no canvas'); return; }
    panel.setAttribute('data-hidden', ''); render();
    const r = c.getBoundingClientRect(), x = r.left + r.width / 2, y = r.top + r.height * 0.55;
    c.dispatchEvent(new PointerEvent('pointermove', { clientX: x, clientY: y, pointerType: 'mouse', isPrimary: true, bubbles: true }));
    const times = [];
    await new Promise(done => {
      const t0 = performance.now();
      const step = t => {
        times.push(t);
        const phase = (t - t0) / 3000;
        if (phase >= 1) { done(); return; }
        c.dispatchEvent(new WheelEvent('wheel', { clientX: x, clientY: y, deltaY: phase < 0.5 ? 40 : -40, deltaMode: 0, bubbles: true, cancelable: true }));
        requestAnimationFrame(step);
      };
      requestAnimationFrame(step);
    });
    panel.removeAttribute('data-hidden'); render();
    const gaps = times.slice(1).map((t, i) => t - times[i]).sort((a, b) => a - b);
    const at = q => gaps[Math.min(gaps.length - 1, Math.floor(q * gaps.length))]?.toFixed(1);
    const long = gaps.filter(g => g > 25).length;
    set('frames', long > gaps.length * 0.1 ? 'fail' : 'ok', `${gaps.length} frames in 3 s · p50 ${at(0.5)} ms · p95 ${at(0.95)} ms · max ${gaps.at(-1)?.toFixed(1)} ms · ${long} over 25 ms`);
  }

  ready.then(render);
  render();
}

function escape(s) {
  return String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
}
