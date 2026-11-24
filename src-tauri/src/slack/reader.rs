//! One entry point for on-demand Slack Reference reads. Core keeps the thread, window and
//! directory policy. This module chooses the matching resolver and commits its result.

use std::sync::Arc;

use relay_core::v2::{ReferenceKey, ThreadResolution, V2Store, WindowResolution};
use tauri::AppHandle;

use super::{thread, window};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlackKind {
    Thread,
    Window,
}

fn kind(key: &ReferenceKey) -> Option<SlackKind> {
    match key {
        ReferenceKey::Slack { .. } => Some(SlackKind::Thread),
        ReferenceKey::SlackWindow { .. } => Some(SlackKind::Window),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub(crate) enum SlackRead {
    Thread(ThreadResolution),
    Window(WindowResolution),
}

impl SlackRead {
    pub(crate) fn row(&self) -> &relay_core::v2::V2PickerRow {
        match self {
            Self::Thread(read) => &read.row,
            Self::Window(read) => &read.row,
        }
    }

    pub(crate) fn coalesced(&self) -> bool {
        match self {
            Self::Thread(read) => read.coalesced,
            Self::Window(read) => read.coalesced,
        }
    }

    pub(crate) fn recovered(&self) -> bool {
        match self {
            Self::Thread(read) => read.thread.is_some(),
            Self::Window(read) => read.window.is_some(),
        }
    }
}

pub(crate) async fn resolve(
    app: &AppHandle,
    store: &Arc<V2Store>,
    key: &ReferenceKey,
) -> Option<SlackRead> {
    match kind(key)? {
        SlackKind::Thread => Some(SlackRead::Thread(thread::resolve(app, store, key).await)),
        SlackKind::Window => Some(SlackRead::Window(window::resolve(app, store, key).await)),
    }
}

pub(crate) async fn persist(
    app: &AppHandle,
    store: &Arc<V2Store>,
    key: &ReferenceKey,
    read: &SlackRead,
) {
    match read {
        SlackRead::Thread(read) => thread::persist(app, store, key, read).await,
        SlackRead::Window(read) => window::persist(app, store, key, read).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_recovery_routes_to_window_policy() {
        let key = ReferenceKey::SlackWindow {
            id: "01JTEST".into(),
            channel_id: "C123".into(),
            host: "slack.com".into(),
            name: Some("general".into()),
        };
        assert_eq!(kind(&key), Some(SlackKind::Window));
    }

    #[test]
    fn thread_recovery_routes_to_thread_policy() {
        let key = ReferenceKey::Slack {
            channel_id: "C123".into(),
            thread_ts: "1731000000.000100".into(),
            host: "slack.com".into(),
            permalink_ts: None,
        };
        assert_eq!(kind(&key), Some(SlackKind::Thread));
    }
}
