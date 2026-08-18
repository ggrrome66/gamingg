//! Opening a mine on terrain the generator actually produced.
//!
//! The unit tests build flat slabs and neat hillsides, which is right for
//! pinning down geometry but says nothing about whether any of it survives real
//! ground. Generated terrain has overhangs, cliffs, water, bodies that breach
//! the surface in awkward places and bodies wedged under mountains. This runs
//! the whole loop — find an outcrop by eye, plan, dig, haul — against exactly
//! that.
//!
//! The seed and position are the ones the screenshots use, so a failure here
//! and a bad screenshot are the same bug.

use vx_agent::{find_body, is_ore, options, MineMethod, Operation, RunOutcome, VoxelAabb};
use vx_core::{BlockPos, ChunkPos, EventBus};
use vx_world::World;

/// The seed the game ships with, and a spot with a copper outcrop on it.
const SEED: u64 = 2024;
const OUTCROP: (i32, i32) = (146, 30);

fn world_around(at: (i32, i32), radius: i32) -> World {
    let mut world = World::new(SEED);
    world.load_around(BlockPos::new(at.0, 0, at.1).chunk(), radius);
    world
}

fn solid_in(world: &World, region: VoxelAabb) -> u64 {
    region
        .clamped_to_world()
        .blocks()
        .filter(|pos| world.is_solid(*pos))
        .count() as u64
}

#[test]
fn there_is_a_findable_outcrop_where_the_screenshots_say() {
    // If this fails the fixture has drifted, and every other test in this file
    // is testing nothing.
    let world = world_around(OUTCROP, 4);
    let body = find_body(&world, OUTCROP, 48).expect("no outcrop near the documented spot");

    let ore = body
        .blocks()
        .filter(|pos| is_ore(&world, *pos))
        .count();
    assert!(ore > 100, "the body under the outcrop is only {ore} blocks");
    eprintln!("body {:?}..{:?}, {ore} ore blocks", body.min, body.max);
}

#[test]
fn every_applicable_method_opens_a_real_mine() {
    // The end-to-end claim, on ground nobody arranged: mark a body, let the
    // game plan it, and a single drone cuts its way in, digs it out and hauls
    // it home. Not "a flow field says it could" — it actually did.
    let radius = 8;
    let reference = world_around(OUTCROP, radius);
    let body = find_body(&reference, OUTCROP, 48).expect("no outcrop to mine");
    let ore_in_body = body.blocks().filter(|pos| is_ore(&reference, *pos)).count() as u64;

    let plans = options(&reference, body, vx_agent::DEFAULT_GRADE);
    assert!(!plans.is_empty(), "no method applies to a real orebody");
    eprintln!(
        "options: {:?}",
        plans
            .iter()
            .map(|plan| (plan.method.name(), plan.volume, plan.cost()))
            .collect::<Vec<_>>()
    );

    for plan in &plans {
        // A fresh world each time: one method's excavation must not make the
        // next one's job easier.
        let mut world = world_around(OUTCROP, radius);
        let start = vx_agent::settle(&world, plan.portal);

        let mut operation = Operation::new(start);
        operation.add_drone(start);
        operation.post_plan(plan);

        let events = EventBus::new();
        let before = solid_in(&world, body.expanded(48).clamped_to_world());
        let (outcome, ticks) = operation.run(&mut world, &events, 400_000);

        assert_eq!(
            outcome,
            RunOutcome::Finished,
            "{}: stopped after {ticks} ticks with {} jobs left and the drone {:?}",
            plan.method.name(),
            operation.board.len(),
            operation.drones[0].state
        );

        let left = body.blocks().filter(|pos| is_ore(&world, *pos)).count();
        assert_eq!(left, 0, "{}: {left} ore blocks left in the ground", plan.method.name());

        let removed = before - solid_in(&world, body.expanded(48).clamped_to_world());
        assert_eq!(
            operation.accounted_blocks(),
            removed,
            "{}: the pile holds {} but {removed} blocks left the world",
            plan.method.name(),
            operation.accounted_blocks()
        );
        assert!(
            operation.stockpile.count("engine:copper_ore") >= ore_in_body,
            "{}: only {} of {ore_in_body} ore reached the pile",
            plan.method.name(),
            operation.stockpile.count("engine:copper_ore")
        );

        eprintln!(
            "{}: {ticks} ticks, {removed} blocks moved, {} ore",
            plan.method.name(),
            operation.stockpile.count("engine:copper_ore")
        );
    }
}

#[test]
fn the_recommendation_is_one_of_the_options_and_the_cheapest() {
    let world = world_around(OUTCROP, 8);
    let body = find_body(&world, OUTCROP, 48).expect("no outcrop to mine");

    let all = options(&world, body, vx_agent::DEFAULT_GRADE);
    let chosen = vx_agent::propose(&world, body, vx_agent::DEFAULT_GRADE).expect("nothing proposed");

    assert!(all.iter().any(|plan| plan.method == chosen.method));
    assert!(all.iter().all(|plan| plan.cost() >= chosen.cost()));
}

#[test]
fn an_overridden_method_still_finishes() {
    // The override is only worth offering if disagreeing with the ranking
    // actually works. Force the pit even though it is not what was proposed.
    let mut world = world_around(OUTCROP, 8);
    let body = find_body(&world, OUTCROP, 48).expect("no outcrop to mine");

    let Some(plan) = vx_agent::plan(&world, body, vx_agent::DEFAULT_GRADE, MineMethod::Pit) else {
        eprintln!("no pit applies here; nothing to check");
        return;
    };

    let start = vx_agent::settle(&world, plan.portal);
    let mut operation = Operation::new(start);
    operation.add_drone(start);
    operation.post_plan(&plan);

    let events = EventBus::new();
    let (outcome, _) = operation.run(&mut world, &events, 400_000);
    assert_eq!(outcome, RunOutcome::Finished);
    assert_eq!(body.blocks().filter(|pos| is_ore(&world, *pos)).count(), 0);
}

#[test]
fn planning_the_same_ground_twice_gives_the_same_answer() {
    // Worldgen is a pure function of the seed, and so is planning on top of it.
    // A mine that came out differently on a reload would break saves.
    let a = world_around(OUTCROP, 4);
    let b = world_around(OUTCROP, 4);
    let body = find_body(&a, OUTCROP, 48).expect("no outcrop to mine");

    assert_eq!(find_body(&b, OUTCROP, 48), Some(body));
    assert_eq!(
        options(&a, body, vx_agent::DEFAULT_GRADE),
        options(&b, body, vx_agent::DEFAULT_GRADE)
    );
}

#[test]
fn a_chunk_boundary_does_not_split_a_body() {
    // Deposits are generated from a lattice that knows nothing about chunks, so
    // a body straddling a boundary is the common case rather than the odd one.
    // Meshing and planning both key off world coordinates; this is the check
    // that nothing quietly clips at the seam.
    let world = world_around(OUTCROP, 4);
    let body = find_body(&world, OUTCROP, 48).expect("no outcrop to mine");

    let min_chunk = ChunkPos::new(body.min.x.div_euclid(16), body.min.z.div_euclid(16));
    let max_chunk = ChunkPos::new(body.max.x.div_euclid(16), body.max.z.div_euclid(16));
    if min_chunk == max_chunk {
        eprintln!("this body sits in one chunk; nothing to check");
        return;
    }

    for pos in body.blocks().filter(|pos| is_ore(&world, *pos)) {
        assert!(world.is_loaded(pos.chunk()), "ore in an unloaded chunk at {pos:?}");
    }
}
