//! `Esc`, and the world holds still.
//!
//! # Why pause is nearly free here
//!
//! The simulation advances only when ticks are *issued*. Movement runs off a
//! [`crate::movement::Ticker`] fed elapsed time, the mining fleet runs off its
//! own tick rate, and the day rolls forward on the frame's `dt` — so pausing
//! is not a matter of finding every system and telling it to stop. It is a
//! matter of handing the frame a `dt` of zero. Nothing is recorded, nothing
//! drifts, the journal never learns the player made coffee, and the world hash
//! on resume is the hash from the moment `Esc` was pressed.
//!
//! That is a claim worth testing rather than asserting, and `main` does test
//! it: a paused minute writes zero journal bytes and leaves the world hash
//! untouched.
//!
//! # What it carries
//!
//! Resume, Settings, Operator Panel, Save & Quit. The panel entry opens the
//! gold overlay that `F10` already owns — the same panel, reachable the way a
//! tester's thumb expects. It is present in every build for now because the
//! game *is* a test build; the release gating (`--gold`, compiled out) stays
//! in place as machinery, so on a build without the feature the entry simply
//! is not offered rather than opening nothing.
//!
//! Settings holds the feel toggles and the look sensitivity. It is four
//! switches and a slider standing where a settings screen will one day be,
//! and saying so is more honest than padding it.

use vx_render::font::{self, LINE_HEIGHT};

use crate::feel::FeelSettings;

/// Panel size in texture pixels, at the shop's display scale.
pub const PAUSE_WIDTH: u32 = 300;
pub const PAUSE_HEIGHT: u32 = 200;

const TEXT: [u8; 4] = [235, 235, 235, 255];
const DIM: [u8; 4] = [150, 150, 155, 255];
const GOLD: [u8; 4] = [255, 200, 40, 255];
const OFF: [u8; 4] = [130, 130, 138, 255];
const ON: [u8; 4] = [120, 220, 120, 255];
/// Dark and mostly opaque, but not entirely: the frozen world stays faintly
/// visible behind it, which is what makes a pause feel like a pause rather
/// than like the game having gone somewhere else.
const BACKGROUND: [u8; 4] = [8, 8, 12, 232];

/// The lowest and highest look sensitivity the slider offers, and the step it
/// moves in. The default (0.0022) sits a little below the middle.
pub const SENS_MIN: f32 = 0.0005;
pub const SENS_MAX: f32 = 0.0080;
pub const SENS_STEP: f32 = 0.0005;

/// Which screen the menu is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Root,
    Settings,
}

/// What activating a row asks the game to do.
///
/// The menu owns no mutation path of its own beyond its own settings: it
/// returns one of these and `main` carries it out, which keeps quitting and
/// panel-opening in the one place that already knows how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Close the menu and re-grab the cursor.
    Resume,
    /// Open the operator's console.
    OperatorPanel,
    /// Save the world and close the game.
    SaveAndQuit,
    /// Handled inside the menu; the caller need do nothing.
    Handled,
}

/// The root menu's entries, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Resume,
    Settings,
    OperatorPanel,
    SaveAndQuit,
}

impl Entry {
    pub fn title(self) -> &'static str {
        match self {
            Entry::Resume => "RESUME",
            Entry::Settings => "SETTINGS",
            Entry::OperatorPanel => "OPERATOR PANEL",
            Entry::SaveAndQuit => "SAVE & QUIT",
        }
    }
}

/// The settings rows, in order. The five feel toggles, then the slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    SprintFov,
    SlideFov,
    LandingDip,
    ViewBob,
    StrafeRoll,
    SlideTilt,
    Sensitivity,
    Back,
}

impl Setting {
    pub const ALL: [Setting; 8] = [
        Setting::SprintFov,
        Setting::SlideFov,
        Setting::LandingDip,
        Setting::ViewBob,
        Setting::StrafeRoll,
        Setting::SlideTilt,
        Setting::Sensitivity,
        Setting::Back,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Setting::SprintFov => "SPRINT FOV KICK",
            Setting::SlideFov => "SLIDE FOV KICK",
            Setting::LandingDip => "LANDING DIP",
            Setting::ViewBob => "VIEW BOB",
            Setting::StrafeRoll => "STRAFE ROLL",
            Setting::SlideTilt => "SLIDE TILT",
            Setting::Sensitivity => "LOOK SENSITIVITY",
            Setting::Back => "BACK",
        }
    }

    /// Which flag of [`FeelSettings`] this row switches, if any.
    fn flag(self, settings: &mut FeelSettings) -> Option<&mut bool> {
        match self {
            Setting::SprintFov => Some(&mut settings.sprint_fov),
            Setting::SlideFov => Some(&mut settings.slide_fov),
            Setting::LandingDip => Some(&mut settings.landing_dip),
            Setting::ViewBob => Some(&mut settings.view_bob),
            Setting::StrafeRoll => Some(&mut settings.strafe_roll),
            Setting::SlideTilt => Some(&mut settings.slide_tilt),
            Setting::Sensitivity | Setting::Back => None,
        }
    }
}

