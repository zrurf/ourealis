//! Tag classification and Overpass response decoding.

use std::collections::HashMap;

use ourealis_geo_import::GeoBounds;
use ourealis_geo_import::osm::{AreaKind, class, parse_response};
use ourealis_map_format::surface;

fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn classifies_the_common_highway_values() {
    let residential = class::classify(&tags(&[("highway", "residential")])).expect("a road");
    assert_eq!(residential.surface, surface::ASPHALT);
    assert!(residential.traffic > 0.2 && residential.traffic < 0.5);

    let steps = class::classify(&tags(&[("highway", "steps")])).expect("a way");
    assert_eq!(steps.surface, surface::STEPS);

    let footway = class::classify(&tags(&[("highway", "footway")])).expect("a path");
    assert_eq!(footway.surface, surface::PAVING);
    assert_eq!(footway.traffic, 0.0);
}

#[test]
fn an_unmapped_highway_value_is_ignored() {
    assert!(class::classify(&tags(&[("highway", "construction")])).is_none());
    assert!(class::classify(&tags(&[("amenity", "bench")])).is_none());
}

#[test]
fn detail_tags_override_the_highway_default() {
    let gravel =
        class::classify(&tags(&[("highway", "footway"), ("surface", "gravel")])).expect("a path");
    assert_eq!(gravel.surface, surface::GRAVEL);

    let paved = class::classify(&tags(&[("highway", "track"), ("paved", "yes")])).expect("a track");
    assert_eq!(paved.surface, surface::ASPHALT);

    let unlit =
        class::classify(&tags(&[("highway", "residential"), ("lit", "no")])).expect("a road");
    assert!(unlit.lighting <= 0.15);

    let wide =
        class::classify(&tags(&[("highway", "residential"), ("width", "9 m")])).expect("a road");
    assert_eq!(wide.half_width_m, 4.5);
}

#[test]
fn a_running_track_is_rubber() {
    let running = class::classify(&tags(&[("leisure", "track"), ("sport", "running;soccer")]))
        .expect("a track");
    assert_eq!(running.surface, surface::TRACK);

    // A track of another sport is still a venue, but it is not rubber and it is
    // not a running surface: it is bare ground.
    let other =
        class::classify(&tags(&[("leisure", "track"), ("sport", "cycling")])).expect("a track");
    assert_eq!(other.surface, surface::GROUND);
}

#[test]
fn a_sports_centre_that_hosts_running_is_a_track() {
    // Campus grounds are routinely tagged with the surrounding facility rather
    // than `leisure=track`, so the sport is what has to decide.
    let centre = class::classify(&tags(&[
        ("leisure", "sports_centre"),
        ("sport", "soccer;running"),
    ]))
    .expect("a venue");
    assert_eq!(centre.surface, surface::TRACK);
    assert!(class::is_running_venue(&tags(&[
        ("leisure", "sports_centre"),
        ("sport", "soccer;running"),
    ])));

    let pitch =
        class::classify(&tags(&[("leisure", "pitch"), ("sport", "athletics")])).expect("a venue");
    assert_eq!(pitch.surface, surface::TRACK);

    // A court of another sport is walkable ground, not a track.
    let court =
        class::classify(&tags(&[("leisure", "pitch"), ("sport", "basketball")])).expect("a venue");
    assert_eq!(court.surface, surface::GROUND);

    // A leisure value that is not a venue carries no route, so `classify` has no
    // opinion about it; the parser still imports a park as green ground.
    assert!(class::classify(&tags(&[("leisure", "park")])).is_none());
}

#[test]
fn a_rubber_court_is_not_a_track() {
    // `tartan` names the rubber of a running track, but the same value is used on a
    // basketball or tennis court. Only a running venue is a track; anywhere else the
    // surface is bare ground, so the court is not painted terracotta and gets no tangent
    // that would send a route round a circle that is not there.
    let court = class::classify(&tags(&[
        ("leisure", "pitch"),
        ("sport", "basketball"),
        ("surface", "tartan"),
    ]))
    .expect("a court");
    assert_eq!(court.surface, surface::GROUND);

    // The same surface on a running venue is still the track it names.
    let track = class::classify(&tags(&[
        ("leisure", "sports_centre"),
        ("sport", "soccer;running"),
        ("surface", "tartan"),
    ]))
    .expect("a track");
    assert_eq!(track.surface, surface::TRACK);
}

