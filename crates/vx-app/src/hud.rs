//! The heads-up display and the menus.
//!
//! Pure layout: every function here takes state and an [`OverlayBuilder`] and
//! draws into it. Nothing touches the GPU, the window or the world, so the
//! whole UI can be exercised in tests by inspecting the geometry it emits.
//!
//! The look is deliberate — phosphor green on black, a bitmap face, scanlines,
//! everything shouting in uppercase. It is a monitor in a dark room in 1995.

use glam::Vec3;
use vx_core::BlockPos;
use vx_render::OverlayBuilder;

/// Phosphor green, the colour everything is by default.
pub const GREEN: [f32; 4] = [0.0, 1.0, 0.25, 1.0];
/// Burnt-in older text and secondary labels.
pub const DIM: [f32; 4] = [0.0, 0.55, 0.16, 1.0];
/// The bright core of a freshly lit pixel.
pub const BRIGHT: [f32; 4] = [0.72, 1.0, 0.80, 1.0];
/// Panel fill: not quite black, tinted toward the phosphor.
pub const PANEL: [f32; 4] = [0.01, 0.05, 0.02, 0.92];
/// Full-screen dim behind a menu.
pub const SHADE: [f32; 4] = [0.0, 0.02, 0.01, 0.78];
/// Text drawn on top of a lit selection bar.
pub const INVERSE: [f32; 4] = [0.02, 0.10, 0.03, 1.0];
/// The scanline overlay.
pub const SCANLINE: [f32; 4] = [0.0, 0.0, 0.0, 0.16];
/// A limit being hit. Amber rather than green: it should not look routine.
pub const ALERT: [f32; 4] = [1.0, 0.72, 0.10, 1.0];

/// What the player is looking at, ready to print.
pub struct Target {
    pub position: BlockPos,
    pub name: String,
}

/// Everything the HUD reports.
pub struct HudState {
    pub fps: f32,
    pub camera: Vec3,
    pub chunks_loaded: usize,
    pub chunks_meshed: usize,
    pub triangles: u32,
    pub seed: u64,
    pub hotbar: Vec<String>,
    pub selected_slot: usize,
    pub target: Option<Target>,
    /// Occupied inventory slots as display labels.
    pub inventory_lines: Vec<String>,
    /// Recipe labels, paired with whether they are currently craftable.
    pub recipes: Vec<(String, bool)>,
    /// "WALK" or "FLY", for the status block.
    pub mode: &'static str,
    /// Simulation counters. These exist to make the engine's resource
    /// ceilings visible: a limit that is silently absorbed looks identical to
    /// one that is never reached.
    pub sim: SimStats,
}

/// What the simulation is doing, and what it is having to refuse.
#[derive(Debug, Clone, Copy, Default)]
pub struct SimStats {
    /// Scheduled ticks waiting to run.
    pub pending_ticks: usize,
    /// Block updates waiting to be examined.
    pub pending_updates: usize,
    /// Schedule requests turned away because a ceiling was reached.
    pub refused: u64,
    /// Notifications discarded because the queue was full.
    pub dropped: u64,
    /// Simulation steps abandoned to keep catch-up bounded.
    pub skipped: u64,
}

impl SimStats {
    /// True when any ceiling has been hit. Worth showing loudly: it means the
    /// world is producing more work than the engine will accept.
    pub fn strained(&self) -> bool {
        self.refused > 0 || self.dropped > 0 || self.skipped > 0
    }
}

/// Which screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// No menu; the world has input.
    Playing,
    Main,
    Controls,
    World,
    /// Carried items and the crafting list.
    Inventory,
}

/// What activating a menu entry asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Resume,
    Quit,
}

const MAIN_ITEMS: [&str; 4] = ["RESUME", "CONTROLS", "WORLD DATA", "DISCONNECT"];

/// Menu navigation state.
#[derive(Debug, Clone, Copy)]
pub struct Menus {
    screen: Screen,
    selected: usize,
    /// Entries on the current screen when they are not the static list —
    /// the recipe count, for the inventory screen.
    dynamic_count: usize,
}

impl Default for Menus {
    fn default() -> Self {
        Menus {
            screen: Screen::Playing,
            selected: 0,
            dynamic_count: 0,
        }
    }
}