/// The pause menu's own state.
#[derive(Debug, Clone, Copy)]
pub struct Pause {
    pub open: bool,
    screen: Screen,
    /// Which row is focused, on whichever screen is showing.
    cursor: usize,
    /// The feel toggles live here so the menu is their one owner; `main`
    /// copies them onto the live `Feel` each frame.
    pub feel: FeelSettings,
    pub sensitivity: f32,
    /// True when the operator's console can actually be opened. A build with
    /// the panel compiled out does not offer the row at all, rather than
    /// offering one that does nothing.
    panel_available: bool,
}

impl Default for Pause {
    fn default() -> Self {
        Pause {
            open: false,
            screen: Screen::Root,
            cursor: 0,
            feel: FeelSettings::default(),
            sensitivity: 0.0022,
            panel_available: false,
        }
    }
}

impl Pause {
    pub fn new(panel_available: bool, sensitivity: f32) -> Self {
        Pause {
            sensitivity,
            panel_available,
            ..Pause::default()
        }
    }

    // Read by the tests, and by anything outside this module that wants to
    // know where the menu is standing. The drawing below reaches the fields
    // directly, being in the same module.
    #[allow(dead_code)]
    pub fn screen(&self) -> Screen {
        self.screen
    }

    #[allow(dead_code)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The root entries this build offers.
    pub fn entries(&self) -> Vec<Entry> {
        let mut entries = vec![Entry::Resume, Entry::Settings];
        if self.panel_available {
            entries.push(Entry::OperatorPanel);
        }
        entries.push(Entry::SaveAndQuit);
        entries
    }

    /// Open at the root, focus on Resume. Always the root: coming back to a
    /// pause menu still sitting in a submenu from ten minutes ago is a small
    /// unpleasant surprise.
    pub fn open(&mut self) {
        self.open = true;
        self.screen = Screen::Root;
        self.cursor = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.screen = Screen::Root;
        self.cursor = 0;
    }

    /// How many rows the current screen has.
    fn rows(&self) -> usize {
        match self.screen {
            Screen::Root => self.entries().len(),
            Screen::Settings => Setting::ALL.len(),
        }
    }

    /// Move the focus, wrapping at both ends.
    pub fn step(&mut self, delta: i32) {
        let rows = self.rows() as i32;
        if rows == 0 {
            return;
        }
        self.cursor = (self.cursor as i32 + delta).rem_euclid(rows) as usize;
    }

    /// Nudge the focused row sideways: the slider's left and right.
    ///
    /// Toggles answer to this too, so a left/right press on a switch does the
    /// obvious thing instead of nothing.
    pub fn nudge(&mut self, delta: i32) {
        if self.screen != Screen::Settings {
            return;
        }
        let row = Setting::ALL[self.cursor.min(Setting::ALL.len() - 1)];
        if row == Setting::Sensitivity {
            let moved = self.sensitivity + SENS_STEP * delta as f32;
            self.sensitivity = moved.clamp(SENS_MIN, SENS_MAX);
        } else if let Some(flag) = row.flag(&mut self.feel) {
            *flag = delta > 0;
        }
    }

    /// Activate the focused row.
    pub fn activate(&mut self) -> Action {
        match self.screen {
            Screen::Root => {
                let entries = self.entries();
                match entries[self.cursor.min(entries.len() - 1)] {
                    Entry::Resume => {
                        self.close();
                        Action::Resume
                    }
                    Entry::Settings => {
                        self.screen = Screen::Settings;
                        self.cursor = 0;
                        Action::Handled
                    }
                    Entry::OperatorPanel => {
                        self.close();
                        Action::OperatorPanel
                    }
                    Entry::SaveAndQuit => Action::SaveAndQuit,
                }
            }
            Screen::Settings => {
                let row = Setting::ALL[self.cursor.min(Setting::ALL.len() - 1)];
                match row {
                    Setting::Back => {
                        self.screen = Screen::Root;
                        self.cursor = 0;
                    }
                    Setting::Sensitivity => {}
                    other => {
                        if let Some(flag) = other.flag(&mut self.feel) {
                            *flag = !*flag;
                        }
                    }
                }
                Action::Handled
            }
        }
    }

