#!/usr/bin/env python3
"""
Audio recording program with automatic start/stop based on signal detection
Optionally displays VU meter while recording
"""

import threading
import queue
import wave
import os
import sys
import numpy as np
import curses
import argparse
from vu_meter import VUMeter


class AudioRecorder:
    """Records audio to WAV files with automatic start/stop based on signal detection"""
    
    def __init__(self, base_filename, rate, channels, format, bytes_per_sample):
        """
        Initialize the audio recorder
        
        Args:
            base_filename: Base filename without .wav extension
            rate: Sample rate in Hz
            channels: Number of audio channels
            format: Sample format (s16, s32, etc.)
            bytes_per_sample: Bytes per sample
        """
        self.base_filename = base_filename
        self.rate = rate
        self.channels = channels
        self.format = format
        self.bytes_per_sample = bytes_per_sample
        
        # Determine WAV parameters
        if format in ["s16", "s16le"]:
            self.sampwidth = 2
            self.dtype = np.int16
        elif format in ["s32", "s32le"]:
            self.sampwidth = 4
            self.dtype = np.int32
        else:
            raise ValueError(f"Unsupported format: {format}")
        
        # Recording state
        self.recording = False
        self.current_file = None
        self.wav_file = None
        self.recording_thread = None
        self.queue = queue.Queue()
        self.stop_thread = threading.Event()
        
        # Start the recording thread
        self.recording_thread = threading.Thread(target=self._recording_worker, daemon=True)
        self.recording_thread.start()
    
    def _get_next_filename(self):
        """Find the next available filename with auto-incrementing number"""
        base = self.base_filename
        if not base.endswith('.wav'):
            base = base + '.wav'
        
        # Extract base without extension for numbering
        if base.endswith('.wav'):
            base_no_ext = base[:-4]
        else:
            base_no_ext = base
        
        # Find the smallest unused number
        n = 1
        while True:
            filename = f"{base_no_ext}.{n}.wav"
            if not os.path.exists(filename):
                return filename
            n += 1
    
    def _recording_worker(self):
        """Worker thread that handles writing audio data to files"""
        while not self.stop_thread.is_set():
            try:
                item = self.queue.get(timeout=0.1)
                
                if item is None:  # Stop signal
                    break
                
                command, data = item
                
                if command == "start":
                    if not self.recording:
                        self.current_file = self._get_next_filename()
                        self.wav_file = wave.open(self.current_file, 'wb')
                        self.wav_file.setnchannels(self.channels)
                        self.wav_file.setsampwidth(self.sampwidth)
                        self.wav_file.setframerate(self.rate)
                        self.recording = True
                        print(f"\nStarted recording to {self.current_file}", flush=True)
                
                elif command == "write":
                    if self.recording and self.wav_file:
                        # Write audio data
                        self.wav_file.writeframes(data.tobytes())
                
                elif command == "stop":
                    if self.recording and self.wav_file:
                        self.wav_file.close()
                        self.recording = False
                        print(f"\nStopped recording to {self.current_file}", flush=True)
                        self.current_file = None
                        self.wav_file = None
                
                self.queue.task_done()
                
            except queue.Empty:
                continue
            except Exception as e:
                print(f"\nRecording error: {e}", flush=True)
    
    def write_audio(self, audio_data, is_on):
        """
        Write audio data if recording
        
        Args:
            audio_data: Audio data as numpy array
            is_on: Whether signal is currently on
        """
        if is_on:
            if not self.recording:
                self.queue.put(("start", None))
            self.queue.put(("write", audio_data))
        elif self.recording:
            self.queue.put(("stop", None))
    
    def close(self):
        """Clean up and close the recorder"""
        if self.recording:
            self.queue.put(("stop", None))
        
        # Wait for queue to empty
        self.queue.join()
        
        # Stop the thread
        self.stop_thread.set()
        self.queue.put(None)
        if self.recording_thread:
            self.recording_thread.join(timeout=2)


