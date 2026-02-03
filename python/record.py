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
import time
import numpy as np
import curses
import argparse
from vu_meter import VUMeter
from pipewire_utils import get_available_targets, list_targets, validate_and_select_target


class AudioRecorder:
    """Records audio to WAV files with automatic start/stop based on signal detection"""
    
    def __init__(self, base_filename, rate, channels, format, bytes_per_sample, min_length=600):
        """
        Initialize the audio recorder
        
        Args:
            base_filename: Base filename without .wav extension
            rate: Sample rate in Hz
            channels: Number of audio channels
            format: Sample format (s16, s32, etc.)
            bytes_per_sample: Bytes per sample
            min_length: Minimum recording length in seconds (default: 600)
        """
        self.base_filename = base_filename
        self.rate = rate
        self.channels = channels
        self.format = format
        self.bytes_per_sample = bytes_per_sample
        self.min_length = min_length
        
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
        self.recording_start_time = None
        self.clipping_detected = False  # Track if clipping occurred during current recording
        
        # Initialize file counter by checking existing files
        base_no_ext = base_filename
        if base_no_ext.endswith('.wav'):
            base_no_ext = base_no_ext[:-4]
        
        # Find the highest existing file number (check both normal and clipped)
        n = 1
        while os.path.exists(f"{base_no_ext}.{n}.wav") or os.path.exists(f"{base_no_ext}.{n}.clipped.wav"):
            n += 1
        self.next_file_number = n
        
        self.recording_thread = None
        self.queue = queue.Queue()
        self.stop_thread = threading.Event()
        
        # Start the recording thread
        self.recording_thread = threading.Thread(target=self._recording_worker, daemon=True)
        self.recording_thread.start()
    
    def _get_next_filename(self):
        """Find the next available filename with auto-incrementing number"""
        # Remove .wav extension if present
        base_no_ext = self.base_filename
        if base_no_ext.endswith('.wav'):
            base_no_ext = base_no_ext[:-4]
        
        # Use current file number
        filename = f"{base_no_ext}.{self.next_file_number}.wav"
        return filename
    
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
                        self.recording_start_time = time.time()
                        self.clipping_detected = False  # Reset clipping flag
                        print(f"\nStarted recording to {self.current_file}", flush=True)
                
                elif command == "write":
                    if self.recording and self.wav_file:
                        # Write audio data
                        self.wav_file.writeframes(data.tobytes())
                
                elif command == "clipping":
                    # Mark that clipping was detected
                    self.clipping_detected = True
                
                elif command == "stop":
                    if self.recording and self.wav_file:
                        self.wav_file.close()
                        self.recording = False
                        
                        # Check recording duration
                        duration = time.time() - self.recording_start_time if self.recording_start_time else 0
                        
                        if duration < self.min_length:
                            print(f"\nRecording too short ({duration:.1f}s < {self.min_length}s), deleting {self.current_file}", flush=True)
                            try:
                                os.remove(self.current_file)
                            except Exception as e:
                                print(f"\nError deleting file: {e}", flush=True)
                            # Don't increment counter, reuse this number
                        else:
                            # Rename file if clipping was detected
                            final_file = self.current_file
                            if self.clipping_detected:
                                # Insert .clipped before .wav extension
                                clipped_file = self.current_file.replace('.wav', '.clipped.wav')
                                try:
                                    os.rename(self.current_file, clipped_file)
                                    final_file = clipped_file
                                    print(f"\nStopped recording to {final_file} (duration: {duration:.1f}s) [CLIPPED]", flush=True)
                                except Exception as e:
                                    print(f"\nStopped recording to {final_file} (duration: {duration:.1f}s) - Error renaming: {e}", flush=True)
                            else:
                                print(f"\nStopped recording to {final_file} (duration: {duration:.1f}s)", flush=True)
                            # Only increment counter when file is kept
                            self.next_file_number += 1
                        
                        self.current_file = None
                        self.wav_file = None
                        self.recording_start_time = None
                
                self.queue.task_done()
                
            except queue.Empty:
                continue
            except Exception as e:
                print(f"\nRecording error: {e}", flush=True)
    
    def write_audio(self, audio_data, is_on, has_clipped=False):
        """
        Write audio data if recording
        
        Args:
            audio_data: Audio data as numpy array
            is_on: Whether signal is currently on
            has_clipped: Whether clipping was detected in this chunk
        """
        if is_on:
            if not self.recording:
                self.queue.put(("start", None))
            self.queue.put(("write", audio_data))
            if has_clipped:
                self.queue.put(("clipping", None))
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
                
                # Handle recording - active if ANY channel is on
                any_channel_on = any(is_on_status)
                any_clipping = any(clip_status)
                self.recorder.write_audio(audio, any_channel_on, any_clipping)
                
                # Draw the VU meter
                self._draw_display(stdscr, db_levels, peak_db_levels, max_db_levels, max_peak_db_levels, is_on_status, clip_status)
                
        finally:
            self.stop_recording()
            self.recorder.close()
    
    def _draw_display(self, stdscr, db_levels, peak_db_levels, max_db_levels, max_peak_db_levels, is_on_status, clip_status):
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
        right_label_width = 30
        bar_width = width - left_label_width - right_label_width - 1
        
        for ch, (db, peak_db, max_db, max_peak_db, is_on, has_clipped) in enumerate(zip(db_levels, peak_db_levels, max_db_levels, max_peak_db_levels, is_on_status, clip_status)):
            row = start_row + ch * 2
            if row >= height - 1:
                break
            
            normalized = (db - self.min_db) / self.db_range
            bar_length = int(normalized * bar_width)
            
            peak_normalized = (max_peak_db - self.min_db) / self.db_range
            peak_pos = int(peak_normalized * bar_width)
            
            max_normalized = (max_db - self.min_db) / self.db_range
            max_pos = int(max_normalized * bar_width)
            
            left_label = f" {db:5.1f}dB "
            if has_clipped:
                right_label = f" >{max_peak_db:5.1f} RMS:{max_db:5.1f} CLIP"
            else:
                right_label = f" >{max_peak_db:5.1f} RMS:{max_db:5.1f}"
            
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
                    # Draw peak indicator (>) at current peak position
                    elif i == peak_pos and peak_pos >= bar_length:
                        stdscr.addch(row, pos, '>', curses.color_pair(3) | curses.A_BOLD)
                    # Draw max RMS indicator
                    elif i == max_pos and max_pos >= bar_length and max_pos != peak_pos:
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
            clip_status = []
            for ch in range(vu_meter.channels):
                db = vu_meter.calculate_db(audio[:, ch])
                peak_db = vu_meter.calculate_peak_db(audio[:, ch])
                is_clipping = vu_meter.detect_clipping(audio[:, ch])
                _, _, is_on, has_clipped = vu_meter.update_history(ch, db, peak_db, is_clipping)
                is_on_status.append(is_on)
                clip_status.append(has_clipped)
            
            any_channel_on = any(is_on_status)
            any_clipping = any(clip_status)
            recorder.write_audio(audio, any_channel_on, any_clipping)
            
    except KeyboardInterrupt:
        pass
    finally:
        vu_meter.stop_recording()
        recorder.close()


def main():
    """Main entry point"""
    parser = argparse.ArgumentParser(description='Record audio with automatic start/stop')
    parser.add_argument('record_file', nargs='?', default='recording',
                        help='Base filename for recordings (default: recording)')
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
                        help='Duration of silence before recording stops in seconds (default: 10)')
    parser.add_argument('--min-length', type=int, default=600,
                        help='Minimum recording length in seconds (default: 600)')
    parser.add_argument('--no-vumeter', action='store_true',
                        help='Disable VU meter display')
    
    args = parser.parse_args()
    
    # Handle --list-targets
    if args.list_targets:
        return list_targets()
    
    # Validate or auto-select target
    target, error_code = validate_and_select_target(args.target)
    if error_code != 0:
        return error_code
    args.target = target
    
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
        bytes_per_sample=bytes_per_sample,
        min_length=args.min_length
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
            off_threshold=args.off_threshold,
            silence_duration=args.silence_duration
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
    sys.exit(main() or 0)
