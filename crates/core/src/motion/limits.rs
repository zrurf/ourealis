//! Speed limits: physiology, curvature, downhill braking and look-ahead.
//!
//! The upper bound on speed at a point is the minimum of four constraints. They
//! are computed separately and combined here, because each has a different
//! failure mode: the physiological model breaks at extreme grades, the curvature
//! limit protects the turn, the downhill cap protects the landing, and the
//! look-ahead term is what makes a runner react to a hill *before* reaching it.

use glam::DVec2;

use crate::math::sampling::lerp;
use crate::path::Path;
use crate::terrain::Terrain;

/// Absolute grade beyond which the Minetti polynomial is clamped.
pub const MINETTI_CLAMP: f64 = 0.25;

/// Lower bound of the parallel-curve denominator `1 - offset * curvature`.
pub const CURVATURE_DENOMINATOR_FLOOR: f64 = 0.3;

/// Cost of running one metre of level ground per kilogram of body mass,
/// joules per kilogram per metre.
pub const MINETTI_LEVEL_COST: f64 = 3.6;

/// How the look-ahead window is aggregated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookAheadMode {
    /// Distance-weighted mean of the grade over the window.
    DistanceWeightedMean = 0,
    /// Largest uphill grade in the window, modelling a cautious runner.
    WorstCase = 1,
}

/// Metabolic cost of running at a grade, in joules per kilogram per metre of
/// *horizontal* distance.
///
/// Minetti's polynomial is a fit to measured data and turns negative for steep
/// descents, which is physically meaningless; the input is clamped to the
/// validity range before evaluation.
pub fn minetti_cost(grade: f64) -> f64 {
    let i = grade.clamp(-MINETTI_CLAMP, MINETTI_CLAMP);
    let i2 = i * i;
    let i3 = i2 * i;
    let i4 = i2 * i2;
    let i5 = i4 * i;
    155.4 * i5 - 30.4 * i4 - 43.3 * i3 + 46.3 * i2 + 19.5 * i + MINETTI_LEVEL_COST
}

/// Speed along the path at a grade, assuming a constant metabolic rate.
///
/// The `sqrt(1 + i^2)` factor converts the horizontal-distance cost model into
/// a speed along the arc length; dropping it would misprice steep ground.
pub fn slope_speed(target_speed: f64, grade: f64) -> f64 {
    let cost = minetti_cost(grade).max(0.1);
    let horizontal = target_speed * MINETTI_LEVEL_COST / cost;
    horizontal * (1.0 + grade * grade).sqrt()
}

/// Curvature speed limit from the lateral acceleration budget.
pub fn curvature_speed_limit(a_lat_max: f64, curvature: f64) -> f64 {
    let kappa = curvature.abs();
    if kappa < 1e-6 {
        f64::INFINITY
    } else {
        (a_lat_max / kappa).sqrt()
    }
}

/// Downhill speed cap: constant-energy assumptions overestimate descending
/// speed because braking, landing impact and cadence all limit it.
pub fn downhill_cap(target_speed: f64, k_down: f64, grade: f64) -> f64 {
    if grade < 0.0 {
        k_down * target_speed
    } else {
        f64::INFINITY
    }
}

/// Aggregates the grade over a look-ahead window.
pub fn look_ahead_grade(
    path: &Path,
    terrain: &Terrain,
    from_s: f64,
    window_m: f64,
    mode: LookAheadMode,
) -> f64 {
    if window_m <= 0.0 {
        return grade_at(path, terrain, from_s);
    }
    let total = path.total_length();
    let end = (from_s + window_m).min(total);
    let samples = ((end - from_s) / 2.0).ceil().max(2.0) as usize;

    let mut sum = 0.0;
    let mut weight = 0.0;
    let mut worst = f64::NEG_INFINITY;
    for index in 0..=samples {
        let s = lerp(from_s, end, index as f64 / samples as f64).min(total);
        let grade = grade_at(path, terrain, s);
        // Nearer ground matters more: a runner reacts most to what is imminent.
        let w = 1.0 - 0.5 * (index as f64 / samples as f64);
        sum += grade * w;
        weight += w;
        worst = worst.max(grade);
    }
    match mode {
        LookAheadMode::DistanceWeightedMean => {
            if weight > 0.0 {
                sum / weight
            } else {
                0.0
            }
        }
        LookAheadMode::WorstCase => {
            // Only a significant climb changes the decision; on flat or rolling
            // ground the weighted mean is the better predictor.
            if worst > 0.05 {
                worst
            } else {
                sum / weight.max(1e-9)
            }
        }
    }
}

