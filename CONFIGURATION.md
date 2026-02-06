# Configuration File Support

The autorecord program now supports saving default configuration values to a file at `~/.state/autorec/defaults.toml`.

## How It Works

The configuration system has a three-level hierarchy:

1. **Built-in defaults** - The original hard-coded defaults
2. **Saved defaults** - User preferences saved to `~/.state/autorec/defaults.toml`
3. **Command-line options** - Options specified when running the command

Each level overrides the previous one. This means:
- Saved defaults override built-in defaults
- Command-line options override both saved and built-in defaults

## Usage Examples

### Save your preferred settings as defaults

```bash
# Set and save your preferred recording settings
record --rate 48000 --channels 1 --off-threshold -50 --save-defaults
```

This will create or update `~/.state/autorec/defaults.toml` with your settings.

### View saved defaults

```bash
# See what defaults you have saved
record --show-saved-defaults
```

### View built-in defaults

```bash
# See the original built-in default values
record --show-defaults
```

### Override saved defaults temporarily

```bash
# Use saved defaults but temporarily change the rate
record --rate 96000
```

This will use your saved defaults for everything except the rate, which will be 96000 for this recording only. Your saved defaults remain unchanged.

### Update saved defaults incrementally

```bash
# First, save some basic settings
record --rate 48000 --channels 1 --save-defaults

# Later, add or update just the threshold setting
record --off-threshold -45 --save-defaults
```

The second command will update only the `off-threshold` value while keeping your previously saved `rate` and `channels` settings.

## Configuration File Location

The configuration file is stored at:
```
~/.state/autorec/defaults.toml
```

The directory will be created automatically when you first save defaults.

## Configuration Options

All command-line options can be saved as defaults:

- `source` - Audio source address
- `rate` - Sample rate (Hz)
- `channels` - Number of channels
- `format` - Sample format (s16, s32)
- `interval` - Update interval (seconds)
- `db_range` - dB range to display
- `max_db` - Maximum dB level
- `off_threshold` - Threshold for on/off detection (dB)
- `silence_duration` - Duration of silence before stopping (seconds)
- `min_length` - Minimum recording length (seconds)
- `no_vumeter` - Disable VU meter display
- `no_keyboard` - Disable keyboard shortcuts

## Example Configuration File

The TOML file format is simple and human-readable:

```toml
rate = 48000
channels = 1
format = "s16"
db_range = 60.0
off_threshold = -50.0
```

You can also edit this file manually if you prefer, though using `--save-defaults` is recommended.