#[test]
fn a_way_below_ground_paints_nothing() {
    // A tunnel runs through the hill, so painting its cell would lay asphalt
    // across the slope and merge the road with every road it passes under.
    assert!(
        class::classify(&tags(&[
            ("highway", "primary"),
            ("tunnel", "yes"),
            ("layer", "-2"),
        ]))
        .is_none()
    );
    // `layer` alone says the same where the level was tagged without the tunnel.
    assert!(class::classify(&tags(&[("highway", "primary"), ("layer", "-1")])).is_none());
}

#[test]
fn a_way_on_or_above_the_ground_is_kept() {
    // A bridge is a surface a runner can use, so it stays in the map.
    let bridge = class::classify(&tags(&[
        ("highway", "primary"),
        ("bridge", "yes"),
        ("layer", "1"),
    ]))
    .expect("a bridge");
    assert_eq!(bridge.surface, surface::ASPHALT);

    // `tunnel=no` is an explicit statement of the ordinary case.
    let open = class::classify(&tags(&[("highway", "primary"), ("tunnel", "no")]))
        .expect("a road on the ground");
    assert_eq!(open.surface, surface::ASPHALT);
}

const RESPONSE: &str = r#"{
  "version": 0.6,
  "elements": [
    {
      "type": "way",
      "id": 1,
      "tags": { "highway": "residential" },
      "geometry": [
        { "lat": 29.5300, "lon": 106.6000 },
        { "lat": 29.5300, "lon": 106.6100 }
      ]
    },
    {
      "type": "way",
      "id": 2,
      "tags": { "building": "university" },
      "geometry": [
        { "lat": 29.5310, "lon": 106.6010 },
        { "lat": 29.5310, "lon": 106.6020 },
        { "lat": 29.5320, "lon": 106.6020 },
        { "lat": 29.5320, "lon": 106.6010 },
        { "lat": 29.5310, "lon": 106.6010 }
      ]
    },
    {
      "type": "way",
      "id": 3,
      "tags": { "natural": "water" },
      "geometry": [
        { "lat": 29.5350, "lon": 106.6050 },
        { "lat": 29.5350, "lon": 106.6060 },
        { "lat": 29.5360, "lon": 106.6060 },
        { "lat": 29.5360, "lon": 106.6050 },
        { "lat": 29.5350, "lon": 106.6050 }
      ]
    },
    {
      "type": "way",
      "id": 4,
      "tags": { "leisure": "track", "sport": "running" },
      "geometry": [
        { "lat": 29.5330, "lon": 106.6080 },
        { "lat": 29.5330, "lon": 106.6090 },
        { "lat": 29.5340, "lon": 106.6090 },
        { "lat": 29.5340, "lon": 106.6080 },
        { "lat": 29.5330, "lon": 106.6080 }
      ]
    },
    {
      "type": "way",
      "id": 5,
      "tags": { "highway": "path" },
      "geometry": [
        { "lat": 29.5290, "lon": 106.6000 },
        null,
        { "lat": 29.5290, "lon": 106.6010 }
      ]
    },
    {
      "type": "relation",
      "id": 6,
      "tags": { "natural": "water", "type": "multipolygon" }
    }
  ]
}"#;

#[test]
fn decodes_ways_into_roads_and_areas() {
    let data = parse_response(RESPONSE).expect("a usable response");
    assert_eq!(data.roads.len(), 1);
    assert_eq!(data.areas.len(), 3);

    let road = &data.roads[0];
    assert_eq!(road.class.surface, surface::ASPHALT);
    assert_eq!(road.points.len(), 2);

    // The closing vertex is dropped so an outline has no zero-length edge.
    let building = data
        .areas
        .iter()
        .find(|area| area.kind == AreaKind::Building)
        .expect("a building");
    assert_eq!(building.outline.len(), 4);

    assert!(data.areas.iter().any(|area| area.kind == AreaKind::Water));
    assert!(
        data.areas
            .iter()
            .any(|area| area.kind == AreaKind::Surface(surface::TRACK))
    );
}

#[test]
fn a_response_with_nothing_usable_is_an_error() {
    let empty = r#"{ "elements": [ { "type": "relation", "id": 1, "tags": {} } ] }"#;
    assert!(parse_response(empty).is_err());
    assert!(parse_response("not json").is_err());
}

