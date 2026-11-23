//! The host's own Luau, compiled once per process. Every plugin VM loads the same short pieces
//! of host Luau at its start (the `@opencode/plugin` module, `I18n`'s remembered answers, the
//! wrappers around native calls): about forty a VM, each parsed and compiled again from its text
//! by `Lua::load`. Several VMs start before a launch's first frame and the rest one per turn
//! after it, all on the app's thread, where compiling that text was 2.2 ms of the 7.4 ms five
//! starts took (strobe `rec_0e9acd`, Windows). [`load`] keeps each piece's bytecode after its
//! first compile, so the next VM loads it as it loads a built-in plugin's own modules.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use mlua::Lua;
use mlua::chunk::{Chunk, ChunkMode, Compiler};

/// Bytecode by the text's address and length: a `'static` text is the same text for the whole run.
static COMPILED: Mutex<BTreeMap<(usize, usize), Arc<[u8]>>> = Mutex::new(BTreeMap::new());

/// `lua.load(source)` for a piece of host Luau that never changes: compiled the first time it is
/// asked for, with the settings Rune compiles a script's modules with (`rune_luau`'s `compiler`),
/// and loaded from that bytecode from then on. Text that doesn't compile loads as text, so its
/// error reads as it always did.
pub fn load(lua: &Lua, source: &'static str) -> Chunk<'static> {
    let key = (source.as_ptr() as usize, source.len());
    let kept = COMPILED.lock().unwrap_or_else(PoisonError::into_inner).get(&key).cloned();
    let bytes = match kept {
        Some(bytes) => bytes,
        None => {
            let compiler = Compiler::new().set_optimization_level(2).set_type_info_level(1).set_debug_level(1);
            let Ok(bytes) = compiler.compile(source) else { return lua.load(source) };
            let bytes: Arc<[u8]> = bytes.into();
            COMPILED.lock().unwrap_or_else(PoisonError::into_inner).insert(key, bytes.clone());
            bytes
        }
    };
    lua.load(bytes.to_vec()).set_mode(ChunkMode::Binary)
}
