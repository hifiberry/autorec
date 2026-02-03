"""
Common PipeWire utilities for target detection and validation
"""

import subprocess


def get_available_targets():
    """Get list of available PipeWire recording targets"""
    try:
        result = subprocess.run(
            ["pw-cli", "list-objects"],
            capture_output=True,
            text=True,
            timeout=5
        )
        
        if result.returncode == 0:
            lines = result.stdout.split('\n')
            sources = []
            current_obj = {}
            
            for line in lines:
                line = line.strip()
                
                if 'id' in line and ('type' in line or 'Node' in line):
                    if current_obj.get('name') and current_obj.get('is_source'):
                        sources.append(current_obj)
                    current_obj = {}
                elif 'node.name' in line:
                    parts = line.split('"')
                    if len(parts) >= 2:
                        current_obj['name'] = parts[1]
                elif 'node.description' in line or 'node.nick' in line:
                    parts = line.split('"')
                    if len(parts) >= 2:
                        current_obj['description'] = parts[1]
                elif 'media.class' in line:
                    if 'Source' in line or 'source' in line or 'Input' in line:
                        current_obj['is_source'] = True
            
            # Don't forget the last object
            if current_obj.get('name') and current_obj.get('is_source'):
                sources.append(current_obj)
            
            return sources
        
        return []
    
    except Exception:
        return []


def list_targets():
    """List available PipeWire recording targets"""
    sources = get_available_targets()
    
    if not sources:
        print("No recording sources found or could not query PipeWire.")
        print("Make sure PipeWire is running and pw-cli is installed.")
        return 1
    
    print("Available PipeWire recording targets:")
    print()
    for src in sources:
        name = src.get('name', 'unknown')
        desc = src.get('description', '')
        print(f"  {name}")
        if desc:
            print(f"    {desc}")
        print()
    
    return 0


def validate_and_select_target(specified_target, verbose=True):
    """
    Validate or auto-select a PipeWire target
    
    Args:
        specified_target: Target name or None for auto-detection
        verbose: Whether to print messages
    
    Returns:
        tuple: (target_name, error_code) where error_code is 0 for success, 1 for error
    """
    # Get available targets for validation
    available_targets = get_available_targets()
    target_names = [src.get('name') for src in available_targets if src.get('name')]
    
    # Auto-detect target if not specified
    if specified_target is None:
        if not target_names:
            if verbose:
                print("Error: No recording targets found. Make sure PipeWire is running.")
                print("Run with --list-targets to see available targets.")
            return None, 1
        
        target = target_names[0]
        if verbose:
            print(f"Auto-detected target: {target}")
            # Show description if available
            for src in available_targets:
                if src.get('name') == target and src.get('description'):
                    print(f"  {src['description']}")
            print()
        return target, 0
    else:
        # Validate that the specified target exists
        if target_names and specified_target not in target_names:
            if verbose:
                print(f"Error: Target '{specified_target}' not found.")
                print("\nAvailable targets:")
                for name in target_names:
                    print(f"  {name}")
                print("\nRun with --list-targets for more details.")
            return None, 1
        
        return specified_target, 0
