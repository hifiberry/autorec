//! Shazam song recognition API client.
//!
//! Uses the shazamio-core fingerprinting engine (included as a git submodule)
//! and the reverse-engineered Shazam HTTP API to identify songs.
//!
//! # Example
//! ```no_run
//! use autorec::shazam::Shazam;
//!
//! let shazam = Shazam::new();
//! // From raw 16-bit 16 kHz mono PCM samples:
//! // let result = shazam.recognize_from_pcm(&samples)?;
//! // From a file:
//! // let result = shazam.recognize_from_file("song.mp3")?;
//! ```

use crate::fingerprinting::algorithm::SignatureGenerator;
use crate::fingerprinting::communication::get_signature_json;
use rand::seq::SliceRandom;
use std::error::Error;
use std::fmt;

// ---------------------------------------------------------------------------
// Shazam API URLs (mirroring ShazamUrl from the Python library)
// ---------------------------------------------------------------------------

const SEARCH_FROM_FILE_URL: &str = concat!(
    "https://amp.shazam.com/discovery/v5/{language}/{endpoint_country}/{device}/-/tag",
    "/{uuid_1}/{uuid_2}?sync=true&webv3=true&sampling=true",
    "&connected=&shazamapiversion=v3&sharehub=true&hubv5minorversion=v5.1&hidelb=true&video=v3"
);

const ABOUT_TRACK_URL: &str = concat!(
    "https://www.shazam.com/discovery/v5/{language}/{endpoint_country}/web/-/track",
    "/{track_id}?shazamapiversion=v3&video=v3"
);
#[allow(dead_code)]const TOP_TRACKS_PLAYLIST_URL: &str = concat!(
    "https://www.shazam.com/services/amapi/v1/catalog/{endpoint_country}",
    "/playlists/{playlist_id}/tracks?limit={limit}&offset={offset}",
    "&l={language}&relate[songs]=artists,music-videos"
);

const SEARCH_MUSIC_URL: &str = concat!(
    "https://www.shazam.com/services/search/v3/{language}/{endpoint_country}/web",
    "/search?query={query}&numResults={limit}&offset={offset}&types=songs"
);

const RELATED_SONGS_URL: &str = concat!(
    "https://cdn.shazam.com/shazam/v3/{language}/{endpoint_country}/web/-/tracks",
    "/track-similarities-id-{track_id}?startFrom={offset}&pageSize={limit}&connected=&channel="
);

// ---------------------------------------------------------------------------
// User agents (subset — Apple devices, matching the Python library)
// ---------------------------------------------------------------------------

const USER_AGENTS: &[&str] = &[
    "Mozilla/5.0 (iPhone; CPU iPhone OS 15_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.0 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 14_7_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/14.1.2 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (iPad; CPU OS 15_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.0 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 14_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/14.1.1 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (iPad; CPU OS 14_7_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/14.1.2 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 13_6_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/13.1.2 Mobile/15E148 Safari/604.1",
];

const DEVICES: &[&str] = &["iphone", "android", "web"];

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// The result of a song recognition request.
#[derive(Debug, Clone)]
pub struct RecognizeResult {
    /// Track title, if recognized.
    pub title: Option<String>,
    /// Artist name, if recognized.
    pub artist: Option<String>,
    /// Album name, if available.
    pub album: Option<String>,
    /// Shazam track ID, if recognized.
    pub track_id: Option<String>,
    /// Cover art URL, if available.
    pub cover_art: Option<String>,
    /// The full raw JSON response from Shazam.
    pub raw: serde_json::Value,
}

impl fmt::Display for RecognizeResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.title, &self.artist) {
            (Some(t), Some(a)) => write!(f, "{} — {}", a, t),
            (Some(t), None) => write!(f, "{}", t),
            _ => write!(f, "(not recognized)"),
        }
    }
}

impl RecognizeResult {
    /// Returns true if a track was found.
    pub fn is_recognized(&self) -> bool {
        self.title.is_some()
    }