    /// `Esc` again: out of the submenu, or out of the menu entirely.
    pub fn back(&mut self) -> Action {
        match self.screen {
            Screen::Settings => {
                self.screen = Screen::Root;
                self.cursor = 0;
                Action::Handled
            }
            Screen::Root => {
                self.close();
                Action::Resume
            }
        }
    }
}

/// Draw the menu into an RGBA buffer.
///
/// Reads nothing but the menu's own state, so a capture of a paused frame is
/// the same capture every time.
pub fn render_pause(pause: &Pause) -> Vec<u8> {
    let mut pixels = vec![0u8; (PAUSE_WIDTH * PAUSE_HEIGHT * 4) as usize];
    for texel in pixels.chunks_exact_mut(4) {
        texel.copy_from_slice(&BACKGROUND);
    }
    for x in 0..PAUSE_WIDTH {
        put(&mut pixels, x, 0, GOLD);
        put(&mut pixels, x, PAUSE_HEIGHT - 1, GOLD);
    }
    for y in 0..PAUSE_HEIGHT {
        put(&mut pixels, 0, y, GOLD);
        put(&mut pixels, PAUSE_WIDTH - 1, y, GOLD);
    }

    let margin = 12i32;
    let mut y = margin;

    let heading = match pause.screen {
        Screen::Root => "PAUSED",
        Screen::Settings => "SETTINGS",
    };
    font::draw_text(&mut pixels, PAUSE_WIDTH, margin, y, 2, GOLD, heading);
    y += (LINE_HEIGHT * 2) as i32 + 8;

    match pause.screen {
        Screen::Root => {
            for (index, entry) in pause.entries().into_iter().enumerate() {
                draw_row(&mut pixels, margin, y, index == pause.cursor, entry.title());
                y += LINE_HEIGHT as i32 + 6;
            }
        }
        Screen::Settings => {
            for (index, row) in Setting::ALL.into_iter().enumerate() {
                let focused = index == pause.cursor;
                draw_row(&mut pixels, margin, y, focused, row.title());

                // The value, right-aligned.
                let value = value_of(pause, row);
                if let Some((text, tint)) = value {
                    let x = PAUSE_WIDTH as i32
                        - margin
                        - font::text_width(&text, 1) as i32;
                    font::draw_text(&mut pixels, PAUSE_WIDTH, x, y, 1, tint, &text);
                }
                y += LINE_HEIGHT as i32 + 3;
            }
        }
    }

    pixels
}

/// The value column for a settings row: `ON`/`OFF`, or the sensitivity.
fn value_of(pause: &Pause, row: Setting) -> Option<(String, [u8; 4])> {
    match row {
        Setting::Back => None,
        Setting::Sensitivity => Some((
            // Displayed in the units a player can reason about rather than
            // radians per mouse count.
            format!("{:.0}", pause.sensitivity / SENS_STEP),
            TEXT,
        )),
        other => {
            let mut settings = pause.feel;
            let on = other.flag(&mut settings).map(|flag| *flag).unwrap_or(false);
            Some((
                if on { "ON".to_string() } else { "OFF".to_string() },
                if on { ON } else { OFF },
            ))
        }
    }
}

fn draw_row(pixels: &mut [u8], x: i32, y: i32, focused: bool, title: &str) {
    let tint = if focused { GOLD } else { DIM };
    if focused {
        font::draw_text(pixels, PAUSE_WIDTH, x, y, 1, GOLD, ">");
    }
    font::draw_text(pixels, PAUSE_WIDTH, x + 10, y, 1, tint, title);
}