/// Grade along the path direction at an arc length.
///
/// A Z-axis link carries its own elevation profile, and where the path has one it
/// is the grade: the ramp up a stair is a real slope to the physiology, and the
/// terrain under the stairwell has nothing to say about it. Without this the
/// speed limit would price a climb as flat ground, which is the one place the
/// runner cannot go fast.
pub fn grade_at(path: &Path, terrain: &Terrain, s: f64) -> f64 {
    if let Some(grade) = path.grade_at(s) {
        return grade;
    }
    let position = path.position_at(s);
    let tangent = path.tangent_at(s);
    terrain.directional_slope_at(position, tangent)
}

/// Grade across the path (left normal), used for the roll channel.
pub fn cross_grade_at(path: &Path, terrain: &Terrain, s: f64) -> f64 {
    let position = path.position_at(s);
    let normal = path.normal_at(s);
    terrain.directional_slope_at(position, normal)
}

/// Parameters of the speed limit composition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedLimitParams {
    /// Slope look-ahead distance, metres.
    pub look_ahead_m: f64,
    /// Look-ahead aggregation mode.
    pub look_ahead_mode: LookAheadMode,
    /// Lateral acceleration budget, m/s^2.
    pub a_lat_max: f64,
    /// Downhill cap coefficient.
    pub k_down: f64,
    /// Grade clamp of the physiological model.
    pub grade_clamp: f64,
}

impl Default for SpeedLimitParams {
    fn default() -> Self {
        Self {
            look_ahead_m: 20.0,
            look_ahead_mode: LookAheadMode::WorstCase,
            a_lat_max: 2.5,
            k_down: 1.15,
            grade_clamp: MINETTI_CLAMP,
        }
    }
}

/// The four limits at one arc length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimitComponents {
    /// Physiological limit from the look-ahead grade.
    pub slope: f64,
    /// Curvature limit.
    pub curvature: f64,
    /// Fatigue limit at the current time.
    pub fatigue: f64,
    /// Downhill braking cap.
    pub downhill: f64,
}

impl LimitComponents {
    /// Combined limit: the minimum of the four.
    pub fn combined(&self) -> f64 {
        self.slope
            .min(self.curvature)
            .min(self.fatigue)
            .min(self.downhill)
    }
}

/// Computes the limit components at one arc length.
pub fn components_at(
    path: &Path,
    terrain: &Terrain,
    s: f64,
    target_speed: f64,
    fatigue_speed: f64,
    params: &SpeedLimitParams,
) -> LimitComponents {
    let grade = look_ahead_grade(
        path,
        terrain,
        s,
        params.look_ahead_m,
        params.look_ahead_mode,
    );
    let grade = grade.clamp(-params.grade_clamp, params.grade_clamp);
    LimitComponents {
        slope: slope_speed(target_speed, grade),
        curvature: curvature_speed_limit(params.a_lat_max, path.curvature_at(s)),
        fatigue: fatigue_speed,
        downhill: downhill_cap(target_speed, params.k_down, grade),
    }
}

/// Effective curvature of an offset trajectory (parallel curve).
///
/// `kappa_eff = kappa / (1 - d * kappa)`; the denominator is protected so an
/// extreme offset on a tight bend cannot flip the sign of the curvature.
pub fn effective_curvature(curvature: f64, offset: f64) -> f64 {
    // On the inside of a bend the offset shrinks the radius; a denominator below
    // the floor would mean the runner sits past the centre of the arc, which
    // reverses the geometry. Clamping from below keeps the curvature's sign and
    // magnitude meaningful.
    let denominator = (1.0 - offset * curvature).max(CURVATURE_DENOMINATOR_FLOOR);
    curvature / denominator
}

/// Allowed lateral offset on the inside of a bend for a given speed.
///
/// Returns `None` when the speed already exceeds the limit on the centre line,
/// in which case no positive inner offset is admissible.
pub fn allowed_inner_offset(curvature: f64, speed: f64, a_lat_max: f64) -> Option<f64> {
    let kappa = curvature.abs();
    if kappa < 1e-9 {
        return Some(f64::INFINITY);
    }
    let margin = 1.0 - speed * speed * kappa / a_lat_max;
    if margin <= 0.0 {
        return None;
    }
    Some(margin / kappa)
}

/// Direction of a curve: `true` when the centre of curvature lies to the left.
pub fn turns_left(curvature: f64) -> bool {
    curvature > 0.0
}

/// Heading-consistent grade sample, exposed for the attitude model.
pub fn grade_and_cross_grade(path: &Path, terrain: &Terrain, s: f64) -> (f64, f64, DVec2, DVec2) {
    (
        grade_at(path, terrain, s),
        cross_grade_at(path, terrain, s),
        path.tangent_at(s),
        path.normal_at(s),
    )
}
