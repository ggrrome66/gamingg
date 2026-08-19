//! The supply shop: sell the pile, buy the good stuff.
//!
//! A modal panel over the game (overlay slot 2), opened by pressing E at the
//! shop counter. Selling drains the **fleet base stockpile** — the pile the
//! flier ferries home — which closes the loop: mine → ferry → sell → upgrade.
//! Buying raises a [`crate::wallet`] upgrade line, and those effects apply on
//! the next frame, to machines already in the field. Retroactive upgrades
//! feel good; that is deliberate.
//!
//! Prices are data: a name-keyed table like everything else in the game, so
//! the shelf grows by adding rows.

use vx_agent::Stockpile;
use vx_render::font::{self, LINE_HEIGHT};

use vx_world::town::TownSite;

use crate::economy::{self, Market};
use crate::wallet::{self, Wallet};

/// Panel size in texture pixels; displayed at [`SHOP_SCALE`].
pub const SHOP_WIDTH: u32 = 240;
pub const SHOP_HEIGHT: u32 = 150;
pub const SHOP_SCALE: f32 = 2.0;

const TEXT: [u8; 4] = [235, 235, 235, 255];
const DIM: [u8; 4] = [150, 150, 155, 255];
const ACCENT: [u8; 4] = [255, 170, 60, 255];
const GOOD: [u8; 4] = [120, 220, 120, 255];
const BACKGROUND: [u8; 4] = [10, 12, 16, 235];

/// What this town pays per block, by namespaced name.
///
/// Used to be a constant table — copper ore was eight credits everywhere in the
/// world. It is a *local* question now: a counter pays what its own town's
/// books say, so ore is cheap at a mine sitting on a mountain of it and dear at
/// a refinery that needs it. Kinds the network does not trade are not listed
/// for sale at all.
pub fn sell_price(market: &Market, name: &str) -> Option<u64> {
    economy::good_index(name).map(|good| market.price(good))
}

/// What the next level of an upgrade line costs: 100, 200, 400, ...
pub fn upgrade_cost(next_level: u32) -> u64 {
    100u64 << (next_level.saturating_sub(1)).min(32)
}

/// One selectable line of the shop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// Sell the whole stack of this stockpile kind.
    Sell(String),
    /// Buy one level of this upgrade line.
    Buy(&'static str),
}

/// The shop's interaction state.
#[derive(Debug, Default)]
pub struct Shop {
    pub open: bool,
    cursor: usize,
    /// The last trade's outcome, shown until the next action.
    pub feedback: Option<String>,
}

impl Shop {
    pub fn new() -> Self {
        Shop::default()
    }

    pub fn open_at_counter(&mut self) {
        self.open = true;
        self.cursor = 0;
        self.feedback = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.feedback = None;
    }

