//! The heads-up display: a bottom-left panel of words and bars.
//!
//! First real user of the bitmap font. Everything that used to go to the log
//! — mining status, ferry status — now lands where the player is looking,
//! along with the skill sheet and the drill's progress. The panel is a plain
//! RGBA buffer stamped with text and rectangles, shipped through overlay slot
//! 1; the compositor knows nothing about what the pixels mean, exactly like
//! the minimap in slot 0.

use vx_render::font::{self, LINE_HEIGHT};

use crate::clock::TimeOfDay;
use crate::skills::{self, Skills};

/// Panel size in texture pixels. Displayed 2x, so text is comfortably
/// readable without a big texture.
pub const HUD_WIDTH: u32 = 220;
pub const HUD_HEIGHT: u32 = 64;

/// On-screen scale factor for the panel.
pub const HUD_SCALE: f32 = 2.0;

const TEXT: [u8; 4] = [235, 235, 235, 255];
const DIM: [u8; 4] = [150, 150, 155, 255];
const ACCENT: [u8; 4] = [255, 170, 60, 255];
const BAR_BACK: [u8; 4] = [45, 48, 55, 255];
const BACKGROUND: [u8; 4] = [12, 14, 18, 165];
const NIGHT: [u8; 4] = [130, 150, 200, 255];

/// Everything the HUD shows this frame.
pub struct HudContent<'a> {
    pub skills: &'a Skills,
    /// The hour, passed in rather than read: the HUD is a pure function of
    /// its inputs, and a wall-clock read here would make captures wobble.
    pub time: TimeOfDay,
    /// The one-line activity readout (mining/ferrying/scanning), if any.
    pub status: Option<String>,
    /// Drill progress through the current block, while digging.
    pub drilling: Option<f32>,
    /// A recent level-up to celebrate: (skill, new level).
    pub level_up: Option<(String, u32)>,
    /// A villager's line, freshly spoken.
    pub greeting: Option<String>,
    /// True while the body waits for its own ground to stream back in after a
    /// feed. Worth saying out loud: the controls are briefly dead on purpose.
    pub reconnecting: bool,
}

/// Paint a horizontal bar with a filled fraction.
fn draw_bar(pixels: &mut [u8], x: u32, y: u32, width: u32, height: u32, fraction: f32, colour: [u8; 4]) {
    let filled = (width as f32 * fraction.clamp(0.0, 1.0)) as u32;
    for py in y..y + height {
        for px in x..x + width {
            let at = ((py * HUD_WIDTH + px) * 4) as usize;
            let texel = if px < x + filled { colour } else { BAR_BACK };
            pixels[at..at + 4].copy_from_slice(&texel);
        }
    }
}

