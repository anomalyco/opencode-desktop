//! A plugin slot's lifecycle as an explicit state machine (`plugins-design.md` §2.5 A).
//!
//! ```text
//! Discovered ─┬─ UnsupportedOs / ReservedId
//!             ├─ Disabled ──enable──┐
//!             └─ LoadingLib ◄───────┴──── Failed (retry)
//!                  ├─ Failed (dlopen, ABI magic/epoch/layout mismatch)
//!                  └─ PreparingStores ─ SettingUp ─┬─ Active ─┬─ Reloading ─ Active
//!                                                  └─ Failed  ├─ Disposing ─ Disabled
//!                                                             └─ Failed (handler or poll panic)
//! ```
//! A render panic doesn't leave `Active`: the host rolls back that render, shows an error card
//! and keeps the plugin running.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PluginState {
    #[default]
    Discovered,
    UnsupportedOs,
    ReservedId,
    Disabled,
    LoadingLib,
    PreparingStores,
    SettingUp,
    Active,
    Reloading,
    Disposing,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    WrongOs,
    Reserved,
    UserDisabled,
    Load,
    LibFailed,
    LibLoaded,
    StoresReady,
    SetupOk,
    SetupFailed,
    Reload,
    ReloadSettled,
    Dispose,
    Disposed,
    Fault,
}

impl PluginState {
    /// The state `event` moves to, or `None` when it doesn't apply here.
    pub fn next(self, event: Event) -> Option<PluginState> {
        use Event as E;
        use PluginState as S;
        Some(match (self, event) {
            (S::Discovered, E::WrongOs) => S::UnsupportedOs,
            (S::Discovered, E::Reserved) => S::ReservedId,
            (S::Discovered | S::Failed, E::UserDisabled) => S::Disabled,
            (S::Discovered | S::Disabled | S::Failed, E::Load) => S::LoadingLib,
            (S::LoadingLib, E::LibFailed) => S::Failed,
            (S::LoadingLib, E::LibLoaded) => S::PreparingStores,
            (S::PreparingStores, E::StoresReady) => S::SettingUp,
            (S::SettingUp, E::SetupOk) => S::Active,
            (S::SettingUp, E::SetupFailed) => S::Failed,
            (S::Active, E::Reload) => S::Reloading,
            (S::Reloading, E::ReloadSettled) => S::Active,
            (S::Active, E::Dispose) => S::Disposing,
            (S::Disposing, E::Disposed) => S::Disabled,
            (S::Active, E::Fault) => S::Failed,
            _ => return None,
        })
    }

    /// The word Settings shows on the plugin's row.
    pub fn label(self) -> &'static str {
        match self {
            Self::Discovered => "Found",
            Self::UnsupportedOs => "Not for this OS",
            Self::ReservedId => "Reserved id",
            Self::Disabled => "Off",
            Self::LoadingLib | Self::PreparingStores | Self::SettingUp => "Loading",
            Self::Active => "On",
            Self::Reloading => "Reloading",
            Self::Disposing => "Turning off",
            Self::Failed => "Failed",
        }
    }
}
