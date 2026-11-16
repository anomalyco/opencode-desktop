// Gives the page's warming workers (index.html) what they call: the functions the page runs
// first, in the order it first runs them, split into lanes (one worker each), in a table of their
// own (`__warm`), with the stack pointer (`__warm_sp`) and each lane's length and each function's
// parameter types (custom section `rune.warm`).
//
// Chromium compiles a WebAssembly function the first time it runs, on the thread that runs it:
// about 7,000 functions, some 180 ms of the page's thread, before the first frame, and 700 more
// under the first click and key. Workers given the same module share its compiled code with the
// page. Each calls its lane's functions with zeros and the stack pointer near 0: a function
// compiles, then traps at its first write to its stack frame, or runs to its end on zeros. What
// runs never reaches the page: each worker's instance has its own memory and throwing imports.
//
// The list (apps/web/warm.txt) holds keys of the functions' names, so it outlives a build; a name the
// module no longer has is skipped, and a function new to the path compiles on the page as before.
// operation-detox's harness/web/warm-list.mjs makes it from wasm-bindgen's output (names kept): the order from a
// trace of the page, and without the functions that ran on (or ran long) when called this way.
//
// Usage: node (or bun) scripts/web-warm.mjs <app_bg.wasm> <warm.txt> [--keep-names]
import { readFileSync, writeFileSync } from 'node:fs';

const [path, listPath] = process.argv.slice(2);
const keepNames = process.argv.includes('--keep-names');
const bytes = readFileSync(path);

let p = 0;
const u32 = () => {
  let result = 0, shift = 0, byte;
  do { byte = bytes[p++]; result += (byte & 0x7f) * 2 ** shift; shift += 7; } while (byte & 0x80);
  return result;
};
const s33 = () => { let byte; do { byte = bytes[p++]; } while (byte & 0x80); };
const leb = (n) => { const out = []; do { let b = n & 0x7f; n = Math.floor(n / 128); if (n) b |= 0x80; out.push(b); } while (n); return out; };
const name = () => { const n = u32(); const s = bytes.toString('utf8', p, p + n); p += n; return s; };
const limits = () => { const flags = bytes[p++]; u32(); if (flags & 1) u32(); };

const sections = [];
p = 8;
while (p < bytes.length) {
  const head = p, id = bytes[p++], size = u32(), start = p;
  const custom = id === 0 ? name() : undefined;
  sections.push({ id, head, start, end: start + size, custom });
  p = start + size;
}
const find = (id, custom) => sections.find(s => s.id === id && s.custom === custom);

const lanes = readLanes();
const warm = resolve();
writeFileSync(path, rebuild(warm));
if (warm) console.log(`web-warm: ${warm.lanes.map(l => l.length).join(' + ')} functions to warm in ${warm.lanes.length} lanes (${warm.missing} listed names not in this module)`);

// The list's lanes: `# lane` starts one, and each other line is a function's key.
function readLanes() {
  const lanes = [];
  for (const line of readFileSync(listPath, 'utf8').split('\n').map(l => l.trim()).filter(Boolean)) {
    if (line.startsWith('# lane')) lanes.push([]);
    else lanes.at(-1)?.push(line);
  }
  return lanes;
}