fn put(pixels: &mut [u8], x: u32, y: u32, colour: [u8; 4]) {
    if x >= PAUSE_WIDTH || y >= PAUSE_HEIGHT {
        return;
    }
    let at = ((y * PAUSE_WIDTH + x) * 4) as usize;
    pixels[at..at + 4].copy_from_slice(&colour);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> Pause {
        Pause::new(true, 0.0022)
    }

    #[test]
    fn a_fresh_menu_is_closed() {
        assert!(!Pause::default().open);
    }

    #[test]
    fn opening_always_lands_on_resume_at_the_root() {
        let mut pause = menu();
        pause.open();
        pause.step(1);
        pause.activate(); // into Settings
        assert_eq!(pause.screen(), Screen::Settings);
        pause.close();

        // Re-opening does not resume where it left off: a menu that comes back
        // in a submenu from ten minutes ago is a small nasty surprise.
        pause.open();
        assert_eq!(pause.screen(), Screen::Root);
        assert_eq!(pause.cursor(), 0);
    }

    #[test]
    fn resume_closes_the_menu() {
        let mut pause = menu();
        pause.open();
        assert_eq!(pause.activate(), Action::Resume);
        assert!(!pause.open);
    }

    #[test]
    fn the_focus_wraps_at_both_ends() {
        let mut pause = menu();
        pause.open();
        let rows = pause.entries().len();

        pause.step(-1);
        assert_eq!(pause.cursor(), rows - 1, "stepping up from the top did not wrap");
        pause.step(1);
        assert_eq!(pause.cursor(), 0, "stepping down from the bottom did not wrap");
    }

    #[test]
    fn a_build_without_the_console_does_not_offer_the_row() {
        // The release gating stays real: an entry that opened nothing would be
        // worse than no entry.
        let plain = Pause::new(false, 0.0022);
        assert!(!plain.entries().contains(&Entry::OperatorPanel));
        assert!(plain.entries().contains(&Entry::SaveAndQuit));

        let dev = Pause::new(true, 0.0022);
        assert!(dev.entries().contains(&Entry::OperatorPanel));
    }

    #[test]
    fn the_operator_panel_entry_closes_the_menu_and_asks_for_the_console() {
        let mut pause = menu();
        pause.open();
        pause.step(2); // Resume, Settings, Operator Panel
        assert_eq!(pause.activate(), Action::OperatorPanel);
        assert!(!pause.open, "the menu stayed up over the console");
    }

    #[test]
    fn save_and_quit_leaves_the_menu_up_until_the_caller_acts() {
        // Quitting is the caller's to carry out; the menu must not close
        // itself first and leave a frame with neither menu nor game.
        let mut pause = menu();
        pause.open();
        pause.step(3);
        assert_eq!(pause.activate(), Action::SaveAndQuit);
        assert!(pause.open);
    }

    #[test]
    fn escape_backs_out_of_settings_before_it_closes_the_menu() {
        let mut pause = menu();
        pause.open();
        pause.step(1);
        pause.activate();
        assert_eq!(pause.screen(), Screen::Settings);

        assert_eq!(pause.back(), Action::Handled, "the first Esc left the menu entirely");
        assert_eq!(pause.screen(), Screen::Root);
        assert!(pause.open);

        assert_eq!(pause.back(), Action::Resume);
        assert!(!pause.open);
    }

    /// Put the menu on a given settings row.
    fn on_row(row: Setting) -> Pause {
        let mut pause = menu();
        pause.open();
        pause.step(1);
        pause.activate();
        let index = Setting::ALL.iter().position(|r| *r == row).unwrap();
        pause.cursor = index;
        pause
    }

    #[test]
    fn activating_a_toggle_flips_exactly_that_one() {
        let mut pause = on_row(Setting::ViewBob);
        let before = pause.feel;
        pause.activate();

        assert_ne!(pause.feel.view_bob, before.view_bob, "the toggle did not flip");
        assert_eq!(pause.feel.sprint_fov, before.sprint_fov);
        assert_eq!(pause.feel.landing_dip, before.landing_dip);
        assert_eq!(pause.feel.strafe_roll, before.strafe_roll);
        assert_eq!(pause.feel.slide_tilt, before.slide_tilt);

        // And back again.
        pause.activate();
        assert_eq!(pause.feel.view_bob, before.view_bob);
    }

    #[test]
    fn strafe_roll_starts_off_in_the_menu_too() {
        // The menu is the toggles' owner, so its default has to agree with the
        // feel layer's or the first frame would switch it on.
        assert!(!menu().feel.strafe_roll);
        assert_eq!(menu().feel, FeelSettings::default());
    }

    #[test]
    fn the_sensitivity_slider_moves_and_stops_at_its_ends() {
        let mut pause = on_row(Setting::Sensitivity);
        let start = pause.sensitivity;

        pause.nudge(1);
        assert!(pause.sensitivity > start, "the slider did not move right");
        pause.nudge(-1);
        assert!(
            (pause.sensitivity - start).abs() < 1e-9,
            "a step each way did not come home"
        );

        for _ in 0..200 {
            pause.nudge(1);
        }
        assert_eq!(pause.sensitivity, SENS_MAX, "the slider ran past its top");
        for _ in 0..200 {
            pause.nudge(-1);
        }
        assert_eq!(pause.sensitivity, SENS_MIN, "the slider ran past its bottom");
    }

    #[test]
    fn nudging_a_switch_sets_it_rather_than_toggling() {
        // Left is off, right is on — pressing right twice must not switch it
        // back off, which a toggle bound to both directions would.
        let mut pause = on_row(Setting::StrafeRoll);
        pause.nudge(1);
        assert!(pause.feel.strafe_roll);
        pause.nudge(1);
        assert!(pause.feel.strafe_roll, "a second right press switched it off");
        pause.nudge(-1);
        assert!(!pause.feel.strafe_roll);
    }

    #[test]
    fn the_slider_ignores_nudges_from_the_root_menu() {
        let mut pause = menu();
        pause.open();
        let before = pause.sensitivity;
        pause.nudge(1);
        assert_eq!(pause.sensitivity, before);
    }

    #[test]
    fn the_panel_draws_at_its_declared_size_on_both_screens() {
        let mut pause = menu();
        pause.open();
        let root = render_pause(&pause);
        assert_eq!(root.len(), (PAUSE_WIDTH * PAUSE_HEIGHT * 4) as usize);

        pause.step(1);
        pause.activate();
        let settings = render_pause(&pause);
        assert_eq!(settings.len(), root.len());
        assert_ne!(settings, root, "settings drew the same pixels as the root");
    }

    #[test]
    fn the_drawn_panel_shows_the_focus_moving() {
        let mut pause = menu();
        pause.open();
        let first = render_pause(&pause);
        pause.step(1);
        let second = render_pause(&pause);
        assert_ne!(first, second, "moving the focus changed nothing on screen");
    }

    /// The claim the whole design rests on, tested rather than asserted.
    ///
    /// The mechanism is a `dt` of zero, so what has to be shown is that every
    /// system fed by that `dt` genuinely stops: the ticker issues nothing, the
    /// journal grows by nothing, the clock does not move and the world hash is
    /// the hash from the moment `Esc` was pressed.
    #[test]
    fn a_paused_minute_issues_no_ticks_and_changes_nothing() {
        use crate::journal::CommandLog;
        use crate::movement::{self, MoveCommand, Movement, Ticker};
        use vx_core::ChunkPos;
        use vx_world::{world_hash, PlayerBody, World};

        let mut world = World::new(4242);
        world.load_around(ChunkPos::new(0, 0), 1);
        let mut body = PlayerBody {
            position: glam::Vec3::new(0.5, world.surface_y(0, 0).unwrap() as f32 + 1.0, 0.5),
            ..PlayerBody::default()
        };
        let mut movement = Movement::default();
        let mut ticker = Ticker::default();
        let mut journal = CommandLog::default();
        let mut clock = crate::clock::TimeOfDay::default();

        // Hold every key that would normally move the world along.
        let command = MoveCommand::looking(
            movement::FWD | movement::SPRINT | movement::JUMP,
            0.3,
            0.0,
        );

        let mut pause = menu();
        pause.open();

        let hash_before = world_hash(&world);
        let journal_before = journal.len();
        let tick_before = journal.tick();
        let clock_before = clock;
        let position_before = body.position;

        // A minute of frames at sixty a second, every one of them paused.
        for _ in 0..3600 {
            let dt = if pause.open { 0.0 } else { 1.0 / 60.0 };
            clock = clock.advance(dt);
            let ticks = ticker.take(dt);
            journal.record(crate::journal::Command::Advance { ticks });
            for _ in 0..ticks {
                movement.advance(&mut body, &world, command, command.mass(), movement::MOVE_TICK);
            }
        }

        assert_eq!(ticker.take(0.0), 0, "the ticker had work saved up");
        assert_eq!(journal.len(), journal_before, "the paused minute wrote to the journal");
        assert_eq!(journal.tick(), tick_before, "the paused minute advanced the tick");
        assert_eq!(world_hash(&world), hash_before, "the world changed while paused");
        assert_eq!(body.position, position_before, "the body moved while paused");
        assert_eq!(clock, clock_before, "the day rolled on while paused");

        // And the same loop unpaused does move: the test above must not be
        // passing because the harness itself does nothing.
        pause.close();
        for _ in 0..60 {
            let dt = if pause.open { 0.0 } else { 1.0 / 60.0 };
            clock = clock.advance(dt);
            let ticks = ticker.take(dt);
            for _ in 0..ticks {
                movement.advance(&mut body, &world, command, command.mass(), movement::MOVE_TICK);
            }
        }
        assert_ne!(body.position, position_before, "the unpaused second moved nothing");
        assert_ne!(clock, clock_before, "the unpaused second did not move the clock");
    }

    #[test]
    fn drawing_is_a_function_of_the_menu_alone() {
        // No wall clock, no counters: a paused frame captures the same twice.
        let mut pause = menu();
        pause.open();
        pause.step(1);
        assert_eq!(render_pause(&pause), render_pause(&pause));
    }
}
