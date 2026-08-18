//! The beacon console: the panel the radio mast puts in front of you.
//!
//! The shop's twin (overlay slot 5), and deliberately so — the same cursor,
//! the same Enter-confirms, the same one-line feedback. What differs is where
//! the rows come from: the shop sells from a table, the board reads work off
//! [`crate::beacon::postings_for`], which is derived from the town rather than
//! stored anywhere.
//!
//! Settlement lives here rather than in [`crate::beacon`] because it is the
//! only place that has the pile, the wallet and the survey record to hand at
//! once; the ledger itself stays a record of what happened, not a rule about
//! what may.

use vx_agent::Stockpile;
use vx_render::font::{self, LINE_HEIGHT};
use vx_world::town::TownSite;

use crate::beacon::{Ledger, Posting, Task};
use crate::shop::display_name;
use crate::wallet::Wallet;

/// Panel size in texture pixels; displayed at [`BOARD_SCALE`].
pub const BOARD_WIDTH: u32 = 256;
pub const BOARD_HEIGHT: u32 = 156;
pub const BOARD_SCALE: f32 = 2.0;

const TEXT: [u8; 4] = [235, 235, 235, 255];
const DIM: [u8; 4] = [150, 150, 155, 255];
const ACCENT: [u8; 4] = [255, 170, 60, 255];
const GOOD: [u8; 4] = [120, 220, 120, 255];
const BACKGROUND: [u8; 4] = [10, 12, 16, 235];

/// One selectable line of the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// Work due here, ready to sign for.
    HandIn(u64),
    /// Work on offer here.
    Accept(u64),
    /// On offer here, but already on your sheet.
    Taken(u64),
}

impl Row {
    pub fn id(&self) -> u64 {
        match self {
            Row::HandIn(id) | Row::Accept(id) | Row::Taken(id) => *id,
        }
    }
}

/// The console's interaction state.
#[derive(Debug, Default)]
pub struct Board {
    pub open: bool,
    cursor: usize,
    /// The last action's outcome, shown until the next one.
    pub feedback: Option<String>,
}

impl Board {
    pub fn new() -> Self {
        Board::default()
    }

    pub fn open_at_beacon(&mut self) {
        self.open = true;
        self.cursor = 0;
        self.feedback = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.feedback = None;
    }

    /// The rows this console shows: work due here first, because arriving
    /// with a full pack and having to scroll past adverts would be rude.
    pub fn rows(here: (i32, i32), postings: &[Posting], ledger: &Ledger) -> Vec<Row> {
        let mut rows: Vec<Row> = ledger
            .due_at(here)
            .iter()
            .map(|posting| Row::HandIn(posting.id))
            .collect();
        for posting in postings {
            if ledger.is_settled(posting.id) {
                continue;
            }
            if ledger.is_accepted(posting.id) {
                // A posting settled at its issuer is already listed above.
                if !rows.iter().any(|row| row.id() == posting.id) {
                    rows.push(Row::Taken(posting.id));
                }
            } else {
                rows.push(Row::Accept(posting.id));
            }
        }
        rows
    }

    pub fn move_cursor(&mut self, delta: i32, rows: usize) {
        if rows == 0 {
            self.cursor = 0;
            return;
        }
        self.cursor = (self.cursor as i32 + delta).clamp(0, rows as i32 - 1) as usize;
    }