class RecordingVUMeter(VUMeter):
    """VUMeter subclass that also handles recording"""
    
    def __init__(self, recorder, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.recorder = recorder
    
    def draw_vu_meter(self, stdscr):
        """Override to add recording logic"""
        curses.curs_set(0)
        stdscr.nodelay(1)
        stdscr.timeout(100)
        
        # Initialize colors
        curses.start_color()
        curses.init_pair(1, curses.COLOR_GREEN, curses.COLOR_BLACK)
        curses.init_pair(2, curses.COLOR_YELLOW, curses.COLOR_BLACK)
        curses.init_pair(3, curses.COLOR_RED, curses.COLOR_BLACK)
        curses.init_pair(4, curses.COLOR_WHITE, curses.COLOR_BLACK)
        
        self.start_recording()
        
        # Wait a moment for process to start
        import time
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
                
                # Process audio and get status
                db_levels = []
                max_db_levels = []
                is_on_status = []
                clip_status = []
                
                for ch in range(self.channels):
                    db = self.calculate_db(audio[:, ch])
                    is_clipping = self.detect_clipping(audio[:, ch])
                    max_db, is_on, has_clipped = self.update_history(ch, db, is_clipping)
                    db_levels.append(db)
                    max_db_levels.append(max_db)
                    is_on_status.append(is_on)
                    clip_status.append(has_clipped)
                
                # Handle recording - active if ANY channel is on
                any_channel_on = any(is_on_status)
                self.recorder.write_audio(audio, any_channel_on)
                
                # Draw the VU meter
                self._draw_display(stdscr, db_levels, max_db_levels, is_on_status, clip_status)
                
        finally:
            self.stop_recording()
            self.recorder.close()
    
    def _draw_display(self, stdscr, db_levels, max_db_levels, is_on_status, clip_status):
        """Draw the VU meter display"""
        stdscr.clear()
        height, width = stdscr.getmaxyx()
        
        # Draw header
        rec_status = " [RECORDING]" if self.recorder.recording else ""
        header = f"VU Meter - {self.target}{rec_status} | Press 'q' or ESC to quit"
        try:
            stdscr.addstr(0, 0, header[:width-1], curses.A_BOLD if self.recorder.recording else 0)
        except:
            pass
        
        # Draw VU meters for each channel
        start_row = 2
        left_label_width = 10
        right_label_width = 20
        bar_width = width - left_label_width - right_label_width - 1
        
        for ch, (db, max_db, is_on, has_clipped) in enumerate(zip(db_levels, max_db_levels, is_on_status, clip_status)):
            row = start_row + ch * 2
            if row >= height - 1:
                break
            
            normalized = (db - self.min_db) / self.db_range
            bar_length = int(normalized * bar_width)
            
            max_normalized = (max_db - self.min_db) / self.db_range
            max_pos = int(max_normalized * bar_width)
            
            left_label = f" {db:5.1f}dB "
            if has_clipped:
                right_label = f" Peak:{max_db:5.1f} CLIP"
            else:
                right_label = f" Peak:{max_db:5.1f}"
            
            try:
                stdscr.addstr(row, 0, left_label)
                
                for i in range(bar_width):
                    pos = left_label_width + i
                    
                    if i < bar_length:
                        if not is_on:
                            color = curses.color_pair(4) | curses.A_DIM
                        elif db < -20:
                            color = curses.color_pair(1)
                        elif db < -10:
                            color = curses.color_pair(2)
                        else:
                            color = curses.color_pair(3)
                        stdscr.addch(row, pos, '█', color)
                    elif i == max_pos and max_pos >= bar_length:
                        stdscr.addch(row, pos, '│', curses.color_pair(2) | curses.A_BOLD)
                
                stdscr.addstr(row, left_label_width + bar_width, right_label[:right_label_width])
                
                # Draw scale markers
                if row == start_row:
                    scale_row = row + 1
                    for db_marker in range(int(self.min_db), int(self.max_db) + 1, 10):
                        if db_marker < self.min_db or db_marker > self.max_db:
                            continue
                        
                        marker_normalized = (db_marker - self.min_db) / self.db_range
                        marker_pos = left_label_width + int(marker_normalized * bar_width)
                        
                        if marker_pos < left_label_width + bar_width:
                            try:
                                stdscr.addstr(scale_row, marker_pos, "│", curses.A_DIM)
                                label = f"{db_marker:d}dB" if db_marker == 0 else f"{db_marker:d}"
                                label_pos = marker_pos - len(label) // 2
                                if label_pos >= 0 and label_pos + len(label) < width:
                                    stdscr.addstr(scale_row + 1, label_pos, label, curses.A_DIM)
                            except:
                                pass
            except:
                pass
        
        stdscr.refresh()


def run_without_vumeter(recorder, vu_meter):
    """Run recording without VU meter display"""
    import time
    
    print("Recording started. Press Ctrl+C to stop.")
    print("Waiting for signal...")
    
    try:
        while True:
            audio = vu_meter.read_audio_chunk()
            if audio is None:
                break
            
            # Process audio to determine if signal is on
            is_on_status = []
            for ch in range(vu_meter.channels):
                db = vu_meter.calculate_db(audio[:, ch])
                is_clipping = vu_meter.detect_clipping(audio[:, ch])
                _, is_on, _ = vu_meter.update_history(ch, db, is_clipping)
                is_on_status.append(is_on)
            
            any_channel_on = any(is_on_status)
            recorder.write_audio(audio, any_channel_on)
            
    except KeyboardInterrupt:
        pass
    finally:
        vu_meter.stop_recording()
        recorder.close()


def main():
    """Main entry point"""
    parser = argparse.ArgumentParser(description='Record audio with automatic start/stop')
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
    parser.add_argument('--record-file', default='recording',
                        help='Base filename for recordings (default: recording)')
    parser.add_argument('--no-vumeter', action='store_true',
                        help='Disable VU meter display')
    
    args = parser.parse_args()
    
    # Determine bytes per sample
    if args.format in ["s32", "s32le"]:
        bytes_per_sample = 4
    elif args.format in ["s16", "s16le"]:
        bytes_per_sample = 2
    else:
        bytes_per_sample = 2
    
    # Create recorder
    recorder = AudioRecorder(
        base_filename=args.record_file,
        rate=args.rate,
        channels=args.channels,
        format=args.format,
        bytes_per_sample=bytes_per_sample
    )
    
    if args.no_vumeter:
        # Run without VU meter
        vu_meter = VUMeter(
            target=args.target,
            rate=args.rate,
            channels=args.channels,
            update_interval=args.interval,
            db_range=args.db_range,
            max_db=args.max_db,
            format=args.format,
            off_threshold=args.off_threshold
        )
        vu_meter.start_recording()
        run_without_vumeter(recorder, vu_meter)
    else:
        # Run with VU meter
        meter = RecordingVUMeter(
            recorder=recorder,
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
    sys.exit(main() or 0)