    /// Parse a [`RecognizeResult`] from the raw Shazam JSON response.
    fn from_json(raw: serde_json::Value) -> Self {
        let track = raw.get("track");
        let title = track
            .and_then(|t| t.get("title"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let artist = track
            .and_then(|t| t.get("subtitle"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let album = track
            .and_then(|t| t.get("sections"))
            .and_then(|s| s.as_array())
            .and_then(|arr| {
                arr.iter().find_map(|sec| {
                    if sec.get("type").and_then(|v| v.as_str()) == Some("SONG") {
                        sec.get("metadata")
                            .and_then(|m| m.as_array())
                            .and_then(|meta| {
                                meta.iter().find_map(|item| {
                                    if item.get("title").and_then(|v| v.as_str()) == Some("Album")
                                    {
                                        item.get("text")
                                            .and_then(|v| v.as_str())
                                            .map(String::from)
                                    } else {
                                        None
                                    }
                                })
                            })
                    } else {
                        None
                    }
                })
            });
        let track_id = track
            .and_then(|t| t.get("key"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let cover_art = track
            .and_then(|t| t.get("images"))
            .and_then(|i| i.get("coverarthq"))
            .and_then(|v| v.as_str())
            .map(String::from);

        RecognizeResult {
            title,
            artist,
            album,
            track_id,
            cover_art,
            raw,
        }
    }
}

// ---------------------------------------------------------------------------
// Shazam client
// ---------------------------------------------------------------------------

/// Shazam API client for song recognition and metadata queries.
pub struct Shazam {
    language: String,
    endpoint_country: String,
    agent: ureq::Agent,
}

impl Default for Shazam {
    fn default() -> Self {
        Self::new()
    }
}

impl Shazam {
    /// Create a new client with default settings (language="en-US", country="GB").
    pub fn new() -> Self {
        Self::with_config("en-US", "GB")
    }

    /// Create a new client with custom language and endpoint country.
    pub fn with_config(language: &str, endpoint_country: &str) -> Self {
        Self {
            language: language.to_string(),
            endpoint_country: endpoint_country.to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(20))
                .build(),
        }
    }

    // ------- Recognition ---------------------------------------------------

    /// Recognize a song from raw signed 16-bit **16 kHz mono** PCM samples.
    ///
    /// This is the primary entry point when working with live audio from
    /// PipeWire / ALSA.  Pass at least ~3 seconds of audio for best results.
    pub fn recognize_from_pcm(
        &self,
        samples: &[i16],
    ) -> Result<RecognizeResult, Box<dyn Error>> {
        let signature = SignatureGenerator::make_signature_from_buffer(samples.to_vec());
        let sig = get_signature_json(&signature)?;
        self.send_recognize_request(&sig)
    }

    /// Recognize a song from an audio file (WAV, MP3, OGG, FLAC).
    ///
    /// The file is decoded, converted to 16 kHz mono, and a centered segment
    /// (default 12 s) is used for fingerprinting.
    pub fn recognize_from_file(
        &self,
        path: &str,
        segment_seconds: Option<u32>,
    ) -> Result<RecognizeResult, Box<dyn Error>> {
        let signature =
            SignatureGenerator::make_signature_from_file(path, Some(segment_seconds.unwrap_or(12)))?;
        let sig = get_signature_json(&signature)?;
        self.send_recognize_request(&sig)
    }

    /// Recognize a song from raw file bytes (any supported format).
    pub fn recognize_from_bytes(
        &self,
        bytes: &[u8],
        segment_seconds: Option<u32>,
    ) -> Result<RecognizeResult, Box<dyn Error>> {
        let signature = SignatureGenerator::make_signature_from_bytes(
            bytes.to_vec(),
            Some(segment_seconds.unwrap_or(12)),
        )?;
        let sig = get_signature_json(&signature)?;
        self.send_recognize_request(&sig)
    }

    // ------- Metadata queries ----------------------------------------------

    /// Get information about a track by its Shazam ID.
    pub fn track_about(&self, track_id: &str) -> Result<serde_json::Value, Box<dyn Error>> {
        let url = ABOUT_TRACK_URL
            .replace("{language}", &self.language)
            .replace("{endpoint_country}", &self.endpoint_country)
            .replace("{track_id}", track_id);
        self.get_json(&url)
    }

    /// Search for tracks by text query.
    pub fn search_track(
        &self,
        query: &str,
        limit: u32,
        offset: u32,
    ) -> Result<serde_json::Value, Box<dyn Error>> {
        let url = SEARCH_MUSIC_URL
            .replace("{language}", &self.language)
            .replace("{endpoint_country}", &self.endpoint_country)
            .replace("{query}", &urlencoding(query))
            .replace("{limit}", &limit.to_string())
            .replace("{offset}", &offset.to_string());
        self.get_json(&url)
    }

    /// Get tracks related to a given track ID.
    pub fn related_tracks(
        &self,
        track_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<serde_json::Value, Box<dyn Error>> {
        let url = RELATED_SONGS_URL
            .replace("{language}", &self.language)
            .replace("{endpoint_country}", &self.endpoint_country)
            .replace("{track_id}", track_id)
            .replace("{limit}", &limit.to_string())
            .replace("{offset}", &offset.to_string());
        self.get_json(&url)
    }

    // ------- Internal helpers ----------------------------------------------

    fn send_recognize_request(
        &self,
        sig: &crate::fingerprinting::communication::Signature,
    ) -> Result<RecognizeResult, Box<dyn Error>> {
        let mut rng = rand::thread_rng();

        let device = DEVICES.choose(&mut rng).unwrap_or(&"web");
        let uuid_1 = uuid::Uuid::new_v4().to_string().to_uppercase();
        let uuid_2 = uuid::Uuid::new_v4().to_string().to_uppercase();

        let url = SEARCH_FROM_FILE_URL
            .replace("{language}", &self.language)
            .replace("{endpoint_country}", &self.endpoint_country)
            .replace("{device}", device)
            .replace("{uuid_1}", &uuid_1)
            .replace("{uuid_2}", &uuid_2);

        let user_agent = USER_AGENTS.choose(&mut rng).unwrap_or(&USER_AGENTS[0]);

        let payload = serde_json::json!({
            "timezone": sig.timezone,
            "signature": {
                "uri": sig.signature.uri,
                "samplems": sig.signature.samples,
            },
            "timestamp": sig.timestamp,
            "context": {},
            "geolocation": {},
        });

        let resp: serde_json::Value = self
            .agent
            .post(&url)
            .set("X-Shazam-Platform", "IPHONE")
            .set("X-Shazam-AppVersion", "14.1.0")
            .set("Accept", "*/*")
            .set("Accept-Language", &self.language)
            .set("Accept-Encoding", "gzip, deflate")
            .set("User-Agent", user_agent)
            .send_json(payload)?
            .into_json()?;

        Ok(RecognizeResult::from_json(resp))
    }

    fn get_json(&self, url: &str) -> Result<serde_json::Value, Box<dyn Error>> {
        let mut rng = rand::thread_rng();
        let user_agent = USER_AGENTS.choose(&mut rng).unwrap_or(&USER_AGENTS[0]);

        let resp: serde_json::Value = self
            .agent
            .get(url)
            .set("X-Shazam-Platform", "IPHONE")
            .set("X-Shazam-AppVersion", "14.1.0")
            .set("Accept", "*/*")
            .set("Accept-Language", &self.language)
            .set("User-Agent", user_agent)
            .call()?
            .into_json()?;

        Ok(resp)
    }
}

/// Minimal percent-encoding for query strings.
fn urlencoding(s: &str) -> String {
    s.replace('%', "%25")
        .replace(' ', "%20")
        .replace('&', "%26")
        .replace('=', "%3D")
        .replace('+', "%2B")
        .replace('#', "%23")
}
