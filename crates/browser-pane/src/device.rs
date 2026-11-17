//! Browser device viewport (`viewport-layout.ts` and `mobile-emulation.ts`, 1cb0633c8b).
//! Shared with the CEF executable so its wire types don't drift.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    #[default]
    Desktop,
    Phone,
    Tablet,
    WindowsTablet,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub width: u16,
    pub height: u16,
    pub preset: Option<String>,
    pub profile: Profile,
    /// None fits the available area; a manual scale may overflow it.
    pub scale: Option<f32>,
}

impl Device {
    pub fn fit(&self, area: [f32; 2]) -> f32 {
        self.scale.unwrap_or_else(|| {
            (area[0].floor().max(1.0) / self.width as f32)
                .min(area[1].floor().max(1.0) / self.height as f32)
                .min(1.0)
        })
    }
}
