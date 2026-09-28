//! Turning OSM tags into the resistance channels the map stores.
//!
//! OSM describes a way with a `highway` (or `leisure`) value plus a long tail of
//! optional detail tags. The importer collapses that into one [`RoadClass`]: the
//! surface category, the three scalar channels and a half-width. The mapping is a
//! judgement call baked into the importer rather than data read from anywhere, so
//! the tables below are the single place where it lives. Categories are the shared
//! ids of [`ourealis_map_format::surface`]; [`PALETTE`] is this importer's
//! resistance for each of them, which the cost model is tuned against.

use std::collections::HashMap;

use ourealis_map_format::surface;

/// Resistance of each surface category, indexed by id, in [`surface`] order.
///
/// Lower is faster: a rubber track scores best, a stair or an unmaintained
/// surface scores worst.
pub const PALETTE: [f32; surface::COUNT] = [0.35, 0.45, 0.10, 0.65, 0.80, 0.85, 1.00, 1.00, 1.00];

/// Crowding of a cell that no way or area covers: open ground on campus.
pub const OPEN_CROWDING: f32 = 0.15;

/// Lighting of a cell that no way or area covers.
pub const OPEN_LIGHTING: f32 = 0.5;

/// Traffic, crowding and lighting an area of a given surface paints.
pub const fn area_channels(category: u8) -> (f32, f32, f32) {
    match category {
        surface::TRACK => (0.0, 0.05, 0.60),
        surface::STEPS => (0.0, 0.40, 0.45),
        surface::ASPHALT => (0.15, 0.30, 0.70),
        _ => (0.0, OPEN_CROWDING, OPEN_LIGHTING),
    }
}

/// Attributes a way contributes to the raster.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoadClass {
    /// Surface category id from [`surface`].
    pub surface: u8,
    /// Motor traffic exposure in `[0, 1]`.
    pub traffic: f32,
    /// Pedestrian crowding in `[0, 1]`.
    pub crowding: f32,
    /// Street lighting in `[0, 1]`.
    pub lighting: f32,
    /// Half the traversable width in metres.
    pub half_width_m: f64,
}

impl RoadClass {
    /// A class with every scalar channel at its floor.
    const fn new(surface: u8, half_width_m: f64) -> Self {
        Self {
            surface,
            traffic: 0.0,
            crowding: 0.0,
            lighting: 0.0,
            half_width_m,
        }
    }

    /// Sets the three scalar channels.
    const fn channels(mut self, traffic: f32, crowding: f32, lighting: f32) -> Self {
        self.traffic = traffic;
        self.crowding = crowding;
        self.lighting = lighting;
        self
    }
}

/// Resolves the class of a way, or `None` for a way the map does not care about.
///
/// Detail tags override the `highway` default, which is what makes a
/// `highway=footway` with `surface=gravel` import as gravel rather than paving.
/// A way that runs below ground has no class at all, see [`is_underground`].
pub fn classify(tags: &HashMap<String, String>) -> Option<RoadClass> {
    if is_underground(tags) {
        return None;
    }
    let mut class = if let Some(highway) = tags.get("highway") {
        highway_class(highway)?
    } else if let Some(leisure) = tags.get("leisure") {
        leisure_class(leisure, tags)?
    } else if tags.contains_key("sport") {
        sports_class(tags)
    } else {
        return None;
    };
    apply_surface_tag(&mut class, tags);
    apply_lit_tag(&mut class, tags);
    apply_width_tag(&mut class, tags);
    Some(class)
}

/// True when a way runs under the ground rather than on it.
///
/// A tunnel is not a surface: the terrain cell above it is the hill it passes through, so
/// painting the way there would both invent a road across the slope and merge it with the
/// roads it passes under, which is a crossing the raster cannot then tell apart. `layer`
/// says the same thing on its own where a mapper tagged the level without the tunnel.
fn is_underground(tags: &HashMap<String, String>) -> bool {
    if tags.get("tunnel").is_some_and(|value| value != "no") {
        return true;
    }
    tags.get("layer")
        .and_then(|value| value.trim().parse::<i32>().ok())
        .is_some_and(|level| level < 0)
}

/// `leisure` values that can describe a running surface.
///
/// Campus athletics grounds are mapped under any of these: a bare track, the
/// pitch it encloses, the surrounding sports centre, or the stadium as a whole.
const VENUE_LEISURE: [&str; 5] = ["track", "pitch", "sports_centre", "stadium", "sports_hall"];

