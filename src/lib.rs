pub mod audio_stream;
pub mod config;
pub mod decibel;
pub mod display;
pub mod pause_detector;
pub mod pipewire_utils;
pub mod recorder;
pub mod song_detect;
pub mod vu_meter;

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
pub use config::Config;
pub use display::display_vu_meter;
pub use pause_detector::AdaptivePauseDetector;
pub use pipewire_utils::{get_available_targets, list_targets, validate_and_select_target};
pub use recorder::AudioRecorder;
pub use song_detect::SongDetectScheduler;
pub use vu_meter::{process_audio_chunk, ChannelMetrics, SampleFormat, VUMeter};