impl Menus {
    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// True while a menu is showing, so the caller knows to stop feeding the
    /// world input and to release the pointer.
    pub fn is_open(&self) -> bool {
        self.screen != Screen::Playing
    }

    pub fn open(&mut self) {
        self.screen = Screen::Main;
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.screen = Screen::Playing;
        self.selected = 0;
    }

    /// Jump straight to a screen. Used to capture a given menu headlessly.
    pub fn set_screen(&mut self, screen: Screen) {
        self.screen = screen;
        self.selected = 0;
    }

    /// Open the inventory screen, with `recipe_count` selectable entries.
    pub fn open_inventory(&mut self, recipe_count: usize) {
        self.screen = Screen::Inventory;
        self.selected = 0;
        self.dynamic_count = recipe_count;
    }

    /// Entries on the current screen. Sub-screens are read-only, so they have
    /// none.
    pub fn items(&self) -> &'static [&'static str] {
        match self.screen {
            Screen::Main => &MAIN_ITEMS,
            _ => &[],
        }
    }

    /// Move the highlight, wrapping at both ends.
    pub fn move_selection(&mut self, delta: i32) {
        let count = match self.screen {
            Screen::Inventory => self.dynamic_count,
            _ => self.items().len(),
        };
        if count == 0 {
            return;
        }
        let next = (self.selected as i32 + delta).rem_euclid(count as i32);
        self.selected = next as usize;
    }

    /// Activate the highlighted entry.
    pub fn activate(&mut self) -> Option<MenuAction> {
        match self.screen {
            Screen::Main => match MAIN_ITEMS.get(self.selected).copied() {
                Some("RESUME") => {
                    self.close();
                    Some(MenuAction::Resume)
                }
                Some("CONTROLS") => {
                    self.screen = Screen::Controls;
                    None
                }
                Some("WORLD DATA") => {
                    self.screen = Screen::World;
                    None
                }
                Some("DISCONNECT") => Some(MenuAction::Quit),
                _ => None,
            },
            // Crafting is handled by the app, which owns the inventory.
            Screen::Inventory => None,
            // A sub-screen has nothing to activate; treat it as "go back".
            Screen::Controls | Screen::World => {
                self.back();
                None
            }
            Screen::Playing => None,
        }
    }

    /// Step back one level: sub-screen to main menu, main menu to the world.
    /// The inventory is not part of the pause menu, so it closes outright.
    pub fn back(&mut self) {
        match self.screen {
            Screen::Inventory => self.close(),
            Screen::Controls | Screen::World => {
                self.screen = Screen::Main;
                self.selected = 0;
            }
            Screen::Main => self.close(),
            Screen::Playing => {}
        }
    }
}

/// Draw the crosshair: a gapped cross, so it marks the centre without hiding
/// the block underneath it.
pub fn draw_crosshair(ui: &mut OverlayBuilder) {
    let (width, height) = ui.size();
    let (cx, cy) = (width / 2.0, height / 2.0);
    let scale = ui.scale();

    let arm = 5.0 * scale;
    let gap = 2.0 * scale;
    let thickness = scale.max(1.0);

    // A dark backing one pixel out on each side keeps the cross readable
    // against bright terrain as well as dark.
    for (colour, bleed) in [([0.0, 0.0, 0.0, 0.55], thickness), (GREEN, 0.0)] {
        let t = thickness + bleed * 2.0;
        let offset = bleed;

        ui.rect(cx - gap - arm - offset, cy - t / 2.0, arm, t, colour);
        ui.rect(cx + gap - offset, cy - t / 2.0, arm + 2.0 * offset, t, colour);
        ui.rect(cx - t / 2.0, cy - gap - arm - offset, t, arm, colour);
        ui.rect(cx - t / 2.0, cy + gap - offset, t, arm + 2.0 * offset, colour);
    }
}

/// Horizontal CRT scanlines across the whole frame.
pub fn draw_scanlines(ui: &mut OverlayBuilder) {
    let (width, height) = ui.size();
    let step = (2.0 * ui.scale()).max(2.0);

    let mut y = 0.0;
    while y < height {
        ui.rect(0.0, y, width, step / 2.0, SCANLINE);
        y += step;
    }
}