/// `leisure` values that are planted ground rather than a route.
pub const GREEN_LEISURE: [&str; 8] = [
    "park",
    "garden",
    "nature_reserve",
    "common",
    "golf_course",
    "recreation_ground",
    "village_green",
    "dog_park",
];

/// `natural` values that are planted ground rather than a route.
pub const GREEN_NATURAL: [&str; 7] = [
    "wood",
    "scrub",
    "grass",
    "grassland",
    "heath",
    "wetland",
    "fell",
];

/// `landuse` values that are planted ground rather than a route.
pub const GREEN_LANDUSE: [&str; 7] = [
    "grass",
    "forest",
    "meadow",
    "recreation_ground",
    "village_green",
    "allotments",
    "orchard",
];

/// True when the tags describe a lawn, wood or other planted ground.
///
/// A park, a wood or a grass verge is mapped ground that is not a route: it is
/// imported so the raster can paint it green and *block* it, which is what stops
/// the planner from cutting across a lawn. A sports pitch is not on this list —
/// a field is walked on, a lawn is not.
pub fn is_green(tags: &HashMap<String, String>) -> bool {
    value_in(tags, "leisure", &GREEN_LEISURE)
        || value_in(tags, "natural", &GREEN_NATURAL)
        || value_in(tags, "landuse", &GREEN_LANDUSE)
}

fn value_in(tags: &HashMap<String, String>, key: &str, values: &[&str]) -> bool {
    tags.get(key)
        .is_some_and(|value| values.contains(&value.as_str()))
}

/// True when the tags describe a venue whose surface is a running track.
///
/// The leisure value alone is ambiguous — the same tag covers velodromes and
/// equestrian tracks — so the sport decides.
pub fn is_running_venue(tags: &HashMap<String, String>) -> bool {
    tags.get("leisure")
        .is_some_and(|value| VENUE_LEISURE.contains(&value.as_str()))
        && sport_is_running(tags)
}

fn highway_class(highway: &str) -> Option<RoadClass> {
    Some(match highway {
        "motorway" | "motorway_link" | "trunk" | "trunk_link" => {
            RoadClass::new(surface::ASPHALT, 8.0).channels(0.95, 0.10, 0.90)
        }
        "primary" | "primary_link" => {
            RoadClass::new(surface::ASPHALT, 6.0).channels(0.85, 0.15, 0.90)
        }
        "secondary" | "secondary_link" => {
            RoadClass::new(surface::ASPHALT, 5.0).channels(0.70, 0.20, 0.85)
        }
        "tertiary" | "tertiary_link" => {
            RoadClass::new(surface::ASPHALT, 4.0).channels(0.50, 0.25, 0.80)
        }
        "residential" | "unclassified" => {
            RoadClass::new(surface::ASPHALT, 3.5).channels(0.35, 0.30, 0.75)
        }
        "service" => RoadClass::new(surface::ASPHALT, 2.5).channels(0.15, 0.30, 0.65),
        "living_street" => RoadClass::new(surface::ASPHALT, 3.0).channels(0.10, 0.45, 0.70),
        "pedestrian" => RoadClass::new(surface::PAVING, 3.0).channels(0.0, 0.60, 0.70),
        "footway" | "path" | "cycleway" | "bridleway" => {
            RoadClass::new(surface::PAVING, 1.2).channels(0.0, 0.35, 0.45)
        }
        "steps" => RoadClass::new(surface::STEPS, 1.5).channels(0.0, 0.40, 0.45),
        "track" => RoadClass::new(surface::GRAVEL, 2.0).channels(0.0, 0.10, 0.30),
        _ => return None,
    })
}

/// Class of a `leisure` way the importer models; `None` for one it does not.
///
/// A venue the importer does not model still has a walkable surface, so it is
/// imported as paving rather than dropped.
fn leisure_class(leisure: &str, tags: &HashMap<String, String>) -> Option<RoadClass> {
    if is_running_venue(tags) {
        return Some(RoadClass::new(surface::TRACK, 5.0).channels(0.0, 0.10, 0.60));
    }
    if VENUE_LEISURE.contains(&leisure) {
        // A pitch inside a sports centre is a field, not a track: it is painted as
        // bare ground, and its own `surface` tag refines that when it has one. The
        // distinction matters because the surrounding centre may itself be an oval,
        // and the field has to override the ring it sits in.
        return Some(RoadClass::new(surface::GROUND, 5.0).channels(0.0, 0.20, 0.40));
    }
    if leisure == "playground" {
        return Some(RoadClass::new(surface::PAVING, 5.0).channels(0.0, 0.35, 0.45));
    }
    None
}

