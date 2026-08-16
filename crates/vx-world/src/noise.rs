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

/// Hash a 3D lattice point to a float in `[0, 1)`.
#[inline]
fn hash_3d(seed: u64, x: i32, y: i32, z: i32) -> f32 {
    let key = seed
        ^ mix64(x as i64 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ mix64(y as i64 as u64).wrapping_mul(0x85eb_ca6b_c2b2_ae35)
        ^ mix64(z as i64 as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
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

/// Sample 3D value noise in `[0, 1)`. Caves need a field that varies with
/// height, which a heightmap cannot express.
pub fn value_3d(seed: u64, x: f32, y: f32, z: f32) -> f32 {
    let (x0, y0, z0) = (x.floor(), y.floor(), z.floor());
    let (tx, ty, tz) = (smooth(x - x0), smooth(y - y0), smooth(z - z0));
    let (xi, yi, zi) = (x0 as i32, y0 as i32, z0 as i32);

    let corner = |dx, dy, dz| hash_3d(seed, xi + dx, yi + dy, zi + dz);

    let c00 = lerp(corner(0, 0, 0), corner(1, 0, 0), tx);
    let c10 = lerp(corner(0, 1, 0), corner(1, 1, 0), tx);
    let c01 = lerp(corner(0, 0, 1), corner(1, 0, 1), tx);
    let c11 = lerp(corner(0, 1, 1), corner(1, 1, 1), tx);

    lerp(lerp(c00, c10, ty), lerp(c01, c11, ty), tz)
}

/// Push a `[0, 1]` value away from its middle, so a noise stack that clusters
/// around its mean produces contrast instead of a narrow band.
///
/// This is the shaping step that turns rolling sameness into lowland and
/// highland. `strength` of 1 leaves the value untouched; higher values
/// flatten the middle and steepen the ends.
pub fn contrast(value: f32, strength: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value < 0.5 {
        0.5 * (2.0 * value).powf(strength)
    } else {
        1.0 - 0.5 * (2.0 * (1.0 - value)).powf(strength)
    }
}

/// Fold noise about its midpoint to make ridges: the fold leaves a crease
/// where the underlying field crosses 0.5, which reads as a ridgeline.
pub fn ridged(value: f32) -> f32 {
    1.0 - (value * 2.0 - 1.0).abs()
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
    /// Sum the octaves in three dimensions, normalised to `[0, 1]`.
    pub fn sample_3d(&self, seed: u64, x: f32, y: f32, z: f32) -> f32 {
        let mut total = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = self.frequency;
        let mut max_amplitude = 0.0;

        for octave in 0..self.octaves.max(1) {
            // Each octave gets its own seed, or they would all sample the same
            // lattice and reinforce into visible grid artefacts.
            let seed = seed ^ mix64(octave as u64 + 0x51ed);
            total += value_3d(seed, x * frequency, y * frequency, z * frequency) * amplitude;
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
}
