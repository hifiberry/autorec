# Boundary Finder

Automatic song boundary detection for vinyl recordings with CUE file generation.

## Overview

`boundary_finder` is a command-line tool that analyzes WAV files from vinyl recordings and automatically:
- Detects groove-in (lead-in silence) and groove-out (lead-out silence)
- Finds song boundaries between tracks
- Looks up release information from MusicBrainz
- Generates CUE sheet files for proper track indexing

The tool is specifically designed for vinyl recordings where there is no true silence between tracks, only brief energy dips in the continuous groove noise.

## Features

### Automatic Boundary Detection
Uses a three-pass algorithm:
1. **Pass 1**: Compute RMS energy in small windows across the entire file
2. **Pass 2**: Detect groove-in/groove-out using noise floor vs music level thresholds
3. **Pass 3**: Find valleys (local energy minima) that represent song boundaries

The algorithm works without requiring any external metadata and handles the continuous groove noise present in vinyl recordings.

### MusicBrainz Integration
- Parses filename in `artist_album.N.wav` format (underscores represent spaces)
- Tries all possible artist/album name splits
- Searches MusicBrainz database for matching releases
- Ranks results by duration match (using music duration with grooves removed)
- Handles multi-side vinyl releases automatically
- Works with complex names like "dj_shadow_endtroducing.1.wav"

### CUE File Generation
Automatically creates `.cue` files with:
- Proper track indexing with MM:SS:FF timestamps
- Track titles from MusicBrainz (when available)
- Artist and album information
- Compatible with audio players and ripping software

## Usage

### Basic Usage

Process a single file:
```bash
boundary_finder recording.wav
```

Process multiple files:
```bash
boundary_finder side_a.wav side_b.wav
```

Process entire directory:
```bash
boundary_finder --directory /music/at33ptg
```

### Options

| Option | Description |
|--------|-------------|
| `--verbose`, `-v` | Show detailed analysis including RMS levels, valley scores |
| `--directory <DIR>`, `-d` | Process all WAV files in directory |
| `--no-lookup` | Skip MusicBrainz release lookup |
| `--no-cue` | Don't generate CUE files |
| `--min-prominence <DB>` | Minimum valley depth below local average (default: 3.0) |
| `--min-song <SEC>` | Minimum song duration in seconds (default: 30) |
| `--smooth-window <SEC>` | RMS smoothing window in seconds (default: 3.0) |
| `--chunk-ms <MS>` | RMS window size in milliseconds (default: 200) |
| `--dump` | Dump RMS curve data for plotting |

### Examples

**Process directory and create CUE files:**
```bash
boundary_finder --directory /music/vinyl_recordings
```

**Verbose analysis of a single file:**
```bash
boundary_finder --verbose kanonenfieber_soldatenschicksale.1.wav
```

**Process without MusicBrainz lookup:**
```bash
boundary_finder --no-lookup my_recording.wav
```

**Adjust sensitivity for difficult recordings:**
```bash
boundary_finder --min-prominence 2.5 --min-song 20 recording.wav
```

## File Naming Convention

For automatic MusicBrainz lookup, name your files as:
```
artist_album.N.wav
```

Where:
- `artist` and `album` use underscores instead of spaces
- `N` is the side number (1, 2, 3, 4, etc.)

Examples:
- `kanonenfieber_soldatenschicksale.1.wav`
- `dj_shadow_endtroducing.1.wav`
- `poppy_empty_hands.2.wav`

Special characters may be omitted (e.g., "Endtroducing....." → "endtroducing").

## How It Works

### Groove Detection

Vinyl recordings have characteristic noise patterns:
- **Noise floor**: -30 to -45 dB (continuous groove noise)
- **Music level**: -15 to -25 dB (typical music)
- **Groove-in**: 0.5-5 seconds of quiet noise before music starts
- **Groove-out**: Can be minutes of quiet at the end

The tool estimates these levels using percentile analysis and detects transitions.

### Boundary Detection

Real song boundaries have distinct characteristics:
- Drop **well below** the noise floor (7-16 dB below typical groove noise)
- Show clear energy valleys in smoothed RMS curve
- Have sufficient prominence above surrounding music
- Maintain minimum spacing (default 30 seconds)

False positives (quiet passages within songs) are filtered out by:
- **Score gap ratio**: Removes low-scoring candidates
- **Depth threshold**: Must reach `noise_floor - 5 dB` or deeper

### Duration Matching

When multiple releases are found on MusicBrainz:
1. Fetch full track listings for all candidates
2. Calculate total duration for each release
3. For multi-side releases, find best side split
4. Rank by duration match error
5. Accept if error < 5% or 30 seconds (whichever is larger)

