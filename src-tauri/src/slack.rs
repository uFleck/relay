//! Slack adapters kept by v2: the headless `claude -p` transport and the on-demand
//! thread, window, directory and message-search paths. The retired v1 app-token
//! Socket Mode/Web API runtime, JEV scoring, episode capture, analysis jobs and
//! scheduled message sync have no place here.

mod claude;
pub(crate) mod directory;
pub(crate) mod thread;
pub(crate) mod window;

/// Shared by the v2 directory, thread and window runners and by Settings status.
pub(crate) use claude::find_claude;