    /// The selectable rows, in stable order: priced pile kinds (the
    /// stockpile iterates sorted), then the upgrade lines still below cap.
    pub fn rows(pile: Option<&Stockpile>, walletbook: &Wallet, market: &Market) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(pile) = pile {
            for (name, count) in pile.entries() {
                if count > 0 && sell_price(market, name).is_some() {
                    rows.push(Row::Sell(name.to_string()));
                }
            }
        }
        for line in [wallet::DRILL, wallet::CARGO] {
            if walletbook.upgrade(line) < wallet::MAX_UPGRADE {
                rows.push(Row::Buy(line));
            }
        }
        rows
    }

    /// Move the cursor, clamped to the current rows.
    pub fn move_cursor(&mut self, delta: i32, rows: usize) {
        if rows == 0 {
            self.cursor = 0;
            return;
        }
        self.cursor = (self.cursor as i32 + delta).clamp(0, rows as i32 - 1) as usize;
    }

    /// Confirm the selected row against the pile and the wallet.
    /// Confirm the selected row against the pile, the wallet and the town.
    ///
    /// Selling *moves the town's books*, so dumping forty loads on a small
    /// market visibly craters what the next forty fetch. That is the whole
    /// reason a counter had to learn which town it stands in.
    pub fn confirm(
        &mut self,
        pile: Option<&mut Stockpile>,
        walletbook: &mut Wallet,
        market: &mut Market,
    ) {
        let rows = Shop::rows(pile.as_deref(), walletbook, market);
        let Some(row) = rows.get(self.cursor) else {
            self.feedback = Some("NOTHING TO TRADE".into());
            return;
        };
        match row {
            Row::Sell(name) => {
                let Some(pile) = pile else {
                    self.feedback = Some("NO BASE PILE".into());
                    return;
                };
                let (sold, earned) = sell_all(pile, walletbook, market, name);
                self.feedback =
                    Some(format!("SOLD {sold} {} FOR {earned} CR", display_name(name)));
            }
            Row::Buy(line) => {
                if buy(walletbook, line) {
                    let level = walletbook.upgrade(line);
                    self.feedback =
                        Some(format!("{} MK{level} FITTED", line.to_uppercase()));
                } else {
                    self.feedback = Some("NOT ENOUGH CR".into());
                }
            }
        }
        // A sold-out row disappears; keep the cursor on the shelf.
        let rows = Shop::rows(None, walletbook, market).len().max(1);
        self.cursor = self.cursor.min(rows - 1);
    }
}

/// Sell the whole stack of `name`: returns (blocks sold, credits earned).
///
/// Exact by construction — `take` reports what actually left the pile — and the
/// goods land in the town's books, which is what moves the price for whoever
/// sells next.
pub fn sell_all(
    pile: &mut Stockpile,
    walletbook: &mut Wallet,
    market: &mut Market,
    name: &str,
) -> (u64, u64) {
    let Some(good) = economy::good_index(name) else {
        return (0, 0);
    };
    let sold = pile.take(name, u64::MAX);
    if sold == 0 {
        return (0, 0);
    }
    // Priced before the sale lands: you are paid what the board said, and the
    // market moves for the next seller rather than under your own feet.
    let earned = sold * market.price(good);
    market.deposit(good, sold as f32);
    walletbook.earn(earned);
    (sold, earned)
}

/// Buy one level of an upgrade line: spend-then-raise, atomic — a refused
/// spend changes nothing.
pub fn buy(walletbook: &mut Wallet, line: &str) -> bool {
    let next = walletbook.upgrade(line) + 1;
    if next > wallet::MAX_UPGRADE || !walletbook.spend(upgrade_cost(next)) {
        return false;
    }
    walletbook.raise(line);
    true
}

/// `engine:copper_ore` -> `COPPER ORE`, for the shelf labels. Shared with the
/// beacon board so a sack of ore is named the same wherever it is listed.
pub fn display_name(name: &str) -> String {
    name.split_once(':')
        .map_or(name, |(_, bare)| bare)
        .replace('_', " ")
        .to_uppercase()
}