const OPEN_WAYS: &str = r#"{
  "elements": [
    {
      "type": "way",
      "id": 11,
      "tags": { "natural": "water" },
      "geometry": [
        { "lat": 29.5300, "lon": 106.6000 },
        { "lat": 29.5300, "lon": 106.6010 },
        { "lat": 29.5310, "lon": 106.6010 }
      ]
    },
    {
      "type": "way",
      "id": 12,
      "tags": { "building": "no", "highway": "footway" },
      "geometry": [
        { "lat": 29.5300, "lon": 106.6000 },
        { "lat": 29.5300, "lon": 106.6010 }
      ]
    }
  ]
}"#;

#[test]
fn an_outline_that_does_not_close_is_not_filled_as_an_area() {
    let data = parse_response(OPEN_WAYS).expect("a usable response");
    // Filling the open water way would paint a sliver across the map.
    assert!(data.areas.is_empty());
    // `building=no` carries no footprint, so the way stays the footway it is.
    assert_eq!(data.roads.len(), 1);
    assert_eq!(data.roads[0].class.surface, surface::PAVING);
}

#[test]
fn the_query_terminates_every_statement() {
    let bounds = GeoBounds::new(29.5212, 106.5926, 29.5429, 106.6194).expect("a box");
    let text = ourealis_geo_import::osm::query::overpass_query(&bounds);

    // An unterminated union member is a syntax error the server answers with a
    // bare 400 rather than a partial extract.
    let selectors: Vec<&str> = text
        .lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with("way[") || line.starts_with("relation["))
        .collect();
    assert_eq!(selectors.len(), 13);
    for selector in selectors {
        assert!(selector.ends_with(");"), "unterminated: {selector}");
    }

    // A running venue is found through its sport, which every mapping style of
    // one carries; the greens are asked for through the tags the importer judges
    // a park, a wood or a lawn by, and the area kinds as relations too.
    assert!(text.contains("way[\"sport\"]("));
    assert!(text.contains("way[\"leisure\"]("));
    assert!(text.contains("way[\"natural\"]("));
    assert!(text.contains("way[\"landuse\"]("));
    assert!(text.contains("relation[\"building\"]"));
    assert!(text.contains("(29.5212,106.5926,29.5429,106.6194)"));
    assert!(text.ends_with("out geom;"));
}

const MULTIPOLYGON: &str = r#"{
  "elements": [
    {
      "type": "relation",
      "id": 21,
      "tags": { "type": "multipolygon", "building": "university" },
      "members": [
        {
          "type": "way",
          "ref": 1,
          "role": "outer",
          "geometry": [
            { "lat": 29.5310, "lon": 106.6010 },
            { "lat": 29.5310, "lon": 106.6020 }
          ]
        },
        {
          "type": "way",
          "ref": 2,
          "role": "outer",
          "geometry": [
            { "lat": 29.5310, "lon": 106.6020 },
            { "lat": 29.5320, "lon": 106.6020 },
            { "lat": 29.5320, "lon": 106.6010 },
            { "lat": 29.5310, "lon": 106.6010 }
          ]
        },
        {
          "type": "way",
          "ref": 3,
          "role": "inner",
          "geometry": [
            { "lat": 29.5312, "lon": 106.6012 },
            { "lat": 29.5312, "lon": 106.6014 },
            { "lat": 29.5314, "lon": 106.6014 },
            { "lat": 29.5314, "lon": 106.6012 },
            { "lat": 29.5312, "lon": 106.6012 }
          ]
        },
        { "type": "node", "ref": 4, "role": "label" }
      ]
    },
    {
      "type": "relation",
      "id": 22,
      "tags": { "type": "multipolygon", "natural": "water" },
      "members": [
        {
          "type": "way",
          "ref": 5,
          "role": "outer",
          "geometry": [
            { "lat": 29.5350, "lon": 106.6050 },
            { "lat": 29.5350, "lon": 106.6060 },
            { "lat": 29.5360, "lon": 106.6060 }
          ]
        }
      ]
    }
  ]
}"#;

#[test]
fn a_multipolygon_is_stitched_from_its_outer_members() {
    let data = parse_response(MULTIPOLYGON).expect("a usable response");
    assert_eq!(data.areas.len(), 1);
    assert_eq!(data.areas[0].kind, AreaKind::Building);
    // Two halves joined into one ring; the inner ring is dropped because the
    // format cannot express a hole, and the open relation never closes.
    assert_eq!(data.areas[0].outline.len(), 4);
    assert_eq!(data.areas[0].outline[0], (106.602, 29.531));
    assert_eq!(data.areas[0].outline[2], (106.601, 29.532));
}

