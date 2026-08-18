//! The handheld: the fleet in your pocket.
//!
//! Two jobs, deliberately separable. *Viewing* puts the camera behind a
//! machine's eyes while it carries on working — you are a passenger. *Control*
//! is the master override: the machine drops what it was doing and answers to
//! the sticks. Keeping them apart means you can watch a drone work without
//! hijacking it, which is most of what you actually want to do.
//!
//! The panel itself is pixels, exactly like the shop and the HUD: a plain RGBA
//! buffer stamped with the bitmap font and shipped through an overlay slot.
//! The compositor knows nothing about what any of it means.

use vx_render::font::{self, LINE_HEIGHT};

use crate::mining::{MachineListing, MachineRef};

/// Panel size in texture pixels; drawn at [`DEVICE_SCALE`].
pub const DEVICE_WIDTH: u32 = 240;
pub const DEVICE_HEIGHT: u32 = 150;
pub const DEVICE_SCALE: f32 = 2.0;

/// The strip along the top of a live feed.
pub const BANNER_WIDTH: u32 = 240;
pub const BANNER_HEIGHT: u32 = 30;
pub const BANNER_SCALE: f32 = 2.0;

/// Distance at which the signal bar reads empty. Cosmetic, but honest: it is
/// a real measure of how far you have wandered from your own body.
pub const SIGNAL_RANGE: f32 = 320.0;

const TEXT: [u8; 4] = [235, 235, 235, 255];
const DIM: [u8; 4] = [150, 150, 155, 255];
const ACCENT: [u8; 4] = [255, 170, 60, 255];
const LIVE: [u8; 4] = [120, 220, 120, 255];
const BAR_BACK: [u8; 4] = [45, 48, 55, 255];
const BACKGROUND: [u8; 4] = [10, 12, 16, 235];
const BANNER_BACK: [u8; 4] = [10, 12, 16, 170];

/// The handheld's state.
#[derive(Debug, Default)]
pub struct Device {
    /// Is the roster panel up?
    pub open: bool,
    cursor: usize,
    /// The machine the camera is riding, if any.
    viewing: Option<MachineRef>,
    /// The machine actually taking orders, if any.
    piloting: Option<MachineRef>,
    /// The last action's outcome, shown until the next one.
    pub feedback: Option<String>,
}

impl Device {
    pub fn new() -> Self {
        Device::default()
    }

    pub fn open_list(&mut self) {
        self.open = true;
        self.feedback = None;
    }

    /// Close the panel. A live feed keeps running — you put the handheld down
    /// still watching, which is the point of it.
    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn move_cursor(&mut self, delta: i32, rows: usize) {
        if rows == 0 {
            self.cursor = 0;
            return;
        }
        self.cursor = (self.cursor as i32 + delta).clamp(0, rows as i32 - 1) as usize;
    }

    /// The machine the cursor is on.
    pub fn selected(&self, roster: &[MachineListing]) -> Option<MachineRef> {
        roster.get(self.cursor).map(|row| row.machine)
    }

    /// Ride a machine's camera without touching its controls.
    pub fn view(&mut self, machine: MachineRef) {
        self.viewing = Some(machine);
        self.open = false;
        self.feedback = None;
    }

    /// Master override on or off for whatever is being viewed.
    ///
    /// Returns `(machine, taking)` when the caller needs to actually take or
    /// release control, so the state here and the simulation cannot disagree.
    pub fn toggle_control(&mut self) -> Option<(MachineRef, bool)> {
        match (self.piloting, self.viewing) {
            (Some(machine), _) => {
                self.piloting = None;
                Some((machine, false))
            }
            (None, Some(machine)) => {
                self.piloting = Some(machine);
                self.open = false;
                Some((machine, true))
            }
            (None, None) => None,
        }
    }

    /// Whichever machine the camera should sit on.
    pub fn feed(&self) -> Option<MachineRef> {
        self.viewing
    }

    pub fn is_piloting(&self) -> bool {
        self.piloting.is_some()
    }

    /// Hang up. Returns the machine that needs handing back, if one was being
    /// driven.
    pub fn hand_back(&mut self) -> Option<MachineRef> {
        self.viewing = None;
        self.open = false;
        self.feedback = None;
        self.piloting.take()
    }
}

/// Signal strength from the pilot's distance to the machine, 0..1.
pub fn signal(distance: f32) -> f32 {
    (1.0 - distance / SIGNAL_RANGE).clamp(0.0, 1.0)
}

