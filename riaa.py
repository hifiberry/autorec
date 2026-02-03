#!/usr/bin/env python3
"""
Configure and display RIAA settings via HiFiBerry PipeWire API
"""

import requests
import json
import sys
import argparse

BASE_URL = "http://localhost:2716/api/v1/module/riaa"


def get_riaa_config():
    """Get complete RIAA configuration"""
    try:
        response = requests.get(f"{BASE_URL}/config", timeout=5)
        response.raise_for_status()
        return response.json()
    except requests.exceptions.RequestException as e:
        print(f"Error connecting to RIAA API: {e}")
        return None


def print_config(config):
    """Print RIAA configuration in a readable format"""
    if not config:
        return
    
    print("RIAA Configuration:")
    print("=" * 50)
    print(f"Gain:                  {config.get('gain_db', 'N/A')} dB")
    
    # Subsonic filter
    subsonic = config.get('subsonic_filter', 0)
    subsonic_text = {0: "Off", 1: "20 Hz", 2: "30 Hz", 3: "40 Hz"}.get(subsonic, f"Unknown ({subsonic})")
    print(f"Subsonic Filter:       {subsonic_text}")
    
    print(f"RIAA Enable:           {'Yes' if config.get('riaa_enable') else 'No'}")
    print(f"Declick Enable:        {'Yes' if config.get('declick_enable') else 'No'}")
    
    # Spike detection
    print(f"  Spike Threshold:     {config.get('spike_threshold_db', 'N/A')} dB")
    print(f"  Spike Width:         {config.get('spike_width_ms', 'N/A')} ms")
    
    # Notch filter
    print(f"Notch Filter:          {'Enabled' if config.get('notch_filter_enable') else 'Disabled'}")
    print(f"  Frequency:           {config.get('notch_frequency_hz', 'N/A')} Hz")
    print(f"  Q Factor:            {config.get('notch_q_factor', 'N/A')}")
    print("=" * 50)


def set_gain(gain_db):
    """Set RIAA gain"""
    try:
        response = requests.put(
            f"{BASE_URL}/gain",
            json={"gain_db": gain_db},
            timeout=5
        )
        response.raise_for_status()
        result = response.json()
        if result.get('status') == 'ok':
            print(f"Gain set to {gain_db} dB")
            return True
        else:
            print(f"Failed to set gain: {result}")
            return False
    except requests.exceptions.RequestException as e:
        print(f"Error setting gain: {e}")
        return False


def set_subsonic(filter_value):
    """Set subsonic filter (0=Off, 1=20Hz, 2=30Hz, 3=40Hz)"""
    try:
        response = requests.put(
            f"{BASE_URL}/subsonic",
            json={"filter": filter_value},
            timeout=5
        )
        response.raise_for_status()
        result = response.json()
        if result.get('status') == 'ok':
            filter_text = {0: "Off", 1: "20 Hz", 2: "30 Hz", 3: "40 Hz"}.get(filter_value, str(filter_value))
            print(f"Subsonic filter set to {filter_text}")
            return True
        else:
            print(f"Failed to set subsonic filter: {result}")
            return False
    except requests.exceptions.RequestException as e:
        print(f"Error setting subsonic filter: {e}")
        return False


def main():
    """Main entry point"""
    parser = argparse.ArgumentParser(description='Check and configure RIAA settings')
    parser.add_argument('--gain', type=float, help='Set gain in dB')
    parser.add_argument('--subsonic', type=int, choices=[0, 1, 2, 3],
                        help='Set subsonic filter (0=Off, 1=20Hz, 2=30Hz, 3=40Hz)')
    
    args = parser.parse_args()
    
    # Set values if requested
    if args.gain is not None:
        if not set_gain(args.gain):
            return 1
    
    if args.subsonic is not None:
        if not set_subsonic(args.subsonic):
            return 1
    
    # Always show current config
    config = get_riaa_config()
    
    if config:
        print()
        print_config(config)
        return 0
    else:
        print("\nMake sure the HiFiBerry PipeWire API is running.")
        print("Check: http://localhost:2716/api/v1/module/riaa/config")
        return 1


if __name__ == "__main__":
    sys.exit(main())
