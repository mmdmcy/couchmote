use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BrowserStatus {
    Starting,
    Ready,
    #[default]
    Stopped,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MediaState {
    pub available: bool,
    pub playing: bool,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub channel: Option<String>,
    pub duration: Option<String>,
    #[serde(skip_serializing)]
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSnapshot {
    pub status: BrowserStatus,
    pub url: Option<String>,
    pub title: Option<String>,
    pub page_kind: Option<String>,
    pub media: MediaState,
    pub search_results: Vec<SearchResult>,
    pub error: Option<String>,
}

impl Default for BrowserSnapshot {
    fn default() -> Self {
        Self {
            status: BrowserStatus::Stopped,
            url: None,
            title: None,
            page_kind: None,
            media: MediaState::default(),
            search_results: Vec::new(),
            error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VolumeState {
    pub available: bool,
    pub percent: u8,
    pub muted: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteState {
    pub browser: BrowserSnapshot,
    pub volume: VolumeState,
}

#[derive(Debug, Clone)]
pub enum BrowserAction {
    Launch,
    PlayPause,
    Seek { seconds: i16 },
    Next,
    Previous,
    Fullscreen,
    Back,
    Home,
    NavigateUp,
    NavigateDown,
    NavigateLeft,
    NavigateRight,
    NavigateSelect,
}