/// Paint a horizontal fill bar. Takes its rectangle as one value rather than
/// four loose numbers, which is also harder to pass in the wrong order.
fn draw_bar(
    pixels: &mut [u8],
    stride: u32,
    rect: (u32, u32, u32, u32),
    fraction: f32,
    colour: [u8; 4],
) {
    let (x, y, width, height) = rect;
    let filled = (width as f32 * fraction.clamp(0.0, 1.0)) as u32;
    for py in y..y + height {
        for px in x..x + width {
            let at = ((py * stride + px) * 4) as usize;
            if at + 4 > pixels.len() {
                continue;
            }
            let texel = if px < x + filled { colour } else { BAR_BACK };
            pixels[at..at + 4].copy_from_slice(&texel);
        }
    }
}

/// Draw the roster panel.
pub fn render_device(device: &Device, roster: &[MachineListing]) -> Vec<u8> {
    let mut pixels = vec![0u8; (DEVICE_WIDTH * DEVICE_HEIGHT * 4) as usize];
    for texel in pixels.chunks_exact_mut(4) {
        texel.copy_from_slice(&BACKGROUND);
    }

    let margin = 6i32;
    let mut y = margin;
    font::draw_text(&mut pixels, DEVICE_WIDTH, margin, y, 1, ACCENT, "FLEET UPLINK");
    y += LINE_HEIGHT as i32 + 3;

    if roster.is_empty() {
        font::draw_text(&mut pixels, DEVICE_WIDTH, margin, y, 1, DIM, "NO MACHINES IN RANGE");
    }

    for (index, row) in roster.iter().enumerate() {
        let selected = index == device.cursor;
        if selected {
            font::draw_text(&mut pixels, DEVICE_WIDTH, margin, y, 1, ACCENT, ">");
        }
        let colour = if selected { TEXT } else { DIM };
        let line = format!("{} {}", row.name, row.state);
        font::draw_text(&mut pixels, DEVICE_WIDTH, margin + 10, y, 1, colour, &line);
        let detail = format!("{:.0}M {}/{}", row.distance, row.cargo, row.capacity);
        let at = DEVICE_WIDTH as i32 - margin - font::text_width(&detail, 1) as i32;
        font::draw_text(&mut pixels, DEVICE_WIDTH, at, y, 1, DIM, &detail);
        y += LINE_HEIGHT as i32;
    }

    if let Some(feedback) = &device.feedback {
        y += 3;
        font::draw_text(&mut pixels, DEVICE_WIDTH, margin, y, 1, LIVE, feedback);
    }

    font::draw_text(
        &mut pixels,
        DEVICE_WIDTH,
        margin,
        DEVICE_HEIGHT as i32 - LINE_HEIGHT as i32 - 2,
        1,
        DIM,
        "ENTER VIEWS. R TAKES OVER. V CLOSES.",
    );
    pixels
}