/// The in-world readout: status block, target line and hotbar.
pub fn draw_hud(ui: &mut OverlayBuilder, state: &HudState) {
    let (width, height) = ui.size();
    let scale = ui.scale();
    let pad = 6.0 * scale;
    let line = ui.line_height();

    // Status block, upper left, styled as terminal output.
    let lines = [
        format!("> GAMINGG // {} FPS // {}", state.fps.round() as i32, state.mode),
        format!(
            "> POS {} {} {}",
            state.camera.x.floor() as i32,
            state.camera.y.floor() as i32,
            state.camera.z.floor() as i32
        ),
        format!("> CHUNK {}/{}", state.chunks_meshed, state.chunks_loaded),
        format!("> TRIS {}", state.triangles),
        format!(
            "> SIM Q{} U{}",
            state.sim.pending_ticks, state.sim.pending_updates
        ),
    ];
    // Backing panel. Dim green on a sunlit hillside is close to invisible, and
    // the readout has to stay legible over whatever the world puts behind it.
    let widest = lines
        .iter()
        .map(|text| ui.text_width(text))
        .fold(0.0, f32::max);
    let rows = lines.len() as f32 + if state.sim.strained() { 1.0 } else { 0.0 };
    ui.rect(pad / 2.0, pad / 2.0, widest + pad, line * rows + pad, PANEL);

    for (index, text) in lines.iter().enumerate() {
        let colour = if index == 0 { GREEN } else { DIM };
        ui.text(pad, pad + index as f32 * line, text, colour);
    }

    // Only shown when a ceiling has actually been hit, so it reads as a
    // condition rather than as decoration.
    if state.sim.strained() {
        ui.text(
            pad,
            pad + lines.len() as f32 * line,
            &format!(
                "! LIMIT R{} D{} S{}",
                state.sim.refused, state.sim.dropped, state.sim.skipped
            ),
            ALERT,
        );
    }

    // What the crosshair is on, just under the centre so the eye finds it
    // without leaving the middle of the screen.
    if let Some(target) = &state.target {
        let text = format!(
            "[ {} ] {} {} {}",
            target.name, target.position.x, target.position.y, target.position.z
        );
        ui.text_centred(width / 2.0, height / 2.0 + 14.0 * scale, &text, GREEN);
    }

    draw_hotbar(ui, state);
}

/// The block selector along the bottom.
fn draw_hotbar(ui: &mut OverlayBuilder, state: &HudState) {
    if state.hotbar.is_empty() {
        return;
    }

    let (width, height) = ui.size();
    let scale = ui.scale();
    let line = ui.line_height();
    let pad = 3.0 * scale;

    let labels: Vec<String> = state
        .hotbar
        .iter()
        .enumerate()
        .map(|(index, name)| format!("{}:{}", index + 1, name))
        .collect();

    let widths: Vec<f32> = labels
        .iter()
        .map(|label| ui.text_width(label) + pad * 2.0)
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + pad * (labels.len() as f32 - 1.0);

    let mut x = (width - total) / 2.0;
    let y = height - line - pad * 3.0;

    for (index, label) in labels.iter().enumerate() {
        let w = widths[index];
        let held = index == state.selected_slot;

        if held {
            // The held slot inverts: lit bar, dark text. Unmissable at a
            // glance, which is the whole job of a hotbar.
            ui.rect(x, y - pad, w, line + pad, GREEN);
            ui.text(x + pad, y - pad / 2.0, label, INVERSE);
        } else {
            ui.rect(x, y - pad, w, line + pad, PANEL);
            ui.rect_outline(x, y - pad, w, line + pad, scale, DIM);
            ui.text(x + pad, y - pad / 2.0, label, DIM);
        }

        x += w + pad;
    }
}