    /// Act on the selected row.
    ///
    /// `surveyed` answers "has the fleet swept the sector holding this
    /// column?" — passed in rather than read, so this stays testable without
    /// a fleet and the module keeps knowing nothing about fliers.
    pub fn confirm(
        &mut self,
        here: &TownSite,
        postings: &[Posting],
        ledger: &mut Ledger,
        pile: Option<&mut Stockpile>,
        walletbook: &mut Wallet,
        surveyed: &dyn Fn((i32, i32)) -> bool,
    ) {
        let rows = Board::rows(here.centre, postings, ledger);
        let Some(row) = rows.get(self.cursor).cloned() else {
            self.feedback = Some("NO WORK POSTED".into());
            return;
        };

        match row {
            Row::Taken(_) => {
                self.feedback = Some("ALREADY ON YOUR SHEET".into());
            }
            Row::Accept(id) => {
                let Some(posting) = postings.iter().find(|posting| posting.id == id) else {
                    self.feedback = Some("POSTING WITHDRAWN".into());
                    return;
                };
                if ledger.accept(posting) {
                    self.feedback = Some(match &posting.task {
                        Task::Deliver { target, .. } => {
                            format!("ACCEPTED. PINNED {}", target.name)
                        }
                        Task::Survey { .. } => "ACCEPTED. SEND THE FLIER".into(),
                    });
                } else {
                    self.feedback = Some("ALREADY ON YOUR SHEET".into());
                }
            }
            Row::HandIn(id) => {
                self.feedback = Some(settle(id, ledger, pile, walletbook, surveyed));
            }
        }

        let rows = Board::rows(here.centre, postings, ledger).len().max(1);
        self.cursor = self.cursor.min(rows - 1);
    }
}

/// Try to sign off one accepted posting. Reports what happened either way.
///
/// Atomic in the sense that matters: a contract that cannot be paid takes
/// nothing from the pile.
fn settle(
    id: u64,
    ledger: &mut Ledger,
    pile: Option<&mut Stockpile>,
    walletbook: &mut Wallet,
    surveyed: &dyn Fn((i32, i32)) -> bool,
) -> String {
    let Some(posting) = ledger.get(id).cloned() else {
        return "NOT ON YOUR SHEET".into();
    };

    match &posting.task {
        Task::Deliver {
            goods,
            amount,
            target,
        } => {
            let Some(pile) = pile else {
                return "NO BASE PILE".into();
            };
            let held = pile.count(goods);
            if held < *amount {
                return format!(
                    "NEED {amount} {}, HAVE {held}",
                    display_name(goods)
                );
            }
            // `take` reports what actually left, so the books balance even if
            // the pile changed under us.
            let taken = pile.take(goods, *amount);
            walletbook.earn(posting.reward);
            ledger.settle(id);
            format!(
                "SIGNED FOR {taken} {} AT {}. +{} CR",
                display_name(goods),
                target.name,
                posting.reward
            )
        }
        Task::Survey { at } => {
            if !surveyed(*at) {
                return "SECTOR NOT SWEPT YET".into();
            }
            walletbook.earn(posting.reward);
            ledger.settle(id);
            format!("SURVEY FILED. +{} CR", posting.reward)
        }
    }
}

