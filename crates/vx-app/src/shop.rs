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
use crate::garage::{self, Garage};
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
    /// Buy a machine. The reason to mine at all.
    BuyMachine(&'static str),
    /// Order a parcel of this good from another town. It travels by caravan
    /// and lands in the mailbox at your house — index into the offers list.
    Order(usize),
}

/// One good another town is willing to part with by mail.
///
/// Built when the counter opens, from the same books the caravans fly by.
/// Priced then, honoured at confirm — with a re-check, because the books may
/// have moved while the panel was open.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub good: usize,
    /// The town the parcel ships from. The whole site, because confirming the
    /// order needs its market, not just its column.
    pub source: TownSite,
    pub unit_price: u64,
    /// Carriage, priced by the same arithmetic the caravan flies by.
    pub freight: u64,
}

impl Offer {
    /// What the whole parcel costs at the counter.
    pub fn total(&self) -> u64 {
        economy::PARCEL * self.unit_price + self.freight
    }
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
    pub fn rows(
        pile: Option<&Stockpile>,
        walletbook: &Wallet,
        market: &Market,
        offers: &[Offer],
    ) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(pile) = pile {
            for (name, count) in pile.entries() {
                if count > 0 && sell_price(market, name).is_some() {
                    rows.push(Row::Sell(name.to_string()));
                }
            }
        }
        // Machines first among the buys: they are what the ore is *for*, and a
        // player who has just sold their first load should see one.
        for kind in garage::KINDS {
            rows.push(Row::BuyMachine(kind));
        }
        for line in [wallet::DRILL, wallet::CARGO] {
            if walletbook.upgrade(line) < wallet::MAX_UPGRADE {
                rows.push(Row::Buy(line));
            }
        }
        for index in 0..offers.len() {
            rows.push(Row::Order(index));
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
    #[allow(clippy::too_many_arguments)]
    pub fn confirm(
        &mut self,
        pile: Option<&mut Stockpile>,
        walletbook: &mut Wallet,
        market: &mut Market,
        shed: &mut Garage,
        offers: &[Offer],
        post: Option<&mut MailContext>,
    ) {
        let rows = Shop::rows(pile.as_deref(), walletbook, market, offers);
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
            Row::BuyMachine(kind) => {
                let owned = shed.owned(kind);
                if shed.buy(walletbook, kind) {
                    self.feedback = Some(format!(
                        "{} {} DELIVERED",
                        garage::display_name(kind),
                        owned + 1
                    ));
                } else {
                    self.feedback = Some("NOT ENOUGH CR".into());
                }
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
            Row::Order(index) => {
                let Some(offer) = offers.get(*index).cloned() else {
                    self.feedback = Some("THAT OFFER IS GONE".into());
                    return;
                };
                let Some(post) = post else {
                    self.feedback = Some("THE POST IS NOT RUNNING".into());
                    return;
                };
                self.feedback = Some(order(&offer, walletbook, post));
            }
        }
        // A sold-out row disappears; keep the cursor on the shelf.
        let rows = Shop::rows(None, walletbook, market, offers).len().max(1);
        self.cursor = self.cursor.min(rows - 1);
    }
}

/// What placing a mail order needs a mutable line to.
pub struct MailContext<'a> {
    pub economy: &'a mut economy::Economy,
    pub mailbox_held: u64,
    pub now: u64,
}

/// Goods already bought and bound for the mailbox but not yet landed.
pub fn mail_in_flight(economy: &economy::Economy) -> u64 {
    economy
        .shipments()
        .iter()
        .filter(|load| load.owner == economy::Owner::Mail)
        .map(|load| load.amount.round() as u64)
        .sum()
}