/// Draw the banner that sits over a live feed.
pub fn render_feed_banner(listing: &MachineListing, piloting: bool, signal: f32) -> Vec<u8> {
    let mut pixels = vec![0u8; (BANNER_WIDTH * BANNER_HEIGHT * 4) as usize];
    for texel in pixels.chunks_exact_mut(4) {
        texel.copy_from_slice(&BANNER_BACK);
    }

    let margin = 5i32;
    let heading = if piloting {
        format!("PILOTING {}", listing.name)
    } else {
        format!("FEED {}", listing.name)
    };
    let colour = if piloting { ACCENT } else { LIVE };
    font::draw_text(&mut pixels, BANNER_WIDTH, margin, margin, 1, colour, &heading);

    let detail = format!("{} CARGO {}/{}", listing.state, listing.cargo, listing.capacity);
    font::draw_text(
        &mut pixels,
        BANNER_WIDTH,
        margin,
        margin + LINE_HEIGHT as i32,
        1,
        TEXT,
        &detail,
    );

    // Signal strength: how far you have wandered from your own body.
    let label_width = font::text_width("SIG", 1) as i32;
    let bar_x = BANNER_WIDTH as i32 - margin - 60;
    font::draw_text(
        &mut pixels,
        BANNER_WIDTH,
        bar_x - label_width - 4,
        margin,
        1,
        DIM,
        "SIG",
    );
    draw_bar(
        &mut pixels,
        BANNER_WIDTH,
        (bar_x as u32, margin as u32 + 1, 60, 5),
        signal,
        if signal > 0.25 { LIVE } else { ACCENT },
    );
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster() -> Vec<MachineListing> {
        vec![
            MachineListing {
                machine: MachineRef::Digger(0),
                name: "DIGGER 1".into(),
                state: "DIGGING".into(),
                distance: 42.0,
                cargo: 18,
                capacity: 64,
            },
            MachineListing {
                machine: MachineRef::Flier(0),
                name: "FLIER 1".into(),
                state: "SCANNING".into(),
                distance: 96.0,
                cargo: 0,
                capacity: 96,
            },
        ]
    }

    #[test]
    fn the_cursor_clamps_to_the_roster() {
        let mut device = Device::new();
        device.move_cursor(9, 2);
        assert_eq!(device.cursor, 1);
        device.move_cursor(-9, 2);
        assert_eq!(device.cursor, 0);
        // An empty roster must not index anything.
        device.move_cursor(3, 0);
        assert_eq!(device.cursor, 0);
        assert_eq!(device.selected(&[]), None);
    }

    #[test]
    fn viewing_does_not_take_control_but_the_override_does() {
        let mut device = Device::new();
        let roster = roster();
        device.open_list();
        device.move_cursor(1, roster.len());
        let machine = device.selected(&roster).unwrap();
        assert_eq!(machine, MachineRef::Flier(0));

        device.view(machine);
        assert_eq!(device.feed(), Some(machine));
        assert!(!device.is_piloting(), "merely watching took the wheel");
        assert!(!device.open, "viewing should put the panel away");

        assert_eq!(device.toggle_control(), Some((machine, true)));
        assert!(device.is_piloting());
        assert_eq!(device.feed(), Some(machine), "control lost the feed");

        // And back off again, still watching.
        assert_eq!(device.toggle_control(), Some((machine, false)));
        assert!(!device.is_piloting());
        assert_eq!(device.feed(), Some(machine));
    }

    #[test]
    fn the_override_does_nothing_with_no_feed_to_take_over() {
        let mut device = Device::new();
        device.open_list();
        assert_eq!(device.toggle_control(), None);
        assert!(!device.is_piloting());
    }

    #[test]
    fn hand_back_clears_both_the_feed_and_the_control() {
        let mut device = Device::new();
        let machine = MachineRef::Digger(0);
        device.view(machine);
        device.toggle_control();

        assert_eq!(device.hand_back(), Some(machine), "the caller was not told to release");
        assert_eq!(device.feed(), None);
        assert!(!device.is_piloting());
        assert!(!device.open);

        // Hanging up twice is harmless and asks for no second release.
        assert_eq!(device.hand_back(), None);
    }

    #[test]
    fn the_signal_falls_off_with_distance() {
        assert_eq!(signal(0.0), 1.0);
        assert!(signal(SIGNAL_RANGE * 0.5) < 1.0);
        assert_eq!(signal(SIGNAL_RANGE * 2.0), 0.0, "signal went negative");
    }

    #[test]
    fn the_panel_renders_at_its_declared_size_and_is_deterministic() {
        let mut device = Device::new();
        device.open_list();
        let roster = roster();

        let a = render_device(&device, &roster);
        let b = render_device(&device, &roster);
        assert_eq!(a.len(), (DEVICE_WIDTH * DEVICE_HEIGHT * 4) as usize);
        assert_eq!(a, b);

        // An empty fleet reads differently from a stocked one.
        assert_ne!(a, render_device(&device, &[]));

        // And moving the cursor is visible.
        device.move_cursor(1, roster.len());
        assert_ne!(a, render_device(&device, &roster));
    }

    #[test]
    fn the_banner_says_whether_you_are_driving_or_just_watching() {
        let roster = roster();
        let watching = render_feed_banner(&roster[0], false, 1.0);
        let driving = render_feed_banner(&roster[0], true, 1.0);
        assert_eq!(watching.len(), (BANNER_WIDTH * BANNER_HEIGHT * 4) as usize);
        assert_ne!(watching, driving, "the banner does not distinguish the two");

        // And the signal bar actually moves.
        assert_ne!(driving, render_feed_banner(&roster[0], true, 0.1));
    }

    #[test]
    fn every_panel_string_is_drawable_by_the_font() {
        // The font has no '$' and no lower case; anything it cannot draw would
        // silently render as a placeholder box.
        let mut device = Device::new();
        device.open_list();
        device.feedback = Some("CONTROL TAKEN".into());
        let roster = roster();
        for text in [
            "FLEET UPLINK",
            "NO MACHINES IN RANGE",
            "ENTER VIEWS. R TAKES OVER. V CLOSES.",
            "SIG",
            "PILOTING DIGGER 1",
            "FEED DIGGER 1",
            "CONTROL TAKEN",
            &format!("{} {}", roster[0].name, roster[0].state),
            &format!("{:.0}M {}/{}", roster[0].distance, roster[0].cargo, roster[0].capacity),
        ] {
            for character in text.chars() {
                assert!(
                    font::glyph(character) != font::glyph('\u{0}') || character == ' ',
                    "the font cannot draw {character:?} in {text:?}"
                );
            }
        }
    }
}
