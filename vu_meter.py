#!/usr/bin/env python3
"""
VU Meter for PipeWire audio
Displays real-time audio levels with dB scale
"""

import subprocess
import numpy as np
import curses
import sys
import time
from pipewire_utils import validate_and_select_target, list_targets


class VUMeter:
    def __init__(self, target="riaa.monitor", rate=96000, channels=2, update_interval=0.2, db_range=90, max_db=0, format="s32", off_threshold=-60, silence_duration=10):
        self.target = target
        self.rate = rate
        self.channels = channels
        self.update_interval = update_interval
        self.db_range = db_range
        self.max_db = max_db
        self.min_db = max_db - db_range
        self.format = format
        self.off_threshold = off_threshold
        self.silence_duration = silence_duration
        
        # Determine bytes per sample and numpy dtype
        if format in ["s32", "s32le"]:
            self.bytes_per_sample = 4
            self.dtype = np.int32
            self.max_value = 2147483648.0  # 2^31
        elif format in ["s16", "s16le"]:
            self.bytes_per_sample = 2
            self.dtype = np.int16
            self.max_value = 32768.0  # 2^15
        elif format in ["s24", "s24le"]:
            self.bytes_per_sample = 3
            self.dtype = np.int32  # Will need special handling
            self.max_value = 8388608.0  # 2^23
        else:
            raise ValueError(f"Unsupported format: {format}")
        
        self.chunk_size = int(rate * channels * self.bytes_per_sample * update_interval)  # bytes per update
        self.process = None
        # Keep history based on silence_duration for on/off detection
        self.history_seconds = silence_duration
        self.history_size = int(self.history_seconds / update_interval)
        self.db_history = [[] for _ in range(channels)]
        self.clip_history = [[] for _ in range(channels)]  # Track clipping events
        self.peak_history = [[] for _ in range(channels)]  # Track absolute peak values
        
    def start_recording(self):
        """Start the pw-record subprocess"""
        self.process = subprocess.Popen(
            [
                "pw-record",
                "--target", self.target,
                "--rate", str(self.rate),
                "--channels", str(self.channels),
                "--format", self.format,
                "-"
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE
        )
        
    def stop_recording(self):
        """Stop the pw-record subprocess"""
        if self.process:
            self.process.terminate()
            self.process.wait()
            
    def read_audio_chunk(self):
        """Read one chunk of audio data"""
        try:
            raw = self.process.stdout.read(self.chunk_size)
            if len(raw) < self.chunk_size:
                return None
            audio = np.frombuffer(raw, dtype=self.dtype).reshape(-1, self.channels)
            return audio
        except Exception as e:
            return None
            
    def calculate_db(self, audio_channel):
        """Calculate dB level for a channel (RMS)"""
        # Calculate RMS
        rms = np.sqrt(np.mean(audio_channel.astype(np.float64) ** 2))
        
        # Avoid log(0)
        if rms < 1:
            return self.min_db
            
        # Convert to dB (relative to max value for the format)
        db = 20 * np.log10(rms / self.max_value)
        
        # Clamp to reasonable range
        return max(self.min_db, min(self.max_db, db))
    
    def calculate_peak_db(self, audio_channel):
        """Calculate peak dB level for a channel (absolute max)"""
        # Get absolute maximum value
        peak = np.max(np.abs(audio_channel))
        
        # Avoid log(0)
        if peak < 1:
            return self.min_db
            
        # Convert to dB (relative to max value for the format)
        db = 20 * np.log10(peak / self.max_value)
        
        # Clamp to reasonable range
        return max(self.min_db, min(self.max_db, db))
    
    def detect_clipping(self, audio_channel):
        """Detect if any samples exceed 99.9% of full scale"""
        clip_threshold = 0.999 * self.max_value
        return np.any(np.abs(audio_channel) >= clip_threshold)
    
    def update_history(self, channel, db_value, peak_db_value, is_clipping):
        """Update history for a channel and return max RMS, max peak, on/off status, and clipping status"""
        self.db_history[channel].append(db_value)
        self.peak_history[channel].append(peak_db_value)
        self.clip_history[channel].append(is_clipping)
        
        # Keep only last N values (silence_duration worth)
        if len(self.db_history[channel]) > self.history_size:
            self.db_history[channel].pop(0)
        if len(self.peak_history[channel]) > self.history_size:
            self.peak_history[channel].pop(0)
        if len(self.clip_history[channel]) > self.history_size:
            self.clip_history[channel].pop(0)
        
        max_db = max(self.db_history[channel]) if self.db_history[channel] else self.min_db
        max_peak_db = max(self.peak_history[channel]) if self.peak_history[channel] else self.min_db
        # Source is "on" if any value in history exceeded threshold
        is_on = any(db > self.off_threshold for db in self.db_history[channel])
        # Clipping detected if any clip in history
        has_clipped = any(self.clip_history[channel])
        
        return max_db, max_peak_db, is_on, has_clipped
    
    def is_any_channel_on(self):
        """Check if any channel is currently on"""
        for ch_history in self.db_history:
            if any(db > self.off_threshold for db in ch_history):
                return True
        return False
        
    def draw_vu_meter(self, stdscr):
        """Main curses loop to draw the VU meter"""
        curses.curs_set(0)  # Hide cursor
        stdscr.nodelay(1)    # Non-blocking input
        stdscr.timeout(100)  # 100ms timeout for getch()
        
        # Initialize colors
        curses.start_color()
        curses.init_pair(1, curses.COLOR_GREEN, curses.COLOR_BLACK)
        curses.init_pair(2, curses.COLOR_YELLOW, curses.COLOR_BLACK)
        curses.init_pair(3, curses.COLOR_RED, curses.COLOR_BLACK)
        curses.init_pair(4, curses.COLOR_WHITE, curses.COLOR_BLACK)  # Gray for off state
        
        self.start_recording()
        
        # Wait a moment for process to start
        time.sleep(0.1)
        
        # Check if process started successfully
        if self.process.poll() is not None:
            self.stop_recording()
            stderr = self.process.stderr.read().decode('utf-8', errors='replace')
            raise RuntimeError(f"pw-record failed to start: {stderr}")
        
        try:
            while True:
                # Check for 'q' or ESC key to quit
                key = stdscr.getch()
                if key == ord('q') or key == ord('Q') or key == 27:
                    break
                    
                # Read audio chunk
                audio = self.read_audio_chunk()
                if audio is None:
                    break
                    
                # Clear screen
                stdscr.clear()
                
                # Get terminal dimensions
                height, width = stdscr.getmaxyx()
                
                # Calculate dB for each channel and update history
                db_levels = []
                peak_db_levels = []
                max_db_levels = []
                max_peak_db_levels = []
                is_on_status = []
                clip_status = []
                for ch in range(self.channels):
                    db = self.calculate_db(audio[:, ch])
                    peak_db = self.calculate_peak_db(audio[:, ch])
                    is_clipping = self.detect_clipping(audio[:, ch])
                    max_db, max_peak_db, is_on, has_clipped = self.update_history(ch, db, peak_db, is_clipping)
                    db_levels.append(db)
                    peak_db_levels.append(peak_db)
                    max_db_levels.append(max_db)
                    max_peak_db_levels.append(max_peak_db)
                    is_on_status.append(is_on)
                    clip_status.append(has_clipped)
                    
                # Draw header
                header = f"VU Meter - {self.target} | Press 'q' or ESC to quit"
                try:
                    stdscr.addstr(0, 0, header[:width-1])
                except:
                    pass
                    
                # Draw VU meters for each channel
                start_row = 2
                left_label_width = 10  # Width for " -XX.XdB "
                right_label_width = 20  # Width for " Peak: -XX.X CLIP"
                bar_width = width - left_label_width - right_label_width - 1
                
                for ch, (db, peak_db, max_db, max_peak_db, is_on, has_clipped) in enumerate(zip(db_levels, peak_db_levels, max_db_levels, max_peak_db_levels, is_on_status, clip_status)):
                    row = start_row + ch * 2
                    if row >= height - 1:
                        break
                        
                    # Calculate bar length (from min_db to max_db) for RMS
                    normalized = (db - self.min_db) / self.db_range  # 0.0 to 1.0
                    bar_length = int(normalized * bar_width)
                    
                    # Calculate peak position for historical max peak
                    peak_normalized = (max_peak_db - self.min_db) / self.db_range
                    peak_pos = int(peak_normalized * bar_width)
                    
                    # Calculate max RMS position
                    max_normalized = (max_db - self.min_db) / self.db_range
                    max_pos = int(max_normalized * bar_width)
                    
                    # Create labels with both RMS and peak
                    left_label = f" {db:5.1f}dB "
                    if has_clipped:
                        right_label = f" >{max_peak_db:5.1f} RMS:{max_db:5.1f} CLIP"
                    else:
                        right_label = f" >{max_peak_db:5.1f} RMS:{max_db:5.1f}"
                    
                    try:
                        # Draw left label (current dB)
                        stdscr.addstr(row, 0, left_label)
                        
                        # Draw bar with color coding
                        for i in range(bar_width):
                            pos = left_label_width + i
                            
                            # Draw bar up to current RMS level
                            if i < bar_length:
                                # Use gray if source is off, otherwise color based on level
                                if not is_on:
                                    color = curses.color_pair(4) | curses.A_DIM  # Gray
                                elif db < -20:
                                    color = curses.color_pair(1)  # Green
                                elif db < -10:
                                    color = curses.color_pair(2)  # Yellow
                                else:
                                    color = curses.color_pair(3)  # Red
                                stdscr.addch(row, pos, '█', color)
                            # Draw peak indicator (>) at current peak position
                            elif i == peak_pos and peak_pos >= bar_length:
                                stdscr.addch(row, pos, '>', curses.color_pair(3) | curses.A_BOLD)
                            # Draw max RMS indicator
                            elif i == max_pos and max_pos >= bar_length and max_pos != peak_pos:
                                stdscr.addch(row, pos, '│', curses.color_pair(2) | curses.A_BOLD)
                        
                        # Draw right label (max dB)
                        stdscr.addstr(row, left_label_width + bar_width, right_label[:right_label_width])
                            
                        # Draw scale markers every 10dB
                        if row == start_row:
                            scale_row = row + 1
                            
                            # Draw markers every 10dB
                            for db_marker in range(int(self.min_db), int(self.max_db) + 1, 10):
                                if db_marker < self.min_db or db_marker > self.max_db:
                                    continue
                                    
                                marker_normalized = (db_marker - self.min_db) / self.db_range
                                marker_pos = left_label_width + int(marker_normalized * bar_width)
                                
                                if marker_pos < left_label_width + bar_width:
                                    try:
                                        stdscr.addstr(scale_row, marker_pos, "│", curses.A_DIM)
                                        # Add label for start, middle-ish, and end markers
                                        label = f"{db_marker:d}dB" if db_marker == 0 else f"{db_marker:d}"
                                        label_pos = marker_pos - len(label) // 2
                                        if label_pos >= 0 and label_pos + len(label) < width:
                                            stdscr.addstr(scale_row + 1, label_pos, label, curses.A_DIM)
                                    except:
                                        pass
                    except:
                        pass
                        
                stdscr.refresh()
                
        finally:
            self.stop_recording()


def main():
    """Main entry point"""
    import argparse
    
    parser = argparse.ArgumentParser(description='VU Meter for PipeWire audio')
    parser.add_argument('--list-targets', action='store_true',
                        help='List available PipeWire recording targets and exit')
    parser.add_argument('--target', default=None,
                        help='PipeWire target device (default: auto-detect first available)')
    parser.add_argument('--rate', type=int, default=96000,
                        help='Sample rate (default: 96000)')
    parser.add_argument('--channels', type=int, default=2,
                        help='Number of channels (default: 2)')
    parser.add_argument('--format', default='s32',
                        help='Sample format: s16, s32 (default: s32)')
    parser.add_argument('--interval', type=float, default=0.2,
                        help='Update interval in seconds (default: 0.2)')
    parser.add_argument('--db-range', type=int, default=90,
                        help='dB range to display (default: 90)')
    parser.add_argument('--max-db', type=int, default=0,
                        help='Maximum dB (default: 0 dBFS)')
    parser.add_argument('--off-threshold', type=float, default=-60,
                        help='Threshold for on/off detection in dB (default: -60)')
    parser.add_argument('--silence-duration', type=float, default=10,
                        help='Duration of silence before signal is considered off in seconds (default: 10)')
    
    args = parser.parse_args()
    
    # Handle --list-targets
    if args.list_targets:
        sys.exit(list_targets())
    
    # Validate or auto-select target
    target, error_code = validate_and_select_target(args.target)
    if error_code != 0:
        sys.exit(error_code)
    args.target = target
    
    meter = VUMeter(
        target=args.target,
        rate=args.rate,
        channels=args.channels,
        update_interval=args.interval,
        db_range=args.db_range,
        max_db=args.max_db,
        format=args.format,
        off_threshold=args.off_threshold,
        silence_duration=args.silence_duration
    )
    
    try:
        curses.wrapper(meter.draw_vu_meter)
    except KeyboardInterrupt:
        pass
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    main()