/// A park, a wood, a lawn and a footway that happens to carry a lawn's tags.
const GREENS: &str = r#"{
  "elements": [
    {
      "type": "way",
      "id": 41,
      "tags": { "leisure": "park" },
      "geometry": [
        { "lat": 29.5300, "lon": 106.6000 },
        { "lat": 29.5300, "lon": 106.6010 },
        { "lat": 29.5310, "lon": 106.6010 },
        { "lat": 29.5310, "lon": 106.6000 },
        { "lat": 29.5300, "lon": 106.6000 }
      ]
    },
    {
      "type": "way",
      "id": 42,
      "tags": { "natural": "wood" },
      "geometry": [
        { "lat": 29.5320, "lon": 106.6020 },
        { "lat": 29.5320, "lon": 106.6030 },
        { "lat": 29.5330, "lon": 106.6030 },
        { "lat": 29.5330, "lon": 106.6020 },
        { "lat": 29.5320, "lon": 106.6020 }
      ]
    },
    {
      "type": "way",
      "id": 43,
      "tags": { "landuse": "grass" },
      "geometry": [
        { "lat": 29.5340, "lon": 106.6040 },
        { "lat": 29.5340, "lon": 106.6050 },
        { "lat": 29.5350, "lon": 106.6050 },
        { "lat": 29.5350, "lon": 106.6040 },
        { "lat": 29.5340, "lon": 106.6040 }
      ]
    },
    {
      "type": "way",
      "id": 44,
      "tags": { "highway": "footway", "landuse": "grass" },
      "geometry": [
        { "lat": 29.5300, "lon": 106.6000 },
        { "lat": 29.5310, "lon": 106.6010 }
      ]
    }
  ]
}"#;

#[test]
fn planted_ground_is_imported_as_green_and_a_path_across_it_stays_a_path() {
    let data = parse_response(GREENS).expect("a usable response");

    // A park, a wood and a lawn all arrive as green ground…
    assert_eq!(data.areas.len(), 3);
    assert!(data.areas.iter().all(|area| area.kind == AreaKind::Green));
    // …and none of them carries a height, because green is not a building.
    assert!(data.areas.iter().all(|area| area.height_m.is_none()));

    // A way that already is a route stays a route: blocking it would cut the only
    // path across the park.
    assert_eq!(data.roads.len(), 1);
    assert_eq!(data.roads[0].class.surface, surface::PAVING);
}

/// Three buildings: a surveyed height, a storey count, and neither.
const BUILDINGS: &str = r#"{
  "elements": [
    {
      "type": "way",
      "id": 51,
      "tags": { "building": "yes", "height": "24.5 m" },
      "geometry": [
        { "lat": 29.5300, "lon": 106.6000 },
        { "lat": 29.5300, "lon": 106.6010 },
        { "lat": 29.5310, "lon": 106.6010 },
        { "lat": 29.5310, "lon": 106.6000 },
        { "lat": 29.5300, "lon": 106.6000 }
      ]
    },
    {
      "type": "way",
      "id": 52,
      "tags": { "building": "yes", "building:levels": "3" },
      "geometry": [
        { "lat": 29.5320, "lon": 106.6020 },
        { "lat": 29.5320, "lon": 106.6030 },
        { "lat": 29.5330, "lon": 106.6030 },
        { "lat": 29.5330, "lon": 106.6020 },
        { "lat": 29.5320, "lon": 106.6020 }
      ]
    },
    {
      "type": "way",
      "id": 53,
      "tags": { "building": "university" },
      "geometry": [
        { "lat": 29.5340, "lon": 106.6040 },
        { "lat": 29.5340, "lon": 106.6050 },
        { "lat": 29.5350, "lon": 106.6050 },
        { "lat": 29.5350, "lon": 106.6040 },
        { "lat": 29.5340, "lon": 106.6040 }
      ]
    }
  ]
}"#;

#[test]
fn a_building_carries_the_height_its_tags_state() {
    let data = parse_response(BUILDINGS).expect("a usable response");
    assert_eq!(data.areas.len(), 3);
    let heights: Vec<Option<f32>> = data.areas.iter().map(|area| area.height_m).collect();

    // `height` is surveyed and wins, with its unit stripped by the parser.
    assert!(heights.iter().any(|height| *height == Some(24.5)));
    // `building:levels` is the fallback, at a fixed storey height.
    assert!(
        heights
            .iter()
            .any(|height| height.is_some_and(|value| (value - 9.6).abs() < 1e-3))
    );
    // A building with neither still stands: the raster supplies a default.
    assert!(heights.contains(&None));
}
