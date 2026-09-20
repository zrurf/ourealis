//! Individual presets and their parameter schema.
//!
//! The reply drives the web client's person form: the resolved parameters of every
//! preset, the names of the fields a request may override, and the default
//! simulator settings. Overrides travel as a sparse object keyed by those names,
//! so the form does not have to know a second vocabulary.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::routing::get;
use ourealis_core::person::{PersonParams, Preset};
use serde::{Deserialize, Serialize};

use crate::api::dto::simulation::{PersonOverrides, SimulationSettings};
use crate::api::dto::{Page, PageQuery};
use crate::api::error::query_rejection;
use crate::api::system::PRESETS;
use crate::app::AppState;
use crate::error::{Result, ServiceError};

/// One preset as the form needs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetDto {
    /// Preset name to put in `person.preset`.
    pub preset: String,
    /// Resolved parameter vector of the preset.
    pub params: PersonParams,
    /// Names the `person.overrides` object accepts.
    pub override_fields: Vec<String>,
    /// Default simulator settings of a run that does not name any.
    pub defaults: SimulationSettings,
}

/// The presets and their parameter schema.
pub async fn list(
    State(state): State<Arc<AppState>>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<Page<PresetDto>>> {
    let query = query.map_err(query_rejection)?.0;
    let (offset, limit) = state.page(query);
    let defaults = SimulationSettings::default();
    let items: Vec<PresetDto> = PRESETS
        .iter()
        .map(|name| {
            Ok(PresetDto {
                preset: name.to_string(),
                params: PersonParams::preset(preset_of(name)?),
                override_fields: PersonOverrides::FIELDS
                    .iter()
                    .map(|field| field.to_string())
                    .collect(),
                defaults: defaults.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let total = items.len();
    let page = items.into_iter().skip(offset).take(limit).collect();
    Ok(Json(Page::new(page, total, offset)))
}

/// Every route of this module.
pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/presets", get(list))
}

/// The core preset a wire name selects.
fn preset_of(name: &str) -> Result<Preset> {
    match name {
        "jog" => Ok(Preset::Jog),
        "moderate" => Ok(Preset::Moderate),
        "race" => Ok(Preset::Race),
        other => Err(ServiceError::Invalid(format!(
            "unknown preset {other:?}; expected one of {}",
            PRESETS.join(", ")
        ))),
    }
}