/// Render the panel. Deterministic in its inputs, which is what the tests
/// lean on.
pub fn render_hud(content: &HudContent) -> Vec<u8> {
    let mut pixels = vec![0u8; (HUD_WIDTH * HUD_HEIGHT * 4) as usize];
    for texel in pixels.chunks_exact_mut(4) {
        texel.copy_from_slice(&BACKGROUND);
    }

    let margin = 5i32;
    let mut y = margin;

    // Line 1: the skill sheet.
    let sheet = format!(
        "MIN {}  PRO {}  LOG {}",
        content.skills.level(skills::MINING),
        content.skills.level(skills::PROSPECTING),
        content.skills.level(skills::LOGISTICS),
    );
    font::draw_text(&mut pixels, HUD_WIDTH, margin, y, 1, TEXT, &sheet);

    // The hour, right-aligned on the same line, tinted by whether the town is
    // open for business.
    let (hours, minutes) = content.time.hhmm();
    let clock = format!("{hours:02}:{minutes:02}");
    let tint = if content.time.is_daylight() { ACCENT } else { NIGHT };
    font::draw_text(
        &mut pixels,
        HUD_WIDTH,
        HUD_WIDTH as i32 - margin - font::text_width(&clock, 1) as i32,
        y,
        1,
        tint,
        &clock,
    );
    y += LINE_HEIGHT as i32;

    // Line 2: the XP bar for whatever skill last moved.
    if let Some(recent) = content.skills.recent() {
        let xp = content.skills.xp(recent);
        let label: String = recent.chars().take(3).collect();
        font::draw_text(&mut pixels, HUD_WIDTH, margin, y, 1, DIM, &label);
        draw_bar(
            &mut pixels,
            (margin + 24) as u32,
            y as u32 + 1,
            HUD_WIDTH - 34,
            5,
            skills::progress_to_next(xp),
            ACCENT,
        );
    }
    y += LINE_HEIGHT as i32;

    // Line 3: reconnecting outranks everything — the player's controls are
    // dead for a moment and they should know why — then a level-up shout, a
    // villager's word, and finally the activity readout.
    if content.reconnecting {
        font::draw_text(&mut pixels, HUD_WIDTH, margin, y, 1, ACCENT, "RECONNECTING");
    } else if let Some((skill, level)) = &content.level_up {
        let line = format!("{} LEVEL {level}!", skill.to_uppercase());
        font::draw_text(&mut pixels, HUD_WIDTH, margin, y, 1, ACCENT, &line);
    } else if let Some(greeting) = &content.greeting {
        font::draw_text(&mut pixels, HUD_WIDTH, margin, y, 1, ACCENT, greeting);
    } else if let Some(status) = &content.status {
        font::draw_text(&mut pixels, HUD_WIDTH, margin, y, 1, TEXT, status);
    }
    y += LINE_HEIGHT as i32;

    // Line 4: the drill.
    if let Some(progress) = content.drilling {
        let line = format!("DRILLING {:.0}%", progress.clamp(0.0, 1.0) * 100.0);
        font::draw_text(&mut pixels, HUD_WIDTH, margin, y, 1, TEXT, &line);
        draw_bar(
            &mut pixels,
            (margin + font::text_width(&line, 1) as i32 + 6) as u32,
            y as u32 + 1,
            60,
            5,
            progress,
            ACCENT,
        );
    }

    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_content(skills: &Skills) -> HudContent<'_> {
        HudContent {
            skills,
            time: TimeOfDay::NOON,
            status: None,
            drilling: None,
            level_up: None,
            greeting: None,
            reconnecting: false,
        }
    }

    #[test]
    fn reconnecting_outranks_every_other_line() {
        let skills = Skills::new();
        let mut busy = base_content(&skills);
        busy.status = Some("MINING 4 JOBS LEFT".into());
        busy.greeting = Some("MORNIN".into());
        busy.level_up = Some(("mining".into(), 9));

        let mut waiting = base_content(&skills);
        waiting.reconnecting = true;

        let mut waiting_busy = busy;
        waiting_busy.reconnecting = true;
        assert_eq!(
            render_hud(&waiting_busy),
            render_hud(&waiting),
            "something drew over the reconnecting notice"
        );
    }

    #[test]
    fn a_greeting_shows_and_yields_to_a_level_up() {
        let skills = Skills::new();
        let plain = base_content(&skills);
        let mut greeted = base_content(&skills);
        greeted.greeting = Some("MORNIN. FINE DAY FOR DIGGIN.".into());
        assert_ne!(render_hud(&plain), render_hud(&greeted));

        let mut both = base_content(&skills);
        both.greeting = Some("MORNIN. FINE DAY FOR DIGGIN.".into());
        both.level_up = Some(("mining".into(), 3));
        let mut level_only = base_content(&skills);
        level_only.level_up = Some(("mining".into(), 3));
        assert_eq!(render_hud(&both), render_hud(&level_only));
    }

    #[test]
    fn the_panel_is_the_declared_size_and_deterministic() {
        let skills = Skills::new();
        let a = render_hud(&base_content(&skills));
        let b = render_hud(&base_content(&skills));
        assert_eq!(a.len(), (HUD_WIDTH * HUD_HEIGHT * 4) as usize);
        assert_eq!(a, b);
    }

    #[test]
    fn levels_change_the_pixels() {
        let fresh = Skills::new();
        let mut veteran = Skills::new();
        veteran.add_xp(crate::skills::MINING, 50_000);

        let a = render_hud(&base_content(&fresh));
        let b = render_hud(&base_content(&veteran));
        assert_ne!(a, b, "a different level drew the same panel");
    }

    #[test]
    fn drilling_progress_shows_and_moves() {
        let skills = Skills::new();
        let mut early = base_content(&skills);
        early.drilling = Some(0.2);
        let mut late = base_content(&skills);
        late.drilling = Some(0.9);

        let a = render_hud(&early);
        let b = render_hud(&late);
        assert_ne!(a, b, "the drill bar did not move");
    }

    #[test]
    fn a_level_up_outranks_the_status_line() {
        let skills = Skills::new();
        let mut with_status = base_content(&skills);
        with_status.status = Some("FERRYING 40 ABOARD".into());
        let mut with_both = base_content(&skills);
        with_both.status = Some("FERRYING 40 ABOARD".into());
        with_both.level_up = Some(("mining".into(), 7));

        assert_ne!(render_hud(&with_status), render_hud(&with_both));
    }

    #[test]
    fn the_hud_shows_the_hour_and_tells_day_from_night() {
        let skills = Skills::new();
        let mut noon = base_content(&skills);
        noon.time = TimeOfDay::NOON;
        let mut midnight = base_content(&skills);
        midnight.time = TimeOfDay::MIDNIGHT;

        let day = render_hud(&noon);
        let night = render_hud(&midnight);
        assert_ne!(day, night, "the clock does not show on the panel");
        // And the panel stays a pure function of its inputs.
        assert_eq!(day, render_hud(&noon));
    }
}
