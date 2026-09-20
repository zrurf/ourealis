//! Route planning without motion or sensors.
//!
//! Both endpoints run the planner exactly as a job does — the same environment,
//! the same mixed graph, the same configuration and seed — and stop before the
//! motion stage. That is what makes a preview a prediction of the run rather than
//! a second, independent planner: the duplication lives in the stages below, not
//! in the geometry.

use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::routing::post;
use glam::DVec2;
use ourealis_core::environment::Environment;
use ourealis_core::graph::MixedGraph;
use ourealis_core::motion::profile::SpeedProfile;
use ourealis_core::person::PersonParams;
use ourealis_core::plan::{PlannedRoute, plan, plan_loop};
use ourealis_core::sim::{MapSource, SimulationConfig, Simulator};

use crate::api::dto::result::{RoutePreview, RoutePreviewCandidate};
use crate::api::dto::simulation::{MapRef, SimulationRequest};
use crate::api::error::json_rejection;
use crate::app::AppState;
use crate::error::{Result, ServiceError};

/// Plans a route and reports the candidate set, without smoothing for motion.
pub async fn preview(
    State(state): State<Arc<AppState>>,
    body: Result<Json<SimulationRequest>, JsonRejection>,
) -> Result<Json<RoutePreview>> {
    let request = body.map_err(json_rejection)?.0;
    Ok(Json(plan_in_background(state, request, false).await?))
}

/// Plans a route, smooths it and samples the speed-limit curve along it.
pub async fn plan_route(
    State(state): State<Arc<AppState>>,
    body: Result<Json<SimulationRequest>, JsonRejection>,
) -> Result<Json<RoutePreview>> {
    let request = body.map_err(json_rejection)?.0;
    Ok(Json(plan_in_background(state, request, true).await?))
}

/// Every route of this module.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/routes/preview", post(preview))
        .route("/routes/plan", post(plan_route))
}

/// Runs the planner on a blocking worker.
///
/// Planning is CPU-bound and can take seconds on a large map, so it must not run
/// on a reactor thread. Shared by the two facades: the HTTP handlers and the gRPC
/// `Preview` call both go through it.
pub(crate) async fn plan_in_background(
    state: Arc<AppState>,
    request: SimulationRequest,
    with_profile: bool,
) -> Result<RoutePreview> {
    tokio::task::spawn_blocking(move || build_preview(&state, &request, with_profile))
        .await
        .map_err(|error| {
            ServiceError::Internal(format!(
                "the planning worker did not finish cleanly: {error}"
            ))
        })?
}

/// Builds the preview of one request.
fn build_preview(
    state: &AppState,
    request: &SimulationRequest,
    with_profile: bool,
) -> Result<RoutePreview> {
    let source = map_source(state, request)?;
    let person = request.person.resolve()?;
    let mut config = SimulationConfig::default();
    request.settings.apply(&mut config)?;

    let simulator = match &request.route {
        crate::api::dto::simulation::RouteSpec::Standard { .. } => Simulator::builder()
            .map(source)
            .person(person.clone())
            .standard(request.standard_request()?)
            .config(config.clone())
            .seed(request.seed)
            .individual(request.individual)
            .build()?,
        crate::api::dto::simulation::RouteSpec::Loop { .. } => Simulator::builder()
            .map(source)
            .person(person.clone())
            .looped(request.loop_request()?)
            .config(config.clone())
            .seed(request.seed)
            .individual(request.individual)
            .build()?,
        crate::api::dto::simulation::RouteSpec::Dynamic { .. } => {
            let (standard, checkpoints) = request.dynamic_request()?;
            Simulator::builder()
                .map(source)
                .person(person.clone())
                .dynamic(standard, checkpoints)
                .config(config.clone())
                .seed(request.seed)
                .individual(request.individual)
                .build()?
        }
    };

    // The environment is loaded by the simulator itself, which is what keeps the
    // weights, the backend selection and the coarse grid identical to a run.
    let environment = simulator.environment()?;
    let mut graph = MixedGraph::with_coarse(
        &environment.cost,
        &environment.hard,
        Some(&environment.terrain),
        environment.connectors.clone(),
        environment.prm.clone(),
        environment.coarse.clone(),
        person.target_speed,
    );

    let started = Instant::now();
    let planned = plan_route_of(&environment, &mut graph, request, &person, &config)?;
    let planning_ms = started.elapsed().as_secs_f64() * 1000.0;

    let from_library = request.route.mode_name() != "loop"
        && library_supplies(
            &environment,
            &mut graph,
            &person,
            &config,
            planned.legs.first().map(|leg| leg.from),
            planned.legs.last().map(|leg| leg.to),
        );

    let mut preview = RoutePreview {
        candidates: candidates_of(&planned, from_library),
        chosen: planned
            .legs
            .first()
            .map(|leg| leg.chosen)
            .unwrap_or_default(),
        length_m: planned.length_m,
        cost_equiv_m: planned.cost_equiv_m,
        straight_line_m: straight_line_m(&planned),
        planning_ms,
        path: Vec::new(),
        speed_limit_mps: Vec::new(),
        speed_limit_s: Vec::new(),
    };

    if with_profile {
        let profile = speed_profile(&environment, &person, &config, request, &planned)?;
        preview.speed_limit_s = profile.s.clone();
        preview.speed_limit_mps = profile.s.iter().map(|arc| profile.limit_at(*arc)).collect();
        preview.path = planned
            .path
            .points()
            .iter()
            .map(|point| crate::api::dto::Vec2::from(*point))
            .collect();
    }
    Ok(preview)
}

