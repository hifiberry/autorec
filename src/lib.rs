pub mod audio_analysis;
pub mod audio_stream;
pub mod album_identifier;
pub mod config;
pub mod cuefile;
pub mod decibel;
pub mod detection_strategies;
pub mod display;
pub mod musicbrainz;
pub mod pause_detector;
pub mod pipewire_utils;
pub mod recorder;
pub mod song_detect;
pub mod vu_meter;
pub mod wavfile;

// Shazam fingerprinting — mounted from shazamio-core submodule unchanged.
// This makes `crate::fingerprinting::*` paths inside shazamio-core resolve correctly.
#[path = "../shazamio-core/src/fingerprinting/mod.rs"]
pub mod fingerprinting;

// Shazam HTTP API client (our code)
pub mod shazam;

pub use audio_stream::{
    create_input_stream, parse_audio_address, AlsaInputStream, AudioInputStream, AudioStream,
    PipeWireInputStream,
};
pub use album_identifier::{identify_album, identify_album_from_songs, AlbumInfo, IdentifiedSong};
pub use config::Config;
pub use display::display_vu_meter;
pub use pipewire_utils::{get_available_targets, list_targets, validate_and_select_target};
pub use recorder::AudioRecorder;
pub use vu_meter::{process_audio_chunk, ChannelMetrics, SampleFormat, VUMeter};
