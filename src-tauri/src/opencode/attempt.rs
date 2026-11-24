//! One isolated OpenCode attempt. Briefing and Grilling supply typed input and interpret their
//! own result. Trust, private configuration, environment isolation, boundary proof and process
//! lifetime stay behind this interface.

use std::fs;
use std::sync::Arc;

use relay_core::v2::V2Store;
use tauri::{AppHandle, Manager};

use super::{BoundaryCache, GrillServerConfig, Invocation, ProcessRunner, Runner, verify_boundary};
use crate::settings;

pub(crate) struct AttemptInput {
    pub task_id: String,
    pub attempt_id: String,
    pub prompt: String,
    pub agent: &'static str,
    pub scratch_group: &'static str,
    pub grill: Option<GrillServerConfig>,
}

pub(crate) fn run(
    app: &AppHandle,
    store: &Arc<V2Store>,
    input: AttemptInput,
    boundary: &BoundaryCache,
) -> Result<String, String> {
    let config = settings::opencode_config(store).map_err(|error| error.message)?;
    let model = config
        .model
        .ok_or_else(|| "Set an OpenCode provider/model in Settings.".to_string())?;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let scratch = dir.join(input.scratch_group).join(&input.attempt_id);
    fs::create_dir_all(&scratch).map_err(|error| error.to_string())?;
    let invocation = Invocation {
        task_id: input.task_id,
        attempt_id: input.attempt_id,
        prompt: input.prompt,
        model,
        credential: config.key,
        agent: input.agent.to_string(),
        grill: input.grill.clone(),
        scratch,
    };
    verify_boundary(
        boundary,
        app,
        store,
        &invocation.agent,
        input.grill.as_ref(),
    )
    .map_err(|reason| format!("CLI isolation could not be verified: {reason}"))?;
    ProcessRunner.run(&invocation)
}
