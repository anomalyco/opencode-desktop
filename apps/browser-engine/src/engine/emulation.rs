//! Chromium device identities and viewport geometry from `mobile-emulation.ts` (1cb0633c8b).
use crate::wire::device::{Device, Profile};
use serde_json::{Value, json};

pub(super) fn apply(device: Option<&Device>, scale: f32, mut send: impl FnMut(&str, Value)) {
    let profile = device.map_or(Profile::Desktop, |d| d.profile);
    let mobile = profile != Profile::Desktop;
    let agent = if mobile {
        let phone = profile == Profile::Phone;
        let windows = profile == Profile::WindowsTablet;
        let os = if windows {
            "Windows NT 10.0; Win64; x64"
        } else {
            "Linux; Android 10; K"
        };
        let suffix = if phone { " Mobile" } else { "" };
        let major = cef_dll_sys::CHROME_VERSION_MAJOR;
        json!({"userAgent": format!("Mozilla/5.0 ({os}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0{suffix} Safari/537.36"),
            "platform": if windows { "Win32" } else { "Linux armv8l" },
            "userAgentMetadata": {"platform": if windows { "Windows" } else { "Android" }, "platformVersion":"", "architecture":if windows { "x86" } else { "" }, "bitness":if windows { "64" } else { "" }, "model":"", "mobile":phone, "wow64":false, "formFactors":[if phone { "Mobile" } else { "Tablet" }]}})
    } else {
        json!({"userAgent":""})
    };
    send("Emulation.setUserAgentOverride", agent);
    send(
        "Emulation.setTouchEmulationEnabled",
        if mobile {
            json!({"enabled":true,"maxTouchPoints":5})
        } else {
            json!({"enabled":false})
        },
    );
    send(
        "Emulation.setEmitTouchEventsForMouse",
        json!({"enabled":mobile,"configuration":"mobile"}),
    );
    let Some(d) = device else {
        return send("Emulation.clearDeviceMetricsOverride", json!({}));
    };
    let mut metrics = json!({"width":d.width,"height":d.height,"scale":scale,"mobile":mobile,"deviceScaleFactor":0,"dontSetVisibleSize":true});
    if mobile {
        metrics["screenWidth"] = d.width.into();
        metrics["screenHeight"] = d.height.into();
        let landscape = d.width > d.height;
        metrics["screenOrientation"] = json!({"type":if landscape { "landscapePrimary" } else { "portraitPrimary" },"angle":if landscape { 90 } else { 0 }});
    }
    send("Emulation.setDeviceMetricsOverride", metrics);
}