/// Plans the route of the request's mode.
fn plan_route_of(
    environment: &Environment,
    graph: &mut MixedGraph<'_>,
    request: &SimulationRequest,
    person: &PersonParams,
    config: &SimulationConfig,
) -> Result<PlannedRoute> {
    Ok(match &request.route {
        crate::api::dto::simulation::RouteSpec::Loop { .. } => plan_loop(
            environment,
            graph,
            &request.loop_request()?,
            person,
            &config.loop_route,
            request.seed,
            request.individual,
        )?,
        _ => plan(
            environment,
            graph,
            &request.standard_request()?,
            person,
            &config.route,
            request.seed,
            request.individual,
        )?,
    })
}

/// The candidate set of the route.
///
/// A multi-leg request is planned leg by leg with its own Logit draw, and the
/// preview carries one set, so the first leg's candidates are the ones reported;
/// `chosen` indexes that same set.
fn candidates_of(planned: &PlannedRoute, from_library: bool) -> Vec<RoutePreviewCandidate> {
    let Some(leg) = planned.legs.first() else {
        return Vec::new();
    };
    let probabilities = leg.candidates.probabilities();
    leg.candidates
        .candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| RoutePreviewCandidate {
            length_m: candidate.length_m,
            cost_equiv_m: candidate.cost_equiv_m,
            probability: probabilities.get(index).copied().unwrap_or(0.0),
            path_size: candidate.path_size,
            from_library,
            points: candidate
                .points
                .iter()
                .map(|point| crate::api::dto::Vec2::from(*point))
                .collect(),
        })
        .collect()
}

/// Straight-line distance between the ends of the planned route.
fn straight_line_m(planned: &PlannedRoute) -> f64 {
    match (planned.legs.first(), planned.legs.last()) {
        (Some(first), Some(last)) => first.from.distance(last.to),
        _ => 0.0,
    }
}

/// Whether the map's stored candidate library supplies the planned leg.
///
/// The planner does not report where a candidate came from, so the question is
/// answered by repeating the library lookup it makes: same endpoints, same
/// parameters, and the same graph — whose caches the run has already warmed — so
/// the answer is the one the run acted on.
fn library_supplies(
    environment: &Environment,
    graph: &mut MixedGraph<'_>,
    person: &PersonParams,
    config: &SimulationConfig,
    from: Option<DVec2>,
    to: Option<DVec2>,
) -> bool {
    let (Some(from), Some(to)) = (from, to) else {
        return false;
    };
    let hit = ourealis_core::plan::library::from_library(
        environment.kpath.as_ref(),
        graph,
        from,
        to,
        person.beta_logit,
        &config.route.candidates,
        &config.route.search,
        config.route.d_attach_m,
    );
    matches!(hit, Ok(Ok(_)))
}

/// Speed-limit curve of the planned route, sampled on the profile grid.
fn speed_profile(
    environment: &Environment,
    person: &PersonParams,
    config: &SimulationConfig,
    request: &SimulationRequest,
    planned: &PlannedRoute,
) -> Result<SpeedProfile> {
    let mut profile = config.motion.profile.clone();
    profile.modifiers = planned.modifiers.clone();
    profile.stops = planned.stops.clone();
    // A single-lap circuit closes on itself across the seam; everything else
    // starts from rest and ends at rest, exactly as the motion stage builds it.
    let laps = match &request.route {
        crate::api::dto::simulation::RouteSpec::Loop { laps, .. } => *laps,
        _ => 0,
    };
    profile.periodic = request.route.mode_name() == "loop" && laps <= 1;
    Ok(SpeedProfile::build(
        &planned.path,
        &environment.terrain,
        &config.motion.limits,
        person,
        &profile,
        &[],
    )?)
}

/// Resolves the map a request names.
///
/// The same rules the runner applies: an id must exist, an inline image is
/// decoded, a synthetic spec is built, and a request without a map needs a
/// library holding exactly one.
fn map_source(state: &AppState, request: &SimulationRequest) -> Result<MapSource> {
    match &request.map {
        Some(MapRef::Id { id }) => {
            let entry = state.maps.get(id)?;
            Ok(MapSource::bytes(entry.bytes.as_ref().clone()))
        }
        Some(MapRef::Synthetic { spec }) => Ok(MapSource::synthetic(spec.to_spec())),
        Some(MapRef::Inline { omf_base64, .. }) => Ok(MapSource::bytes(
            crate::api::maps::decode_base64(omf_base64)?,
        )),
        None => {
            let maps = state.maps.list();
            match maps.len() {
                0 => Err(ServiceError::Invalid(
                    "no map was given and the library is empty; add a map or name one".to_string(),
                )),
                1 => {
                    let entry = state.maps.get(&maps[0].id)?;
                    Ok(MapSource::bytes(entry.bytes.as_ref().clone()))
                }
                _ => Err(ServiceError::Invalid(format!(
                    "no map was given and the library holds {} maps; name one",
                    maps.len()
                ))),
            }
        }
    }
}
