use std::io::{self, Write};
use crossterm::{
    cursor,
    execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};

use crate::vu_meter::ChannelMetrics;

/// Display VU meters for all channels using crossterm with colored bars.
/// 
/// This function renders a multi-line VU meter display with:
/// - Colored bars (green/yellow/red based on level)
/// - Peak indicators (>)
/// - RMS indicators (│)
/// - Scale markers showing dB values
/// - ON/OFF and CLIP status
/// 
/// # Arguments
/// * `metrics` - Array of channel metrics to display
/// * `db_range` - The dB range to display (e.g., 60.0 for -60 to 0 dB)
/// * `max_db` - Maximum dB value (typically 0.0)
/// * `recording_status` - Optional recording status text (e.g., "[RECORDING]")
///
/// # Example
/// ```no_run
/// use autorec::{display_vu_meter, ChannelMetrics};
/// 
/// let metrics = vec![ChannelMetrics::default()];
/// display_vu_meter(&metrics, 60.0, 0.0, None).ok();
/// ```
pub fn display_vu_meter(
    metrics: &[ChannelMetrics],
    db_range: f64,
    max_db: f64,
    recording_status: Option<&str>,
) -> Result<(), io::Error> {
    let mut stdout = io::stdout();
    let min_db = max_db - db_range;
    
    // Get terminal size and calculate bar width
    // If terminal size detection fails or returns unreasonably small value, use 80 as default
    let (detected_width, _height) = terminal::size().unwrap_or((80, 24));
    let width = if detected_width < 80 { 80 } else { detected_width };
    let left_label_width = 14;  // "Ch0: -XX.XdB |"
    let right_label_width = 27; // "| >-XX.X RMS:-XX.X ON   "
    let bar_width = (width as usize).saturating_sub(left_label_width + right_label_width).max(30);
    
    // Clear screen and move to top (like stdscr.clear() in Python)
    execute!(
        stdout,
        cursor::MoveTo(0, 2),  // Move to row 2 (after header)
        Clear(ClearType::FromCursorDown)
    )?;
    
    // Display recording status if provided
    if let Some(status) = recording_status {
        print!("{}\r\n", status);
    }
    
    // Draw each channel
    for (ch, m) in metrics.iter().enumerate() {
        // Calculate bar components
        let normalized = ((m.db - min_db) / db_range).max(0.0).min(1.0);
        let bar_length = (normalized * bar_width as f64) as usize;
        
        let peak_normalized = ((m.max_peak_db - min_db) / db_range).max(0.0).min(1.0);
        let peak_pos = (peak_normalized * bar_width as f64) as usize;
        
        let max_normalized = ((m.max_db - min_db) / db_range).max(0.0).min(1.0);
        let max_pos = (max_normalized * bar_width as f64) as usize;
        
        // Print label
        print!("Ch{}: {:5.1}dB |", ch, m.db);
        
        // Draw colored bar
        for i in 0..bar_width {
            if i < bar_length {
                // Color based on level
                let color = if !m.is_on {
                    Color::DarkGrey
                } else if m.db < -20.0 {
                    Color::Green
                } else if m.db < -10.0 {
                    Color::Yellow
                } else {
                    Color::Red
                };
                execute!(stdout, SetForegroundColor(color), Print('█'), ResetColor)?;
            } else if i == peak_pos && peak_pos >= bar_length {
                execute!(stdout, SetForegroundColor(Color::Red), Print('>'), ResetColor)?;
            } else if i == max_pos && max_pos >= bar_length && max_pos != peak_pos {
                execute!(stdout, SetForegroundColor(Color::Yellow), Print('│'), ResetColor)?;
            } else {
                print!(" ");
            }
        }
        
        // Status indicators
        let status = if m.is_on { "ON " } else { "OFF" };
        let clip = if m.has_clipped { " CLIP" } else { "     " };
        
        print!("| >{:5.1} RMS:{:5.1} {}{}\r\n", m.max_peak_db, m.max_db, status, clip);
        
        // Print scale line (only for first channel)
        if ch == 0 {
            // Print spaces to align with the bar start (matching "Ch0: -XX.XdB |")
            print!("             ");  // 13 spaces to align with the | before the bar
            
            let mut last_pos = 0;
            for db_marker in (-90..=0).step_by(10) {
                if db_marker < min_db as i32 || db_marker > max_db as i32 {
                    continue;
                }
                let marker_normalized = ((db_marker as f64 - min_db) / db_range).max(0.0).min(1.0);
                let marker_pos = (marker_normalized * bar_width as f64) as usize;
                
                // Print spaces to reach marker position
                let spaces = if marker_pos > last_pos { marker_pos - last_pos } else { 0 };
                for _ in 0..spaces {
                    print!(" ");
                }
                
                // Print marker
                let marker_str = if db_marker == 0 {
                    "0dB"
                } else {
                    &format!("{}", db_marker)
                };
                print!("{}", marker_str);
                last_pos = marker_pos + marker_str.len();
            }
            print!("\r\n");
        }
    }
    
    stdout.flush()?;
    Ok(())
}
