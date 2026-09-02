use anyhow::{Result, anyhow};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

use crate::model::SearchResult;

pub const YOUTUBE_HOME: &str = "https://www.youtube.com/";

#[derive(Debug, Clone, Deserialize)]
pub struct PageProbe {
    pub url: String,
    pub title: String,
    pub page_kind: String,
    pub media: Option<MediaProbe>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MediaProbe {
    pub available: bool,
    pub playing: bool,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SearchProbe {
    title: String,
    channel: Option<String>,
    duration: Option<String>,
    url: String,
}

pub const STATUS_SCRIPT: &str = r###"(() => {
  const finite = value => Number.isFinite(value) ? value : 0;
  const video = document.querySelector("video");
  const titleNode = document.querySelector("h1.ytd-watch-metadata, h1.title, meta[property='og:title']");
  const title = titleNode?.getAttribute?.("content") || titleNode?.textContent || document.title || "";
  const path = window.location.pathname;
  const pageKind = path === "/watch" ? "watch" : path === "/results" ? "search" : "youtube";
  return JSON.stringify({
    url: window.location.href,
    title: title.trim(),
    page_kind: pageKind,
    media: video ? {
      available: true,
      playing: !video.paused,
      position_seconds: finite(video.currentTime),
      duration_seconds: finite(video.duration),
      title: title.trim() || null
    } : {
      available: false,
      playing: false,
      position_seconds: 0,
      duration_seconds: 0,
      title: null
    }
  });
})()"###;

pub const SEARCH_SCRIPT: &str = r###"(() => {
  const seen = new Set();
  const links = Array.from(document.querySelectorAll(
    "ytd-video-renderer a#video-title, ytd-grid-video-renderer a#video-title, a#video-title"
  ));
  const results = [];
  for (const link of links) {
    const url = link.href || "";
    if (!url.includes("/watch?v=") || seen.has(url)) continue;
    const row = link.closest("ytd-video-renderer, ytd-grid-video-renderer, ytd-rich-item-renderer");
    const channel = row?.querySelector("#channel-name a, ytd-channel-name a")?.textContent?.trim() || null;
    const duration = row?.querySelector("ytd-thumbnail-overlay-time-status-renderer span")?.textContent?.trim() || null;
    const title = (link.textContent || "").trim();
    if (!title) continue;
    seen.add(url);
    results.push({ title, channel, duration, url });
    if (results.length >= 12) break;
  }
  return JSON.stringify(results);
})()"###;

pub fn seek_script(seconds: i16) -> String {
    let seconds = seconds as i32;
    format!(
        r###"(() => {{
  const video = document.querySelector("video");
  if (!video) return "missing";
  const duration = Number.isFinite(video.duration) ? video.duration : Number.MAX_SAFE_INTEGER;
  video.currentTime = Math.max(0, Math.min(duration, video.currentTime + {seconds}));
  return "seeked";
}})()"###
    )
}

pub fn youtube_search_url(query: &str) -> String {
    let mut url = Url::parse(YOUTUBE_HOME).expect("constant YouTube URL should parse");
    url.set_path("/results");
    url.query_pairs_mut()
        .append_pair("search_query", query.trim());
    url.to_string()
}

pub fn validate_youtube_watch_url(raw: &str) -> Result<String> {
    let parsed = Url::parse(raw.trim()).map_err(|_| anyhow!("invalid YouTube URL"))?;
    if parsed.scheme() != "https" || !is_youtube_host(parsed.host_str()) {
        return Err(anyhow!("only HTTPS YouTube URLs are allowed"));
    }

    let video_id = if matches!(parsed.host_str(), Some("youtu.be")) {
        parsed
            .path_segments()
            .and_then(|mut segments| segments.next())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("YouTube short URL has no video id"))?
            .to_string()
    } else if parsed.path() == "/watch" {
        parsed
            .query_pairs()
            .find(|(key, _)| key == "v")
            .map(|(_, value)| value.into_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("YouTube watch URL has no video id"))?
    } else {
        return Err(anyhow!("only YouTube watch URLs are allowed"));
    };

    if video_id.len() > 64
        || !video_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(anyhow!("invalid YouTube video id"));
    }

    Ok(format!("https://www.youtube.com/watch?v={video_id}"))
}

pub fn parse_search_results(value: &str) -> Result<Vec<SearchResult>> {
    let probes: Vec<SearchProbe> = serde_json::from_str(value)
        .map_err(|error| anyhow!("invalid browser search results: {error}"))?;

    let mut results = Vec::new();
    for probe in probes.into_iter().take(12) {
        let Ok(url) = validate_youtube_watch_url(&probe.url) else {
            continue;
        };
        let id = result_id(&url);
        if results.iter().any(|result: &SearchResult| result.id == id) {
            continue;
        }
        results.push(SearchResult {
            id,
            title: probe.title.trim().to_string(),
            channel: probe.channel.map(|value| value.trim().to_string()),
            duration: probe.duration.map(|value| value.trim().to_string()),
            url,
        });
    }
    Ok(results)
}

fn result_id(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_youtube_host(host: Option<&str>) -> bool {
    matches!(
        host,
        Some("youtube.com") | Some("www.youtube.com") | Some("m.youtube.com") | Some("youtu.be")
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_search_results, validate_youtube_watch_url, youtube_search_url};

    #[test]
    fn canonicalizes_supported_watch_urls() {
        assert_eq!(
            validate_youtube_watch_url("https://www.youtube.com/watch?v=abc_123&list=ignored")
                .expect("URL should be accepted"),
            "https://www.youtube.com/watch?v=abc_123"
        );
        assert_eq!(
            validate_youtube_watch_url("https://youtu.be/xyz-789?t=20")
                .expect("short URL should be accepted"),
            "https://www.youtube.com/watch?v=xyz-789"
        );
    }

    #[test]
    fn rejects_non_youtube_and_non_watch_urls() {
        for url in [
            "http://www.youtube.com/watch?v=abc",
            "https://example.com/watch?v=abc",
            "https://www.youtube.com/results?search_query=abc",
            "javascript:alert(1)",
        ] {
            assert!(
                validate_youtube_watch_url(url).is_err(),
                "{url} should reject"
            );
        }
    }

    #[test]
    fn encodes_search_queries() {
        let url = youtube_search_url("cats & dogs");
        assert!(url.contains("search_query=cats+%26+dogs"));
    }

    #[test]
    fn filters_and_deduplicates_search_results() {
        let raw = r#"[
          {"title":"One","channel":"A","duration":"1:00","url":"https://www.youtube.com/watch?v=one"},
          {"title":"One again","channel":"A","duration":"1:00","url":"https://www.youtube.com/watch?v=one"},
          {"title":"Bad","channel":null,"duration":null,"url":"https://example.com/watch?v=bad"}
        ]"#;
        let results = parse_search_results(raw).expect("results should parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "One");
    }
}