// The lanes as function indices, with each type's parameters and the stack pointer's global; or
// nothing, with a warning, when the module isn't one this can add to (the page then compiles as
// it always has).
function resolve() {
  const skip = (why) => { console.warn(`web-warm: ${why}; nothing to warm`); return undefined; };
  // Each type's parameters, one letter each: i32 i, i64 I, f32 f, f64 d, v128 v, a reference r.
  const types = [];
  const typeSection = find(1);
  if (typeSection) {
    p = typeSection.start;
    for (let n = u32(), i = 0; i < n; i++) {
      if (bytes[p++] !== 0x60) return skip('a type that is not a plain function type');
      const params = [];
      for (const into of [params, null]) {
        for (let k = u32(), j = 0; j < k; j++) {
          const t = bytes[p++];
          if (t === 0x63 || t === 0x64) { s33(); into?.push('r'); continue; }
          const letter = ({ 0x7f: 'i', 0x7e: 'I', 0x7d: 'f', 0x7c: 'd', 0x7b: 'v' })[t] ?? (t >= 0x69 && t <= 0x74 ? 'r' : undefined);
          if (!letter) return skip(`value type 0x${t.toString(16)}`);
          into?.push(letter);
        }
      }
      types.push(params.join(''));
    }
  }
  // The function and table index spaces start with their imports.
  const funcTypes = [];
  let tables = 0;
  const importSection = find(2);
  if (importSection) {
    p = importSection.start;
    for (let n = u32(), i = 0; i < n; i++) {
      name(); name();
      const kind = bytes[p++];
      if (kind === 0x00) funcTypes.push(u32());
      else if (kind === 0x01) { p++; limits(); tables++; }
      else if (kind === 0x02) limits();
      else if (kind === 0x03) p += 2;
      else if (kind === 0x04) { p++; u32(); }
      else return skip(`import kind ${kind}`);
    }
  }
  const functionSection = find(3);
  if (functionSection) { p = functionSection.start; for (let n = u32(), i = 0; i < n; i++) funcTypes.push(u32()); }
  const tableSection = find(4);
  if (!tableSection || !find(7) || !find(9)) return skip('no table, export or element section to add to');
  if (bytes.includes(Buffer.from('\u0006__warm'))) return skip('the module already has its warming table');
  p = tableSection.start;
  tables += u32();
  // The functions' names and the stack pointer's global.
  const nameSection = find(0, 'name');
  if (!nameSection) return skip('no name section');
  const byName = new Map();
  let stackPointer = -1;
  p = nameSection.start;
  name();
  while (p < nameSection.end) {
    const sub = bytes[p++], size = u32(), end = p + size;
    if (sub === 1 || sub === 7) {
      for (let n = u32(), i = 0; i < n; i++) {
        const index = u32(), label = name();
        if (sub === 7 && label === '__stack_pointer') stackPointer = index;
        if (sub === 1) { const key = plain(label); byName.set(key, [...(byName.get(key) ?? []), index]); }
      }
    }
    p = end;
  }
  if (stackPointer < 0) return skip('no __stack_pointer global');
  // A function whose parameters can't come from JavaScript (v128) is left out, and each goes in
  // the first lane that names it.
  const seen = new Set();
  let missing = 0;
  const resolved = lanes.map(names => names.flatMap(label => {
    const indices = byName.get(label);
    if (!indices) { missing++; return []; }
    return indices.filter(i => !seen.has(i) && !types[funcTypes[i]].includes('v') && seen.add(i));
  }));
  if (!resolved.some(l => l.length)) return skip('no listed function is in this module');
  return { lanes: resolved, missing, tables, stackPointer, signatures: resolved.flat().map(i => types[funcTypes[i]]) };
}

// A function's key: two 32-bit FNV-1a hashes of its name without its crates' disambiguating hashes
// (`[0123456789abcdef]`), so a key holds while those change.
function plain(label) {
  const name = label.replace(/\[[0-9a-f]{16}\]/g, '');
  let a = 0x811c9dc5, b = 0x01000193 ^ 0x5bd1e995;
  for (let i = 0; i < name.length; i++) { const c = name.charCodeAt(i); a = Math.imul(a ^ c, 0x01000193); b = Math.imul(b ^ c, 0x01000193 + 2); }
  return (a >>> 0).toString(16).padStart(8, '0') + (b >>> 0).toString(16).padStart(8, '0');
}

function rebuild(warm) {
  const out = [bytes.subarray(0, 8)];
  const section = (id, ...body) => {
    const size = body.reduce((n, b) => n + b.length, 0);
    out.push(Buffer.from([id, ...leb(size)]), ...body);
  };
  // A vector section with `added` more entries after its own.
  const extend = (s, added, entries) => {
    p = s.start;
    const count = u32();
    section(s.id, Buffer.from(leb(count + added)), bytes.subarray(p, s.end), Buffer.from(entries));
  };
  const str = (s) => { const b = [...Buffer.from(s)]; return [...leb(b.length), ...b]; };
  const all = warm?.lanes.flat() ?? [];
  for (const s of sections) {
    if (s.id === 0 && s.custom === 'name' && !keepNames) continue;
    if (warm && s.id === 4) { extend(s, 1, [0x70, 0x01, ...leb(all.length), ...leb(all.length)]); continue; }
    if (warm && s.id === 9) { extend(s, 1, [0x02, ...leb(warm.tables), 0x41, 0x00, 0x0b, 0x00, ...leb(all.length), ...all.flatMap(leb)]); continue; }
    if (warm && s.id === 7) { extend(s, 2, [...str('__warm'), 0x01, ...leb(warm.tables), ...str('__warm_sp'), 0x03, ...leb(warm.stackPointer)]); continue; }
    out.push(bytes.subarray(s.head, s.end));
  }
  if (warm) section(0, Buffer.from(str('rune.warm')), Buffer.from(`${warm.lanes.map(l => l.length).join(' ')};${warm.signatures.join(',')}`));
  return Buffer.concat(out);
}
