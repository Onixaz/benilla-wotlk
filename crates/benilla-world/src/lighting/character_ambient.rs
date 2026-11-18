//! The character ambient: the factor a unit's ambient light takes over the scene's, from the
//! `AreaTable.dbc` row of the player's area. The world frame (`0x7831a0`) sets it to `1 + 2 × m`,
//! easing toward a new value, and the light selection (`0x7c1730`) multiplies a unit's ambient by it,
//! each channel held at 1. Units alone: the entity creator (`0x743760`) flags unit bodies, players
//! included, and no other object.

use bevy::prelude::*;

/// The live factor and who sets it. The game writes it each frame from the player's area
/// ([`Self::follow_area`]); the `characterAmbient` console command pins it ([`Self::pin`]).
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct CharacterAmbient {
    /// The multiplier on a unit's ambient, packed on the shared light's row 0 `.w`; 1 at boot.
    pub factor: f32,
    /// Pinned by the console: the area no longer drives the factor.
    pinned: bool,
}

impl Default for CharacterAmbient {
    fn default() -> Self {
        Self {
            factor: 1.0,
            pinned: false,
        }
    }
}

impl CharacterAmbient {
    /// One frame of the area law: the target `1 + 2 × multiplier`, reached by a step of
    /// `dt × delta` while the gap passes 0.01 and the frame is under a second, else at once.
    /// Pinned, the area is not read.
    pub fn follow_area(&mut self, multiplier: f32, dt: f32) {
        if self.pinned {
            return;
        }
        let target = multiplier + multiplier + 1.0;
        let delta = target - self.factor;
        self.factor = if delta.abs() > 0.01 && dt < 1.0 {
            self.factor + dt * delta
        } else {
            target
        };
    }

    /// The `characterAmbient <v>` command (`0x77f750`): `v` in [0, 1] sets `1 + 2v`; a non-zero
    /// `v` stops the area driving it, a zero one hands it back. `Err` is the reference's line.
    pub fn pin(&mut self, v: f32) -> Result<(), &'static str> {
        if !(0.0..=1.0).contains(&v) {
            return Err("Ambient multiply range 0.0 - 1.0.");
        }
        self.factor = v + v + 1.0;
        self.pinned = v > 0.0;
        Ok(())
    }
}

/// A unit's ambient: the scene's times the factor, each channel held at 1 (`0x7c1968`).
pub(crate) fn unit_ambient(ambient: [f32; 3], factor: f32) -> [f32; 3] {
    ambient.map(|c| (c * factor).min(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_area_sets_one_plus_twice_its_multiplier_and_eases_there() {
        let mut c = CharacterAmbient::default();
        // A long frame lands at once: Hellfire's 0.6 is 2.2.
        c.follow_area(0.6, 1.5);
        assert!((c.factor - 2.2).abs() < 1e-6);
        // A short frame steps `dt × delta`: a tenth of a second covers a tenth of the gap.
        c.follow_area(0.0, 0.1);
        assert!((c.factor - (2.2 + 0.1 * (1.0 - 2.2))).abs() < 1e-5);
        // Within 0.01 it snaps.
        let mut near = CharacterAmbient {
            factor: 1.005,
            pinned: false,
        };
        near.follow_area(0.0, 0.016);
        assert_eq!(near.factor, 1.0);
    }

    #[test]
    fn the_console_pins_the_factor_and_zero_releases_it() {
        let mut c = CharacterAmbient::default();
        assert!(c.pin(1.5).is_err());
        assert!(c.pin(-0.1).is_err());
        c.pin(0.5).unwrap();
        assert_eq!(c.factor, 2.0);
        c.follow_area(0.0, 2.0);
        assert_eq!(c.factor, 2.0, "pinned: the area is not read");
        c.pin(0.0).unwrap();
        assert_eq!(c.factor, 1.0);
        c.follow_area(0.6, 2.0);
        assert!(
            (c.factor - 2.2).abs() < 1e-6,
            "released: the area drives it again"
        );
    }

    #[test]
    fn a_unit_ambient_is_held_at_one_per_channel() {
        let a = unit_ambient([0.28, 0.5, 0.42], 2.2);
        assert!((a[0] - 0.616).abs() < 1e-6 && a[1] == 1.0 && (a[2] - 0.924).abs() < 1e-6);
        assert_eq!(unit_ambient([0.2, 0.3, 0.4], 1.0), [0.2, 0.3, 0.4]);
    }
}
