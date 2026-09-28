//! Overpass QL queries.
//!
//! One query asks for every way the importer consumes so a request returns
//! everything in a single response. `out geom` is what makes that possible: the
//! vertices come back inline instead of as node references that would each need
//! another request.

use crate::geo::GeoBounds;

/// Overpass selector templates, with `{}` standing for the bounding box.
///
/// The tags are requested whole rather than by value: which `leisure`, `natural`
/// or `landuse` value a mapper picked decides whether a polygon is a pitch, a
/// park or a wood, and the importer is the only place that judgement is made. A
/// value the importer does not model is dropped while parsing, so asking for the
/// tag is cheaper than enumerating every value it might take.
///
/// `sport` is queried on its own as well because a campus athletics ground is
/// mapped as a track, a pitch, a sports centre or a stadium depending on the
/// mapper; the sport tag is the only one they share.
///
/// A large water body or a courtyard building is often a multipolygon relation
/// instead of a way, so both element types are requested for the area kinds.
const SELECTORS: [&str; 13] = [
    "way[\"highway\"]({})",
    "way[\"leisure\"]({})",
    "way[\"sport\"]({})",
    "way[\"building\"]({})",
    "way[\"natural\"]({})",
    "way[\"landuse\"]({})",
    "way[\"waterway\"~\"riverbank|canal|dock\"]({})",
    "relation[\"leisure\"]({})",
    "relation[\"sport\"]({})",
    "relation[\"building\"]({})",
    "relation[\"natural\"]({})",
    "relation[\"landuse\"]({})",
    "relation[\"waterway\"~\"riverbank|canal|dock\"]({})",
];

/// Builds the Overpass QL text for a box.
///
/// The server-side timeout is deliberately generous: the extract is one
/// request over a campus-sized box, and a shorter one would return a partial
/// answer with no indication that anything was left out.
pub fn overpass_query(bounds: &GeoBounds) -> String {
    let bbox = bounds.overpass_bbox();
    let mut query = String::from("[out:json][timeout:180];\n(\n");
    for selector in SELECTORS {
        query.push_str("  ");
        query.push_str(&selector.replace("{}", &bbox));
        // Overpass QL terminates every statement, including the ones inside a
        // union block; leaving it out is a syntax error the server rejects
        // with a bare 400.
        query.push_str(";\n");
    }
    query.push_str(");\nout geom;");
    query
}
