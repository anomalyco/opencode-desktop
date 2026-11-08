//! A steer's Pending label: it never rides the bubble's flight, and delivery never shows the
//! model line it hides (`steers-pending`).

use super::{DT, Run};

/// Plays `secs`, checking each frame that every Pending label draws under its own row's
/// bubble, outside it, at that row's trailing edge, and that no prompt's model line shows
/// while nothing hovers it.
pub fn pending_holds(r: &mut Run, secs: f64) {
    for _ in 0..(secs / DT).round() as usize {
        r.play(DT);
        let els = r.rt.elements();
        for e in els.iter().filter(|e| !e.exiting && e.key.as_deref() == Some("pending")) {
            assert!(!e.path.contains("/bubble/"), "Pending rides the bubble: {}", e.path);
            let row = e.path.split('/').find(|p| p.starts_with("u-")).unwrap_or_default();
            let room = els.iter().find(|m| !m.exiting && m.key.as_deref() == Some("meta-room") && m.path.split('/').any(|p| p == row));
            let room = room.unwrap_or_else(|| panic!("Pending outside its row's meta room: {}", e.path));
            let (end, room_end) = (e.rect[0] + e.rect[2], room.rect[0] + room.rect[2]);
            assert!((end - room_end).abs() < 0.5, "Pending left its row's trailing edge: {end} vs {room_end}");
            assert!(e.rect[1] >= room.rect[1], "Pending above its row's meta room: {:?} vs {:?}", e.rect, room.rect);
        }
        for m in els.iter().filter(|e| !e.exiting && e.key.as_deref() == Some("meta-model")) {
            assert!(m.opacity < 0.01, "a model line shows with nothing hovered: {} at {}", m.path, m.opacity);
        }
    }
}
