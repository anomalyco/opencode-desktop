// Injected by opencode-browser-engine via private in-process CDP pipe into chrome-extension://
// service workers and extension pages BEFORE line 1 of extension code executes.
//
// Bridges shimmed chrome.* / browser.* APIs to Rust via Runtime.addBinding("__ocExtCall") and
// receives replies + events via globalThis.__ocExtRecv(msg).
// Keeps all working native Chromium APIs (declarativeNetRequest, storage, scripting, alarms,
// offscreen, runtime, tabs.sendMessage, tabs.connect, webRequest, cookies) native!
(() => {
  const LOCKDOWN_APIS = ["tabs", "windows", "action", "browserAction", "pageAction", "contextMenus", "webNavigation"];
  const neutralizeChrome = (win) => {
    if (!win) return;
    for (const rootName of ["chrome", "browser"]) {
      let root;
      try { root = win[rootName]; } catch { continue; }
      if (!root) continue;
      for (const api of LOCKDOWN_APIS) {
        try {
          Object.defineProperty(root, api, { value: undefined, configurable: false, enumerable: false, writable: false });
        } catch {}
      }
    }
  };
  const sweepFrames = (win) => {
    if (!win) return;
    let len = 0;
    try { len = win.length | 0; } catch { return; }
    for (let i = 0; i < len; i++) {
      let cw;
      try { cw = win[i]; } catch { continue; }
      if (cw && cw !== globalThis && !cw.__ocExtInstalled) {
        neutralizeChrome(cw);
        sweepFrames(cw);
      }
    }
  };

  const isSubframe = Boolean(globalThis.window && globalThis.top && globalThis.window !== globalThis.top);
  if (globalThis.location) {
    const href = String(globalThis.location.href || "");
    const origin = String(globalThis.location.origin || "");
    const extId = globalThis.chrome?.runtime?.id;
    // Neutralize chrome.tabs/windows/etc. in ANY child subframe (about:blank, about:srcdoc,
    // same-origin iframe, <object>, <embed>) instead of leaving native APIs reachable (READER-12 C2).
    if (isSubframe || href === "about:blank" || href === "about:srcdoc" || origin === "null") {
      neutralizeChrome(globalThis);
      sweepFrames(globalThis);
      return "NEUTRALIZED";
    }
    if (!extId || origin !== "chrome-extension://" + extId) {
      return "DENIED";
    }
  }
  if (!globalThis.chrome?.runtime?.getManifest) return "WAIT";
  if (globalThis.__ocExtInstalled) return "ALREADY";
  globalThis.__ocExtInstalled = true;

  // Harden synchronous child frame creation (`about:blank`, `srcdoc`, `<object>`, `<embed>`,
  // `innerHTML`, `insertAdjacentHTML`, `document.write`/`open`, `window[0]`, `window.open`).
  if (typeof globalThis.window !== "undefined") {
    const patchAccessor = (proto, prop, after) => {
      if (!proto) return;
      const desc = Object.getOwnPropertyDescriptor(proto, prop);
      if (!desc) return;
      try {
        Object.defineProperty(proto, prop, {
          configurable: false,
          enumerable: desc.enumerable,
          get: desc.get
            ? function () {
                const v = desc.get.call(this);
                after(v, this);
                return v;
              }
            : undefined,
          set: desc.set
            ? function (val) {
                const r = desc.set.call(this, val);
                after(val, this);
                return r;
              }
            : undefined,
        });
      } catch {}
    };
    const patchMethod = (proto, method, after) => {
      if (!proto) return;
      const orig = proto[method];
      if (typeof orig !== "function") return;
      try {
        Object.defineProperty(proto, method, {
          configurable: false,
          writable: false,
          value: function (...a) {
            const res = orig.apply(this, a);
            after(res, this, a);
            return res;
          },
        });
      } catch {}
    };
    for (const ctor of [globalThis.HTMLIFrameElement, globalThis.HTMLFrameElement, globalThis.HTMLObjectElement]) {
      patchAccessor(ctor?.prototype, "contentWindow", (cw) => {
        try {
          if (cw && cw !== globalThis && !cw.__ocExtInstalled) neutralizeChrome(cw);
        } catch {}
      });
      patchAccessor(ctor?.prototype, "contentDocument", (doc) => {
        try {
          const cw = doc?.defaultView;
          if (cw && cw !== globalThis && !cw.__ocExtInstalled) neutralizeChrome(cw);
        } catch {}
      });
    }
    const sweepAll = () => sweepFrames(globalThis);
    for (const m of ["appendChild", "insertBefore", "replaceChild"]) {
      patchMethod(globalThis.Node?.prototype, m, sweepAll);
    }
    for (const m of ["append", "prepend", "after", "before", "replaceWith", "insertAdjacentElement", "insertAdjacentHTML"]) {
      patchMethod(globalThis.Element?.prototype, m, sweepAll);
    }
    patchAccessor(globalThis.Element?.prototype, "innerHTML", sweepAll);
    patchAccessor(globalThis.Element?.prototype, "outerHTML", sweepAll);
    patchAccessor(globalThis.ShadowRoot?.prototype, "innerHTML", sweepAll);
    for (const m of ["write", "writeln", "open"]) {
      patchMethod(globalThis.Document?.prototype, m, (_res, doc) => {
        try {
          const cw = doc?.defaultView;
          if (cw && cw !== globalThis && !cw.__ocExtInstalled) neutralizeChrome(cw);
        } catch {}
        sweepFrames(globalThis);
      });
    }
    patchMethod(globalThis.Window?.prototype || globalThis, "open", (w) => {
      try {
        if (w && w !== globalThis && !w.__ocExtInstalled) neutralizeChrome(w);
      } catch {}
    });
  }

  const isPopup = __IS_POPUP__;
  const manifest = chrome.runtime.getManifest();
  const declared = new Set([
    ...(manifest.permissions || []),
    ...(manifest.host_permissions || []),
  ]);

  let nextReq = 1;
  const inflight = new Map();
  const listeners = new Map();
  const activeTabInfo = { windowId: 1, activeTabId: -1 };

  // Save native references we still delegate to or use for Tab ID probing.
  const nativeTabsGet = chrome.tabs?.get?.bind(chrome.tabs);
  const nativeTabsGetCurrent = chrome.tabs?.getCurrent?.bind(chrome.tabs);
  const nativeRuntimeOnMsg = chrome.runtime?.onMessage;

  globalThis.__ocExtRecv = (msg) => {
    if (!msg) return;
    if (msg.kind === "reply") {
      const entry = inflight.get(msg.reqId);
      if (!entry) return;
      inflight.delete(msg.reqId);
      if (msg.ok) entry.resolve(msg.value);
      else entry.reject(new Error(msg.error || "Extension API call failed."));
      return;
    }
    if (msg.kind === "active_tab") {
      activeTabInfo.activeTabId = msg.tabId ?? -1;
      return;
    }
    if (msg.kind === "event") {
      const registered = [...(listeners.get(msg.name) || [])];
      for (const fn of registered) {
        try {
          fn(...(msg.args || []));
        } catch (err) {
          console.error(err);
        }
      }
      return;
    }
    if (msg.kind === "probe_tabs") {
      // Zero-guess SessionID verification using native chrome.tabs.get(id):
      // ExtensionTabUtil::GetTabById only resolves WebContents in this extension's own Profile,
      // so isolated preview/remote CefRequestContexts automatically return errors and stay hidden.
      (async () => {
        const found = [];
        if (nativeTabsGet && Array.isArray(msg.candidates)) {
          await Promise.all(
            msg.candidates.map(async (id) => {
              try {
                const t = await nativeTabsGet(id);
                if (t && typeof t.id === "number") {
                  found.push({
                    id: t.id,
                    url: t.url || "",
                    title: t.title || "",
                    width: t.width || 0,
                    height: t.height || 0,
                  });
                }
              } catch {}
            }),
          );
        }
        found.sort((a, b) => a.id - b.id);
        sendRaw({ op: "probe_tabs_reply", probeId: msg.probeId, found });
      })();
    }
  };

  function sendRaw(obj) {
    if (typeof globalThis.__ocExtCall === "function") {
      globalThis.__ocExtCall(JSON.stringify(obj));
    }
  }

  function invoke(name, args) {
    return new Promise((resolve, reject) => {
      const reqId = nextReq++;
      inflight.set(reqId, { resolve, reject });
      sendRaw({ op: "call", reqId, name, args });
    });
  }

  function subscribe(name, listen) {
    sendRaw({ op: "subscribe", name, listen });
  }

  // Marshal ImageData and plain objects for JSON transport across CDP binding.
  function clean(val) {
    if (typeof ImageData !== "undefined" && val instanceof ImageData) {
      if (val.width <= 0 || val.height <= 0 || val.width > 128 || val.height > 128) return undefined;
      const bytes = new Uint8Array(val.data.buffer, val.data.byteOffset, val.data.byteLength);
      let bin = "";
      for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
      return { __imageData: true, width: val.width, height: val.height, rgba_b64: btoa(bin) };
    }
    if (Array.isArray(val)) return val.map(clean);
    if (val && typeof val === "object" && !(val instanceof ArrayBuffer) && !ArrayBuffer.isView(val)) {
      const out = {};
      for (const [k, v] of Object.entries(val)) {
        if (typeof v !== "function" && v !== undefined) out[k] = clean(v);
      }
      return out;
    }
    return typeof val === "function" ? undefined : val;
  }

  function makeEvent(name) {
    return {
      addListener(fn) {
        if (typeof fn !== "function") return;
        let set = listeners.get(name);
        if (!set) {
          set = new Set();
          listeners.set(name, set);
        }
        if (set.size === 0) subscribe(name, true);
        set.add(fn);
      },
      removeListener(fn) {
        const set = listeners.get(name);
        if (set && set.delete(fn) && set.size === 0) subscribe(name, false);
      },
      hasListener(fn) {
        return Boolean(listeners.get(name)?.has(fn));
      },
      hasListeners() {
        return (listeners.get(name)?.size || 0) > 0;
      },
    };
  }

  function makeMethod(name, isSyncCreate) {
    return function (...rawArgs) {
      const args = [...rawArgs];
      const cb = typeof args[args.length - 1] === "function" ? args.pop() : undefined;
      if (isSyncCreate && args[0] && typeof args[0] === "object" && args[0].id === undefined) {
        args[0] = { ...args[0], id: crypto.randomUUID() };
      }
      const syncId = isSyncCreate ? args[0]?.id : undefined;

      const promise = invoke(name, clean(args));
      if (!cb) return isSyncCreate ? syncId : promise;

      promise.then(
        (value) => cb(value),
        (err) => {
          const last = { message: err?.message || String(err) };
          const rt = chrome.runtime;
          const prev = Object.getOwnPropertyDescriptor(rt, "lastError");
          let read = false;
          Object.defineProperty(rt, "lastError", {
            configurable: true,
            get() {
              read = true;
              return last;
            },
          });
          try {
            cb();
          } finally {
            if (prev) Object.defineProperty(rt, "lastError", prev);
            else delete rt.lastError;
            if (!read) console.error(`Unchecked runtime.lastError: ${last.message}`);
          }
        },
      );
      return syncId;
    };
  }

  function def(target, key, value) {
    try {
      Object.defineProperty(target, key, {
        value,
        configurable: true,
        enumerable: true,
        writable: true,
      });
    } catch (e) {
      console.error(`oc-ext shim ${key}:`, e);
    }
  }

  // Suppress the synthetic `onInstalled({ reason: "update", previousVersion === currentVersion })`
  // fired by Chromium when our inert startup exemption stub upgrades to the verified unpacked
  // extension at the exact same version (matching upstream `extension-api/host.ts:244`).
  const nativeOnInstalled = chrome.runtime?.onInstalled;
  const wrappedOnInstalled = new WeakMap();
  if (nativeOnInstalled && typeof nativeOnInstalled.addListener === "function") {
    const origAdd = nativeOnInstalled.addListener.bind(nativeOnInstalled);
    const origRem = nativeOnInstalled.removeListener.bind(nativeOnInstalled);
    const origHas = nativeOnInstalled.hasListener.bind(nativeOnInstalled);
    def(nativeOnInstalled, "addListener", function (fn) {
      if (typeof fn !== "function") return origAdd(fn);
      let wrap = wrappedOnInstalled.get(fn);
      if (!wrap) {
        wrap = function (details) {
          if (details && details.reason === "update" && details.previousVersion === manifest.version) {
            return;
          }
          return fn(details);
        };
        wrappedOnInstalled.set(fn, wrap);
      }
      return origAdd(wrap);
    });
    def(nativeOnInstalled, "removeListener", function (fn) {
      return origRem(wrappedOnInstalled.get(fn) || fn);
    });
    def(nativeOnInstalled, "hasListener", function (fn) {
      return origHas(wrappedOnInstalled.get(fn) || fn);
    });
  }

  // Patch sender.tab.windowId (-1 -> 1) and sender.tab.active inside runtime.onMessage so
  // content-script messages match our shimmed tabs.query / windows.getCurrent!
  const wrappedOnMsg = new WeakMap();
  if (nativeRuntimeOnMsg && typeof nativeRuntimeOnMsg.addListener === "function") {
    const origAdd = nativeRuntimeOnMsg.addListener.bind(nativeRuntimeOnMsg);
    const origRem = nativeRuntimeOnMsg.removeListener.bind(nativeRuntimeOnMsg);
    const origHas = nativeRuntimeOnMsg.hasListener.bind(nativeRuntimeOnMsg);
    def(nativeRuntimeOnMsg, "addListener", function (fn) {
      if (typeof fn !== "function") return origAdd(fn);
      let wrap = wrappedOnMsg.get(fn);
      if (!wrap) {
        wrap = function (message, sender, sendResponse) {
          if (sender && sender.tab) {
            try {
              if (sender.tab.windowId === -1) sender.tab.windowId = activeTabInfo.windowId;
              if (activeTabInfo.activeTabId > 0) {
                sender.tab.active = sender.tab.id === activeTabInfo.activeTabId;
              }
            } catch {}
            // Opportunistically report real Chromium sender.tab.id to engine!
            if (typeof sender.tab.id === "number" && sender.tab.id > 0 && sender.tab.url) {
              sendRaw({ op: "tab_seen", id: sender.tab.id, url: sender.tab.url });
            }
          }
          return fn(message, sender, sendResponse);
        };
        wrappedOnMsg.set(fn, wrap);
      }
      return origAdd(wrap);
    });
    def(nativeRuntimeOnMsg, "removeListener", function (fn) {
      return origRem(wrappedOnMsg.get(fn) || fn);
    });
    def(nativeRuntimeOnMsg, "hasListener", function (fn) {
      return origHas(wrappedOnMsg.get(fn) || fn);
    });
  }

  const NAMESPACES = [
    {
      path: "tabs",
      methods: [
        "query", "get", "getCurrent", "create", "update", "remove", "reload", "highlight",
        "goBack", "goForward", "setZoom", "getZoom", "setZoomSettings", "getZoomSettings",
        "captureVisibleTab", "duplicate", "move", "discard", "group", "ungroup",
        "detectLanguage", "executeScript", "insertCSS", "removeCSS",
      ],
      events: ["onCreated", "onUpdated", "onActivated", "onRemoved", "onHighlighted", "onReplaced", "onMoved"],
      constants: { TAB_ID_NONE: -1, TAB_INDEX_NONE: -1 },
    },
    {
      path: "windows",
      methods: ["get", "getCurrent", "getLastFocused", "getAll", "create", "update", "remove"],
      events: ["onCreated", "onRemoved", "onFocusChanged", "onBoundsChanged"],
      constants: { WINDOW_ID_NONE: -1, WINDOW_ID_CURRENT: -2 },
    },
    {
      path: "action",
      methods: [
        "setTitle", "getTitle", "setIcon", "setPopup", "getPopup",
        "setBadgeText", "getBadgeText", "setBadgeBackgroundColor", "getBadgeBackgroundColor",
        "setBadgeTextColor", "getBadgeTextColor", "enable", "disable", "isEnabled",
        "getUserSettings", "openPopup",
      ],
      events: ["onClicked", "onUserSettingsChanged"],
    },
    {
      path: "runtime",
      methods: ["openOptionsPage"],
    },
    {
      path: "webNavigation",
      permissions: ["webNavigation"],
      methods: ["getFrame", "getAllFrames"],
      events: [
        "onBeforeNavigate", "onCommitted", "onDOMContentLoaded", "onCompleted",
        "onErrorOccurred", "onCreatedNavigationTarget", "onReferenceFragmentUpdated",
        "onHistoryStateUpdated", "onTabReplaced",
      ],
    },
    {
      path: "contextMenus",
      permissions: ["contextMenus"],
      methods: ["create", "update", "remove", "removeAll"],
      events: ["onClicked"],
      sync: ["create"],
      constants: {
        ACTION_MENU_TOP_LEVEL_LIMIT: 6,
        ContextType: {
          ALL: "all", PAGE: "page", FRAME: "frame", SELECTION: "selection", LINK: "link",
          EDITABLE: "editable", IMAGE: "image", VIDEO: "video", AUDIO: "audio",
          LAUNCHER: "launcher", BROWSER_ACTION: "browser_action", PAGE_ACTION: "page_action", ACTION: "action",
        },
        ItemType: { NORMAL: "normal", CHECKBOX: "checkbox", RADIO: "radio", SEPARATOR: "separator" },
      },
    },
    {
      path: "notifications",
      permissions: ["notifications"],
      methods: ["create", "update", "clear", "getAll", "getPermissionLevel"],
      events: ["onClosed", "onClicked", "onButtonClicked", "onPermissionLevelChanged", "onShowSettings"],
      constants: {
        TemplateType: { BASIC: "basic", IMAGE: "image", LIST: "list", PROGRESS: "progress" },
        PermissionLevel: { GRANTED: "granted", DENIED: "denied" },
      },
    },
    {
      path: "permissions",
      methods: ["contains", "getAll", "request", "remove"],
      events: ["onAdded", "onRemoved"],
    },
    {
      path: "cookies",
      permissions: ["cookies"],
      methods: ["getAllCookieStores", "getPartitionKey"],
    },
    {
      path: "sidePanel",
      permissions: ["sidePanel"],
      methods: ["setOptions", "getOptions", "setPanelBehavior", "getPanelBehavior", "open", "close"],
      events: ["onOpened", "onClosed"],
    },
    {
      path: "fontSettings",
      permissions: ["fontSettings"],
      methods: ["getFontList", "getFont", "setFont", "clearFont"],
    },
    {
      path: "commands",
      methods: ["getAll"],
      events: ["onCommand", "onChanged"],
    },
    {
      path: "extension",
      methods: ["isAllowedFileSchemeAccess", "isAllowedIncognitoAccess", "setUpdateUrlData"],
    },
  ];

  // Chromium exposes both `chrome` and `browser` (uBlock Origin Lite reads `browser` first!).
  const roots = [...new Set([globalThis.chrome, globalThis.browser].filter(Boolean))];

  for (const ns of NAMESPACES) {
    if (ns.permissions && !ns.permissions.some((p) => declared.has(p))) continue;
    const members = {};
    for (const m of ns.methods || []) {
      members[m] = makeMethod(`${ns.path}.${m}`, Boolean(ns.sync?.includes(m)));
    }
    for (const [k, v] of Object.entries(ns.constants || {})) {
      members[k] = v;
    }
    const eventObjs = {};
    for (const ev of ns.events || []) {
      eventObjs[ev] = makeEvent(`${ns.path}.${ev}`);
    }

    const targets = new Set();
    for (const root of roots) {
      let cur = root;
      for (const seg of ns.path.split(".")) {
        let next;
        try {
          next = cur[seg]; // triggers Chromium lazy API getter!
        } catch {}
        if (!next) {
          next = {};
          def(cur, seg, next);
        }
        cur = next;
      }
      targets.add(cur);
    }

    const nativeAllow = ns.path === "tabs" ? new Set(["sendMessage", "connect"]) : null;
    for (const target of targets) {
      for (const [k, v] of Object.entries(members)) def(target, k, v);
      for (const [k, v] of Object.entries(eventObjs)) def(target, k, v);
      // Sweep any remaining native methods on `chrome.tabs` / `chrome.windows` so newly added
      // Chromium methods cannot bypass our RPC/agent_driven guards (READER-12 C2).
      if (ns.path === "tabs" || ns.path === "windows") {
        for (const prop of Object.getOwnPropertyNames(target)) {
          if (Object.prototype.hasOwnProperty.call(members, prop)) continue;
          if (Object.prototype.hasOwnProperty.call(eventObjs, prop)) continue;
          if (nativeAllow?.has(prop)) continue;
          let curVal;
          try { curVal = target[prop]; } catch { continue; }
          if (typeof curVal === "function") {
            def(target, prop, makeMethod(`${ns.path}.${prop}`, false));
          }
        }
      }
    }
  }

  // Also stub privacy.* settings if "privacy" is declared (Bitwarden / password managers).
  if (declared.has("privacy")) {
    const PRIV = {
      "privacy.network.networkPredictionEnabled": true,
      "privacy.network.webRTCIPHandlingPolicy": "default",
      "privacy.services.autofillAddressEnabled": true,
      "privacy.services.autofillCreditCardEnabled": true,
      "privacy.services.autofillEnabled": true,
      "privacy.services.passwordSavingEnabled": true,
      "privacy.services.safeBrowsingEnabled": true,
      "privacy.websites.hyperlinkAuditingEnabled": true,
      "privacy.websites.referrersEnabled": true,
      "privacy.websites.thirdPartyCookiesAllowed": true,
      "privacy.websites.doNotTrackEnabled": false,
    };
    for (const [fullPath, defaultVal] of Object.entries(PRIV)) {
      const segs = fullPath.split(".");
      for (const root of roots) {
        let cur = root;
        for (const s of segs) {
          if (!cur[s]) def(cur, s, {});
          cur = cur[s];
        }
        def(cur, "get", makeMethod(`${fullPath}.get`, false));
        def(cur, "set", makeMethod(`${fullPath}.set`, false));
        def(cur, "clear", makeMethod(`${fullPath}.clear`, false));
        def(cur, "onChange", makeEvent(`${fullPath}.onChange`));
      }
    }
  }

  // Auto-report popup document content size for anchored OSR popups (Chrome limits: 25..800 x 25..600).
  if (isPopup && typeof document !== "undefined") {
    const reportSize = () => {
      const de = document.documentElement;
      const body = document.body;
      if (!de) return;
      const w = Math.max(de.scrollWidth || 0, body?.scrollWidth || 0, Math.ceil(de.getBoundingClientRect().width || 0));
      const h = Math.max(de.scrollHeight || 0, body?.scrollHeight || 0, Math.ceil(de.getBoundingClientRect().height || 0));
      if (w >= 25 && h >= 25) {
        sendRaw({ op: "popup_size", width: Math.min(800, w), height: Math.min(600, h) });
      }
    };
    const hook = () => {
      reportSize();
      if (typeof ResizeObserver !== "undefined") {
        const ro = new ResizeObserver(reportSize);
        if (document.documentElement) ro.observe(document.documentElement);
        if (document.body) ro.observe(document.body);
      }
    };
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", hook, { once: true });
    } else {
      hook();
    }
    window.addEventListener("load", reportSize);
  }

  // Expose native getCurrent once for seed probe pages if needed.
  if (nativeTabsGetCurrent) {
    globalThis.__ocNativeGetCurrent = nativeTabsGetCurrent;
  }

  sendRaw({ op: "shim_ready" });
  return "OK";
})();