/// Render the panel. Pure in its inputs.
pub fn render_board(
    board: &Board,
    here: &TownSite,
    postings: &[Posting],
    ledger: &Ledger,
    walletbook: &Wallet,
) -> Vec<u8> {
    let mut pixels = vec![0u8; (BOARD_WIDTH * BOARD_HEIGHT * 4) as usize];
    for texel in pixels.chunks_exact_mut(4) {
        texel.copy_from_slice(&BACKGROUND);
    }

    let margin = 6i32;
    let mut y = margin;
    let heading = format!("{} BEACON", here.name);
    font::draw_text(&mut pixels, BOARD_WIDTH, margin, y, 1, ACCENT, &heading);
    let credits = format!("CR {}", walletbook.credits());
    font::draw_text(
        &mut pixels,
        BOARD_WIDTH,
        BOARD_WIDTH as i32 - margin - font::text_width(&credits, 1) as i32,
        y,
        1,
        GOOD,
        &credits,
    );
    y += LINE_HEIGHT as i32;
    let place = format!(
        "{} AT {} {}",
        here.speciality.name(),
        here.centre.0,
        here.centre.1
    );
    font::draw_text(&mut pixels, BOARD_WIDTH, margin, y, 1, DIM, &place);
    y += LINE_HEIGHT as i32 + 3;

    let rows = Board::rows(here.centre, postings, ledger);
    if rows.is_empty() {
        font::draw_text(&mut pixels, BOARD_WIDTH, margin, y, 1, DIM, "NO WORK POSTED");
        y += LINE_HEIGHT as i32;
    }
    for (index, row) in rows.iter().enumerate() {
        let selected = index == board.cursor;
        if selected {
            font::draw_text(&mut pixels, BOARD_WIDTH, margin, y, 1, ACCENT, ">");
        }
        let posting = postings
            .iter()
            .find(|posting| posting.id == row.id())
            .or_else(|| ledger.get(row.id()));
        let Some(posting) = posting else { continue };

        let (prefix, colour) = match row {
            Row::HandIn(_) => ("SIGN", GOOD),
            Row::Accept(_) => ("TAKE", if selected { TEXT } else { DIM }),
            Row::Taken(_) => ("HELD", DIM),
        };
        let label = format!("{prefix} {}", posting.title());
        font::draw_text(&mut pixels, BOARD_WIDTH, margin + 10, y, 1, colour, &label);
        y += LINE_HEIGHT as i32;
        // Where it settles, and whether the player has any idea where that
        // is. "UNCHARTED" is the hook: the pin is on the map, the ground
        // around it is not.
        let settles = posting.settles_at();
        let place = if ledger.knows(settles) {
            format!("{} {}", settles.0, settles.1)
        } else {
            "UNCHARTED".to_string()
        };
        let pay = format!("{} CR - {place}", posting.reward);
        font::draw_text(&mut pixels, BOARD_WIDTH, margin + 18, y, 1, DIM, &pay);
        y += LINE_HEIGHT as i32;
    }

    if let Some(feedback) = &board.feedback {
        y += 3;
        font::draw_text(&mut pixels, BOARD_WIDTH, margin, y, 1, GOOD, feedback);
    }

    font::draw_text(
        &mut pixels,
        BOARD_WIDTH,
        margin,
        BOARD_HEIGHT as i32 - LINE_HEIGHT as i32 - 2,
        1,
        DIM,
        "ARROWS PICK. ENTER ACTS. E LEAVES.",
    );
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beacon::postings_for;
    use vx_world::town;

    fn frontier() -> Vec<vx_world::town::TownSite> {
        town::towns_near(7, (0, 0), 2_000, &|_, _| 90)
    }

    /// The hometown's board, and a town it posts freight to.
    fn home_board() -> (vx_world::town::TownSite, Vec<crate::beacon::Posting>) {
        let home = town::home_site();
        let postings = postings_for(&home, &frontier());
        (home, postings)
    }

    /// A town whose board carries the kind of work a test needs. Boards are
    /// derived, so which town that is is a property of the seed, not
    /// something a fixture gets to choose.
    fn board_with(
        wanted: fn(&crate::beacon::Posting) -> bool,
    ) -> (
        vx_world::town::TownSite,
        Vec<crate::beacon::Posting>,
        crate::beacon::Posting,
    ) {
        let frontier = frontier();
        for site in std::iter::once(town::home_site()).chain(frontier.iter().copied()) {
            let postings = postings_for(&site, &frontier);
            if let Some(found) = postings.iter().find(|posting| wanted(posting)).cloned() {
                return (site, postings, found);
            }
        }
        panic!("no town on the fixture frontier posts that kind of work");
    }

    fn is_freight(posting: &crate::beacon::Posting) -> bool {
        matches!(posting.task, Task::Deliver { .. })
    }

    fn is_survey(posting: &crate::beacon::Posting) -> bool {
        matches!(posting.task, Task::Survey { .. })
    }

    fn never(_: (i32, i32)) -> bool {
        false
    }

    fn always(_: (i32, i32)) -> bool {
        true
    }

    #[test]
    fn a_fresh_board_offers_everything_and_nothing_is_due() {
        let (home, postings) = home_board();
        let ledger = Ledger::new();
        let rows = Board::rows(home.centre, &postings, &ledger);
        assert_eq!(rows.len(), postings.len());
        assert!(rows.iter().all(|row| matches!(row, Row::Accept(_))));
    }

    #[test]
    fn taking_freight_pins_it_and_it_is_not_due_where_you_took_it() {
        let (home, postings, haul) = board_with(is_freight);
        let mut ledger = Ledger::new();
        let mut board = Board::new();
        board.open_at_beacon();

        // Put the cursor on the haul.
        let index = postings.iter().position(|posting| posting.id == haul.id).unwrap();
        board.move_cursor(index as i32, postings.len());

        let mut wallet = Wallet::new();
        board.confirm(&home, &postings, &mut ledger, None, &mut wallet, &never);

        assert!(ledger.is_accepted(haul.id), "the haul was not taken");
        assert!(ledger.pins().contains(&haul.settles_at()));

        // Back at the issuing town it now reads as held, not as due.
        let rows = Board::rows(home.centre, &postings, &ledger);
        assert!(rows.contains(&Row::Taken(haul.id)));
        assert!(!rows.contains(&Row::HandIn(haul.id)));
        assert!(ledger.due_at(home.centre).iter().all(|due| due.id != haul.id));
    }

    #[test]
    fn freight_pays_only_at_the_far_end_and_only_with_the_goods_in_hand() {
        let (home, postings, haul) = board_with(is_freight);
        let Task::Deliver { goods, amount, target } = haul.task.clone() else {
            unreachable!()
        };
        let there = town::TownSite {
            centre: target.centre,
            ..home
        };

        let mut ledger = Ledger::new();
        ledger.accept(&haul);
        let mut wallet = Wallet::new();
        let mut board = Board::new();
        board.open_at_beacon();

        // Short: nothing is paid and nothing leaves the pile.
        let mut pile = Stockpile::new();
        pile.add(&goods, amount - 1);
        board.confirm(&there, &postings, &mut ledger, Some(&mut pile), &mut wallet, &never);
        assert_eq!(wallet.credits(), 0, "an unfulfilled haul paid out");
        assert_eq!(pile.count(&goods), amount - 1, "a refused haul took goods anyway");
        assert!(ledger.is_accepted(haul.id));

        // Enough, plus spare: exactly the contracted amount leaves.
        pile.add(&goods, 5);
        board.confirm(&there, &postings, &mut ledger, Some(&mut pile), &mut wallet, &never);
        assert_eq!(wallet.credits(), haul.reward);
        assert_eq!(pile.count(&goods), 4, "the wrong amount left the pile");
        assert!(ledger.is_settled(haul.id));
        assert!(!ledger.is_accepted(haul.id));

        // And it cannot be signed for twice.
        pile.add(&goods, amount);
        board.confirm(&there, &postings, &mut ledger, Some(&mut pile), &mut wallet, &never);
        assert_eq!(wallet.credits(), haul.reward, "a haul paid out twice");
    }

    #[test]
    fn a_survey_pays_once_the_sector_is_swept_and_not_before() {
        let (home, postings, job) = board_with(is_survey);
        let mut ledger = Ledger::new();
        ledger.accept(&job);
        let mut wallet = Wallet::new();
        let mut board = Board::new();
        board.open_at_beacon();

        board.confirm(&home, &postings, &mut ledger, None, &mut wallet, &never);
        assert_eq!(wallet.credits(), 0, "an unswept sector paid out");
        assert!(ledger.is_accepted(job.id));

        board.confirm(&home, &postings, &mut ledger, None, &mut wallet, &always);
        assert_eq!(wallet.credits(), job.reward);
        assert!(ledger.is_settled(job.id));
    }

    #[test]
    fn work_due_here_is_listed_first() {
        let (home, postings, job) = board_with(is_survey);
        let mut ledger = Ledger::new();
        ledger.accept(&job);
        let rows = Board::rows(home.centre, &postings, &ledger);
        assert_eq!(rows.first(), Some(&Row::HandIn(job.id)));
        // And it is listed once, not once as due and once as held.
        assert_eq!(
            rows.iter().filter(|row| row.id() == job.id).count(),
            1,
            "a posting was listed twice"
        );
    }

    #[test]
    fn an_empty_board_reports_rather_than_panics() {
        let home = town::home_site();
        let mut ledger = Ledger::new();
        let mut wallet = Wallet::new();
        let mut board = Board::new();
        board.open_at_beacon();
        board.confirm(&home, &[], &mut ledger, None, &mut wallet, &never);
        assert_eq!(board.feedback.as_deref(), Some("NO WORK POSTED"));
    }

    #[test]
    fn the_cursor_clamps_to_the_board() {
        let mut board = Board::new();
        board.move_cursor(9, 3);
        assert_eq!(board.cursor, 2);
        board.move_cursor(-9, 3);
        assert_eq!(board.cursor, 0);
        board.move_cursor(1, 0);
        assert_eq!(board.cursor, 0);
    }

    #[test]
    fn the_panel_renders_deterministically_and_shows_uncharted_targets() {
        let (home, postings, haul) = board_with(is_freight);
        let mut board = Board::new();
        board.open_at_beacon();
        let wallet = Wallet::new();

        let mut ledger = Ledger::new();
        ledger.accept(&haul);

        let a = render_board(&board, &home, &postings, &ledger, &wallet);
        let b = render_board(&board, &home, &postings, &ledger, &wallet);
        assert_eq!(a.len(), (BOARD_WIDTH * BOARD_HEIGHT * 4) as usize);
        assert_eq!(a, b);

        // Finding the target town changes the panel: "UNCHARTED" becomes a
        // pair of coordinates.
        assert!(!ledger.knows(haul.settles_at()));
        ledger.visit(haul.settles_at());
        let found = render_board(&board, &home, &postings, &ledger, &wallet);
        assert_ne!(a, found, "discovering the target left the board unchanged");
    }

    #[test]
    fn every_word_the_board_prints_is_one_the_font_can_draw() {
        // A character the font has never heard of draws as a filled box, and
        // a panel full of boxes reads as a broken game.
        let (home, postings, haul) = board_with(is_freight);
        let mut ledger = Ledger::new();
        let mut wallet = Wallet::new();
        let mut board = Board::new();
        board.open_at_beacon();

        let mut said: Vec<String> = vec![
            format!("{} BEACON", home.name),
            format!("{} AT {} {}", home.speciality.name(), home.centre.0, home.centre.1),
            "NO WORK POSTED".to_string(),
            "ARROWS PICK. ENTER ACTS. E LEAVES.".to_string(),
        ];
        for posting in &postings {
            said.push(posting.title());
            said.push(format!("{} CR - UNCHARTED", posting.reward));
        }

        // And every line of feedback the console can produce.
        for surveyed in [&never as &dyn Fn((i32, i32)) -> bool, &always] {
            for cursor in 0..postings.len() {
                board.move_cursor(cursor as i32 - board.cursor as i32, postings.len());
                let mut pile = Stockpile::new();
                pile.add("engine:stone", 1);
                board.confirm(
                    &home,
                    &postings,
                    &mut ledger,
                    Some(&mut pile),
                    &mut wallet,
                    surveyed,
                );
                if let Some(line) = &board.feedback {
                    said.push(line.clone());
                }
            }
        }
        ledger.accept(&haul);
        board.confirm(&home, &postings, &mut ledger, None, &mut wallet, &never);
        if let Some(line) = &board.feedback {
            said.push(line.clone());
        }

        for line in said {
            for character in line.chars() {
                assert!(
                    font::knows(character),
                    "the font cannot draw {character:?} in {line:?}"
                );
            }
        }
    }
}