/// Place one order: pay, take the goods off the source's shelf, put the load
/// in the air. Returns the feedback line.
fn order(offer: &Offer, walletbook: &mut Wallet, post: &mut MailContext) -> String {
    use economy::{Owner, Shipment, MAILBOX_CAP, PARCEL};

    // The cap counts loads still in the air, or you could order a mailbox
    // full three times over and lose two of them.
    let bound = post.mailbox_held + mail_in_flight(post.economy);
    if bound + PARCEL > MAILBOX_CAP {
        return "MAILBOX FULL. COLLECT IT FIRST".into();
    }

    // Re-check the shelf at the moment of sale, not the moment the panel
    // opened: the books move on their own.
    let market = post.economy.market_mut(&offer.source, post.now);
    if market.stock(offer.good) < PARCEL as f32 {
        return format!("SOLD OUT AT {}", offer.source.name);
    }

    if !walletbook.spend(offer.total()) {
        return "NOT ENOUGH CR".into();
    }
    market.withdraw(offer.good, PARCEL as f32);

    let home = vx_world::town::home_site().centre;
    let depart = post.now;
    let arrive = depart + Shipment::travel_ticks(offer.source.centre, home);
    post.economy.ship(Shipment {
        from: offer.source.centre,
        to: home,
        good: offer.good,
        amount: PARCEL as f32,
        depart,
        arrive,
        owner: Owner::Mail,
    });
    "ORDERED. WATCH THE SKY.".into()
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
    shed: &Garage,
    offers: &[Offer],
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

    let rows = Shop::rows(pile, walletbook, market, offers);
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
            Row::BuyMachine(kind) => {
                let owned = shed.owned(kind);
                format!(
                    "BUY {} FOR {} CR - OWN {owned}",
                    garage::display_name(kind),
                    garage::cost(kind, owned)
                )
            }
            Row::Buy(line) => {
                let next = walletbook.upgrade(line) + 1;
                format!(
                    "BUY {} MK{next} FOR {} CR",
                    line.to_uppercase(),
                    upgrade_cost(next)
                )
            }
            Row::Order(index) => match offers.get(*index) {
                Some(offer) => format!(
                    "ORDER {} {} FROM {} - {} CR",
                    economy::PARCEL,
                    display_name(economy::GOODS[offer.good]),
                    offer.source.name,
                    offer.total()
                ),
                None => "ORDER (GONE)".to_string(),
            },
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
    fn the_shelf_lists_goods_then_machines_then_upgrades() {
        let pile = stocked_pile();
        let wallet = Wallet::new();
        let market = market();
        let rows = Shop::rows(Some(&pile), &wallet, &market, &[]);
        assert_eq!(
            rows,
            vec![
                Row::Sell("engine:copper_ore".into()),
                Row::Sell("engine:log".into()),
                Row::Sell("engine:stone".into()),
                // Machines before upgrades: they are what the ore is for, and
                // somebody who has just sold their first load should see one.
                Row::BuyMachine(garage::DRONE),
                Row::BuyMachine(garage::FLIER),
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
        let rows = Shop::rows(Some(&pile), &maxed, &market, &[]);
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
        shop.confirm(None, &mut wallet, &mut market(), &mut Garage::new(), &[], None);
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
        let a = render_shop(&shop, Some(&pile), &wallet, &town(), &market, &Garage::new(), &[]);
        let b = render_shop(&shop, Some(&pile), &wallet, &town(), &market, &Garage::new(), &[]);
        assert_eq!(a.len(), (SHOP_WIDTH * SHOP_HEIGHT * 4) as usize);
        assert_eq!(a, b);

        let empty = render_shop(&shop, None, &wallet, &town(), &market, &Garage::new(), &[]);
        assert_ne!(a, empty, "an empty shop drew a full shelf");
    }

    #[test]
    fn a_machine_can_be_bought_from_the_shelf_and_costs_the_asking_price() {
        // The loop this whole round exists for: ore becomes credits, credits
        // become a drone.
        let mut wallet = Wallet::new();
        wallet.earn(garage::cost(garage::DRONE, 0));
        let mut shed = Garage::new();
        let mut market = market();

        let mut shop = Shop::new();
        shop.open_at_counter();
        let rows = Shop::rows(None, &wallet, &market, &[]);
        let at = rows
            .iter()
            .position(|row| *row == Row::BuyMachine(garage::DRONE))
            .expect("no drone on the shelf");
        shop.move_cursor(at as i32, rows.len());

        shop.confirm(None, &mut wallet, &mut market, &mut shed, &[], None);
        assert_eq!(shed.owned(garage::DRONE), 1, "the drone never arrived");
        assert_eq!(wallet.credits(), 0, "the wrong amount was spent");

        // And a second one is refused, because it costs more than the first.
        shop.confirm(None, &mut wallet, &mut market, &mut shed, &[], None);
        assert_eq!(shed.owned(garage::DRONE), 1, "a drone was bought on credit");
        assert_eq!(shop.feedback.as_deref(), Some("NOT ENOUGH CR"));
    }
}

#[cfg(test)]
mod order_tests {
    use super::*;
    use crate::economy::{Economy, Owner, MAILBOX_CAP, ORE, PARCEL};
    use vx_world::town::home_site;

    fn source_town() -> TownSite {
        TownSite {
            centre: (1024, 512),
            ..home_site()
        }
    }

    /// An offer priced off the live books, as the counter does it.
    fn offer_from(economy: &mut Economy, now: u64) -> Offer {
        let source = source_town();
        let unit_price = economy.market(&source, now).price(ORE);
        Offer {
            good: ORE,
            source,
            unit_price,
            freight: economy::Shipment::travel_ticks(source.centre, home_site().centre) / 10,
        }
    }

    #[test]
    fn ordering_pays_source_price_plus_freight_and_moves_the_source_books() {
        let mut economy = Economy::new();
        let offer = offer_from(&mut economy, 0);
        let total = offer.total();
        let shelf_before = economy.market(&offer.source, 0).stock(ORE);

        let mut wallet = Wallet::new();
        wallet.earn(total + 5);
        let mut post = MailContext {
            economy: &mut economy,
            mailbox_held: 0,
            now: 0,
        };

        let line = super::order(&offer, &mut wallet, &mut post);

        assert_eq!(line, "ORDERED. WATCH THE SKY.");
        assert_eq!(wallet.credits(), 5, "paid something other than the quote");
        let shelf_after = economy.market(&offer.source, 0).stock(ORE);
        assert!(
            (shelf_before - shelf_after - PARCEL as f32).abs() < 0.01,
            "the order did not move the source books: {shelf_before} -> {shelf_after}"
        );
        assert_eq!(economy.shipments().len(), 1);
        assert_eq!(economy.shipments()[0].owner, Owner::Mail);
        assert_eq!(
            economy.shipments()[0].to,
            home_site().centre,
            "mail must always deliver to the hometown mailbox"
        );
    }

    #[test]
    fn a_broke_customer_keeps_the_shelf_and_the_sky_unchanged() {
        let mut economy = Economy::new();
        let offer = offer_from(&mut economy, 0);
        let shelf_before = economy.market(&offer.source, 0).stock(ORE);

        let mut wallet = Wallet::new(); // empty
        let mut post = MailContext {
            economy: &mut economy,
            mailbox_held: 0,
            now: 0,
        };
        let line = super::order(&offer, &mut wallet, &mut post);

        assert_eq!(line, "NOT ENOUGH CR");
        assert!(economy.shipments().is_empty(), "an unpaid load took off");
        let shelf_after = economy.market(&offer.source, 0).stock(ORE);
        assert!((shelf_before - shelf_after).abs() < f32::EPSILON);
    }

    #[test]
    fn a_full_mailbox_refuses_the_order_counting_loads_still_in_the_air() {
        let mut economy = Economy::new();
        let offer = offer_from(&mut economy, 0);
        let mut wallet = Wallet::new();
        wallet.earn(1_000_000);

        // Fill the sky to one parcel under the cap...
        let airborne = MAILBOX_CAP - PARCEL;
        economy.ship(economy::Shipment {
            from: offer.source.centre,
            to: home_site().centre,
            good: ORE,
            amount: airborne as f32,
            depart: 0,
            arrive: 10_000,
            owner: Owner::Mail,
        });

        // ...one more parcel exactly reaches the cap and is allowed...
        let mut post = MailContext {
            economy: &mut economy,
            mailbox_held: 0,
            now: 0,
        };
        assert_eq!(super::order(&offer, &mut wallet, &mut post), "ORDERED. WATCH THE SKY.");

        // ...and the next is refused, because the air already owes a full box.
        let mut post = MailContext {
            economy: &mut economy,
            mailbox_held: 0,
            now: 0,
        };
        assert_eq!(
            super::order(&offer, &mut wallet, &mut post),
            "MAILBOX FULL. COLLECT IT FIRST"
        );
    }

    #[test]
    fn two_orders_can_fly_at_once_under_the_cap() {
        let mut economy = Economy::new();
        let offer = offer_from(&mut economy, 0);
        let mut wallet = Wallet::new();
        wallet.earn(1_000_000);

        for _ in 0..2 {
            let mut post = MailContext {
                economy: &mut economy,
                mailbox_held: 0,
                now: 0,
            };
            assert_eq!(super::order(&offer, &mut wallet, &mut post), "ORDERED. WATCH THE SKY.");
        }
        assert_eq!(mail_in_flight(&economy), 2 * PARCEL);
    }

    #[test]
    fn the_shelf_lists_an_order_row_only_when_an_offer_exists() {
        let wallet = Wallet::new();
        let mut books = Economy::new();
        let market = books.market(&home_site(), 0).clone();

        let bare = Shop::rows(None, &wallet, &market, &[]);
        assert!(
            !bare.iter().any(|row| matches!(row, Row::Order(_))),
            "an order row appeared from nowhere"
        );

        let mut economy = Economy::new();
        let offer = offer_from(&mut economy, 0);
        let stocked = Shop::rows(None, &wallet, &market, &[offer]);
        assert!(stocked.iter().any(|row| matches!(row, Row::Order(0))));
    }

    #[test]
    fn every_order_label_is_drawable() {
        let mut economy = Economy::new();
        let offer = offer_from(&mut economy, 0);
        let label = format!(
            "ORDER {} {} FROM {} - {} CR",
            PARCEL,
            display_name(economy::GOODS[offer.good]),
            offer.source.name,
            offer.total()
        );
        for character in label.chars() {
            assert!(font::knows(character), "undrawable {character:?} in {label}");
        }
    }
}
