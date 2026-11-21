// Injected by opencode-browser-engine via private in-process CDP pipe into chrome-extension://
// service workers and extension pages BEFORE line 1 of extension code executes.
//
// Bridges shimmed chrome.* / browser.* APIs to Rust via Runtime.addBinding("__ocExtCall") and
// receives replies + events via globalThis.__ocExtRecv(msg).
// Keeps all working native Chromium APIs (declarativeNetRequest, storage, scripting, alarms,
// offscreen, runtime, tabs.sendMessage, tabs.connect, webRequest, cookies) native!
(() => {
  // Capture primordials BEFORE any extension code executes so prototype pollution cannot intercept
  // `probe_tabs` / `sendRaw` or exfiltrate `agent_driven` / sibling tab metadata (READER-7 C-S4).
  const _defineProperty = Object.defineProperty;
  const _getOwnPropertyNames = Object.getOwnPropertyNames;
  const _hasOwn = Object.prototype.hasOwnProperty;
  const _create = Object.create;
  const _isArray = Array.isArray;
  const _jsonStringify = JSON.stringify;
  const _extCall = globalThis.__ocExtCall;
  const _Promise = Promise;

  const CHILD_FRAME_ALLOWED = new Set(["runtime", "i18n", "storage", "dom"]);
  const CHILD_ALWAYS_LOCK = [
    "tabs", "windows", "action", "browserAction", "pageAction", "contextMenus", "webNavigation",
    "downloads", "management", "notifications", "permissions", "history", "bookmarks", "sessions",
    "privacy", "fontSettings", "identity", "sidePanel", "search", "contentSettings",
    "desktopCapture", "tabCapture", "pageCapture", "webAuthenticationProxy", "browsingData",
    "system", "userScripts", "tts", "ttsEngine", "debugger", "proxy", "topSites", "readingList",
    "tabGroups", "omnibox", "devtools", "extension", "commands", "webRequest",
    "declarativeNetRequest", "scripting", "offscreen", "cookies", "alarms", "idle", "power",
    "app", "csi", "loadTimes", "gcm", "instanceID", "declarativeContent",
  ];
  const lockUndefined = (obj, prop) => {
    try {
      Object.defineProperty(obj, prop, { value: undefined, configurable: false, enumerable: false, writable: false });
    } catch {}
  };
  // DENY BY DEFAULT in child frames: walk own keys of `chrome`/`browser` and lock everything
  // outside `{runtime, i18n, storage, dom}` (plus `runtime.{connectNative,sendNativeMessage}`).
  const neutralizeChrome = (win) => {
    if (!win) return;
    for (const rootName of ["chrome", "browser"]) {
      let root;
      try { root = win[rootName]; } catch { continue; }
      if (!root || typeof root !== "object") continue;
      let keys = [];
      try { keys = Object.getOwnPropertyNames(root); } catch {}
      for (const k of keys) {
        if (!CHILD_FRAME_ALLOWED.has(k)) lockUndefined(root, k);
      }
      for (const api of CHILD_ALWAYS_LOCK) lockUndefined(root, api);
      try {
        const rt = root.runtime;
        if (rt && typeof rt === "object") {
          lockUndefined(rt, "connectNative");
          lockUndefined(rt, "sendNativeMessage");
        }
      } catch {}
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
    // Neutralize chrome.tabs/windows/etc. in ANY child subframe (about:blank, about:srcdoc,
    // same-origin iframe, <object>, <embed>) instead of leaving native APIs reachable (READER-12 C2).
    if (isSubframe || href === "about:blank" || href === "about:srcdoc" || origin === "null") {
      neutralizeChrome(globalThis);
      sweepFrames(globalThis);
      return "NEUTRALIZED";
    }
    if (!origin.startsWith("chrome-extension://")) {
      return "DENIED";
    }
  }
  // Must check `getManifest` BEFORE matching `chrome.runtime.id`: service workers' initial
  // `Runtime.executionContextCreated` fires before Chromium attaches `chrome.runtime`, so
  // returning `"WAIT"` keeps `cdp.rs` attached until `scriptFirstStatement` rather than detaching.
  if (!globalThis.chrome?.runtime?.getManifest) return "WAIT";
  if (globalThis.location) {
    const extId = globalThis.chrome?.runtime?.id;
    if (!extId || String(globalThis.location.origin || "") !== "chrome-extension://" + extId) {
      return "DENIED";
    }
  }
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
    for (const ctor of [
      globalThis.HTMLIFrameElement,
      globalThis.HTMLFrameElement,
      globalThis.HTMLObjectElement,
      globalThis.HTMLEmbedElement,
    ]) {
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
      patchMethod(ctor?.prototype, "getSVGDocument", (doc) => {
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
    for (const m of ["insertNode", "surroundContents"]) {
      patchMethod(globalThis.Range?.prototype, m, sweepAll);
    }
    for (const m of [
      "append",
      "prepend",
      "after",
      "before",
      "replaceWith",
      "replaceChildren",
      "insertAdjacentElement",
      "insertAdjacentHTML",
      "setHTMLUnsafe",
    ]) {
      patchMethod(globalThis.Element?.prototype, m, sweepAll);
    }
    for (const m of ["append", "prepend", "replaceChildren", "setHTMLUnsafe"]) {
      patchMethod(globalThis.ShadowRoot?.prototype, m, sweepAll);
    }
    for (const m of ["append", "prepend", "replaceChildren"]) {
      patchMethod(globalThis.Document?.prototype, m, sweepAll);
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
  // Merge Rust's verified declared permissions (which retain stripped native permissions such as
  // `downloads` and `management` so official stubs bind while Chromium C++ holds zero native grant).
  const declared = new Set([
    ...(manifest.permissions || []),
    ...(manifest.host_permissions || []),
    ...(__DECLARED_PERMS__ || []),
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
      // Uses captured primordials + null-prototype records so extension prototype pollution
      // (`Array.prototype.{push,map,sort}`, `Promise.all`, `JSON.stringify`, `Object.prototype`
      // setters) cannot observe `agent_driven` or sibling extension tabs (READER-7 C-S4).
      (async () => {
        const items = _create(null);
        let count = 0;
        if (nativeTabsGet && _isArray(msg.candidates)) {
          const len = msg.candidates.length >>> 0;
          const pending = [];
          for (let idx = 0; idx < len; idx++) {
            const id = msg.candidates[idx];
            if (typeof id !== "number") continue;
            pending[pending.length] = _Promise.resolve(nativeTabsGet(id)).then(
              (t) => {
                if (t && typeof t.id === "number") {
                  const rec = _create(null);
                  rec.id = t.id;
                  rec.url = typeof t.url === "string" ? t.url : "";
                  rec.title = typeof t.title === "string" ? t.title : "";
                  rec.width = typeof t.width === "number" ? t.width : 0;
                  rec.height = typeof t.height === "number" ? t.height : 0;
                  items[count++] = rec;
                }
              },
              () => {},
            );
          }
          for (let i = 0; i < pending.length; i++) {
            try { await pending[i]; } catch {}
          }
        }
        // Build sorted JSON payload without touching mutable Array.prototype methods.
        const out = [];
        for (let i = 0; i < count; i++) out[i] = items[i];
        for (let i = 1; i < out.length; i++) {
          const cur = out[i];
          let j = i - 1;
          while (j >= 0 && out[j].id > cur.id) {
            out[j + 1] = out[j];
            j--;
          }
          out[j + 1] = cur;
        }
        let jsonStr = `{"op":"probe_tabs_reply","probeId":${Number(msg.probeId) || 0},"found":[`;
        for (let i = 0; i < out.length; i++) {
          if (i > 0) jsonStr += ",";
          const r = out[i];
          jsonStr += `{"id":${r.id},"url":${_jsonStringify(r.url)},"title":${_jsonStringify(r.title)},"width":${r.width},"height":${r.height}}`;
        }
        jsonStr += "]}";
        if (typeof _extCall === "function") {
          _extCall(jsonStr);
        } else if (typeof globalThis.__ocExtCall === "function") {
          globalThis.__ocExtCall(jsonStr);
        }
      })();
    }
  };

  function sendRaw(obj) {
    const call = typeof _extCall === "function" ? _extCall : globalThis.__ocExtCall;
    if (typeof call === "function") {
      call(_jsonStringify(obj));
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

  function makeLocalEvent() {
    const set = new Set();
    return {
      addListener(fn) {
        if (typeof fn === "function") set.add(fn);
      },
      removeListener(fn) {
        set.delete(fn);
      },
      hasListener(fn) {
        return set.has(fn);
      },
      hasListeners() {
        return set.size > 0;
      },
    };
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

      const promise = invoke(name, clean(args)).then((value) =>
        name === "tabs.getCurrent" && value === null ? undefined : value,
      );
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
      stubEvents: true,
    },
    {
      path: "fontSettings",
      permissions: ["fontSettings"],
      methods: ["getFontList", "getFont", "setFont", "clearFont"],
    },
    {
      path: "history",
      permissions: ["history"],
      methods: ["search", "getVisits", "addUrl", "deleteUrl", "deleteRange", "deleteAll"],
      events: ["onVisited", "onVisitRemoved"],
      stubEvents: true,
    },
    {
      path: "bookmarks",
      permissions: ["bookmarks"],
      methods: [
        "get", "getChildren", "getRecent", "getTree", "getSubTree", "search",
        "create", "move", "update", "remove", "removeTree",
      ],
      events: [
        "onCreated", "onRemoved", "onChanged", "onMoved",
        "onChildrenReordered", "onImportBegan", "onImportEnded",
      ],
      stubEvents: true,
    },
    {
      path: "sessions",
      permissions: ["sessions"],
      methods: ["getRecentlyClosed", "getDevices", "restore"],
      events: ["onChanged"],
      constants: { MAX_SESSION_RESULTS: 25 },
      stubEvents: true,
    },
    {
      path: "search",
      permissions: ["search"],
      methods: ["query"],
    },
    {
      path: "downloads",
      permissions: ["downloads"],
      methods: [
        "download", "search", "pause", "resume", "cancel", "getFileIcon",
        "open", "show", "showDefaultFolder", "erase", "removeFile",
        "acceptDanger", "setUiOptions", "setShelfEnabled",
      ],
      events: ["onCreated", "onErased", "onChanged", "onDeterminingFilename"],
      stubEvents: true,
    },
    {
      // Unconditionally bound (Chromium exposes `management.getSelf`, `uninstallSelf`, and
      // `getPermissionWarningsByManifest` natively even without the `"management"` permission).
      path: "management",
      methods: [
        "getSelf", "getAll", "get", "setEnabled", "uninstall", "uninstallSelf",
        "launchApp", "createAppShortcut", "setLaunchType",
        "getPermissionWarningsById", "getPermissionWarningsByManifest", "generateAppForLink",
      ],
      events: ["onInstalled", "onUninstalled", "onEnabled", "onDisabled"],
      stubEvents: true,
    },
    {
      path: "identity",
      permissions: ["identity"],
      methods: [
        "getAuthToken", "getProfileUserInfo", "getAccounts",
        "removeCachedAuthToken", "clearAllCachedAuthTokens",
        "launchWebAuthFlow", "getRedirectURL",
      ],
      events: ["onSignInChanged"],
      stubEvents: true,
    },
    {
      path: "commands",
      methods: ["getAll"],
      events: ["onCommand", "onChanged"],
    },
    {
      // MV3 `webRequest` is stripped in C++ (matching Electron 44 where MV3 SW listeners hear no
      // requests and cannot observe `Default` agent-tab URLs/tabIds); expose inert no-op events
      // when declared so extensions like Bitwarden never throw at startup.
      path: "webRequest",
      permissions: ["webRequest", "webRequestBlocking"],
      methods: [],
      events: [
        "onBeforeRequest", "onBeforeSendHeaders", "onSendHeaders", "onHeadersReceived",
        "onResponseStarted", "onBeforeRedirect", "onCompleted", "onErrorOccurred",
      ],
      stubEvents: true,
    },
    {
      path: "extension",
      methods: ["isAllowedFileSchemeAccess", "isAllowedIncognitoAccess", "setUpdateUrlData"],
    },
  ];

  // Chromium exposes both `chrome` and `browser` (uBlock Origin Lite reads `browser` first!).
  const roots = [...new Set([globalThis.chrome, globalThis.browser].filter(Boolean))];

  // Match Electron 44: `runtime.connectNative` and `runtime.sendNativeMessage` do not exist
  // (non-configurable `undefined` on both `chrome.runtime` and `browser.runtime`, paired with
  // C++ `PermissionsData` stripping and `native_messaging.blocklist = ["*"]`).
  for (const root of roots) {
    try {
      const rt = root.runtime;
      if (rt && typeof rt === "object") {
        for (const m of ["connectNative", "sendNativeMessage"]) {
          try {
            Object.defineProperty(rt, m, { value: undefined, configurable: false, enumerable: false, writable: false });
          } catch {}
        }
      }
    } catch {}
  }

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
      eventObjs[ev] = ns.stubEvents ? makeLocalEvent() : makeEvent(`${ns.path}.${ev}`);
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
      // Sweep any remaining native methods AND native event objects (`onZoomChange`, `onAttached`,
      // `onDetached`, `onActiveChanged`, `onSelectionChanged`, etc.) on `chrome.tabs` /
      // `chrome.windows` so unlisted Chromium APIs cannot bypass our RPC/agent_driven guards
      // (READER-12 C2, READER-7 C-S5).
      if (ns.path === "tabs" || ns.path === "windows") {
        for (const prop of _getOwnPropertyNames(target)) {
          if (_hasOwn.call(members, prop)) continue;
          if (_hasOwn.call(eventObjs, prop)) continue;
          if (nativeAllow?.has(prop)) continue;
          let curVal;
          try { curVal = target[prop]; } catch { continue; }
          if (typeof curVal === "function") {
            def(target, prop, makeMethod(`${ns.path}.${prop}`, false));
          } else if (curVal && typeof curVal === "object" && typeof curVal.addListener === "function") {
            def(target, prop, makeLocalEvent());
          }
        }
      }
    }
  }

  // Stub all 16 `privacy.*` settings when `"privacy"` is declared (`stubs.ts:6-23`).
  if (declared.has("privacy")) {
    const PRIV = [
      "privacy.network.networkPredictionEnabled",
      "privacy.network.webRTCIPHandlingPolicy",
      "privacy.services.alternateErrorPagesEnabled",
      "privacy.services.autofillAddressEnabled",
      "privacy.services.autofillCreditCardEnabled",
      "privacy.services.autofillEnabled",
      "privacy.services.passwordSavingEnabled",
      "privacy.services.safeBrowsingEnabled",
      "privacy.services.searchSuggestEnabled",
      "privacy.services.spellingServiceEnabled",
      "privacy.services.translationServiceEnabled",
      "privacy.websites.hyperlinkAuditingEnabled",
      "privacy.websites.referrersEnabled",
      "privacy.websites.thirdPartyCookiesAllowed",
      "privacy.websites.doNotTrackEnabled",
      "privacy.websites.protectedContentEnabled",
    ];
    for (const fullPath of PRIV) {
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
        def(cur, "onChange", makeLocalEvent());
      }
    }
  }

  // DENY-BY-DEFAULT top-level namespace sweep: remove any Chromium `chrome.*` / `browser.*` API
  // outside the exact official Electron + Rune supported/stubbed allow-list, strip non-`display`
  // sub-APIs from `chrome.system`, and neutralize `webRequest.onAuthRequired` (P5).
  const TOP_FRAME_ALLOWED = new Set([
    "runtime", "i18n", "dom", "events", "types", "extensionTypes",
    "alarms", "cookies", "declarativeNetRequest", "idle", "offscreen", "power",
    "scripting", "storage", "system", "webRequest",
    "tabs", "windows", "action", "browserAction", "pageAction", "contextMenus",
    "webNavigation", "notifications", "permissions", "management", "commands",
    "sidePanel", "fontSettings", "history", "bookmarks", "sessions", "search",
    "downloads", "identity", "privacy", "extension",
  ]);
  for (const root of roots) {
    let keys = [];
    try { keys = Object.getOwnPropertyNames(root); } catch {}
    for (const k of keys) {
      if (!TOP_FRAME_ALLOWED.has(k)) lockUndefined(root, k);
    }
    try {
      const sys = root.system;
      if (sys && typeof sys === "object") {
        for (const sk of Object.getOwnPropertyNames(sys)) {
          if (sk !== "display") lockUndefined(sys, sk);
        }
        for (const sk of ["cpu", "memory", "storage"]) lockUndefined(sys, sk);
      }
    } catch {}
    try {
      const wr = root.webRequest;
      if (wr && typeof wr === "object") lockUndefined(wr, "onAuthRequired");
    } catch {}
    try {
      const dnr = root.declarativeNetRequest;
      if (dnr && typeof dnr === "object") lockUndefined(dnr, "onRuleMatchedDebug");
    } catch {}
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

  // Bind non-popup extension pages to their own Chromium tab ID so `chrome.tabs.getCurrent()`
  // matches official `tabs.ts:345-349` (do not leak `nativeTabsGetCurrent` onto `globalThis`, C-S4).
  if (nativeTabsGetCurrent && typeof globalThis.window !== "undefined" && !isPopup) {
    _Promise
      .resolve()
      .then(() => nativeTabsGetCurrent())
      .then((t) => {
        if (t && typeof t.id === "number" && t.id > 0) {
          sendRaw({ op: "self_tab", id: t.id });
        }
      })
      .catch(() => {});
  }

  sendRaw({ op: "shim_ready" });
  return "OK";
})();