/// Draw whichever menu screen is open. Does nothing while playing.
pub fn draw_menu(ui: &mut OverlayBuilder, menus: &Menus, state: &HudState) {
    if !menus.is_open() {
        return;
    }

    let (width, height) = ui.size();
    let scale = ui.scale();
    ui.rect(0.0, 0.0, width, height, SHADE);

    let panel_w = (52.0 * (ui.scale() * 6.0)).min(width - 16.0 * scale);
    let panel_h = (height * 0.72).min(height - 16.0 * scale);
    let panel_x = (width - panel_w) / 2.0;
    let panel_y = (height - panel_h) / 2.0;

    ui.rect(panel_x, panel_y, panel_w, panel_h, PANEL);
    // Double rule: a bright frame with a dim one inside it reads as a bevelled
    // terminal window rather than a flat box.
    ui.rect_outline(panel_x, panel_y, panel_w, panel_h, scale, GREEN);
    ui.rect_outline(
        panel_x + 2.0 * scale,
        panel_y + 2.0 * scale,
        panel_w - 4.0 * scale,
        panel_h - 4.0 * scale,
        scale,
        DIM,
    );

    let centre = width / 2.0;
    let inner_x = panel_x + 6.0 * scale;
    let mut y = panel_y + 7.0 * scale;

    // Heading, at double size.
    ui.set_scale(scale * 2.0);
    ui.text_centred(centre, y, "GAMINGG", BRIGHT);
    y += ui.line_height();
    ui.set_scale(scale);

    ui.text_centred(centre, y, "DR.DOOM SYSTEMS // 1995", DIM);
    y += ui.line_height() * 1.5;
    ui.rect(inner_x, y, panel_w - 12.0 * scale, scale, DIM);
    y += ui.line_height();

    match menus.screen() {
        Screen::Main => draw_main_items(ui, menus, inner_x, y, panel_w - 12.0 * scale),
        Screen::Controls => draw_controls(ui, inner_x, y),
        Screen::World => draw_world(ui, inner_x, y, state),
        Screen::Inventory => {
            draw_inventory(ui, menus, state, inner_x, y, panel_w - 12.0 * scale)
        }
        Screen::Playing => {}
    }

    // Footer hint, pinned to the bottom of the panel.
    let footer = match menus.screen() {
        Screen::Main => "[W/S] MOVE   [ENTER] EXEC   [ESC] RESUME",
        Screen::Inventory => "[W/S] PICK   [ENTER] MAKE   [E] CLOSE",
        _ => "[ESC] BACK",
    };
    // Clear of the double border, not tucked under it.
    ui.text_centred(
        centre,
        panel_y + panel_h - 7.0 * scale - ui.line_height(),
        footer,
        DIM,
    );
}

/// Carried stacks, then the crafting list with its selection bar.
fn draw_inventory(
    ui: &mut OverlayBuilder,
    menus: &Menus,
    state: &HudState,
    x: f32,
    mut y: f32,
    w: f32,
) {
    let scale = ui.scale();
    let line = ui.line_height();

    ui.text(x, y, "CARRYING", DIM);
    y += line * 1.3;

    if state.inventory_lines.is_empty() {
        ui.text(x + 3.0 * scale, y, "NOTHING. GO DIG.", DIM);
        y += line;
    }
    // Two columns, so a fuller inventory stays inside the panel.
    let column_rows = 6;
    for (index, label) in state.inventory_lines.iter().take(column_rows * 2).enumerate() {
        let column = (index / column_rows) as f32;
        let row = (index % column_rows) as f32;
        ui.text(x + 3.0 * scale + column * (w / 2.0), y + row * line, label, GREEN);
    }
    if !state.inventory_lines.is_empty() {
        y += line * column_rows.min(state.inventory_lines.len()) as f32;
    }

    y += line * 0.6;
    ui.rect(x, y, w, scale, DIM);
    y += line;

    ui.text(x, y, "FABRICATE", DIM);
    y += line * 1.3;

    for (index, (label, craftable)) in state.recipes.iter().enumerate() {
        let held = index == menus.selected();
        if held {
            ui.rect(x, y - scale, w, line, if *craftable { GREEN } else { DIM });
            ui.text(x + 3.0 * scale, y, &format!("> {label}"), INVERSE);
        } else {
            let colour = if *craftable { GREEN } else { DIM };
            ui.text(x + 3.0 * scale, y, &format!("  {label}"), colour);
        }
        y += line * 1.4;
    }
}

fn draw_main_items(ui: &mut OverlayBuilder, menus: &Menus, x: f32, mut y: f32, w: f32) {
    let scale = ui.scale();
    let line = ui.line_height();

    for (index, item) in menus.items().iter().enumerate() {
        let held = index == menus.selected();
        if held {
            ui.rect(x, y - scale, w, line, GREEN);
            ui.text(x + 3.0 * scale, y, &format!("> {item}"), INVERSE);
        } else {
            ui.text(x + 3.0 * scale, y, &format!("  {item}"), GREEN);
        }
        y += line * 1.4;
    }
}

