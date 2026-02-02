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


class VUMeter:
    def __init__(self, target="riaa.monitor", rate=96000, channels=2, update_interval=0.2, db_range=90, max_db=0, format="s32", off_threshold=-60):
        self.target = target
        self.rate = rate
        self.channels = channels
        self.update_interval = update_interval
        self.db_range = db_range
        self.max_db = max_db
        self.min_db = max_db - db_range
        self.format = format
        self.off_threshold = off_threshold
        
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
        # Keep 5 seconds of history
        self.history_seconds = 5
        self.history_size = int(self.history_seconds / update_interval)
        self.db_history = [[] for _ in range(channels)]
        
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
        """Calculate dB level for a channel"""
        # Calculate RMS
        rms = np.sqrt(np.mean(audio_channel.astype(np.float64) ** 2))
        
        # Avoid log(0)
        if rms < 1:
            return self.min_db
            
        # Convert to dB (relative to max value for the format)
        db = 20 * np.log10(rms / self.max_value)
        
        # Clamp to reasonable range
        return max(self.min_db, min(self.max_db, db))
    
    def update_history(self, channel, db_value):
        """Update history for a channel and return max of last 5 seconds and on/off status"""
        self.db_history[channel].append(db_value)
        # Keep only last N values (5 seconds worth)
        if len(self.db_history[channel]) > self.history_size:
            self.db_history[channel].pop(0)
        
        max_db = max(self.db_history[channel]) if self.db_history[channel] else self.min_db
        # Source is "on" if any value in last 5 seconds exceeded threshold
        is_on = any(db > self.off_threshold for db in self.db_history[channel])
        
        return max_db, is_on
        
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
                max_db_levels = []
                is_on_status = []
                for ch in range(self.channels):
                    db = self.calculate_db(audio[:, ch])
                    max_db, is_on = self.update_history(ch, db)
                    db_levels.append(db)
                    max_db_levels.append(max_db)
                    is_on_status.append(is_on)
                    
                # Draw header
                header = f"VU Meter - {self.target} | Press 'q' or ESC to quit"
                try:
                    stdscr.addstr(0, 0, header[:width-1])
                except:
                    pass
                    
                # Draw VU meters for each channel
                start_row = 2
                left_label_width = 10  # Width for " -XX.XdB "
                right_label_width = 12  # Width for " Max: -XX.X"
                bar_width = width - left_label_width - right_label_width - 1
                
                for ch, (db, max_db, is_on) in enumerate(zip(db_levels, max_db_levels, is_on_status)):
                    row = start_row + ch * 2
                    if row >= height - 1:
                        break
                        
                    # Calculate bar length (from min_db to max_db)
                    normalized = (db - self.min_db) / self.db_range  # 0.0 to 1.0
                    bar_length = int(normalized * bar_width)
                    
                    # Calculate max position
                    max_normalized = (max_db - self.min_db) / self.db_range
                    max_pos = int(max_normalized * bar_width)
                    
                    # Create labels
                    left_label = f" {db:5.1f}dB "
                    right_label = f" Peak:{max_db:5.1f}"
                    
                    try:
                        # Draw left label (current dB)
                        stdscr.addstr(row, 0, left_label)
                        
                        # Draw bar with color coding
                        for i in range(bar_width):
                            pos = left_label_width + i
                            
                            # Draw bar up to current level
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
                            # Draw max indicator
                            elif i == max_pos and max_pos >= bar_length:
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
    parser.add_argument('--target', default='riaa.monitor', 
                        help='PipeWire target device (default: riaa.monitor)')
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
    
    args = parser.parse_args()
    
    meter = VUMeter(
        target=args.target,
        rate=args.rate,
        channels=args.channels,
        update_interval=args.interval,
        db_range=args.db_range,
        max_db=args.max_db,
        format=args.format,
        off_threshold=args.off_threshold
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