/// Class of a way whose only sports tag is `sport`, which is how a bare field is
/// mapped when the mapper left `leisure` off.
fn sports_class(tags: &HashMap<String, String>) -> RoadClass {
    if sport_is_running(tags) {
        RoadClass::new(surface::TRACK, 5.0).channels(0.0, 0.10, 0.60)
    } else {
        RoadClass::new(surface::GROUND, 5.0).channels(0.0, 0.20, 0.40)
    }
}

/// True when the `sport` tag lists running or athletics.
fn sport_is_running(tags: &HashMap<String, String>) -> bool {
    sport_lists(tags, &["running", "athletics"])
}

/// True when the `sport` tag lists any of `values`.
///
/// `sport` is a semicolon-separated list, and a campus ground carries more than one value —
/// `soccer;running` is the common pairing for a stadium — so the values are split before they
/// are compared.
fn sport_lists(tags: &HashMap<String, String>, values: &[&str]) -> bool {
    tags.get("sport").is_some_and(|sports| {
        sports
            .split(';')
            .any(|sport| values.contains(&sport.trim()))
    })
}

/// Maps an OSM `surface` value onto a category; `None` for values the importer
/// has no opinion about, which leaves the `highway` default in place.
fn surface_category(value: &str) -> Option<u8> {
    Some(match value {
        "asphalt" | "chipseal" => surface::ASPHALT,
        "concrete" | "concrete:plates" | "concrete:lanes" | "paving_stones" | "sett"
        | "unhewn_cobblestone" | "cobblestone" | "bricks" | "wood" | "metal" | "rubber" => {
            surface::PAVING
        }
        "tartan" => surface::TRACK,
        "gravel" | "fine_gravel" | "pebblestone" | "compacted" | "crushed_limestone" => {
            surface::GRAVEL
        }
        "dirt" | "earth" | "mud" | "sand" | "ground" => surface::GROUND,
        "grass" | "soil" | "grass_paver" | "artificial_turf" | "turf" => surface::GRASS,
        _ => return None,
    })
}

fn apply_surface_tag(class: &mut RoadClass, tags: &HashMap<String, String>) {
    if let Some(mut category) = tags
        .get("surface")
        .and_then(|value| surface_category(value))
    {
        // `tartan` is the rubber of a running track, but the same tag is used on a
        // basketball or tennis court. Only a running venue is a track: anywhere else the
        // surface is a synthetic court, which is bare ground as far as a route is
        // concerned. Painting it as a track would both colour the court terracotta and
        // give it a tangent, steering a route round a circle that is not there.
        if category == surface::TRACK && !is_running_venue(tags) {
            category = surface::GROUND;
        }
        class.surface = category;
    }
    match tags.get("paved").map(String::as_str) {
        Some("yes") if matches!(class.surface, surface::GRAVEL | surface::GROUND) => {
            class.surface = surface::ASPHALT;
        }
        Some("no") if class.surface == surface::ASPHALT => class.surface = surface::GRAVEL,
        _ => {}
    }
}

fn apply_lit_tag(class: &mut RoadClass, tags: &HashMap<String, String>) {
    match tags.get("lit").map(String::as_str) {
        Some("yes") => class.lighting = class.lighting.max(0.85),
        Some("no" | "disused") => class.lighting = class.lighting.min(0.15),
        _ => {}
    }
}

fn apply_width_tag(class: &mut RoadClass, tags: &HashMap<String, String>) {
    let width = tags
        .get("width")
        .or_else(|| tags.get("est_width"))
        .and_then(|value| parse_metres(value));
    if let Some(width) = width {
        class.half_width_m = (width * 0.5).clamp(0.5, 12.0);
    }
}

/// Parses the leading number of a value such as `3`, `3.5` or `3 m`.
pub(crate) fn parse_metres(value: &str) -> Option<f64> {
    let number: String = value
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let parsed: f64 = number.parse().ok()?;
    (parsed > 0.0 && parsed.is_finite()).then_some(parsed)
}