fn draw_controls(ui: &mut OverlayBuilder, x: f32, mut y: f32) {
    let line = ui.line_height();
    let rows = [
        ("WASD", "MOVE"),
        ("SPACE/LSHIFT", "UP / DOWN"),
        ("LCTRL", "SPRINT"),
        ("F", "WALK / FLY"),
        ("MOUSE", "LOOK"),
        ("LMB", "BREAK BLOCK"),
        ("RMB", "PLACE BLOCK"),
        ("1-9 / WHEEL", "SELECT BLOCK"),
        ("ESC", "MENU"),
    ];

    for (key, action) in rows {
        ui.text(x, y, key, BRIGHT);
        ui.text(x + ui.text_width("1-9 / WHEEL  "), y, action, DIM);
        y += line * 1.25;
    }
}

fn draw_world(ui: &mut OverlayBuilder, x: f32, mut y: f32, state: &HudState) {
    let line = ui.line_height();
    let rows = [
        ("SEED", format!("{}", state.seed)),
        (
            "ORIGIN",
            format!(
                "{} {} {}",
                state.camera.x.floor() as i32,
                state.camera.y.floor() as i32,
                state.camera.z.floor() as i32
            ),
        ),
        ("CHUNKS", format!("{}", state.chunks_loaded)),
        ("MESHED", format!("{}", state.chunks_meshed)),
        ("TRIANGLES", format!("{}", state.triangles)),
        ("HEIGHT", format!("{}", vx_core::CHUNK_HEIGHT)),
    ];

    for (label, value) in rows {
        ui.text(x, y, label, DIM);
        ui.text(x + ui.text_width("TRIANGLES  "), y, &value, GREEN);
        y += line * 1.25;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> HudState {
        HudState {
            fps: 60.0,
            camera: Vec3::new(1.0, 70.0, -2.0),
            chunks_loaded: 100,
            chunks_meshed: 90,
            triangles: 1234,
            seed: 2024,
            hotbar: vec!["STONE".into(), "DIRT".into(), "GRASS".into()],
            selected_slot: 0,
            target: None,
            inventory_lines: vec!["STONE x12".into()],
            recipes: vec![("3 STONE + 1 COAL = 1 LAMP".into(), true)],
            mode: "WALK",
            sim: SimStats::default(),
        }
    }

    fn ui() -> OverlayBuilder {
        OverlayBuilder::new(640, 360, 2.0)
    }

    #[test]
    fn a_new_session_starts_in_the_world_not_a_menu() {
        let menus = Menus::default();
        assert!(!menus.is_open());
        assert_eq!(menus.screen(), Screen::Playing);
    }

    #[test]
    fn opening_and_closing_toggles_world_input() {
        let mut menus = Menus::default();
        menus.open();
        assert!(menus.is_open());
        assert_eq!(menus.screen(), Screen::Main);

        menus.close();
        assert!(!menus.is_open());
    }

    #[test]
    fn selection_wraps_in_both_directions() {
        let mut menus = Menus::default();
        menus.open();
        let count = menus.items().len();

        menus.move_selection(-1);
        assert_eq!(menus.selected(), count - 1, "up from the top should wrap");

        menus.move_selection(1);
        assert_eq!(menus.selected(), 0);
    }

    #[test]
    fn selection_does_nothing_on_a_screen_with_no_items() {
        let mut menus = Menus::default();
        menus.open();
        menus.screen = Screen::Controls;

        menus.move_selection(1);
        assert_eq!(menus.selected(), 0);
    }

    #[test]
    fn resume_closes_the_menu_and_reports_it() {
        let mut menus = Menus::default();
        menus.open();
        assert_eq!(menus.activate(), Some(MenuAction::Resume));
        assert!(!menus.is_open());
    }

    #[test]
    fn sub_screens_open_and_step_back_to_the_main_menu() {
        let mut menus = Menus::default();
        menus.open();
        menus.move_selection(1); // CONTROLS

        assert_eq!(menus.activate(), None);
        assert_eq!(menus.screen(), Screen::Controls);

        menus.back();
        assert_eq!(menus.screen(), Screen::Main);
        // Escaping again leaves the menu entirely.
        menus.back();
        assert!(!menus.is_open());
    }

    #[test]
    fn disconnect_asks_to_quit_without_closing_first() {
        // Closing before the caller acts would drop a frame of world input
        // between the choice and the shutdown.
        let mut menus = Menus::default();
        menus.open();
        menus.move_selection(-1); // last item
        assert_eq!(menus.activate(), Some(MenuAction::Quit));
    }

    #[test]
    fn the_crosshair_leaves_the_exact_centre_clear() {
        // A solid dot would hide the block being targeted.
        let mut ui = ui();
        draw_crosshair(&mut ui);
        assert!(!ui.is_empty());

        let (w, h) = ui.size();
        let centre = [(w / 2.0 / w) * 2.0 - 1.0, 1.0 - (h / 2.0 / h) * 2.0];
        let covered = ui.vertices().chunks_exact(4).any(|quad| {
            let xs: Vec<f32> = quad.iter().map(|v| v.position()[0]).collect();
            let ys: Vec<f32> = quad.iter().map(|v| v.position()[1]).collect();
            let x0 = xs.iter().cloned().fold(f32::INFINITY, f32::min);
            let x1 = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let y0 = ys.iter().cloned().fold(f32::INFINITY, f32::min);
            let y1 = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            x0 < centre[0] && centre[0] < x1 && y0 < centre[1] && centre[1] < y1
        });
        assert!(!covered, "the crosshair covers the point it marks");
    }

    #[test]
    fn the_hud_draws_something_for_every_reading() {
        let mut ui = ui();
        draw_hud(&mut ui, &state());
        assert!(!ui.is_empty());
        assert_eq!(ui.indices().len() % 6, 0);
    }

    #[test]
    fn a_target_adds_geometry_and_no_target_does_not() {
        let mut without = ui();
        draw_hud(&mut without, &state());

        let mut with = ui();
        let mut state = state();
        state.target = Some(Target {
            position: BlockPos::new(1, 2, 3),
            name: "STONE".into(),
        });
        draw_hud(&mut with, &state);

        assert!(
            with.vertices().len() > without.vertices().len(),
            "the target readout drew nothing"
        );
    }

    #[test]
    fn an_empty_hotbar_is_skipped_rather_than_dividing_by_zero() {
        let mut ui = ui();
        let mut state = state();
        state.hotbar.clear();
        draw_hud(&mut ui, &state);
        // Still drew the status block, just no selector.
        assert!(!ui.is_empty());
    }

    #[test]
    fn nothing_is_drawn_for_the_menu_while_playing() {
        let mut ui = ui();
        draw_menu(&mut ui, &Menus::default(), &state());
        assert!(ui.is_empty());
    }

    #[test]
    fn every_menu_screen_draws_and_stays_on_screen() {
        for screen in [Screen::Main, Screen::Controls, Screen::World, Screen::Inventory] {
            let mut menus = Menus::default();
            menus.open();
            menus.screen = screen;

            let mut ui = ui();
            draw_menu(&mut ui, &menus, &state());

            assert!(!ui.is_empty(), "{screen:?} drew nothing");
            // Clip space is -1..1; anything outside is off the display.
            for vertex in ui.vertices() {
                assert!(
                    (-1.05..=1.05).contains(&vertex.position()[0])
                        && (-1.05..=1.05).contains(&vertex.position()[1]),
                    "{screen:?} drew outside the screen at {:?}",
                    vertex.position()
                );
            }
        }
    }

    #[test]
    fn the_heading_scale_is_restored_for_the_body() {
        // set_scale is a mode, so forgetting to restore it would render the
        // whole menu body at heading size and overflow the panel.
        let mut menus = Menus::default();
        menus.open();
        let mut ui = ui();
        let before = ui.scale();

        draw_menu(&mut ui, &menus, &state());

        assert_eq!(ui.scale(), before);
    }

    #[test]
    fn scanlines_cover_the_frame_without_filling_it() {
        let mut ui = ui();
        draw_scanlines(&mut ui);

        let quads = ui.vertices().len() / 4;
        assert!(quads > 10, "only {quads} scanlines");
        // Every line is translucent, so the world stays visible through them.
        assert!(ui.vertices().iter().all(|v| v.colour()[3] < 0.5));
    }
}
