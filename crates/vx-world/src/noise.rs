//! Deterministic value noise.
//!
//! Worldgen must be reproducible: the same seed and coordinates must give the
//! same terrain on every machine and every run, or saved worlds change shape
//! when you reload them. So this uses an integer hash rather than a seeded RNG
//! — there is no sequence state to get out of step, and sampling is pure and
//! order-independent, which lets chunks generate in parallel.

/// Mixes a 64-bit integer into a well-distributed hash.
///
/// A splitmix64-style finaliser: cheap, and good enough that neighbouring
/// lattice points do not visibly correlate.
#[inline]
fn mix64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// Hash a 2D lattice point to a float in `[0, 1)`.
#[inline]
fn hash_2d(seed: u64, x: i32, z: i32) -> f32 {
    let key = seed
        ^ mix64(x as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ mix64(z as i64 as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    // Top 24 bits give a uniform float without precision loss.
    ((mix64(key) >> 40) as f32) / ((1u32 << 24) as f32)
}

/// Smoothstep, so interpolated values meet lattice points with zero gradient
/// and the terrain has no visible grid creases.
#[inline]
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Sample 2D value noise in `[0, 1)`.
pub fn value_2d(seed: u64, x: f32, z: f32) -> f32 {
    let x0 = x.floor();
    let z0 = z.floor();
    let tx = smooth(x - x0);
    let tz = smooth(z - z0);

    let (xi, zi) = (x0 as i32, z0 as i32);
    let c00 = hash_2d(seed, xi, zi);
    let c10 = hash_2d(seed, xi + 1, zi);
    let c01 = hash_2d(seed, xi, zi + 1);
    let c11 = hash_2d(seed, xi + 1, zi + 1);

    lerp(lerp(c00, c10, tx), lerp(c01, c11, tx), tz)
}

/// Settings for a stack of noise octaves.
#[derive(Debug, Clone, Copy)]
pub struct Fbm {
    pub octaves: u32,
    /// Amplitude multiplier per octave.
    pub persistence: f32,
    /// Frequency multiplier per octave.
    pub lacunarity: f32,
    /// Frequency of the first octave, in blocks per lattice cell.
    pub frequency: f32,
}

impl Default for Fbm {
    fn default() -> Self {
        Fbm {
            octaves: 4,
            persistence: 0.5,
            lacunarity: 2.0,
            frequency: 1.0 / 96.0,
        }
    }
}

impl Fbm {
    /// Sum the octaves, normalised to `[0, 1]`.
    pub fn sample(&self, seed: u64, x: f32, z: f32) -> f32 {
        let mut total = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = self.frequency;
        let mut max_amplitude = 0.0;

        for octave in 0..self.octaves {
            // Offsetting the seed per octave keeps the layers independent;
            // reusing one seed would stack correlated patterns.
            let octave_seed = seed ^ mix64(octave as u64 + 1);
            total += value_2d(octave_seed, x * frequency, z * frequency) * amplitude;
            max_amplitude += amplitude;
            amplitude *= self.persistence;
            frequency *= self.lacunarity;
        }

        if max_amplitude > 0.0 {
            total / max_amplitude
        } else {
            0.0
        }
    }
}


/// Sample 2D value noise remapped to `[-1, 1]`.
///
/// The signed form is what ridged noise and domain warping need — both care
/// about the sign, which `value_2d`'s `[0, 1)` range throws away.
pub fn signed_2d(seed: u64, x: f32, z: f32) -> f32 {
    value_2d(seed, x, z) * 2.0 - 1.0
}

/// Fold noise around zero to make a crease instead of a bump.
///
/// `1 - |signed|` turns the zero crossings — which are lines, not points — into
/// sharp ridges. Squaring sharpens them further and keeps the result in
/// `[0, 1]`. This is what produces mountain ridgelines; plain summed octaves
/// only ever give lumpy domes.
pub fn ridged_2d(seed: u64, x: f32, z: f32) -> f32 {
    let folded = 1.0 - signed_2d(seed, x, z).abs();
    folded * folded
}

/// Offset a sample position by a second noise field before sampling.
///
/// Without this, everything downstream inherits the lattice's axis alignment
/// and the terrain reads as a grid no matter how good the shaping splines are.
/// Warping bends the whole coordinate space, so coastlines and ridges wander.
///
/// `strength` is in the same units as `x`/`z` — it is how far a point may be
/// displaced.
pub fn warp_2d(seed: u64, x: f32, z: f32, frequency: f32, strength: f32) -> (f32, f32) {
    // Independent seeds per axis, or the offset is always diagonal.
    let dx = signed_2d(seed ^ 0x5749_4e44, x * frequency, z * frequency);
    let dz = signed_2d(seed ^ 0x5741_5250, x * frequency, z * frequency);
    (x + dx * strength, z + dz * strength)
}

/// A stack of ridged octaves.
#[derive(Debug, Clone, Copy)]
pub struct Ridged {
    pub octaves: u32,
    pub persistence: f32,
    pub lacunarity: f32,
    pub frequency: f32,
}

impl Default for Ridged {
    fn default() -> Self {
        Ridged {
            octaves: 4,
            persistence: 0.5,
            lacunarity: 2.1,
            frequency: 1.0 / 160.0,
        }
    }
}

impl Ridged {
    /// Sum the octaves, normalised to `[0, 1]`.
    pub fn sample(&self, seed: u64, x: f32, z: f32) -> f32 {
        let mut total = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = self.frequency;
        let mut max_amplitude = 0.0;

        for octave in 0..self.octaves {
            let octave_seed = seed ^ mix64(octave as u64 + 0x9e37);
            total += ridged_2d(octave_seed, x * frequency, z * frequency) * amplitude;
            max_amplitude += amplitude;
            amplitude *= self.persistence;
            frequency *= self.lacunarity;
        }

        if max_amplitude > 0.0 {
            (total / max_amplitude).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// A piecewise-linear curve mapping one value to another.
///
/// This is the whole answer to terracing. Summed noise clusters near its mean,
/// so mapping it linearly to height gives a narrow band of samey mid-range
/// terrain. A spline can spend a large output range on a narrow input band — a
/// cliff is simply a steep segment — and can hold an output *flat* across a wide
/// input band, which makes plains genuinely flat by authorial choice rather than
/// by accident of averaging.
#[derive(Debug, Clone, PartialEq)]
pub struct Spline {
    /// Control points, ascending by input. Never empty.
    points: Vec<(f32, f32)>,
}

impl Spline {
    /// Build a spline from control points.
    ///
    /// # Panics
    ///
    /// If `points` is empty or not sorted ascending by input. Both are
    /// programmer errors in a hardcoded terrain curve, and silently accepting
    /// unsorted points would give terrain that is wrong in a way nobody would
    /// trace back to here.
    pub fn new(points: Vec<(f32, f32)>) -> Self {
        assert!(!points.is_empty(), "a spline needs at least one control point");
        assert!(
            points.windows(2).all(|pair| pair[0].0 <= pair[1].0),
            "spline control points must ascend by input: {points:?}"
        );
        Spline { points }
    }

    pub fn points(&self) -> &[(f32, f32)] {
        &self.points
    }

    /// Evaluate the curve, clamping to the end values outside its range.
    pub fn sample(&self, at: f32) -> f32 {
        let first = self.points[0];
        if at <= first.0 {
            return first.1;
        }
        let last = self.points[self.points.len() - 1];
        if at >= last.0 {
            return last.1;
        }

        // Find the segment containing `at`. Terrain splines have a handful of
        // points, so a linear scan beats the bookkeeping of a binary search.
        for pair in self.points.windows(2) {
            let (x0, y0) = pair[0];
            let (x1, y1) = pair[1];
            if at <= x1 {
                // Coincident inputs would divide by zero; treat as a step.
                if (x1 - x0).abs() < f32::EPSILON {
                    return y1;
                }
                return lerp(y0, y1, (at - x0) / (x1 - x0));
            }
        }
        last.1
    }

    /// Smallest and largest output the curve can produce.
    pub fn output_range(&self) -> (f32, f32) {
        self.points.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), (_, y)| {
            (lo.min(*y), hi.max(*y))
        })
    }
}


/// Hash a 3D lattice point to a float in `[0, 1)`.
///
/// The third axis gets its own multiplier so `(x, y, z)` and `(x, z, y)`
/// decorrelate; reusing a 2D constant would fold the lattice onto itself.
#[inline]
fn hash_3d(seed: u64, x: i32, y: i32, z: i32) -> f32 {
    let key = seed
        ^ mix64(x as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ mix64(y as i64 as u64).wrapping_mul(0xd6e8_feb8_6659_fd93)
        ^ mix64(z as i64 as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    ((mix64(key) >> 40) as f32) / ((1u32 << 24) as f32)
}

/// Sample 3D value noise in `[0, 1)`.
///
/// The trilinear sibling of [`value_2d`], and the first genuinely volumetric
/// field in the engine — terrain is a height field, but a cave is a shape *in*
/// the rock, and only a function of all three coordinates can make one.
pub fn value_3d(seed: u64, x: f32, y: f32, z: f32) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let z0 = z.floor();
    let tx = smooth(x - x0);
    let ty = smooth(y - y0);
    let tz = smooth(z - z0);

    let (xi, yi, zi) = (x0 as i32, y0 as i32, z0 as i32);
    let bottom = lerp(
        lerp(hash_3d(seed, xi, yi, zi), hash_3d(seed, xi + 1, yi, zi), tx),
        lerp(hash_3d(seed, xi, yi, zi + 1), hash_3d(seed, xi + 1, yi, zi + 1), tx),
        tz,
    );
    let top = lerp(
        lerp(hash_3d(seed, xi, yi + 1, zi), hash_3d(seed, xi + 1, yi + 1, zi), tx),
        lerp(hash_3d(seed, xi, yi + 1, zi + 1), hash_3d(seed, xi + 1, yi + 1, zi + 1), tx),
        tz,
    );
    lerp(bottom, top, ty)
}

/// Sample 3D value noise remapped to `[-1, 1]`.
pub fn signed_3d(seed: u64, x: f32, y: f32, z: f32) -> f32 {
    value_3d(seed, x, y, z) * 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_is_deterministic() {
        let a = value_2d(42, 12.5, -7.25);
        let b = value_2d(42, 12.5, -7.25);
        assert_eq!(a, b);
    }

    #[test]
    fn different_seeds_give_different_terrain() {
        let a = value_2d(1, 5.5, 5.5);
        let b = value_2d(2, 5.5, 5.5);
        assert_ne!(a, b);
    }

    #[test]
    fn output_stays_in_the_unit_range() {
        for x in -50..50 {
            for z in -50..50 {
                let sample = value_2d(7, x as f32 * 0.37, z as f32 * 0.61);
                assert!((0.0..1.0).contains(&sample), "value_2d out of range: {sample}");
            }
        }
    }

    #[test]
    fn fbm_output_stays_in_the_unit_range() {
        let fbm = Fbm::default();
        for x in -60..60 {
            for z in -60..60 {
                let sample = fbm.sample(99, x as f32 * 3.0, z as f32 * 3.0);
                assert!((0.0..=1.0).contains(&sample), "fbm out of range: {sample}");
            }
        }
    }

    #[test]
    fn noise_is_continuous_between_lattice_points() {
        // Terrain should not step: a small move in x must produce a small
        // move in the sample. This catches a missing interpolation.
        let seed = 5;
        let mut previous = value_2d(seed, 0.0, 0.0);
        for step in 1..200 {
            let x = step as f32 * 0.01;
            let current = value_2d(seed, x, 0.0);
            assert!(
                (current - previous).abs() < 0.15,
                "discontinuity at x={x}: {previous} -> {current}"
            );
            previous = current;
        }
    }

    #[test]
    fn noise_reproduces_lattice_values_at_integer_points() {
        let seed = 11;
        for x in 0..8 {
            for z in 0..8 {
                let at_lattice = value_2d(seed, x as f32, z as f32);
                let expected = hash_2d(seed, x, z);
                assert!(
                    (at_lattice - expected).abs() < 1e-6,
                    "lattice point ({x},{z}) drifted: {at_lattice} vs {expected}"
                );
            }
        }
    }

    #[test]
    fn noise_actually_varies_rather_than_returning_a_constant() {
        let fbm = Fbm::default();
        let samples: Vec<f32> = (0..200)
            .map(|i| fbm.sample(3, i as f32 * 8.0, 0.0))
            .collect();
        let min = samples.iter().copied().fold(f32::INFINITY, f32::min);
        let max = samples.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(max - min > 0.2, "noise is too flat: range {min}..{max}");
    }

    #[test]
    fn more_octaves_add_detail_without_leaving_the_range() {
        let smooth_fbm = Fbm { octaves: 1, ..Fbm::default() };
        let detailed = Fbm { octaves: 6, ..Fbm::default() };

        let mut differed = false;
        for i in 0..100 {
            let x = i as f32 * 4.0;
            let a = smooth_fbm.sample(21, x, 0.0);
            let b = detailed.sample(21, x, 0.0);
            assert!((0.0..=1.0).contains(&b));
            if (a - b).abs() > 1e-4 {
                differed = true;
            }
        }
        assert!(differed, "extra octaves had no effect");
    }

    #[test]
    fn signed_noise_spans_the_full_range_around_zero() {
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for step in 0..2000 {
            let value = signed_2d(13, step as f32 * 0.37, step as f32 * 0.11);
            assert!((-1.0..=1.0).contains(&value), "out of range: {value}");
            min = min.min(value);
            max = max.max(value);
        }
        assert!(min < -0.5 && max > 0.5, "signed noise is squashed: {min}..{max}");
    }

    #[test]
    fn ridged_noise_peaks_where_signed_noise_crosses_zero() {
        // The defining property: the fold puts a maximum on the zero crossing,
        // which is what turns a smooth field into a ridgeline.
        for step in 0..500 {
            let x = step as f32 * 0.05;
            let signed = signed_2d(21, x, 0.0);
            let ridged = ridged_2d(21, x, 0.0);
            assert!((0.0..=1.0).contains(&ridged), "out of range: {ridged}");

            let expected = (1.0 - signed.abs()).powi(2);
            assert!((ridged - expected).abs() < 1e-5);
            if signed.abs() < 0.01 {
                assert!(ridged > 0.97, "no ridge at a zero crossing: {ridged}");
            }
        }
    }

    #[test]
    fn ridged_stacks_stay_in_range_and_actually_vary() {
        let ridged = Ridged::default();
        let samples: Vec<f32> = (0..400)
            .map(|i| ridged.sample(5, i as f32 * 7.0, i as f32 * 3.0))
            .collect();

        for sample in &samples {
            assert!((0.0..=1.0).contains(sample), "out of range: {sample}");
        }
        let min = samples.iter().copied().fold(f32::INFINITY, f32::min);
        let max = samples.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(max - min > 0.3, "ridged stack is flat: {min}..{max}");
    }

    #[test]
    fn warping_moves_a_point_but_stays_bounded() {
        let strength = 40.0;
        for step in -100..100 {
            let (x, z) = (step as f32 * 3.0, step as f32 * -2.0);
            let (wx, wz) = warp_2d(9, x, z, 1.0 / 120.0, strength);
            assert!((wx - x).abs() <= strength + 1e-3, "x moved too far");
            assert!((wz - z).abs() <= strength + 1e-3, "z moved too far");
        }
    }

    #[test]
    fn warping_displaces_the_two_axes_independently() {
        // Sharing one offset for both axes would move every point diagonally,
        // which shows up as a visible 45-degree grain in the terrain.
        let mut same = 0;
        for step in 0..200 {
            let (x, z) = (step as f32 * 5.0, 0.0);
            let (wx, wz) = warp_2d(3, x, z, 1.0 / 80.0, 20.0);
            if ((wx - x) - (wz - z)).abs() < 1e-6 {
                same += 1;
            }
        }
        assert!(same < 5, "{same} of 200 offsets were identical on both axes");
    }

    #[test]
    fn warping_is_deterministic() {
        assert_eq!(
            warp_2d(77, 12.5, -3.25, 0.01, 30.0),
            warp_2d(77, 12.5, -3.25, 0.01, 30.0)
        );
    }

    #[test]
    fn a_spline_interpolates_between_its_control_points() {
        let spline = Spline::new(vec![(0.0, 0.0), (1.0, 10.0)]);

        assert_eq!(spline.sample(0.0), 0.0);
        assert_eq!(spline.sample(1.0), 10.0);
        assert!((spline.sample(0.5) - 5.0).abs() < 1e-5);
        assert!((spline.sample(0.25) - 2.5).abs() < 1e-5);
    }

    #[test]
    fn a_spline_clamps_outside_its_range() {
        let spline = Spline::new(vec![(0.2, 3.0), (0.8, 9.0)]);

        assert_eq!(spline.sample(-100.0), 3.0);
        assert_eq!(spline.sample(0.0), 3.0);
        assert_eq!(spline.sample(1.0), 9.0);
        assert_eq!(spline.sample(100.0), 9.0);
    }

    #[test]
    fn a_spline_can_spend_a_lot_of_output_on_a_little_input() {
        // This is the point of the whole type: a narrow input band mapping to a
        // huge output swing is a cliff, and a wide flat band is a plain.
        let spline = Spline::new(vec![
            (0.0, 10.0),
            (0.45, 12.0),  // wide, nearly flat: a basin
            (0.55, 90.0),  // narrow, very steep: an escarpment
            (1.0, 100.0),
        ]);

        let basin = spline.sample(0.4) - spline.sample(0.1);
        let cliff = spline.sample(0.55) - spline.sample(0.45);
        assert!(basin < 2.0, "the basin is not flat: rose {basin}");
        assert!(cliff > 70.0, "the cliff is not steep: rose only {cliff}");
    }

    #[test]
    fn a_spline_is_monotonic_where_its_points_are() {
        let spline = Spline::new(vec![(0.0, 0.0), (0.3, 20.0), (0.7, 25.0), (1.0, 80.0)]);
        let mut previous = f32::NEG_INFINITY;
        for step in 0..=100 {
            let value = spline.sample(step as f32 / 100.0);
            assert!(value >= previous - 1e-4, "went backwards at {step}");
            previous = value;
        }
    }

    #[test]
    fn a_single_point_spline_is_a_constant() {
        let spline = Spline::new(vec![(0.5, 42.0)]);
        assert_eq!(spline.sample(0.0), 42.0);
        assert_eq!(spline.sample(0.5), 42.0);
        assert_eq!(spline.sample(1.0), 42.0);
    }

    #[test]
    fn a_spline_reports_its_output_range() {
        let spline = Spline::new(vec![(0.0, 5.0), (0.5, 100.0), (1.0, 20.0)]);
        assert_eq!(spline.output_range(), (5.0, 100.0));
    }

    #[test]
    #[should_panic(expected = "ascend by input")]
    fn unsorted_spline_points_are_rejected() {
        Spline::new(vec![(1.0, 0.0), (0.0, 10.0)]);
    }

    #[test]
    #[should_panic(expected = "at least one control point")]
    fn an_empty_spline_is_rejected() {
        Spline::new(vec![]);
    }
}