## Directory Mode

When using `--directory`:
- Processes all `.wav` files in the specified directory
- **Skips files that already have `.cue` files** (prevents re-processing)
- Creates `.cue` files alongside original `.wav` files
- Shows progress for each file

This is ideal for batch processing large vinyl recording collections.

## Output Format

### Console Output
```
Song Boundary Finder
====================
File: /music/at33ptg/kanonenfieber_soldatenschicksale.1.wav

WAV: 96000Hz, 2ch, 32bit, duration: 19:33.20 (1173.2s)

Levels:
  Noise floor: -36.7 dB (groove noise)
  Music level: -20.3 dB (typical music)
  Difference:  16.4 dB

Music region:
  Groove-in:  00:07.20 (7.2s lead-in)
  Groove-out: 18:50.80 (42.4s lead-out)
  Music:      18:43.60 (1123.6s)

MusicBrainz Lookup:
-------------------
Found: Kanonenfieber - Soldatenschicksale
Release ID: 768a1c5f-3657-4e29-aac4-c1de6ee5221f
Format: Other
Tracks: 9
URL: https://musicbrainz.org/release/768a1c5f-3657-4e29-aac4-c1de6ee5221f

Results
=======
Release: Kanonenfieber - Soldatenschicksale [768a1c5f-3657-4e29-aac4-c1de6ee5221f]
Boundaries found: 3
Songs detected: 4

  Song 1: 03:54.60 (starts @ 00:07.20) - #1 Z-Vor!
    --- boundary at 04:01.80 ---
  Song 2: 04:40.00 (starts @ 04:01.80) - #2 Heizer Tenner
    --- boundary at 08:41.80 ---
  Song 3: 04:43.20 (starts @ 08:41.80) - #3 Ubootsperre (2025)
    --- boundary at 13:25.00 ---
  Song 4: 05:25.80 (starts @ 13:25.00) - #4 Kampf und Sturm (2025)

CUE file created: /music/at33ptg/kanonenfieber_soldatenschicksale.1.cue
```

### CUE File Format
```cue
REM GENERATOR "HiFiBerry AutoRec boundary_finder"
PERFORMER "Kanonenfieber"
TITLE "Soldatenschicksale"
FILE "kanonenfieber_soldatenschicksale.1.wav" WAVE
  TRACK 01 AUDIO
    TITLE "Z-Vor!"
    PERFORMER "Kanonenfieber"
    INDEX 01 00:07:15
  TRACK 02 AUDIO
    TITLE "Heizer Tenner"
    PERFORMER "Kanonenfieber"
    INDEX 01 04:01:60
  ...
```

## Troubleshooting

### No boundaries detected
- Try lowering `--min-prominence` (e.g., 2.5 or 2.0)
- Try lowering `--min-song` for albums with short tracks
- Use `--verbose` to see valley candidates and filtering
- Use `--dump` to export RMS curve for visualization

### Wrong number of boundaries
- Check if a short interlude track is being merged (expected behavior)
- Adjust `--min-song` if you have very short tracks
- Use `--verbose` to see why candidates were filtered

### MusicBrainz lookup fails
- Verify filename follows `artist_album.N.wav` convention
- Check that album exists on MusicBrainz
- Try simplifying artist/album names (remove special characters)
- Use `--no-lookup` to skip lookup and just detect boundaries

### CUE file timestamps off by a few seconds
- This is normal for vinyl - recordings rarely align perfectly with official track lengths
- CUE files use detected boundaries which reflect the actual recording
- Players will still work correctly with the generated CUE files

## Technical Details

### Algorithm Parameters

**Noise Floor Estimation**: P5-P10 percentile of smoothed RMS (5th to 10th percentile captures the quietest sustained sections)

**Music Level Estimation**: P60-P80 percentile of smoothed RMS (60th to 80th percentile captures typical music level)

**Groove Threshold**: Midpoint between noise floor and music level

**Valley Prominence**: Difference between valley depth and surrounding local average (within 15-second context window)

**Depth Threshold**: `noise_floor - 5 dB` (real boundaries drop well below groove noise)

**Score Formula**: `min_dip × (1 + prominence × 0.1) × (1 + sqrt(width))`
- Combines valley depth, prominence, and width
- Favors deep, prominent, wide valleys

### Performance

- Typical processing time: 5-10 seconds for a 20-minute recording
- Memory usage: ~50MB for a 20-minute 96kHz/32-bit stereo file
- MusicBrainz lookups add 1-2 seconds per search query (rate limited to 1/second)

## License

Part of the HiFiBerry AutoRec project.