/// Render the panel. Pure in its inputs.
pub fn render_shop(
    shop: &Shop,
    pile: Option<&Stockpile>,
    walletbook: &Wallet,
    town: &TownSite,
    market: &Market,
) -> Vec<u8> {
    let mut pixels = vec![0u8; (SHOP_WIDTH * SHOP_HEIGHT * 4) as usize];
    for texel in pixels.chunks_exact_mut(4) {
        texel.copy_from_slice(&BACKGROUND);
    }

    let margin = 6i32;
    let mut y = margin;
    let heading = format!("{} SUPPLY", town.name);
    font::draw_text(&mut pixels, SHOP_WIDTH, margin, y, 1, ACCENT, &heading);
    let credits = format!("CR {}", walletbook.credits());
    font::draw_text(
        &mut pixels,
        SHOP_WIDTH,
        SHOP_WIDTH as i32 - margin - font::text_width(&credits, 1) as i32,
        y,
        1,
        GOOD,
        &credits,
    );
    y += LINE_HEIGHT as i32 + 3;

    let rows = Shop::rows(pile, walletbook, market);
    if rows.is_empty() {
        font::draw_text(&mut pixels, SHOP_WIDTH, margin, y, 1, DIM, "NOTHING TO SELL");
        y += LINE_HEIGHT as i32;
    }
    for (index, row) in rows.iter().enumerate() {
        let selected = index == shop.cursor;
        let colour = if selected { TEXT } else { DIM };
        if selected {
            font::draw_text(&mut pixels, SHOP_WIDTH, margin, y, 1, ACCENT, ">");
        }
        let label = match row {
            Row::Sell(name) => {
                let count = pile.map_or(0, |pile| pile.count(name));
                let price = sell_price(market, name).unwrap_or(0);
                format!("SELL {} X{count} AT {price} CR", display_name(name))
            }
            Row::Buy(line) => {
                let next = walletbook.upgrade(line) + 1;
                format!(
                    "BUY {} MK{next} FOR {} CR",
                    line.to_uppercase(),
                    upgrade_cost(next)
                )
            }
        };
        font::draw_text(&mut pixels, SHOP_WIDTH, margin + 10, y, 1, colour, &label);
        y += LINE_HEIGHT as i32;
    }

    if let Some(feedback) = &shop.feedback {
        y += 3;
        font::draw_text(&mut pixels, SHOP_WIDTH, margin, y, 1, GOOD, feedback);
    }

    font::draw_text(
        &mut pixels,
        SHOP_WIDTH,
        margin,
        SHOP_HEIGHT as i32 - LINE_HEIGHT as i32 - 2,
        1,
        DIM,
        "ARROWS PICK. ENTER TRADES. E LEAVES.",
    );
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stocked_pile() -> Stockpile {
        let mut pile = Stockpile::new();
        pile.add("engine:copper_ore", 240);
        pile.add("engine:stone", 900);
        pile.add("engine:log", 12);
        // Not a traded good: must never appear on the shelf.
        pile.add("engine:dirt", 60);
        pile
    }

    fn town() -> TownSite {
        vx_world::town::home_site()
    }

    /// A market to trade against. Its books are whatever the hometown opens on.
    fn market() -> Market {
        economy::Economy::new().market(&town(), 0).clone()
    }

    #[test]
    fn the_shelf_lists_priced_goods_and_unbought_upgrades_in_stable_order() {
        let pile = stocked_pile();
        let wallet = Wallet::new();
        let market = market();
        let rows = Shop::rows(Some(&pile), &wallet, &market);
        assert_eq!(
            rows,
            vec![
                Row::Sell("engine:copper_ore".into()),
                Row::Sell("engine:log".into()),
                Row::Sell("engine:stone".into()),
                Row::Buy(wallet::DRILL),
                Row::Buy(wallet::CARGO),
            ],
            "the shelf should list every traded good in the pile and nothing else"
        );

        // A capped line falls off the shelf.
        let mut maxed = Wallet::new();
        for _ in 0..wallet::MAX_UPGRADE {
            maxed.raise(wallet::DRILL);
        }
        let rows = Shop::rows(Some(&pile), &maxed, &market);
        assert!(!rows.contains(&Row::Buy(wallet::DRILL)));
        assert!(rows.contains(&Row::Buy(wallet::CARGO)));
    }

    #[test]
    fn selling_pays_the_towns_price_and_drains_exactly_that_kind() {
        let mut pile = stocked_pile();
        let mut wallet = Wallet::new();
        let mut market = market();

        let rate = market.price(economy::ORE);
        let (sold, earned) = sell_all(&mut pile, &mut wallet, &mut market, "engine:copper_ore");
        assert_eq!((sold, earned), (240, 240 * rate));
        assert_eq!(wallet.credits(), 240 * rate);
        assert_eq!(pile.count("engine:copper_ore"), 0);
        assert_eq!(pile.count("engine:stone"), 900, "the wrong kind drained");
        assert_eq!(pile.count("engine:log"), 12);

        // The goods landed in the town, so the next seller finds a fuller
        // market and a worse price.
        assert!(
            market.price(economy::ORE) < rate,
            "selling 240 ore did not move the price"
        );

        // Kinds the network does not trade cannot be sold even by asking.
        assert_eq!(
            sell_all(&mut pile, &mut wallet, &mut market, "engine:dirt"),
            (0, 0)
        );
        assert_eq!(pile.count("engine:dirt"), 60);
    }

    #[test]
    fn two_towns_pay_differently_for_the_same_ore() {
        // The whole point of a counter knowing where it stands: a mine sitting
        // on a mountain of ore pays less for it than a refinery that needs it.
        let sites = vx_world::town::towns_near(7, (0, 0), 2_000, &|_, _| 90);
        let mut economy = economy::Economy::new();

        let mine = sites
            .iter()
            .find(|site| site.speciality == vx_world::town::Speciality::Mine)
            .expect("no mine on the fixture frontier");
        let refinery = sites
            .iter()
            .find(|site| site.speciality == vx_world::town::Speciality::Refinery)
            .expect("no refinery on the fixture frontier");

        // Let both run a while so their specialities show in the books.
        let at = economy::STEP * 200;
        let at_mine = economy.market(mine, at).price(economy::ORE);
        let at_refinery = economy.market(refinery, at).price(economy::ORE);
        assert!(
            at_refinery > at_mine,
            "a refinery paid {at_refinery} for ore and a mine {at_mine}"
        );
    }

    #[test]
    fn buying_subtracts_the_exact_cost_and_fits_one_level() {
        let mut wallet = Wallet::new();
        wallet.earn(1000);
        assert!(buy(&mut wallet, wallet::DRILL));
        assert_eq!(wallet.credits(), 1000 - upgrade_cost(1));
        assert_eq!(wallet.upgrade(wallet::DRILL), 1);

        // Costs double per level.
        assert!(upgrade_cost(2) == 200 && upgrade_cost(3) == 400);

        // Short funds: a no-op, balance untouched.
        let mut broke = Wallet::new();
        broke.earn(99);
        assert!(!buy(&mut broke, wallet::DRILL));
        assert_eq!(broke.credits(), 99);
        assert_eq!(broke.upgrade(wallet::DRILL), 0);
    }

    #[test]
    fn confirm_with_no_pile_reports_rather_than_panics() {
        let mut shop = Shop::new();
        shop.open_at_counter();
        let mut wallet = Wallet::new();
        // Cursor 0 lands on the first Buy row (no pile rows exist).
        shop.confirm(None, &mut wallet, &mut market());
        assert_eq!(shop.feedback.as_deref(), Some("NOT ENOUGH CR"));
    }

    #[test]
    fn the_cursor_clamps_to_the_shelf() {
        let mut shop = Shop::new();
        shop.move_cursor(5, 3);
        // A big jump clamps to the last row.
        assert_eq!(shop.cursor, 2);
        shop.move_cursor(-10, 3);
        assert_eq!(shop.cursor, 0);
        shop.move_cursor(1, 0);
        assert_eq!(shop.cursor, 0);
    }

    #[test]
    fn the_panel_renders_deterministically_and_reacts_to_stock() {
        let shop = {
            let mut shop = Shop::new();
            shop.open_at_counter();
            shop
        };
        let wallet = Wallet::new();
        let pile = stocked_pile();

        let market = market();
        let a = render_shop(&shop, Some(&pile), &wallet, &town(), &market);
        let b = render_shop(&shop, Some(&pile), &wallet, &town(), &market);
        assert_eq!(a.len(), (SHOP_WIDTH * SHOP_HEIGHT * 4) as usize);
        assert_eq!(a, b);

        let empty = render_shop(&shop, None, &wallet, &town(), &market);
        assert_ne!(a, empty, "an empty shop drew a full shelf");
    }
}
